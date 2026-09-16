use std::fmt;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::clock::monotonic_now_nanos;
use crate::error::{AllocationStep, Condition, Result};

pub const INTERLOCK_SIZE: usize = 24;
pub const CREATION_TTL_NANOS: u64 = 100_000_000; // 100 ms

// -- Layout --

#[repr(C)]
pub struct Interlock {
    pub open_count: AtomicU64,
    pub closed_count: AtomicU64,
    pub expiration_ns: AtomicU64,
}

const _: () = assert!(std::mem::size_of::<Interlock>() == INTERLOCK_SIZE);

// -- Handle --

struct InterlockRegion {
    fd: OwnedFd,
    words: NonNull<Interlock>,
}

impl Drop for InterlockRegion {
    fn drop(&mut self) {
        let ret = unsafe { libc::munmap(self.words.as_ptr().cast(), INTERLOCK_SIZE) };
        if ret != 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            eprintln!(
                "abacus: InterlockRegion munmap failed: ptr={:p}, errno={errno}",
                self.words
            );
        }
    }
}

unsafe impl Send for InterlockRegion {}
unsafe impl Sync for InterlockRegion {}

impl fmt::Debug for InterlockRegion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InterlockRegion")
            .field("fd", &self.fd.as_raw_fd())
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct InterlockHandle(Arc<InterlockRegion>);

impl InterlockHandle {
    pub(crate) fn from_raw(fd: OwnedFd, words: NonNull<Interlock>) -> Self {
        Self(Arc::new(InterlockRegion { fd, words }))
    }

    pub fn words(&self) -> &Interlock {
        unsafe { self.0.words.as_ref() }
    }

    pub fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.fd.as_raw_fd()
    }
}

// -- Create / Open --

pub fn interlock_create() -> Result<InterlockHandle> {
    let raw_fd = memfd_create()?;
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    ftruncate(&fd)?;
    let handle = mmap_interlock(fd)?;
    // Set initial expiration directly. CAS-max interlock_arm would reject
    // the zero-initialized expiration as reaped.
    let deadline = monotonic_now_nanos().saturating_add(CREATION_TTL_NANOS);
    handle.words().expiration_ns.store(deadline, Ordering::Release);
    Ok(handle)
}

pub fn interlock_open(fd: OwnedFd) -> Result<InterlockHandle> {
    mmap_interlock(fd)
}

pub fn interlock_arm(handle: &InterlockHandle, ttl_nanos: u64) -> Result<()> {
    let expiration_ns = &handle.words().expiration_ns;
    let deadline = monotonic_now_nanos().saturating_add(ttl_nanos);
    loop {
        let current = expiration_ns.load(Ordering::Acquire);
        if current == 0 {
            return Err(Condition::InterlockReaped);
        }
        let target = current.max(deadline);
        if target == current {
            return Ok(());
        }
        match expiration_ns.compare_exchange_weak(
            current,
            target,
            Ordering::Release,
            Ordering::Acquire,
        ) {
            Ok(_) => return Ok(()),
            Err(_) => continue,
        }
    }
}

pub fn interlock_reap(handle: &InterlockHandle) {
    let words = handle.words();
    words.expiration_ns.store(0, Ordering::Release);
    crate::clock::futex_wake(&words.open_count);
    crate::clock::futex_wake(&words.closed_count);
}

pub fn interlock_is_reapable(handle: &InterlockHandle) -> bool {
    let exp = handle.words().expiration_ns.load(Ordering::Acquire);
    exp == 0 || exp < monotonic_now_nanos()
}

pub fn interlock_read_expiration(handle: &InterlockHandle) -> u64 {
    handle.words().expiration_ns.load(Ordering::Acquire)
}

pub fn interlock_dup_fd(handle: &InterlockHandle) -> Result<OwnedFd> {
    let raw = unsafe { libc::dup(handle.as_raw_fd()) };
    if raw < 0 {
        return Err(Condition::AllocationFailed {
            step: AllocationStep::MemfdCreate,
            errno: last_errno(),
        });
    }
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

// -- Internal helpers --

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn memfd_create() -> Result<i32> {
    let raw_fd = unsafe { libc::memfd_create(c"abacus-interlock".as_ptr(), libc::MFD_CLOEXEC) };
    if raw_fd < 0 {
        return Err(Condition::AllocationFailed {
            step: AllocationStep::MemfdCreate,
            errno: last_errno(),
        });
    }
    Ok(raw_fd)
}

fn ftruncate(fd: &OwnedFd) -> Result<()> {
    let size = libc::off_t::try_from(INTERLOCK_SIZE).unwrap_or(0);
    let ret = unsafe { libc::ftruncate(fd.as_raw_fd(), size) };
    if ret != 0 {
        return Err(Condition::AllocationFailed {
            step: AllocationStep::Ftruncate,
            errno: last_errno(),
        });
    }
    Ok(())
}

fn mmap_interlock(fd: OwnedFd) -> Result<InterlockHandle> {
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            INTERLOCK_SIZE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(Condition::AllocationFailed {
            step: AllocationStep::Mmap,
            errno: last_errno(),
        });
    }
    let words = NonNull::new(ptr.cast::<Interlock>()).ok_or(Condition::AllocationFailed {
        step: AllocationStep::Mmap,
        errno: 0,
    })?;
    Ok(InterlockHandle::from_raw(fd, words))
}
