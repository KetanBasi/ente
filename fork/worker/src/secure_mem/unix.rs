//! `mmap` + `mlock` regions with guard pages; `memfd_secret` on Linux.

use std::io;
use std::ptr;

use super::{Access, Backing, Error, Options, Result};

pub struct Raw {
    /// Start of the whole mapping, guard pages included.
    base: *mut u8,
    total: usize,
    /// Start and length of the usable, page-rounded middle.
    pub data: *mut u8,
    pub data_len: usize,
    pub backing: Backing,
    fd: Option<libc::c_int>,
}

pub fn page_size() -> usize {
    // SAFETY: sysconf has no preconditions.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(size)
        .ok()
        .filter(|s| *s > 0)
        .unwrap_or(4096)
}

pub fn alloc(len: usize, options: Options) -> Result<Raw> {
    let page = page_size();
    let data_len = len.div_ceil(page) * page;
    let total = data_len + 2 * page;

    // Reserve guard + data + guard as one no-access mapping.
    // SAFETY: a fresh anonymous mapping; no existing memory is touched.
    let base = unsafe {
        libc::mmap(
            ptr::null_mut(),
            total,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    if base == libc::MAP_FAILED {
        return Err(Error::Map(io::Error::last_os_error()));
    }
    let base = base.cast::<u8>();
    // SAFETY: `page` is inside the mapping of `total` bytes.
    let data = unsafe { base.add(page) };
    let mut raw = Raw {
        base,
        total,
        data,
        data_len,
        backing: Backing::Locked,
        fd: None,
    };

    let secret = if options.skip_memfd_secret {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    } else {
        map_secret(&mut raw)
    };
    match secret {
        Ok(()) => {}
        Err(e) if options.require_memfd_secret => {
            free(&mut raw);
            return Err(Error::SecretMemRequired(e));
        }
        Err(_) => {
            // SAFETY: the middle pages are part of our own mapping.
            if unsafe { libc::mprotect(data.cast(), data_len, libc::PROT_READ | libc::PROT_WRITE) }
                != 0
            {
                let e = io::Error::last_os_error();
                free(&mut raw);
                return Err(Error::Protect(e));
            }
        }
    }

    // memfd_secret pages are never swapped and are charged to the memlock
    // limit by the kernel itself (mlock on them fails); lock the rest.
    // SAFETY: the middle pages are mapped and ours.
    if raw.backing == Backing::Locked && unsafe { libc::mlock(data.cast(), data_len) } != 0 {
        let e = io::Error::last_os_error();
        free(&mut raw);
        return Err(Error::Lock(e));
    }
    advise(&raw);
    Ok(raw)
}

#[cfg(target_os = "linux")]
fn map_secret(raw: &mut Raw) -> io::Result<()> {
    let fd = open_memfd_secret()?;
    let len = libc::off_t::try_from(raw.data_len)
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: `fd` is a fresh memfd we own.
    if unsafe { libc::ftruncate(fd, len) } != 0 {
        let e = io::Error::last_os_error();
        // SAFETY: closing our own fd.
        unsafe { libc::close(fd) };
        return Err(e);
    }
    // Replace the reserved middle with the secret mapping, in place.
    // SAFETY: MAP_FIXED over pages we reserved above and nobody else uses.
    let mapped = unsafe {
        libc::mmap(
            raw.data.cast(),
            raw.data_len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_FIXED,
            fd,
            0,
        )
    };
    if mapped == libc::MAP_FAILED {
        let e = io::Error::last_os_error();
        // SAFETY: closing our own fd.
        unsafe { libc::close(fd) };
        return Err(e);
    }
    raw.fd = Some(fd);
    raw.backing = Backing::MemfdSecret;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn map_secret(_raw: &mut Raw) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

#[cfg(target_os = "linux")]
fn open_memfd_secret() -> io::Result<libc::c_int> {
    // SAFETY: memfd_secret takes one flags argument and returns an fd or -1.
    let fd = unsafe { libc::syscall(libc::SYS_memfd_secret, libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    libc::c_int::try_from(fd).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
}

pub fn probe_memfd_secret() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let fd = open_memfd_secret()?;
        // SAFETY: closing the fd we just opened.
        unsafe { libc::close(fd) };
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

#[cfg(target_os = "linux")]
fn advise(raw: &Raw) {
    // Best effort: a failure here weakens dump hygiene, not swap safety.
    // SAFETY: advising on our own mapped pages.
    unsafe {
        libc::madvise(raw.data.cast(), raw.data_len, libc::MADV_DONTDUMP);
        if raw.backing == Backing::Locked {
            libc::madvise(raw.data.cast(), raw.data_len, libc::MADV_WIPEONFORK);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn advise(_raw: &Raw) {}

pub fn protect(raw: &mut Raw, access: Access) -> Result<()> {
    let prot = match access {
        Access::None => libc::PROT_NONE,
        Access::Read => libc::PROT_READ,
        Access::ReadWrite => libc::PROT_READ | libc::PROT_WRITE,
    };
    // SAFETY: changing protection of our own mapped pages.
    if unsafe { libc::mprotect(raw.data.cast(), raw.data_len, prot) } != 0 {
        return Err(Error::Protect(io::Error::last_os_error()));
    }
    Ok(())
}

pub fn free(raw: &mut Raw) {
    // SAFETY: unlocking and unmapping our own mapping, once, at drop.
    unsafe {
        if raw.backing == Backing::Locked {
            libc::munlock(raw.data.cast(), raw.data_len);
        }
        libc::munmap(raw.base.cast(), raw.total);
        if let Some(fd) = raw.fd.take() {
            libc::close(fd);
        }
    }
}
