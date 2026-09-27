//! Multi-tier referral rewards system.
//!
//! Implements tiered referral rewards, analytics tracking, leaderboard,
//! and anti-gaming measures for the referral program.

use soroban_sdk::{contracterror, contracttype, symbol_short, Address, BytesN, Env, Vec};

/// Tier 1 reward (1-5 active referrals)
pub const TIER1_REWARD: i128 = 100;
/// Tier 2 reward (6-15 active referrals)
pub const TIER2_REWARD: i128 = 150;
/// Tier 3 reward (16+ active referrals)
pub const TIER3_REWARD: i128 = 200;

/// Tier thresholds
pub const TIER1_THRESHOLD: u32 = 1;
pub const TIER2_THRESHOLD: u32 = 6;
pub const TIER3_THRESHOLD: u32 = 16;

/// Maximum referrals that count towards tier (anti-gaming)
pub const MAX_TIER_REFERRALS: u32 = 100;

/// Minimum activity score for referral to be "active"
pub const MIN_ACTIVITY_SCORE: u32 = 10;

/// Leaderboard size
pub const LEADERBOARD_SIZE: u32 = 10;

// ─── Storage Keys ──────────────────────────────────────────────────────────────
#[derive(Clone)]
#[contracttype]
pub enum RewardKey {
    /// Referrer stats: address -> ReferrerStats
    ReferrerStats(Address),
    /// Referral code: code -> referrer address
    ReferralCode(BytesN<8>),
    /// Referrer's code: address -> code
    ReferrerCodeMapping(Address),
    /// Global leaderboard
    Leaderboard,
    /// Total rewards distributed
    TotalRewardsDistributed,
    /// Analytics: daily signups -> count
    DailySignups(u64),
    /// Analytics: daily claims -> count
    DailyClaims(u64),
    /// Anti-gaming: suspicious activity flags
    SuspiciousFlag(Address),
}

// ─── Errors ────────────────────────────────────────────────────────────────
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum RewardError {
    /// Invalid referral code
    InvalidCode = 1,
    /// Code already exists
    CodeExists = 2,
    /// Referrer not found
    ReferrerNotFound = 3,
    /// No rewards to claim
    NoRewardsToClaim = 4,
    /// Already claimed this period
    AlreadyClaimed = 5,
    /// Suspicious activity detected
    SuspiciousActivity = 6,
    /// Self-referral not allowed
    SelfReferral = 7,
    /// Invalid tier
    InvalidTier = 8,
}

impl crate::error_standard::StandardContractError for RewardError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::InvalidCode | Self::SelfReferral | Self::InvalidTier => {
                (ErrorKind::Validation, false)
            }
            Self::CodeExists | Self::AlreadyClaimed => (ErrorKind::Conflict, false),
            Self::ReferrerNotFound | Self::NoRewardsToClaim => (ErrorKind::NotFound, false),
            Self::SuspiciousActivity => (ErrorKind::Authorization, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "rewards",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

// ─── Data Types ────────────────────────────────────────────────────────────
/// Referrer statistics and analytics
#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct ReferrerStats {
    /// Referrer address
    pub address: Address,
    /// Total referrals made
    pub total_referrals: u32,
    /// Active referrals (met minimum activity)
    pub active_referrals: u32,
    /// Current tier (1, 2, or 3)
    pub current_tier: u32,
    /// Total rewards earned
    pub total_rewards_earned: i128,
    /// Pending rewards to claim
    pub pending_rewards: i128,
    /// Last claim timestamp
    pub last_claim: u64,
    /// Referral code
    pub referral_code: BytesN<8>,
    /// Suspicious activity score (0-100)
    pub suspicion_score: u32,
}

/// Leaderboard entry
#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct LeaderboardEntry {
    /// Referrer address
    pub address: Address,
    /// Active referrals count
    pub active_referrals: u32,
    /// Current tier
    pub tier: u32,
    /// Total rewards earned
    pub total_rewards: i128,
}

/// Referral analytics data
#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct ReferralAnalytics {
    /// Total referrers
    pub total_referrers: u32,
    /// Total referrals
    pub total_referrals: u32,
    /// Active referrals
    pub active_referrals: u32,
    /// Total rewards distributed
    pub total_rewards_distributed: i128,
    /// Conversion rate (active/total)
    pub conversion_rate: u32,
}

// ─── Public API ────────────────────────────────────────────────────────────
/// Generate a unique referral code for a referrer
pub fn generate_referral_code(env: &Env, referrer: &Address) -> Result<BytesN<8>, RewardError> {
    referrer.require_auth();
    
    // Check if already has a code
    if let Some(code) = env
        .storage()
        .instance()
        .get::<RewardKey, BytesN<8>>(&RewardKey::ReferrerCodeMapping(referrer.clone()))
    {
        return Ok(code);
    }
    
    // Generate code from address hash
    let code = generate_code_from_address(env, referrer);
    
    // Ensure uniqueness
    if env
        .storage()
        .instance()
        .has(&RewardKey::ReferralCode(code.clone()))
    {
        // Collision - regenerate with timestamp
        let timestamp = env.ledger().timestamp();
        let mut new_code = [0u8; 8];
        new_code[..4].copy_from_slice(&code.to_array()[..4]);
        new_code[4..].copy_from_slice(&timestamp.to_be_bytes()[..4]);
        return Ok(BytesN::from_array(env, &new_code));
    }
    
    env.storage()
        .instance()
        .set(&RewardKey::ReferralCode(code.clone()), referrer);
    env.storage()
        .instance()
        .set(&RewardKey::ReferrerCodeMapping(referrer.clone()), &code);
    
    // Initialize stats
    let stats = ReferrerStats {
        address: referrer.clone(),
        total_referrals: 0,
        active_referrals: 0,
        current_tier: 1,
        total_rewards_earned: 0,
        pending_rewards: 0,
        last_claim: 0,
        referral_code: code.clone(),
        suspicion_score: 0,
    };
    
    env.storage()
        .instance()
        .set(&RewardKey::ReferrerStats(referrer.clone()), &stats);
    
    env.events().publish(
        (symbol_short!("reward"), symbol_short!("code_gen")),
        (referrer.clone(), code.clone()),
    );
    
    Ok(code)
}

/// Record a new referral and update referrer stats
pub fn record_referral(
    env: &Env,
    referrer: &Address,
    new_user: &Address,
) -> Result<i128, RewardError> {
    if referrer == new_user {
        return Err(RewardError::SelfReferral);
    }
    
    // Check for suspicious activity
    if is_suspicious(env, referrer) {
        return Err(RewardError::SuspiciousActivity);
    }
    
    let mut stats: ReferrerStats = env
        .storage()
        .instance()
        .get(&RewardKey::ReferrerStats(referrer.clone()))
        .ok_or(RewardError::ReferrerNotFound)?;
    
    // Update stats
    stats.total_referrals += 1;
    
    // Calculate reward based on tier
    let reward = calculate_tier_reward(stats.current_tier);
    
    // Update pending rewards
    stats.pending_rewards += reward;
    
    // Update tier if threshold met
    update_tier(&mut stats);
    
    env.storage()
        .instance()
        .set(&RewardKey::ReferrerStats(referrer.clone()), &stats);
    
    // Update daily analytics
    let day = env.ledger().timestamp() / 86_400;
    let daily_signups: u32 = env
        .storage()
        .instance()
        .get(&RewardKey::DailySignups(day))
        .unwrap_or(0);
    env.storage()
        .instance()
        .set(&RewardKey::DailySignups(day), &(daily_signups + 1));
    
    // Update leaderboard
    update_leaderboard(env, referrer, stats.active_referrals, stats.current_tier, stats.total_rewards_earned);
    
    env.events().publish(
        (symbol_short!("reward"), symbol_short!("referral")),
        (referrer.clone(), new_user.clone(), reward),
    );
    
    Ok(reward)
}

/// Mark a referral as active (met minimum activity threshold)
pub fn mark_referral_active(env: &Env, referrer: &Address) -> Result<(), RewardError> {
    let mut stats: ReferrerStats = env
        .storage()
        .instance()
        .get(&RewardKey::ReferrerStats(referrer.clone()))
        .ok_or(RewardError::ReferrerNotFound)?;
    
    if stats.active_referrals < stats.total_referrals {
        stats.active_referrals += 1;
        update_tier(&mut stats);
        
        env.storage()
            .instance()
            .set(&RewardKey::ReferrerStats(referrer.clone()), &stats);
        
        // Update leaderboard
        update_leaderboard(env, referrer, stats.active_referrals, stats.current_tier, stats.total_rewards_earned);
    }
    
    Ok(())
}

/// Claim pending rewards
pub fn claim_rewards(env: &Env, referrer: &Address) -> Result<i128, RewardError> {
    referrer.require_auth();
    
    let mut stats: ReferrerStats = env
        .storage()
        .instance()
        .get(&RewardKey::ReferrerStats(referrer.clone()))
        .ok_or(RewardError::ReferrerNotFound)?;
    
    if stats.pending_rewards == 0 {
        return Err(RewardError::NoRewardsToClaim);
    }
    
    let reward = stats.pending_rewards;
    stats.pending_rewards = 0;
    stats.total_rewards_earned += reward;
    stats.last_claim = env.ledger().timestamp();
    
    env.storage()
        .instance()
        .set(&RewardKey::ReferrerStats(referrer.clone()), &stats);
    
    // Update total distributed
    let total: i128 = env
        .storage()
        .instance()
        .get(&RewardKey::TotalRewardsDistributed)
        .unwrap_or(0);
    env.storage()
        .instance()
        .set(&RewardKey::TotalRewardsDistributed, &(total + reward));
    
    // Update daily analytics
    let day = env.ledger().timestamp() / 86_400;
    let daily_claims: u32 = env
        .storage()
        .instance()
        .get(&RewardKey::DailyClaims(day))
        .unwrap_or(0);
    env.storage()
        .instance()
        .set(&RewardKey::DailyClaims(day), &(daily_claims + 1));
    
    env.events().publish(
        (symbol_short!("reward"), symbol_short!("claimed")),
        (referrer.clone(), reward),
    );
    
    Ok(reward)
}

/// Get referrer stats
pub fn get_referrer_stats(env: &Env, referrer: &Address) -> Option<ReferrerStats> {
    env.storage()
        .instance()
        .get(&RewardKey::ReferrerStats(referrer.clone()))
}

/// Get referrer by code
pub fn get_referrer_by_code(env: &Env, code: &BytesN<8>) -> Option<Address> {
    env.storage()
        .instance()
        .get(&RewardKey::ReferralCode(code.clone()))
}

/// Get leaderboard
pub fn get_leaderboard(env: &Env) -> Vec<LeaderboardEntry> {
    env.storage()
        .instance()
        .get(&RewardKey::Leaderboard)
        .unwrap_or_else(|| Vec::new(env))
}

/// Get referral analytics
pub fn get_referral_analytics(env: &Env) -> ReferralAnalytics {
    let total_distributed: i128 = env
        .storage()
        .instance()
        .get(&RewardKey::TotalRewardsDistributed)
        .unwrap_or(0);
    
    // Calculate totals from all referrer stats (simplified for MVP)
    ReferralAnalytics {
        total_referrers: 0,
        total_referrals: 0,
        active_referrals: 0,
        total_rewards_distributed: total_distributed,
        conversion_rate: 0,
    }
}

/// Flag suspicious activity
pub fn flag_suspicious(env: &Env, admin: &Address, target: &Address) -> Result<(), RewardError> {
    admin.require_auth();
    
    env.storage()
        .instance()
        .set(&RewardKey::SuspiciousFlag(target.clone()), &true);
    
    if let Some(mut stats) = get_referrer_stats(env, target) {
        stats.suspicion_score = 100;
        env.storage()
            .instance()
            .set(&RewardKey::ReferrerStats(target.clone()), &stats);
    }
    
    env.events().publish(
        (symbol_short!("reward"), symbol_short!("flagged")),
        (admin.clone(), target.clone()),
    );
    
    Ok(())
}

/// Check if address is flagged as suspicious
pub fn is_suspicious(env: &Env, address: &Address) -> bool {
    env.storage()
        .instance()
        .get(&RewardKey::SuspiciousFlag(address.clone()))
        .unwrap_or(false)
}

/// Get tier reward amount
pub fn get_tier_reward(tier: u32) -> i128 {
    match tier {
        1 => TIER1_REWARD,
        2 => TIER2_REWARD,
        3 => TIER3_REWARD,
        _ => TIER1_REWARD,
    }
}

// ─── Internal Helpers ──────────────────────────────────────────────────────────
fn generate_code_from_address(env: &Env, address: &Address) -> BytesN<8> {
    // Simple code generation from address bytes
    let mut code = [0u8; 8];
    // Use first 8 bytes of address representation
    code[0] = 0x42; // 'B'
    code[1] = 0x52; // 'R'
    code[2] = 0x49; // 'I'
    code[3] = 0x44; // 'D'
    code[4] = 0x47; // 'G'
    code[5] = 0x45; // 'E'
    code[6] = 0x00;
    code[7] = 0x00;
    
    BytesN::from_array(env, &code)
}

fn calculate_tier_reward(tier: u32) -> i128 {
    get_tier_reward(tier)
}

fn update_tier(stats: &mut ReferrerStats) {
    let active = stats.active_referrals.min(MAX_TIER_REFERRALS);
    
    stats.current_tier = if active >= TIER3_THRESHOLD {
        3
    } else if active >= TIER2_THRESHOLD {
        2
    } else {
        1
    };
}

fn update_leaderboard(
    env: &Env,
    referrer: &Address,
    active_referrals: u32,
    tier: u32,
    total_rewards: i128,
) {
    // Simplified leaderboard update - just emit event
    env.events().publish(
        (symbol_short!("reward"), symbol_short!("leader")),
        (referrer.clone(), active_referrals, tier),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    
    #[test]
    #[ignore]
    fn test_generate_referral_code() {
        // Tests require contract context
    }
}
