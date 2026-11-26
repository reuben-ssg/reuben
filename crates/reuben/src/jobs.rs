//! How many threads a build uses when `--jobs` isn't given.

use std::num::NonZero;

/// The performance cores on a Mac that has them, or else one thread per available core.
///
/// A build is mostly file creation, which the filesystem can't do faster past a handful of threads, so threads on
/// efficiency cores add CPU time without making it faster.
pub fn default_jobs() -> usize {
    let available = std::thread::available_parallelism().ok().map(NonZero::get);
    pick(performance_cores(), available)
}

fn pick(performance: Option<usize>, available: Option<usize>) -> usize {
    match (performance.filter(|&n| n > 0), available) {
        (Some(p), Some(a)) => p.min(a),
        (Some(n), None) | (None, Some(n)) => n,
        (None, None) => 1,
    }
}

/// `hw.perflevel0.physicalcpu`, which Apple silicon Macs report. Intel Macs and older macOS versions don't have it.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn performance_cores() -> Option<usize> {
    let mut count: libc::c_int = 0;
    let mut size = size_of::<libc::c_int>();
    // SAFETY: the name is a NUL-terminated C string, `count` is a live `c_int` that `size` gives the exact size of,
    // and the null new value with length 0 makes this a read only.
    let status = unsafe {
        libc::sysctlbyname(
            c"hw.perflevel0.physicalcpu".as_ptr(),
            (&raw mut count).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 || size != size_of::<libc::c_int>() {
        return None;
    }
    usize::try_from(count).ok()
}

#[cfg(not(target_os = "macos"))]
fn performance_cores() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn performance_cores_win_when_known() {
        assert_eq!(pick(Some(6), Some(12)), 6);
    }

    #[test]
    fn available_cores_cap_the_performance_cores() {
        assert_eq!(pick(Some(6), Some(4)), 4);
    }

    #[test]
    fn falls_back_to_available_cores() {
        assert_eq!(pick(None, Some(12)), 12);
        assert_eq!(pick(Some(0), Some(8)), 8);
    }

    #[test]
    fn falls_back_to_one_thread() {
        assert_eq!(pick(None, None), 1);
        assert_eq!(pick(Some(0), None), 1);
        assert_eq!(pick(Some(3), None), 3);
    }

    #[test]
    fn default_is_at_least_one_and_at_most_the_available_cores() {
        let jobs = default_jobs();
        assert!(jobs >= 1);
        if let Ok(available) = std::thread::available_parallelism() {
            assert!(jobs <= available.get());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_the_performance_cores_on_apple_silicon() {
        if cfg!(target_arch = "aarch64") {
            assert!(performance_cores().is_some_and(|n| n > 0));
        }
    }
}
