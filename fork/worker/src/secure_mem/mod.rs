//! Guarded, locked memory for the worker's secrets.
//!
//! Every secret the worker holds for longer than one function call (its
//! X25519 private key, the fileKeys it was sent) lives in a [`SecretRegion`]:
//!
//! - its own pages, between two no-access guard pages, so an overrun faults
//!   instead of reading or writing a neighbour;
//! - locked in RAM (`mlock` / `VirtualLock`), so it never reaches swap or the
//!   pagefile. If locking fails the allocation fails: there is no unlocked
//!   fallback;
//! - on Linux, backed by `memfd_secret(2)` when the kernel offers it, which
//!   also removes the pages from the kernel's direct map;
//! - excluded from core dumps (`MADV_DONTDUMP`) and, for private mappings,
//!   from forked children (`MADV_WIPEONFORK`);
//! - no-access whenever no job is reading it (on Windows also encrypted with
//!   `CryptProtectMemory`);
//! - zeroized before it is unlocked and unmapped.
//!
//! What it cannot cover: a key is briefly copied onto the stack (as
//! `ente_core::crypto::Key`, which zeroizes on drop) while a file is being
//! decrypted, and an attacker with root or kernel access to the running host
//! can still read it. See fork/worker/README.md.

#![allow(unsafe_code, reason = "OS memory APIs")]

use std::sync::Mutex;

use zeroize::Zeroize;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as sys;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as sys;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not map secret memory: {0}")]
    Map(std::io::Error),
    #[error(
        "could not lock secret memory in RAM: {0}. Raise the memlock limit \
         (LimitMEMLOCK= in the systemd unit, or `ulimit -l`); the worker \
         refuses to keep keys in swappable memory"
    )]
    Lock(std::io::Error),
    #[error("could not change secret memory protection: {0}")]
    Protect(std::io::Error),
    #[error("memfd_secret is required by config but unavailable: {0}")]
    SecretMemRequired(std::io::Error),
    #[error("key slab is full ({0} slots)")]
    Full(usize),
    #[error("secret has the wrong length: expected {expected}, got {actual}")]
    Length { expected: usize, actual: usize },
}

pub type Result<T> = std::result::Result<T, Error>;

/// How the pages of a region are backed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backing {
    /// Linux `memfd_secret`: locked, and unmapped from the kernel direct map.
    MemfdSecret,
    /// Anonymous pages locked with `mlock` / `VirtualLock`.
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    None,
    Read,
    ReadWrite,
}

/// Options for new regions, from `[memory]` in worker.toml.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Fail instead of falling back to `mlock` when `memfd_secret` is missing.
    pub require_memfd_secret: bool,
    /// Use plain `mlock` even where `memfd_secret` exists (tests).
    #[doc(hidden)]
    pub skip_memfd_secret: bool,
}

/// One guarded, locked allocation. Not `Sync`: wrap it in a lock to share.
pub struct SecretRegion {
    raw: sys::Raw,
    len: usize,
}

// SAFETY: the region owns its mapping exclusively; access goes through
// `&mut self`, so moving it to another thread is sound.
unsafe impl Send for SecretRegion {}

impl SecretRegion {
    /// Allocates at least `len` bytes, zeroed, locked and no-access.
    pub fn new(len: usize, options: Options) -> Result<Self> {
        let raw = sys::alloc(len.max(1), options)?;
        let mut region = Self { raw, len };
        region.set(Access::None)?;
        Ok(region)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn backing(&self) -> Backing {
        self.raw.backing
    }

    /// Runs `f` with the region readable, then makes it no-access again.
    pub fn with<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Result<R> {
        self.set(Access::Read)?;
        // SAFETY: the mapping is live, readable, and at least `len` long.
        let bytes = unsafe { std::slice::from_raw_parts(self.raw.data, self.len) };
        let out = f(bytes);
        self.set(Access::None)?;
        Ok(out)
    }

    /// Runs `f` with the region writable, then makes it no-access again.
    pub fn with_mut<R>(&mut self, f: impl FnOnce(&mut [u8]) -> R) -> Result<R> {
        self.set(Access::ReadWrite)?;
        // SAFETY: the mapping is live, writable, at least `len` long, and
        // `&mut self` makes this the only reference.
        let bytes = unsafe { std::slice::from_raw_parts_mut(self.raw.data, self.len) };
        let out = f(bytes);
        self.set(Access::None)?;
        Ok(out)
    }

    fn set(&mut self, access: Access) -> Result<()> {
        sys::protect(&mut self.raw, access)
    }
}

impl Drop for SecretRegion {
    fn drop(&mut self) {
        if sys::protect(&mut self.raw, Access::ReadWrite).is_ok() {
            // SAFETY: as in `with_mut`; this is the last use of the mapping.
            let bytes = unsafe { std::slice::from_raw_parts_mut(self.raw.data, self.raw.data_len) };
            bytes.zeroize();
        } else {
            log::error!("could not unprotect secret memory to wipe it; unmapping anyway");
        }
        sys::free(&mut self.raw);
    }
}

/// A fixed number of 32 byte key slots in one [`SecretRegion`].
///
/// Keys go in by copy (the source is zeroized) and are only ever lent out
/// inside a closure, as an `ente_core` [`Key`](ente_core::crypto::Key) that
/// zeroizes itself on drop.
pub struct KeySlab {
    inner: Mutex<SlabInner>,
    capacity: usize,
}

struct SlabInner {
    region: SecretRegion,
    used: Vec<bool>,
}

pub const KEY_BYTES: usize = 32;

/// A handle to one slot. Free it with [`KeySlab::remove`]; dropping the
/// slab wipes every slot anyway.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Slot(usize);

impl KeySlab {
    pub fn new(capacity: usize, options: Options) -> Result<Self> {
        let capacity = capacity.max(1);
        Ok(Self {
            inner: Mutex::new(SlabInner {
                region: SecretRegion::new(capacity * KEY_BYTES, options)?,
                used: vec![false; capacity],
            }),
            capacity,
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn backing(&self) -> Backing {
        self.lock().region.backing()
    }

    pub fn in_use(&self) -> usize {
        self.lock().used.iter().filter(|u| **u).count()
    }

    /// Copies `secret` into a free slot and zeroizes `secret`, whether or
    /// not that succeeds.
    pub fn insert(&self, secret: &mut [u8]) -> Result<Slot> {
        let result = self.insert_inner(secret);
        secret.zeroize();
        result
    }

    fn insert_inner(&self, secret: &[u8]) -> Result<Slot> {
        if secret.len() != KEY_BYTES {
            return Err(Error::Length {
                expected: KEY_BYTES,
                actual: secret.len(),
            });
        }
        let mut inner = self.lock();
        let index = inner
            .used
            .iter()
            .position(|u| !*u)
            .ok_or(Error::Full(self.capacity))?;
        inner.region.with_mut(|bytes| {
            bytes[index * KEY_BYTES..(index + 1) * KEY_BYTES].copy_from_slice(secret);
        })?;
        inner.used[index] = true;
        Ok(Slot(index))
    }

    /// Lends the key in `slot` to `f`. The copy `f` sees is zeroized when
    /// `f` returns.
    pub fn with_key<R>(
        &self,
        slot: &Slot,
        f: impl FnOnce(&ente_core::crypto::Key) -> R,
    ) -> Result<R> {
        let mut inner = self.lock();
        inner.region.with(|bytes| {
            let mut buf = [0u8; KEY_BYTES];
            buf.copy_from_slice(&bytes[slot.0 * KEY_BYTES..(slot.0 + 1) * KEY_BYTES]);
            let key = ente_core::crypto::Key::from_bytes(buf);
            buf.zeroize();
            f(&key)
        })
    }

    /// Wipes and frees a slot.
    pub fn remove(&self, slot: Slot) -> Result<()> {
        let mut inner = self.lock();
        inner.region.with_mut(|bytes| {
            bytes[slot.0 * KEY_BYTES..(slot.0 + 1) * KEY_BYTES].zeroize();
        })?;
        inner.used[slot.0] = false;
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SlabInner> {
        // A panic while holding the lock leaves the region no-access or
        // wiped, never half-written in a way that matters: keep going.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Whether this kernel offers `memfd_secret` (always false off Linux).
pub fn memfd_secret_available() -> std::result::Result<(), std::io::Error> {
    sys::probe_memfd_secret()
}

pub fn page_size() -> usize {
    sys::page_size()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both_backings() -> [Options; 2] {
        [
            Options::default(),
            Options {
                skip_memfd_secret: true,
                ..Options::default()
            },
        ]
    }

    #[test]
    fn region_roundtrip_and_page_rounding() {
        for options in both_backings() {
            let mut r = SecretRegion::new(100, options).unwrap();
            assert_eq!(r.len(), 100);
            r.with(|b| assert!(b.iter().all(|x| *x == 0))).unwrap();
            r.with_mut(|b| b[..4].copy_from_slice(b"abcd")).unwrap();
            r.with(|b| assert_eq!(&b[..4], b"abcd")).unwrap();
        }
        let plain = SecretRegion::new(1, both_backings()[1]).unwrap();
        assert_eq!(plain.backing(), Backing::Locked);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn uses_memfd_secret_when_the_kernel_has_it() {
        let r = SecretRegion::new(1, Options::default()).unwrap();
        let expected = if memfd_secret_available().is_ok() {
            Backing::MemfdSecret
        } else {
            Backing::Locked
        };
        assert_eq!(r.backing(), expected);
    }

    #[test]
    fn slab_insert_lend_remove() {
        let slab = KeySlab::new(2, Options::default()).unwrap();
        let mut a = [7u8; KEY_BYTES];
        let slot_a = slab.insert(&mut a).unwrap();
        assert_eq!(a, [0u8; KEY_BYTES], "source is wiped");
        let mut b = vec![9u8; KEY_BYTES];
        let slot_b = slab.insert(&mut b).unwrap();
        assert!(b.iter().all(|x| *x == 0));

        let mut c = [1u8; KEY_BYTES];
        assert!(matches!(slab.insert(&mut c), Err(Error::Full(2))));
        assert_eq!(c, [0u8; KEY_BYTES], "wiped even on failure");

        slab.with_key(&slot_a, |k| assert_eq!(k.as_bytes(), &[7u8; KEY_BYTES]))
            .unwrap();
        slab.with_key(&slot_b, |k| assert_eq!(k.as_bytes(), &[9u8; KEY_BYTES]))
            .unwrap();
        assert_eq!(slab.in_use(), 2);
        slab.remove(slot_a).unwrap();
        assert_eq!(slab.in_use(), 1);
        let mut d = [3u8; KEY_BYTES];
        let slot_d = slab.insert(&mut d).unwrap();
        slab.with_key(&slot_d, |k| assert_eq!(k.as_bytes(), &[3u8; KEY_BYTES]))
            .unwrap();
    }

    #[test]
    fn slab_rejects_wrong_length() {
        let slab = KeySlab::new(1, Options::default()).unwrap();
        let mut short = vec![1u8; 16];
        assert!(matches!(
            slab.insert(&mut short),
            Err(Error::Length {
                expected: 32,
                actual: 16
            })
        ));
        assert!(short.iter().all(|x| *x == 0));
    }

    // The region must be inaccessible between uses. Touching it from a
    // child process must crash that child with SIGSEGV.
    #[cfg(target_os = "linux")]
    #[test]
    fn idle_region_faults_on_read() {
        for options in both_backings() {
            idle_faults(options);
        }
    }

    #[cfg(target_os = "linux")]
    fn idle_faults(options: Options) {
        let r = SecretRegion::new(64, options).unwrap();
        let ptr = r.raw.data;
        // SAFETY: fork in a test; the child only reads one byte and exits.
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            // SAFETY: deliberately reads a PROT_NONE page; expected to fault.
            let v = unsafe { std::ptr::read_volatile(ptr) };
            // SAFETY: exit without running test harness destructors.
            unsafe { libc::_exit(i32::from(v) + 10) };
        }
        let mut status = 0;
        // SAFETY: waits for the child created above.
        unsafe { libc::waitpid(pid, &mut status, 0) };
        assert!(libc::WIFSIGNALED(status), "child read idle secret memory");
        assert_eq!(libc::WTERMSIG(status), libc::SIGSEGV);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn guard_page_after_region_faults() {
        let mut r = SecretRegion::new(16, Options::default()).unwrap();
        let after = r.raw.data.wrapping_add(r.raw.data_len);
        r.with_mut(|_| {
            // SAFETY: as above; the guard page must fault.
            let pid = unsafe { libc::fork() };
            if pid == 0 {
                let v = unsafe { std::ptr::read_volatile(after) };
                unsafe { libc::_exit(i32::from(v) + 10) };
            }
            let mut status = 0;
            unsafe { libc::waitpid(pid, &mut status, 0) };
            assert!(libc::WIFSIGNALED(status), "guard page was readable");
        })
        .unwrap();
    }
}
