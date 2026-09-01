//! Lock helpers that survive a poisoned mutex.
//!
//! `Mutex::lock().unwrap()` panics whenever *another* thread panicked while
//! holding that lock. In a metrics dashboard that turns one unlucky thread
//! into a cascade: the collector dies, every HTTP handler that touches the
//! same lock then dies too, and launchd restarts the whole process.
//!
//! None of the state behind these locks carries an invariant that a partial
//! write could corrupt in a way we'd rather crash over — it is a ring buffer
//! of samples and a SQLite connection. Recovering the guard and carrying on
//! serving is strictly better than taking the process down.

use std::sync::LockResult;

/// Take a guard, ignoring lock poisoning.
///
/// Works for both `Mutex` and `RwLock` since both return
/// `Result<Guard, PoisonError<Guard>>`.
pub fn guard<T>(result: LockResult<T>) -> T {
    result.unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex, RwLock};

    #[test]
    fn returns_the_guard_when_the_lock_is_healthy() {
        let m = Mutex::new(7);
        assert_eq!(*guard(m.lock()), 7);
    }

    #[test]
    fn recovers_the_value_after_a_holder_panicked() {
        let m = Arc::new(Mutex::new(vec![1, 2, 3]));

        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _held = m2.lock().unwrap();
            panic!("poison the lock");
        })
        .join();

        assert!(m.lock().is_err(), "precondition: lock should be poisoned");
        assert_eq!(*guard(m.lock()), vec![1, 2, 3]);
    }

    #[test]
    fn works_for_rwlock_too() {
        let rw = RwLock::new(String::from("ok"));
        assert_eq!(*guard(rw.read()), "ok");
        guard(rw.write()).push('!');
        assert_eq!(*guard(rw.read()), "ok!");
    }
}
