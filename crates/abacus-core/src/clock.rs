use std::sync::atomic::{AtomicU64, Ordering};

const NANOS_PER_SEC: u64 = 1_000_000_000;
const NANOS_PER_MS: u64 = 1_000_000;

pub fn monotonic_now_nanos() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let ret = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    if ret != 0 {
        eprintln!("abacus: clock_gettime failed; aborting");
        std::process::abort();
    }
    match nanos_from_timespec(&ts) {
        Some(n) => n,
        None => {
            eprintln!(
                "abacus: CLOCK_MONOTONIC returned unrepresentable timespec \
                 (tv_sec={}, tv_nsec={}); aborting",
                ts.tv_sec, ts.tv_nsec
            );
            std::process::abort();
        }
    }
}

fn nanos_from_timespec(ts: &libc::timespec) -> Option<u64> {
    let secs = u64::try_from(ts.tv_sec).ok()?;
    let nsecs = u64::try_from(ts.tv_nsec).ok()?;
    secs.checked_mul(NANOS_PER_SEC)?.checked_add(nsecs)
}

pub fn expiration_alive(expiration_ns: u64, now: u64) -> bool {
    expiration_ns > now
}

pub fn sleep_until_nanos(target: u64) {
    loop {
        let now = monotonic_now_nanos();
        if now >= target {
            return;
        }
        let delta = target - now;
        let ts = libc::timespec {
            tv_sec: (delta / NANOS_PER_SEC) as libc::time_t,
            tv_nsec: (delta % NANOS_PER_SEC) as libc::c_long,
        };
        unsafe {
            libc::nanosleep(&ts, std::ptr::null_mut());
        }
    }
}

pub fn ms_to_nanos(ms: u64) -> u64 {
    ms.saturating_mul(NANOS_PER_MS)
}

pub fn futex_wake(word: &AtomicU64) {
    let ptr = futex_addr(word);
    unsafe {
        libc::syscall(
            libc::SYS_futex,
            ptr,
            libc::FUTEX_WAKE,
            i32::MAX,
            std::ptr::null::<libc::timespec>(),
            std::ptr::null::<u32>(),
            0u32,
        );
    }
}

/// Block until the low 32 bits of `word` differ from `expected_lo32`, or until
/// `timeout_nanos` nanoseconds elapse. Pass `0` for `timeout_nanos` to block
/// indefinitely.
///
/// Returns the raw syscall result: 0 on wake, -1 on error (check errno for
/// EAGAIN if value already changed, ETIMEDOUT on timeout, EINTR on signal).
pub fn futex_wait(word: &AtomicU64, expected_lo32: u32, timeout_nanos: u64) -> libc::c_long {
    let ptr = futex_addr(word);
    if timeout_nanos == 0 {
        unsafe {
            libc::syscall(
                libc::SYS_futex,
                ptr,
                libc::FUTEX_WAIT,
                expected_lo32,
                std::ptr::null::<libc::timespec>(),
                std::ptr::null::<u32>(),
                0u32,
            ) as libc::c_long
        }
    } else {
        let ts = libc::timespec {
            tv_sec: (timeout_nanos / NANOS_PER_SEC) as libc::time_t,
            tv_nsec: (timeout_nanos % NANOS_PER_SEC) as libc::c_long,
        };
        unsafe {
            libc::syscall(
                libc::SYS_futex,
                ptr,
                libc::FUTEX_WAIT,
                expected_lo32,
                &ts as *const libc::timespec,
                std::ptr::null::<u32>(),
                0u32,
            ) as libc::c_long
        }
    }
}

/// Extract the low 32 bits of a u64 for futex comparison.
pub fn futex_word(value: u64) -> u32 {
    value as u32
}

fn futex_addr(word: &AtomicU64) -> *const u32 {
    (word as *const AtomicU64).cast::<u32>()
}
