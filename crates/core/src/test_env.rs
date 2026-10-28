//! Serialised access to process environment variables in tests.

use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Sets environment variables for the lifetime of the guard and restores them on drop.
///
/// Holding the guard also holds a process-wide lock, so tests touching the environment
/// never run concurrently.
pub(crate) struct EnvGuard {
    saved: Vec<(String, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Sets each `(name, Some(value))` and removes each `(name, None)`.
    pub(crate) fn set(vars: &[(&str, Option<&str>)]) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved = vars
            .iter()
            .map(|(name, value)| {
                let previous = std::env::var_os(name);
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
                ((*name).to_owned(), previous)
            })
            .collect();
        EnvGuard { saved, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, previous) in self.saved.drain(..).rev() {
            match previous {
                Some(value) => std::env::set_var(&name, value),
                None => std::env::remove_var(&name),
            }
        }
    }
}
