//! Opt-in clock for offline evaluation binaries. No algorithm or database changes.
use std::cell::Cell;

thread_local! {
    static NOW: Cell<Option<i64>> = const { Cell::new(None) };
}

pub(crate) fn now() -> Option<i64> {
    NOW.get()
}

/// Applies only on this thread, restoring the previous clock even during unwind.
pub fn at_time<T>(millis: i64, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<i64>);
    impl Drop for Restore {
        fn drop(&mut self) {
            NOW.set(self.0);
        }
    }
    let _restore = Restore(NOW.replace(Some(millis)));
    f()
}

#[cfg(test)]
mod tests {
    #[test]
    fn restores_nested_clock_and_does_not_leak_to_other_threads() {
        super::at_time(1234, || {
            assert_eq!(crate::feeds::now_ms(), 1234);
            super::at_time(5678, || assert_eq!(crate::feeds::now_ms(), 5678));
            assert_eq!(crate::feeds::now_ms(), 1234);
            assert_eq!(std::thread::spawn(super::now).join().unwrap(), None);
            let _ = std::panic::catch_unwind(|| super::at_time(9, || panic!("test")));
            assert_eq!(super::now(), Some(1234));
        });
        assert_eq!(super::now(), None);
    }
}
