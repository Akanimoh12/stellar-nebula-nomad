use soroban_sdk::{contracttype, contracterror, symbol_short, Address, Env};

/// Session time-to-live: 24 hours in seconds.
pub const SESSION_TTL: u64 = 86_400;
/// Maximum concurrent active sessions per player.
pub const MAX_SESSIONS_PER_PLAYER: u32 = 3;

// ─── Storage Keys ─────────────────────────────────────────────────────────────

#[derive(Clone)]
#[contracttype]
pub enum SessionKey {
    /// Individual session data keyed by session ID.
    Session(u64),
    /// Active session count for a player (enforces 3-session cap).
    PlayerSessionCount(Address),
    /// Global auto-increment counter for session IDs.
    SessionCount,
}

// ─── Data Types ───────────────────────────────────────────────────────────────

/// A timed nebula exploration session tied to a ship.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct Session {
    pub id: u64,
    pub ship_id: u64,
    pub owner: Address,
    pub started_at: u64,
    pub expires_at: u64,
    pub active: bool,
}

// ─── Errors ───────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SessionError {
    SessionNotFound = 1,
    SessionExpired = 2,
    TooManySessions = 3,
    NotOwner = 4,
}

impl crate::error_standard::StandardContractError for SessionError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::SessionNotFound => (ErrorKind::NotFound, false),
            Self::SessionExpired => (ErrorKind::Conflict, false),
            Self::TooManySessions => (ErrorKind::ResourceLimit, false),
            Self::NotOwner => (ErrorKind::Authorization, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "session_manager",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

// ─── Functions ────────────────────────────────────────────────────────────────

/// Start a timed nebula exploration session for `owner` using `ship_id`.
///
/// Enforces a cap of `MAX_SESSIONS_PER_PLAYER` concurrent active sessions.
/// TTL is pulled from the `SESSION_TTL` constant (24 h). Emits `SessionStarted`.
pub fn start_session(env: &Env, owner: Address, ship_id: u64) -> Result<u64, SessionError> {
    owner.require_auth();

    let count_key = SessionKey::PlayerSessionCount(owner.clone());
    let active_count: u32 = env
        .storage()
        .persistent()
        .get(&count_key)
        .unwrap_or(0u32);

    if active_count >= MAX_SESSIONS_PER_PLAYER {
        return Err(SessionError::TooManySessions);
    }

    let id: u64 = env
        .storage()
        .instance()
        .get(&SessionKey::SessionCount)
        .unwrap_or(0u64)
        + 1;
    env.storage().instance().set(&SessionKey::SessionCount, &id);

    let now = env.ledger().timestamp();
    let session = Session {
        id,
        ship_id,
        owner: owner.clone(),
        started_at: now,
        expires_at: now + SESSION_TTL,
        active: true,
    };

    env.storage()
        .persistent()
        .set(&SessionKey::Session(id), &session);
    env.storage()
        .persistent()
        .set(&count_key, &(active_count + 1));

    env.events().publish(
        (symbol_short!("session"), symbol_short!("started")),
        (owner, id, ship_id),
    );

    Ok(id)
}

/// Close a session. Anyone may close a session that has passed its TTL.
/// Only the owner may force-close an active session.
///
/// Decrements the player's active session counter. Emits `SessionExpired`.
pub fn expire_session(env: &Env, caller: Address, session_id: u64) -> Result<(), SessionError> {
    caller.require_auth();

    let mut session: Session = env
        .storage()
        .persistent()
        .get(&SessionKey::Session(session_id))
        .ok_or(SessionError::SessionNotFound)?;

    if !session.active {
        return Err(SessionError::SessionExpired);
    }

    let now = env.ledger().timestamp();
    // Owner can close any time; others only after TTL has elapsed.
    if session.owner != caller && session.expires_at > now {
        return Err(SessionError::NotOwner);
    }

    session.active = false;
    env.storage()
        .persistent()
        .set(&SessionKey::Session(session_id), &session);

    // Decrement active session counter for the owner.
    let count_key = SessionKey::PlayerSessionCount(session.owner.clone());
    let count: u32 = env
        .storage()
        .persistent()
        .get(&count_key)
        .unwrap_or(0u32);
    if count > 0 {
        env.storage().persistent().set(&count_key, &(count - 1));
    }

    env.events().publish(
        (symbol_short!("session"), symbol_short!("expired")),
        (caller, session_id),
    );

    Ok(())
}

/// Retrieve session data by ID.
pub fn get_session(env: &Env, session_id: u64) -> Result<Session, SessionError> {
    env.storage()
        .persistent()
        .get(&SessionKey::Session(session_id))
        .ok_or(SessionError::SessionNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        contract, contractimpl,
        testutils::{Address as _, Ledger, LedgerInfo},
    };

    #[contract]
    struct Stub;
    #[contractimpl]
    impl Stub {}

    fn make_env() -> (Env, Address) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            protocol_version: 22,
            sequence_number: 100,
            timestamp: 1_000_000,
            network_id: [0u8; 32],
            base_reserve: 10,
            min_temp_entry_ttl: 100,
            min_persistent_entry_ttl: 1000,
            max_entry_ttl: 10_000,
        });
        let id = env.register(Stub, ());
        (env, id)
    }

    #[test]
    fn test_get_session_missing_returns_not_found() {
        let (env, contract_id) = make_env();
        env.as_contract(&contract_id, || {
            assert_eq!(get_session(&env, 999), Err(SessionError::SessionNotFound));
        });
    }

    #[test]
    fn test_start_session_enforces_max_concurrent_cap() {
        let (env, contract_id) = make_env();
        let owner = Address::generate(&env);
        env.as_contract(&contract_id, || {
            for _ in 0..MAX_SESSIONS_PER_PLAYER {
                start_session(&env, owner.clone(), 1).unwrap();
            }
            let result = start_session(&env, owner.clone(), 1);
            assert_eq!(result, Err(SessionError::TooManySessions));
        });
    }

    #[test]
    fn test_expire_session_by_non_owner_before_ttl_fails() {
        let (env, contract_id) = make_env();
        let owner = Address::generate(&env);
        let stranger = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let id = start_session(&env, owner, 1).unwrap();
            let result = expire_session(&env, stranger, id);
            assert_eq!(result, Err(SessionError::NotOwner));
        });
    }

    #[test]
    fn test_expire_session_by_non_owner_after_ttl_succeeds() {
        let (env, contract_id) = make_env();
        let owner = Address::generate(&env);
        let stranger = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let id = start_session(&env, owner, 1).unwrap();
            env.ledger().with_mut(|li| {
                li.timestamp += SESSION_TTL + 1;
            });
            expire_session(&env, stranger, id).unwrap();
            let session = get_session(&env, id).unwrap();
            assert!(!session.active);
        });
    }

    #[test]
    fn test_expire_session_twice_fails_second_time() {
        let (env, contract_id) = make_env();
        let owner = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let id = start_session(&env, owner.clone(), 1).unwrap();
            expire_session(&env, owner.clone(), id).unwrap();
            let result = expire_session(&env, owner, id);
            assert_eq!(result, Err(SessionError::SessionExpired));
        });
    }

    #[test]
    fn test_expire_session_frees_slot_for_new_session() {
        let (env, contract_id) = make_env();
        let owner = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let id = start_session(&env, owner.clone(), 1).unwrap();
            for _ in 0..(MAX_SESSIONS_PER_PLAYER - 1) {
                start_session(&env, owner.clone(), 1).unwrap();
            }
            expire_session(&env, owner.clone(), id).unwrap();
            // Slot freed, so one more session should now succeed.
            start_session(&env, owner, 1).unwrap();
        });
    }
}
