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
//!
//! ## Protection pattern (Issue #472)
//!
//! Every value-moving entry point follows the same shape:
//!
//! 1. **Auth** — `require_auth` for the acting address.
//! 2. **Lock** — [`with_guard`] acquires the contract-wide mutex. The lock is
//!    global rather than per-function, so it also stops *cross-function*
//!    reentrancy (e.g. a claim callback that tries to dissolve the bond it is
//!    claiming from).
//! 3. **Checks → Effects → Interactions** — inside the lock, validate inputs
//!    and state, write every balance/status change, and only then publish
//!    events or call out. A nested call therefore never observes half-applied
//!    state, and even if it could bypass the lock, the state it reads is
//!    already final (e.g. a vault is marked `claimed` before the payout is
//!    reported).
//!
//! Guarded entry points that compose each other must not nest [`with_guard`]:
//! the second `acquire` would reject the caller's own call. Each guarded
//! `pub fn foo` is therefore a thin wrapper around a private `foo_unguarded`
//! body, and composite paths (e.g.
//! `dex_integration::harvest_and_list` → `resource_minter::harvest_resources`)
//! call the unguarded body while already holding the lock.
//!
//! Guarded entry points:
//!
//! | Module | Functions |
//! |---|---|
//! | `resource_minter` | `mint_resource`, `harvest_resources`, `auto_list_on_dex` |
//! | `dex_integration` | `harvest_and_list`, `cancel_listing` (and `list_at_market` via `harvest_and_list`) |
//! | `trading` | `place_limit_order`, `cancel_limit_order`, `record_trade`, `add_liquidity`, `remove_liquidity`, `swap_exact_input` |
//! | `escrow_trader` | `complete_escrow`, `cancel_escrow` |
//! | `nomad_bonding` | `delegate_yield`, `claim_yield`, `dissolve_bond` |
//! | `treasure_vault` | `deposit_treasure`, `claim_treasure` |
//!
//! Low-level ledger primitives (`resource_minter::credit_balance`,
//! `debit_balance`, `move_balance`, …) are deliberately *not* guarded: they
//! are only reachable from other modules' already-guarded or auth-checked
//! paths, and guarding them would make those callers self-block.

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

        /// A guarded section that calls a *different* guarded section —
        /// cross-function reentrancy, e.g. a claim callback trying to
        /// dissolve the bond it is claiming from.
        pub fn cross(env: Env) -> Result<u32, ReentrancyError> {
            with_guard(&env, || Self::single(env.clone()))
        }

        /// A guarded section whose body fails. The lock must still be
        /// released so the contract is not left permanently locked.
        pub fn fail(env: Env) -> Result<u32, ReentrancyError> {
            with_guard(&env, || Err(ReentrancyError::ReentrantCall))
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

    #[test]
    fn blocks_cross_function_reentry() {
        let env = Env::default();
        let id = env.register(GuardTestContract, ());
        let client = GuardTestContractClient::new(&env, &id);

        assert_eq!(client.try_cross(), Err(Ok(ReentrancyError::ReentrantCall)));
        // The outer failure rolled back cleanly; unrelated calls still work.
        assert!(!client.locked());
        assert_eq!(client.single(), 42);
    }

    #[test]
    fn releases_lock_when_body_errors() {
        let env = Env::default();
        let id = env.register(GuardTestContract, ());

        env.as_contract(&id, || {
            let result: Result<u32, ReentrancyError> =
                with_guard(&env, || Err(ReentrancyError::ReentrantCall));
            assert!(result.is_err());
            assert!(!is_locked(&env));
        });

        let client = GuardTestContractClient::new(&env, &id);
        assert!(client.try_fail().is_err());
        assert!(!client.locked());
        assert_eq!(client.single(), 42);
    }

    #[test]
    fn acquire_release_round_trip() {
        let env = Env::default();
        let id = env.register(GuardTestContract, ());

        env.as_contract(&id, || {
            assert!(!is_locked(&env));
            acquire(&env).expect("lock should be free");
            assert!(is_locked(&env));
            assert_eq!(acquire(&env), Err(ReentrancyError::ReentrantCall));
            release(&env);
            assert!(!is_locked(&env));
        });
    }
}
