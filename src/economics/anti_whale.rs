use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env};

/// Timeframe in seconds for daily activity window (24 hours = 86,400 seconds).
pub const DAILY_WINDOW_SECONDS: u64 = 86_400;

/// Default daily operation cap per user (1,000,000 units).
pub const DEFAULT_DAILY_CAP: u64 = 1_000_000;

/// Tier 1 threshold for diminishing returns (100,000 units).
pub const TIER1_THRESHOLD: u64 = 100_000;

/// Tier 2 threshold for diminishing returns (500,000 units).
pub const TIER2_THRESHOLD: u64 = 500_000;

/// Tier 1 multiplier (100% = 10,000 basis points).
pub const TIER1_MULTIPLIER_BPS: u64 = 10_000;

/// Tier 2 multiplier (80% = 8,000 basis points).
pub const TIER2_MULTIPLIER_BPS: u64 = 8_000;

/// Tier 3 multiplier (50% = 5,000 basis points).
pub const TIER3_MULTIPLIER_BPS: u64 = 5_000;

/// Progressive fee basis points for large transactions above Tier 2 (5% = 500 basis points).
pub const PROGRESSIVE_FEE_BPS: u64 = 500;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AntiWhaleKey {
    /// Daily cumulative volume for (user, day_index).
    DailyVolume(Address, u64),
    /// Daily custom cap setting.
    DailyCap,
    /// Whitelisted account (exempt from whale restrictions).
    Exempt(Address),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum AntiWhaleError {
    /// Requested amount exceeds the daily cap for this user.
    DailyCapExceeded = 300,
    /// Arithmetic overflow in volume or fee calculation.
    ArithmeticOverflow = 301,
    /// Amount must be greater than zero.
    InvalidAmount = 302,
}

/// Calculate current day index from ledger timestamp.
pub fn get_day_index(env: &Env) -> u64 {
    env.ledger().timestamp() / DAILY_WINDOW_SECONDS
}

/// Get current daily cap per user.
pub fn get_daily_cap(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&AntiWhaleKey::DailyCap)
        .unwrap_or(DEFAULT_DAILY_CAP)
}

/// Set custom daily cap.
pub fn set_daily_cap(env: &Env, cap: u64) {
    env.storage().instance().set(&AntiWhaleKey::DailyCap, &cap);
}

/// Check if an account is exempt from anti-whale limits.
pub fn is_exempt(env: &Env, user: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&AntiWhaleKey::Exempt(user.clone()))
        .unwrap_or(false)
}

/// Set exemption status for an account.
pub fn set_exempt(env: &Env, user: &Address, exempt: bool) {
    env.storage()
        .persistent()
        .set(&AntiWhaleKey::Exempt(user.clone()), &exempt);
}

/// Get daily cumulative volume for user on current day.
pub fn get_user_daily_volume(env: &Env, user: &Address) -> u64 {
    let day = get_day_index(env);
    env.storage()
        .persistent()
        .get(&AntiWhaleKey::DailyVolume(user.clone(), day))
        .unwrap_or(0)
}

/// Calculate effective output amount applying diminishing returns based on volume tier.
pub fn calculate_diminishing_returns(volume_before: u64, amount: u64) -> u64 {
    if amount == 0 {
        return 0;
    }

    let mut remaining = amount;
    let mut current_vol = volume_before;
    let mut total_effective: u64 = 0;

    // Segment 1: Volume within Tier 1 (0 .. TIER1_THRESHOLD)
    if current_vol < TIER1_THRESHOLD {
        let available_tier1 = TIER1_THRESHOLD.saturating_sub(current_vol);
        let in_tier1 = remaining.min(available_tier1);
        let effective_tier1 = (in_tier1 as u128 * TIER1_MULTIPLIER_BPS as u128 / 10_000) as u64;
        total_effective = total_effective.saturating_add(effective_tier1);
        remaining = remaining.saturating_sub(in_tier1);
        current_vol = current_vol.saturating_add(in_tier1);
    }

    // Segment 2: Volume within Tier 2 (TIER1_THRESHOLD .. TIER2_THRESHOLD)
    if remaining > 0 && current_vol < TIER2_THRESHOLD {
        let available_tier2 = TIER2_THRESHOLD.saturating_sub(current_vol);
        let in_tier2 = remaining.min(available_tier2);
        let effective_tier2 = (in_tier2 as u128 * TIER2_MULTIPLIER_BPS as u128 / 10_000) as u64;
        total_effective = total_effective.saturating_add(effective_tier2);
        remaining = remaining.saturating_sub(in_tier2);
        current_vol = current_vol.saturating_add(in_tier2);
    }

    // Segment 3: Volume above Tier 2 (> TIER2_THRESHOLD)
    if remaining > 0 {
        let effective_tier3 = (remaining as u128 * TIER3_MULTIPLIER_BPS as u128 / 10_000) as u64;
        total_effective = total_effective.saturating_add(effective_tier3);
    }

    total_effective
}

/// Calculate progressive fee for large transaction volumes above TIER2_THRESHOLD.
pub fn calculate_progressive_fee(amount: u64) -> u64 {
    if amount <= TIER2_THRESHOLD {
        0
    } else {
        let excess = amount.saturating_sub(TIER2_THRESHOLD);
        (excess as u128 * PROGRESSIVE_FEE_BPS as u128 / 10_000) as u64
    }
}

/// Process anti-whale mechanics for an action:
/// Checks daily cap, applies diminishing returns, and updates daily volume.
/// Returns `Ok((effective_amount, progressive_fee))`.
pub fn process_anti_whale_action(
    env: &Env,
    user: &Address,
    amount: u64,
) -> Result<(u64, u64), AntiWhaleError> {
    if amount == 0 {
        return Err(AntiWhaleError::InvalidAmount);
    }

    if is_exempt(env, user) {
        return Ok((amount, 0));
    }

    let day = get_day_index(env);
    let key = AntiWhaleKey::DailyVolume(user.clone(), day);
    let volume_before: u64 = env.storage().persistent().get(&key).unwrap_or(0);

    let new_volume = volume_before
        .checked_add(amount)
        .ok_or(AntiWhaleError::ArithmeticOverflow)?;

    let cap = get_daily_cap(env);
    if new_volume > cap {
        return Err(AntiWhaleError::DailyCapExceeded);
    }

    let effective_amount = calculate_diminishing_returns(volume_before, amount);
    let progressive_fee = calculate_progressive_fee(amount);

    env.storage().persistent().set(&key, &new_volume);

    // Emit event if diminishing returns or progressive fee applied
    if effective_amount < amount || progressive_fee > 0 {
        env.events().publish(
            (symbol_short!("Whale"), symbol_short!("scale")),
            (user.clone(), amount, effective_amount, progressive_fee),
        );
    }

    Ok((effective_amount, progressive_fee))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    #[test]
    fn test_diminishing_returns_tiers() {
        // Tier 1: 100% yield
        assert_eq!(calculate_diminishing_returns(0, 50_000), 50_000);
        assert_eq!(calculate_diminishing_returns(0, 100_000), 100_000);

        // Tier 2: 80% yield on portion above 100k
        // 100k @ 100% + 100k @ 80% = 100k + 80k = 180k
        assert_eq!(calculate_diminishing_returns(0, 200_000), 180_000);

        // Tier 3: 50% yield on portion above 500k
        // 100k @ 100% + 400k @ 80% + 100k @ 50% = 100k + 320k + 50k = 470k
        assert_eq!(calculate_diminishing_returns(0, 600_000), 470_000);
    }

    #[test]
    fn test_progressive_fee() {
        assert_eq!(calculate_progressive_fee(100_000), 0);
        assert_eq!(calculate_progressive_fee(500_000), 0);

        // 100k above 500k @ 5% = 5,000
        assert_eq!(calculate_progressive_fee(600_000), 5_000);
    }

    #[test]
    fn test_daily_cap_enforcement() {
        let env = Env::default();
        let user = Address::generate(&env);

        // First action within cap
        let (eff1, fee1) = process_anti_whale_action(&env, &user, 500_000).unwrap();
        assert_eq!(eff1, 420_000); // 100k @ 1.0 + 400k @ 0.8
        assert_eq!(fee1, 0);

        // Second action reaching cap
        let (eff2, _fee2) = process_anti_whale_action(&env, &user, 500_000).unwrap();
        assert_eq!(eff2, 250_000); // 500k @ 0.5

        // Third action exceeding daily cap (1M + 1 = 1,000,001 > 1M)
        let err = process_anti_whale_action(&env, &user, 1).unwrap_err();
        assert_eq!(err, AntiWhaleError::DailyCapExceeded);
    }

    #[test]
    fn test_exemption() {
        let env = Env::default();
        let user = Address::generate(&env);

        set_exempt(&env, &user, true);

        // Exempt user bypasses diminishing returns & daily cap
        let (eff, fee) = process_anti_whale_action(&env, &user, 2_000_000).unwrap();
        assert_eq!(eff, 2_000_000);
        assert_eq!(fee, 0);
    }
}
