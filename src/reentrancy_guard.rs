//! Reusable storage-based reentrancy guard for cross-contract calls.
//!
//! Any contract function that invokes an external contract can be re-entered:
//! the callee may call back into the same guarded function before its first
//! invocation has finished, observing half-updated state. This module provides
//! a lightweight mutual-exclusion lock, backed by instance storage, that such
//! functions wrap around their critical section.
//!
//! ## Usage
//!
//! ```ignore
//! use crate::reentrancy_guard::with_guard;
//!
//! pub fn do_external_call(env: &Env) -> Result<(), MyError> {
//!     with_guard(env, || {
//!         // ... perform the cross-contract call and state updates here ...
//!         Ok(())
//!     })
//! }
//! ```
//!
//! The lock lives in instance storage, so it is automatically rolled back if
//! the transaction panics — a failed guarded call can never leave the contract
//! permanently locked.

use soroban_sdk::{contracterror, contracttype, Env};

/// Storage key for the reentrancy lock flag.
#[derive(Clone)]
#[contracttype]
pub enum GuardKey {
    /// Global reentrancy lock flag (instance storage, cheapest to read).
    ReentrancyLock,
}

/// Error raised when a guarded section is re-entered.
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ReentrancyError {
    /// A guarded section was entered while another was still in progress.
    ReentrantCall = 1,
}

impl crate::error_standard::StandardContractError for ReentrancyError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::ReentrantCall => (ErrorKind::Conflict, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "reentrancy_guard",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

/// Acquire the global reentrancy lock.
///
/// Returns [`ReentrancyError::ReentrantCall`] if the lock is already held,
/// which means an in-progress guarded section is being re-entered.
pub fn acquire(env: &Env) -> Result<(), ReentrancyError> {
    let locked: bool = env
        .storage()
        .instance()
        .get(&GuardKey::ReentrancyLock)
        .unwrap_or(false);
    if locked {
        return Err(ReentrancyError::ReentrantCall);
    }
    env.storage()
        .instance()
        .set(&GuardKey::ReentrancyLock, &true);
    Ok(())
}

/// Release the global reentrancy lock. Call only after a successful [`acquire`].
pub fn release(env: &Env) {
    env.storage()
        .instance()
        .set(&GuardKey::ReentrancyLock, &false);
}

/// Returns `true` while a guarded section is currently executing.
pub fn is_locked(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&GuardKey::ReentrancyLock)
        .unwrap_or(false)
}

/// Run `body` inside the reentrancy guard, releasing the lock afterwards.
///
/// The lock is released on both the success and error paths, so a guarded call
/// that returns an error never leaves the contract locked. Any error type that
/// can be built from [`ReentrancyError`] is supported, letting callers keep
/// their own domain error enum.
pub fn with_guard<T, E, F>(env: &Env, body: F) -> Result<T, E>
where
    F: FnOnce() -> Result<T, E>,
    E: From<ReentrancyError>,
{
    acquire(env).map_err(E::from)?;
    let result = body();
    release(env);
    result
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, Env};

    #[contract]
    struct GuardTestContract;

    #[contractimpl]
    impl GuardTestContract {
        /// Runs a guarded section that attempts to re-enter the guard from
        /// within itself — simulating a malicious cross-contract callback.
        pub fn reenter(env: Env) -> Result<u32, ReentrancyError> {
            with_guard(&env, || {
                // Second acquire while still inside the guard must be rejected.
                acquire(&env)?;
                Ok(0u32)
            })
        }

        /// A normal guarded call that does no re-entry.
        pub fn single(env: Env) -> Result<u32, ReentrancyError> {
            with_guard(&env, || Ok(42u32))
        }

        /// Exposes the lock flag so tests can assert it is released.
        pub fn locked(env: Env) -> bool {
            is_locked(&env)
        }
    }

    #[test]
    fn blocks_reentrant_call() {
        let env = Env::default();
        let id = env.register(GuardTestContract, ());
        let client = GuardTestContractClient::new(&env, &id);
        // The re-entrant attempt surfaces as a contract error.
        assert_eq!(client.try_reenter(), Err(Ok(ReentrancyError::ReentrantCall)));
    }

    #[test]
    fn allows_sequential_calls_and_releases_lock() {
        let env = Env::default();
        let id = env.register(GuardTestContract, ());
        let client = GuardTestContractClient::new(&env, &id);

        assert_eq!(client.single(), 42);
        // Lock must be released once the guarded call completes.
        assert_eq!(client.locked(), false);
        // A subsequent call still succeeds (the guard is not stuck).
        assert_eq!(client.single(), 42);
    }
}
