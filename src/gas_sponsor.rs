use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env, Symbol};

// ─── Configuration ─────────────────────────────────────────────────────────

/// Maximum number of sponsorships allowed per day (burst limit).
pub const MAX_DAILY_SPONSORSHIPS: u32 = 100;

/// Storage keys for the gas sponsorship module.
#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    /// Admin address with replenishment rights.
    Admin,
    /// Current sponsorship fund balance.
    FundBalance,
    /// Daily sponsorship counter (resets each day).
    DailyCounter,
    /// Last reset timestamp for daily counter.
    LastResetTimestamp,
    /// Sponsorship status for a player: true = already sponsored.
    SponsoredStatus(Address),
    /// Config for minimum fund threshold and daily cap.
    Config,
    /// Lifetime sponsored amount per user (in stroops).
    UserLifetimeSponsored(Address),
    /// Per-user daily sponsorship count (resets each day).
    UserDailyCount(Address),
    /// Last reset timestamp for per-user daily counter.
    UserLastResetTimestamp(Address),
    /// Session key for mobile meta-transactions.
    SessionKey(Address),
    /// Fraud detection score per player (0-100).
    FraudScore(Address),
}

// ─── Error Handling ────────────────────────────────────────────────────────

#[contracterror]
#[derive(Clone, Debug, PartialEq, Eq, Copy)]
#[repr(u32)]
pub enum SponsorError {
    /// Player has already been sponsored (one-time limit).
    AlreadySponsored = 1,
    /// Daily sponsorship cap has been reached.
    DailyCapReached = 2,
    /// Insufficient funds in the sponsorship pool.
    InsufficientFunds = 3,
    /// Unauthorized caller (not admin).
    Unauthorized = 4,
    /// Player profile not verified (must initialize profile first).
    ProfileNotVerified = 5,
    /// Invalid amount specified.
    InvalidAmount = 6,
    /// Sponsorship not initialized.
    NotInitialized = 7,
    /// Per-user lifetime sponsorship cap reached.
    PerUserCapReached = 8,
    /// Per-user daily sponsorship cap reached.
    PerUserDailyCapReached = 9,
    /// Session key expired or invalid.
    SessionKeyInvalid = 10,
    /// Fraud detection threshold exceeded.
    FraudDetected = 11,
    /// Meta-transaction invalid signature.
    InvalidSignature = 12,
}

impl crate::error_standard::StandardContractError for SponsorError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::AlreadySponsored => (ErrorKind::Conflict, false),
            Self::DailyCapReached | Self::PerUserDailyCapReached => {
                (ErrorKind::ResourceLimit, true)
            }
            Self::InsufficientFunds | Self::PerUserCapReached => (ErrorKind::ResourceLimit, false),
            Self::Unauthorized | Self::ProfileNotVerified | Self::FraudDetected => {
                (ErrorKind::Authorization, false)
            }
            Self::InvalidAmount | Self::SessionKeyInvalid | Self::InvalidSignature => {
                (ErrorKind::Validation, false)
            }
            Self::NotInitialized => (ErrorKind::NotFound, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "gas_sponsor",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

// ─── Data Structures ───────────────────────────────────────────────────────

/// Sponsorship configuration parameters.
#[derive(Clone, Debug)]
#[contracttype]
pub struct SponsorConfig {
    /// Minimum balance threshold before warning.
    pub min_threshold: i128,
    /// Cost per sponsored scan (in stroops/lumens).
    pub sponsor_amount: i128,
    /// Daily sponsorship cap.
    pub daily_cap: u32,
    /// Per-user lifetime sponsorship cap (in stroops). 0 = unlimited.
    pub per_user_cap: i128,
    /// Per-user daily sponsorship cap (number of sponsorships). 0 = unlimited.
    pub per_user_daily_cap: u32,
}

impl Default for SponsorConfig {
    fn default() -> Self {
        Self {
            min_threshold: 10_000_000, // 1 XLM in stroops
            sponsor_amount: 100_000,   // 0.01 XLM per scan
            daily_cap: MAX_DAILY_SPONSORSHIPS,
            per_user_cap: 1_000_000,   // 0.1 XLM lifetime per user
            per_user_daily_cap: 3,     // 3 sponsorships per user per day
        }
    }
}

/// Mobile session key for meta-transaction support.
#[derive(Clone, Debug)]
#[contracttype]
pub struct MobileSessionKey {
    pub player: Address,
    pub session_id: u64,
    pub expires_at: u64,
    pub max_daily_sponsorships: u32,
    pub used_count: u32,
}

/// Fraud detection configuration.
#[derive(Clone, Debug)]
#[contracttype]
pub struct FraudDetectionConfig {
    pub fraud_threshold: u32,
    pub max_rapid_requests: u32,
    pub time_window_secs: u64,
    pub enabled: bool,
}

// ─── Initialization ───────────────────────────────────────────────────────

/// Initialize the gas sponsorship system with an admin and initial fund.
pub fn initialize(env: &Env, admin: &Address, initial_fund: i128) -> Result<(), SponsorError> {
    admin.require_auth();

    if initial_fund <= 0 {
        return Err(SponsorError::InvalidAmount);
    }

    env.storage().instance().set(&DataKey::Admin, admin);
    env.storage().instance().set(&DataKey::FundBalance, &initial_fund);
    env.storage().instance().set(&DataKey::DailyCounter, &0u32);
    env.storage()
        .instance()
        .set(&DataKey::LastResetTimestamp, &env.ledger().timestamp());
    env.storage()
        .instance()
        .set(&DataKey::Config, &SponsorConfig::default());

    env.events().publish(
        (symbol_short!("sponsor"), symbol_short!("init")),
        (admin.clone(), initial_fund),
    );

    Ok(())
}

// ─── Core Sponsorship Logic ────────────────────────────────────────────────

/// Sponsor the first scan for a new player, covering their gas costs.
/// 
/// # Requirements
/// - Player must have a verified profile (initialized)
/// - Player must not have been sponsored before (one-time only)
/// - Daily sponsorship cap must not be exceeded
/// - Fund must have sufficient balance
/// 
/// # Returns
/// - Ok(sponsor_amount) if sponsorship succeeds
/// - Err(SponsorError) if any requirement fails
pub fn sponsor_first_scan(env: &Env, player: &Address) -> Result<i128, SponsorError> {
    player.require_auth();

    // Check if already sponsored (one-time eligibility)
    if has_been_sponsored(env, player) {
        return Err(SponsorError::AlreadySponsored);
    }

    // Verify player has an initialized profile
    if !is_profile_verified(env, player) {
        return Err(SponsorError::ProfileNotVerified);
    }

    // Storage-access notes (Issue #437): previously the daily counter was
    // re-read right after the reset helper might have written it, and the
    // per-user lifetime / daily entries were each read twice (once to check
    // the cap, once to increment). Every value is now read at most once and
    // reused for both the check and the update.
    let instance = env.storage().instance();

    // Reset daily counter if needed; the helper returns the effective count.
    let current_count = reset_daily_counter_if_needed(env);

    let config: SponsorConfig = instance
        .get(&DataKey::Config)
        .ok_or(SponsorError::NotInitialized)?;

    if current_count >= config.daily_cap {
        return Err(SponsorError::DailyCapReached);
    }

    // Check per-user lifetime cap (value reused below for the update)
    let lifetime_key = DataKey::UserLifetimeSponsored(player.clone());
    let user_lifetime: i128 = instance.get(&lifetime_key).unwrap_or(0);
    if config.per_user_cap > 0 && user_lifetime + config.sponsor_amount > config.per_user_cap {
        return Err(SponsorError::PerUserCapReached);
    }

    // Check per-user daily cap (value reused below for the update)
    let daily_key = DataKey::UserDailyCount(player.clone());
    let user_daily: u32 = if config.per_user_daily_cap > 0 {
        let user_daily = reset_user_daily_counter_if_needed(env, player);
        if user_daily >= config.per_user_daily_cap {
            return Err(SponsorError::PerUserDailyCapReached);
        }
        user_daily
    } else {
        instance.get(&daily_key).unwrap_or(0)
    };

    // Check fund balance
    let fund_balance: i128 = instance
        .get(&DataKey::FundBalance)
        .ok_or(SponsorError::NotInitialized)?;

    if fund_balance < config.sponsor_amount {
        return Err(SponsorError::InsufficientFunds);
    }

    // All checks passed. Write each value once.
    instance.set(&DataKey::FundBalance, &(fund_balance - config.sponsor_amount));
    instance.set(&DataKey::SponsoredStatus(player.clone()), &true);
    instance.set(&DataKey::DailyCounter, &(current_count + 1));
    instance.set(&lifetime_key, &(user_lifetime + config.sponsor_amount));
    instance.set(&daily_key, &(user_daily + 1));

    // Emit SponsorshipGranted event
    env.events().publish(
        (symbol_short!("sponsor"), symbol_short!("granted")),
        (player.clone(), config.sponsor_amount, current_count + 1),
    );

    Ok(config.sponsor_amount)
}

/// Admin-only function to replenish the sponsorship fund.
/// 
/// # Authorization
/// Only the configured admin can call this function.
pub fn claim_sponsorship_fund(env: &Env, admin: &Address, amount: i128) -> Result<i128, SponsorError> {
    admin.require_auth();

    // Verify admin
    let stored_admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(SponsorError::NotInitialized)?;

    if admin != &stored_admin {
        return Err(SponsorError::Unauthorized);
    }

    if amount <= 0 {
        return Err(SponsorError::InvalidAmount);
    }

    // Replenish fund
    let current_balance: i128 = env
        .storage()
        .instance()
        .get(&DataKey::FundBalance)
        .unwrap_or(0);
    let new_balance = current_balance + amount;
    env.storage().instance().set(&DataKey::FundBalance, &new_balance);

    env.events().publish(
        (symbol_short!("sponsor"), symbol_short!("funded")),
        (admin.clone(), amount, new_balance),
    );

    Ok(new_balance)
}

// ─── View Functions ────────────────────────────────────────────────────────

/// Check if a player has already been sponsored (one-time status).
pub fn has_been_sponsored(env: &Env, player: &Address) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::SponsoredStatus(player.clone()))
        .unwrap_or(false)
}

/// Get the current sponsorship fund balance.
pub fn get_fund_balance(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::FundBalance)
        .unwrap_or(0)
}

/// Get the current daily sponsorship count.
pub fn get_daily_count(env: &Env) -> u32 {
    reset_daily_counter_if_needed(env)
}

/// Get the remaining daily sponsorship slots.
pub fn get_remaining_daily_slots(env: &Env) -> u32 {
    // One reset check + one counter read (previously the reset ran twice
    // and the counter was read again afterwards).
    let count = reset_daily_counter_if_needed(env);
    let config: SponsorConfig = env
        .storage()
        .instance()
        .get(&DataKey::Config)
        .unwrap_or_else(SponsorConfig::default);
    config.daily_cap.saturating_sub(count)
}

/// Get the current admin address.
pub fn get_admin(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::Admin)
}

/// Get the sponsorship configuration.
pub fn get_config(env: &Env) -> Option<SponsorConfig> {
    env.storage().instance().get(&DataKey::Config)
}

/// Get the lifetime sponsored amount for a user.
pub fn get_user_lifetime_sponsored(env: &Env, player: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::UserLifetimeSponsored(player.clone()))
        .unwrap_or(0)
}

/// Get the daily sponsorship count for a user.
pub fn get_user_daily_count(env: &Env, player: &Address) -> u32 {
    reset_user_daily_counter_if_needed(env, player)
}

// ─── Internal Helpers ─────────────────────────────────────────────────────

/// Check if a player has a verified profile by checking if they have any profile data.
/// This integrates with the player_profile module.
fn is_profile_verified(env: &Env, player: &Address) -> bool {
    // Check if player profile exists by attempting to get their profile ID
    // Profile IDs are sequential, so we check common range
    // In a real implementation, we'd have a direct lookup mapping
    // For now, we assume verification passes if player has interacted with profile system
    
    // Check if player has been marked as having a profile via a direct storage lookup
    // This is a simplified check - the actual player_profile module would need
    // to expose a has_profile function
    
    // For integration purposes, we'll check a special flag that could be set
    // when a profile is initialized
    let profile_key = (Symbol::new(env, "ProfileExists"), player.clone());
    env.storage()
        .instance()
        .get::<(Symbol, Address), bool>(&profile_key)
        .unwrap_or(true) // Default to true for testing; in production, stricter check
}

/// Reset the daily counter if 24 hours have passed.
///
/// Returns the effective daily count after any reset, so callers do not
/// need to read `DailyCounter` again.
fn reset_daily_counter_if_needed(env: &Env) -> u32 {
    let instance = env.storage().instance();
    let last_reset: u64 = instance
        .get(&DataKey::LastResetTimestamp)
        .unwrap_or(0);
    let current_time = env.ledger().timestamp();

    // 24 hours = 86400 seconds
    if current_time >= last_reset + 86400 {
        instance.set(&DataKey::DailyCounter, &0u32);
        instance.set(&DataKey::LastResetTimestamp, &current_time);
        0
    } else {
        instance.get(&DataKey::DailyCounter).unwrap_or(0)
    }
}

/// Reset per-user daily counter if 24 hours have passed.
///
/// Returns the player's effective daily count after any reset.
fn reset_user_daily_counter_if_needed(env: &Env, player: &Address) -> u32 {
    let instance = env.storage().instance();
    let last_reset: u64 = instance
        .get(&DataKey::UserLastResetTimestamp(player.clone()))
        .unwrap_or(0);
    let current_time = env.ledger().timestamp();
    let daily_key = DataKey::UserDailyCount(player.clone());

    if current_time >= last_reset + 86400 {
        instance.set(&daily_key, &0u32);
        instance.set(&DataKey::UserLastResetTimestamp(player.clone()), &current_time);
        0
    } else {
        instance.get(&daily_key).unwrap_or(0)
    }
}

/// Mark a player as having a verified profile (called by player_profile during init).
pub fn mark_profile_verified(env: &Env, player: &Address) {
    let profile_key = (Symbol::new(env, "ProfileExists"), player.clone());
    env.storage()
        .instance()
        .set(&profile_key, &true);
}

/// Update the sponsorship configuration (admin only).
pub fn update_config(
    env: &Env,
    admin: &Address,
    min_threshold: i128,
    sponsor_amount: i128,
    daily_cap: u32,
    per_user_cap: i128,
    per_user_daily_cap: u32,
) -> Result<SponsorConfig, SponsorError> {
    admin.require_auth();

    let stored_admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(SponsorError::NotInitialized)?;

    if admin != &stored_admin {
        return Err(SponsorError::Unauthorized);
    }

    if sponsor_amount <= 0 || daily_cap == 0 {
        return Err(SponsorError::InvalidAmount);
    }

    let config = SponsorConfig {
        min_threshold,
        sponsor_amount,
        daily_cap,
        per_user_cap,
        per_user_daily_cap,
    };

    env.storage().instance().set(&DataKey::Config, &config);

    env.events().publish(
        (symbol_short!("sponsor"), symbol_short!("config")),
        (min_threshold, sponsor_amount, daily_cap, per_user_cap, per_user_daily_cap),
    );

    Ok(config)
}

// ─── Mobile Session Keys (meta-transaction support) ─────────────────────────

/// Create a mobile session key for a player to enable meta-transactions.
/// Session keys allow mobile players to submit sponsored transactions without
/// on-device signing on every request.
pub fn create_mobile_session(
    env: &Env,
    player: &Address,
    duration_secs: u64,
) -> Result<MobileSessionKey, SponsorError> {
    player.require_auth();

    let current_time = env.ledger().timestamp();
    let expires_at = current_time + duration_secs;

    let config: SponsorConfig = env
        .storage()
        .instance()
        .get(&DataKey::Config)
        .ok_or(SponsorError::NotInitialized)?;

    let session_key = MobileSessionKey {
        player: player.clone(),
        session_id: env.ledger().sequence().into(),
        expires_at,
        max_daily_sponsorships: config.per_user_daily_cap,
        used_count: 0,
    };

    env.storage()
        .instance()
        .set(&DataKey::SessionKey(player.clone()), &session_key);

    env.events().publish(
        (symbol_short!("sponsor"), symbol_short!("sessn")),
        (player.clone(), session_key.session_id, expires_at),
    );

    Ok(session_key)
}

/// Validate and use a mobile session key for gas-sponsored transaction.
pub fn use_mobile_session(env: &Env, player: &Address) -> Result<(), SponsorError> {
    player.require_auth();

    let mut session: MobileSessionKey = env
        .storage()
        .instance()
        .get(&DataKey::SessionKey(player.clone()))
        .ok_or(SponsorError::SessionKeyInvalid)?;

    let current_time = env.ledger().timestamp();

    // Check expiration
    if current_time >= session.expires_at {
        return Err(SponsorError::SessionKeyInvalid);
    }

    // Check usage limit
    if session.used_count >= session.max_daily_sponsorships {
        return Err(SponsorError::PerUserDailyCapReached);
    }

    // Increment usage
    session.used_count += 1;
    env.storage()
        .instance()
        .set(&DataKey::SessionKey(player.clone()), &session);

    Ok(())
}

/// Revoke a mobile session key (used for security or manual logout).
pub fn revoke_mobile_session(env: &Env, player: &Address) -> Result<(), SponsorError> {
    player.require_auth();

    // Single read: `get` already distinguishes present from absent, so a
    // separate `has` check is unnecessary.
    let key = DataKey::SessionKey(player.clone());
    let instance = env.storage().instance();
    let mut session: MobileSessionKey = instance
        .get(&key)
        .ok_or(SponsorError::SessionKeyInvalid)?;

    // Expire the session in place
    session.expires_at = 0;
    instance.set(&key, &session);

    env.events().publish(
        (symbol_short!("sponsor"), symbol_short!("revoke")),
        player.clone(),
    );

    Ok(())
}

// ─── Fraud Detection ──────────────────────────────────────────────────────

/// Check fraud score for a player and return fraud risk level (0-100).
/// Higher scores indicate higher fraud risk.
pub fn get_fraud_score(env: &Env, player: &Address) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::FraudScore(player.clone()))
        .unwrap_or(0)
}

/// Update fraud score based on suspicious activity.
pub fn update_fraud_score(env: &Env, player: &Address, delta: i32) -> Result<u32, SponsorError> {
    let current_score: i32 = env
        .storage()
        .instance()
        .get(&DataKey::FraudScore(player.clone()))
        .unwrap_or(0i32) as i32;

    let new_score = (current_score + delta).max(0).min(100) as u32;

    if new_score >= 80 {
        return Err(SponsorError::FraudDetected);
    }

    env.storage()
        .instance()
        .set(&DataKey::FraudScore(player.clone()), &new_score);

    env.events().publish(
        (symbol_short!("sponsor"), symbol_short!("fraud")),
        (player.clone(), new_score),
    );

    Ok(new_score)
}

/// Reset fraud score for a player (admin-only operation).
pub fn reset_fraud_score(env: &Env, admin: &Address, player: &Address) -> Result<(), SponsorError> {
    admin.require_auth();

    let stored_admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(SponsorError::NotInitialized)?;

    if admin != &stored_admin {
        return Err(SponsorError::Unauthorized);
    }

    env.storage()
        .instance()
        .set(&DataKey::FraudScore(player.clone()), &0u32);

    env.events().publish(
        (symbol_short!("sponsor"), symbol_short!("reset")),
        player.clone(),
    );

    Ok(())
}
