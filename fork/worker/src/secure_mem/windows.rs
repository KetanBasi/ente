// `VirtualAlloc` + `VirtualLock` regions with guard pages, encrypted with
// `CryptProtectMemory` while idle.

use std::ffi::c_void;
use std::io;
use std::ptr;

use windows_sys::Win32::Security::Cryptography::{
    CRYPTPROTECTMEMORY_SAME_PROCESS, CryptProtectMemory, CryptUnprotectMemory,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_PROTECTION_FLAGS, PAGE_READONLY,
    PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualLock, VirtualProtect, VirtualUnlock,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

use super::{Access, Backing, Error, Options, Result};

pub struct Raw {
    base: *mut u8,
    pub data: *mut u8,
    pub data_len: usize,
    pub backing: Backing,
    // Whether the data pages are currently CryptProtectMemory-encrypted.
    sealed: bool,
}

pub fn page_size() -> usize {
    let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: GetSystemInfo fills the struct we pass.
    unsafe { GetSystemInfo(&mut info) };
    usize::try_from(info.dwPageSize)
        .ok()
        .filter(|s| *s > 0)
        .unwrap_or(4096)
}

pub fn alloc(len: usize, options: Options) -> Result<Raw> {
    if options.require_memfd_secret {
        return Err(Error::SecretMemRequired(io::Error::from(
            io::ErrorKind::Unsupported,
        )));
    }
    let page = page_size();
    let data_len = len.div_ceil(page) * page;
    let total = data_len + 2 * page;
    // SAFETY: a fresh reservation; no existing memory is touched.
    let base = unsafe { VirtualAlloc(ptr::null(), total, MEM_RESERVE | MEM_COMMIT, PAGE_NOACCESS) };
    if base.is_null() {
        return Err(Error::Map(io::Error::last_os_error()));
    }
    let base = base.cast::<u8>();
    // SAFETY: `page` is inside the allocation.
    let data = unsafe { base.add(page) };
    let mut raw = Raw {
        base,
        data,
        data_len,
        backing: Backing::Locked,
        sealed: false,
    };
    if let Err(e) = set_protection(&raw, PAGE_READWRITE) {
        free(&mut raw);
        return Err(e);
    }
    // SAFETY: locking our own committed, accessible pages.
    if unsafe { VirtualLock(data.cast(), data_len) } == 0 {
        let e = io::Error::last_os_error();
        free(&mut raw);
        return Err(Error::Lock(e));
    }
    Ok(raw)
}

fn set_protection(raw: &Raw, flags: PAGE_PROTECTION_FLAGS) -> Result<()> {
    let mut old: PAGE_PROTECTION_FLAGS = 0;
    // SAFETY: changing protection of our own pages.
    if unsafe { VirtualProtect(raw.data.cast::<c_void>(), raw.data_len, flags, &mut old) } == 0 {
        return Err(Error::Protect(io::Error::last_os_error()));
    }
    Ok(())
}

pub fn protect(raw: &mut Raw, access: Access) -> Result<()> {
    if raw.sealed {
        set_protection(raw, PAGE_READWRITE)?;
        // SAFETY: decrypting our own page-aligned, block-multiple region.
        let len = u32::try_from(raw.data_len)
            .map_err(|_| Error::Protect(io::Error::from(io::ErrorKind::InvalidInput)))?;
        if unsafe { CryptUnprotectMemory(raw.data.cast(), len, CRYPTPROTECTMEMORY_SAME_PROCESS) }
            == 0
        {
            return Err(Error::Protect(io::Error::last_os_error()));
        }
        raw.sealed = false;
    }
    match access {
        Access::ReadWrite => set_protection(raw, PAGE_READWRITE),
        Access::Read => set_protection(raw, PAGE_READONLY),
        Access::None => {
            set_protection(raw, PAGE_READWRITE)?;
            let len = u32::try_from(raw.data_len)
                .map_err(|_| Error::Protect(io::Error::from(io::ErrorKind::InvalidInput)))?;
            // SAFETY: encrypting our own page-aligned, block-multiple region.
            if unsafe { CryptProtectMemory(raw.data.cast(), len, CRYPTPROTECTMEMORY_SAME_PROCESS) }
                == 0
            {
                return Err(Error::Protect(io::Error::last_os_error()));
            }
            raw.sealed = true;
            set_protection(raw, PAGE_NOACCESS)
        }
    }
}

pub fn free(raw: &mut Raw) {
    // SAFETY: unlocking and releasing our own allocation, once, at drop.
    unsafe {
        VirtualUnlock(raw.data.cast(), raw.data_len);
        VirtualFree(raw.base.cast(), 0, MEM_RELEASE);
    }
}

pub fn probe_memfd_secret() -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}
