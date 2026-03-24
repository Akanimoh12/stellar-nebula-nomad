#![cfg(test)]

use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger, LedgerInfo},
    Address, Env,
};

use stellar_nebula_nomad::resource_minter::{
    ResourceError, ResourceMinter, ResourceMinterClient, ResourceType, LEDGERS_PER_DAY,
};

// ─── Mock contracts ───────────────────────────────────────────────────────────

/// Mock Ship Registry: always confirms ownership so tests focus on minter logic.
#[contract]
pub struct MockShipRegistry;

#[contractimpl]
impl MockShipRegistry {
    pub fn owns_ship(_env: Env, _owner: Address, _ship_id: u64) -> bool {
        true
    }
}

/// Mock Nebula Explorer: always confirms anomaly existence.
#[contract]
pub struct MockNebulaExplorer;

#[contractimpl]
impl MockNebulaExplorer {
    pub fn has_anomaly(_env: Env, _ship_id: u64, _anomaly_index: u32) -> bool {
        true
    }
}

// ─── Test helpers ─────────────────────────────────────────────────────────────

/// Boot a fresh environment with all three contracts registered and initialised.
/// Returns (env, client_contract_id, admin_address, player_address).
fn setup_env() -> (Env, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let ship_id = env.register_contract(None, MockShipRegistry);
    let nebula_id = env.register_contract(None, MockNebulaExplorer);
    let contract_id = env.register_contract(None, ResourceMinter);

    let admin = Address::generate(&env);
    let player = Address::generate(&env);

    ResourceMinterClient::new(&env, &contract_id).init(
        &admin,
        &ship_id,
        &nebula_id,
        &500u32,          // 5 % APY
        &1_000i128,       // daily harvest cap
        &LEDGERS_PER_DAY, // min stake duration ≈ 1 day
    );

    (env, contract_id, admin, player)
}

/// Advance the Stellar ledger by `n` sequence numbers (≈ n × 5 s wall-clock).
fn advance_ledgers(env: &Env, n: u32) {
    let seq = env.ledger().sequence();
    let ts = env.ledger().timestamp();
    env.ledger().set(LedgerInfo {
        sequence_number: seq + n,
        timestamp: ts + (n as u64 * 5),
        protocol_version: 20,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 16,
        min_persistent_entry_ttl: 4096,
        max_entry_ttl: 6_312_000,
    });
}

// ─── Harvest tests ────────────────────────────────────────────────────────────

#[test]
fn test_harvest_base_amount() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    // anomaly_index = 0 → base 100 + 0 × 10 = 100
    let minted = client.harvest_resource(&player, &1u64, &0u32);
    assert_eq!(minted, 100);
    assert_eq!(client.get_balance(&player, &ResourceType::Stardust), 100);
}

#[test]
fn test_harvest_rarity_bonus() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    // anomaly_index = 5 → 100 + 5 × 10 = 150
    assert_eq!(client.harvest_resource(&player, &1u64, &5u32), 150);
}

#[test]
fn test_harvest_multiple_ships_have_independent_caps() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    // Drain ship 1's daily cap (10 × 100 = 1000)
    for _ in 0..10 {
        client.harvest_resource(&player, &1u64, &0u32);
    }
    // Ship 2 uses its own independent cap — must succeed
    assert_eq!(client.harvest_resource(&player, &2u64, &0u32), 100);
}

#[test]
fn test_harvest_daily_cap_enforced() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    for _ in 0..10 {
        client.harvest_resource(&player, &1u64, &0u32);
    }
    let err = client.try_harvest_resource(&player, &1u64, &0u32);
    assert_eq!(err, Err(Ok(ResourceError::DailyCapExceeded)));
}

#[test]
fn test_harvest_cap_resets_after_one_day() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    for _ in 0..10 {
        client.harvest_resource(&player, &1u64, &0u32);
    }
    advance_ledgers(&env, LEDGERS_PER_DAY);
    // Should succeed again after window reset
    assert_eq!(client.harvest_resource(&player, &1u64, &0u32), 100);
}

#[test]
fn test_harvest_amount_capped_near_daily_limit() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    // 9 × 100 = 900 harvested; 100 remaining in cap
    for _ in 0..9 {
        client.harvest_resource(&player, &1u64, &0u32);
    }
    // Raw amount from anomaly_index=5 would be 150, but only 100 left → capped
    assert_eq!(client.harvest_resource(&player, &1u64, &5u32), 100);
}

// ─── Staking tests ────────────────────────────────────────────────────────────

#[test]
fn test_stake_deducts_liquid_balance() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    client.harvest_resource(&player, &1u64, &0u32); // 100 stardust
    client.stake_for_yield(&player, &ResourceType::Stardust, &100i128, &LEDGERS_PER_DAY);
    assert_eq!(client.get_balance(&player, &ResourceType::Stardust), 0);
    assert_eq!(client.get_stake(&player).unwrap().amount, 100);
}

#[test]
fn test_stake_insufficient_resources_rejected() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    let err = client.try_stake_for_yield(
        &player, &ResourceType::Stardust, &100i128, &LEDGERS_PER_DAY,
    );
    assert_eq!(err, Err(Ok(ResourceError::InsufficientResources)));
}

#[test]
fn test_stake_below_min_duration_rejected() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    client.harvest_resource(&player, &1u64, &0u32);
    // 1000 < 17280 min
    let err = client.try_stake_for_yield(
        &player, &ResourceType::Stardust, &100i128, &1_000u32,
    );
    assert_eq!(err, Err(Ok(ResourceError::InvalidDuration)));
}

#[test]
fn test_stake_zero_amount_rejected() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    let err = client.try_stake_for_yield(
        &player, &ResourceType::Stardust, &0i128, &LEDGERS_PER_DAY,
    );
    assert_eq!(err, Err(Ok(ResourceError::InvalidAmount)));
}

#[test]
fn test_duplicate_stake_rejected() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    // Harvest twice so there is enough balance for a second attempted stake
    client.harvest_resource(&player, &1u64, &0u32);
    client.harvest_resource(&player, &2u64, &0u32);
    client.stake_for_yield(&player, &ResourceType::Stardust, &100i128, &LEDGERS_PER_DAY);
    let err = client.try_stake_for_yield(
        &player, &ResourceType::Stardust, &100i128, &LEDGERS_PER_DAY,
    );
    assert_eq!(err, Err(Ok(ResourceError::AlreadyStaked)));
}

// ─── 24-hour yield simulation ─────────────────────────────────────────────────

#[test]
fn test_claim_yield_after_24h() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    client.harvest_resource(&player, &1u64, &0u32); // 100 stardust
    client.stake_for_yield(&player, &ResourceType::Stardust, &100i128, &LEDGERS_PER_DAY);

    advance_ledgers(&env, LEDGERS_PER_DAY); // +1 day

    let yield_earned = client.claim_yield(&player);
    // 100 × 5% / 365 ≈ 0.0136 → integer truncates to 0; accumulates over weeks
    assert!(yield_earned >= 0);
    assert_eq!(client.get_balance(&player, &ResourceType::Plasma), yield_earned);
}

#[test]
fn test_claim_yield_after_1_year() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    client.harvest_resource(&player, &1u64, &0u32); // 100 stardust
    client.stake_for_yield(&player, &ResourceType::Stardust, &100i128, &LEDGERS_PER_DAY);

    advance_ledgers(&env, LEDGERS_PER_DAY * 365); // +365 days

    let yield_earned = client.claim_yield(&player);
    // 100 × 500 / 10_000 × (17280×365) / (17280×365) = 5 plasma
    assert_eq!(yield_earned, 5);
    assert_eq!(client.get_balance(&player, &ResourceType::Plasma), 5);
}

#[test]
fn test_pending_yield_matches_claim_amount() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    client.harvest_resource(&player, &1u64, &0u32);
    client.stake_for_yield(&player, &ResourceType::Stardust, &100i128, &LEDGERS_PER_DAY);

    advance_ledgers(&env, LEDGERS_PER_DAY * 365);

    let pending = client.get_pending_yield(&player);
    let claimed = client.claim_yield(&player);
    assert_eq!(pending, claimed);
}

#[test]
fn test_yield_accumulates_across_partial_claims() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    client.harvest_resource(&player, &1u64, &0u32);
    // Use a 2-year lock so we can keep claiming
    client.stake_for_yield(&player, &ResourceType::Stardust, &100i128, &(LEDGERS_PER_DAY * 365 * 2));

    // Claim at ~6 months then at ~12 months
    advance_ledgers(&env, LEDGERS_PER_DAY * 182);
    let y1 = client.claim_yield(&player);

    advance_ledgers(&env, LEDGERS_PER_DAY * 183);
    let y2 = client.claim_yield(&player);

    // Total ≈ 5 (5 % of 100); allow ±1 for integer truncation across two windows
    let total = y1 + y2;
    assert!(total >= 4 && total <= 5);
}

// ─── Unstake / time-lock tests ────────────────────────────────────────────────

#[test]
fn test_unstake_blocked_immediately_after_stake() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    client.harvest_resource(&player, &1u64, &0u32);
    client.stake_for_yield(&player, &ResourceType::Stardust, &100i128, &LEDGERS_PER_DAY);
    let err = client.try_unstake(&player);
    assert_eq!(err, Err(Ok(ResourceError::TimeLockActive)));
}

#[test]
fn test_unstake_allowed_after_timelock_expires() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    client.harvest_resource(&player, &1u64, &0u32);
    client.stake_for_yield(&player, &ResourceType::Stardust, &100i128, &LEDGERS_PER_DAY);

    advance_ledgers(&env, LEDGERS_PER_DAY);

    let returned = client.unstake(&player);
    assert_eq!(returned, 100);
    assert!(client.get_stake(&player).is_none());
    assert_eq!(client.get_balance(&player, &ResourceType::Stardust), 100);
}

#[test]
fn test_unstake_auto_claims_residual_yield() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    client.harvest_resource(&player, &1u64, &0u32);
    client.stake_for_yield(&player, &ResourceType::Stardust, &100i128, &LEDGERS_PER_DAY);

    advance_ledgers(&env, LEDGERS_PER_DAY * 365); // 1 year → 5 plasma

    client.unstake(&player);

    assert_eq!(client.get_balance(&player, &ResourceType::Plasma), 5);
    assert_eq!(client.get_balance(&player, &ResourceType::Stardust), 100);
}

#[test]
fn test_unstake_then_restake_succeeds() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    client.harvest_resource(&player, &1u64, &0u32);
    client.stake_for_yield(&player, &ResourceType::Stardust, &100i128, &LEDGERS_PER_DAY);

    advance_ledgers(&env, LEDGERS_PER_DAY);
    client.unstake(&player);

    // Re-staking after unstake must succeed
    client.stake_for_yield(&player, &ResourceType::Stardust, &100i128, &LEDGERS_PER_DAY);
    assert_eq!(client.get_stake(&player).unwrap().amount, 100);
}

// ─── Multiple resource types ──────────────────────────────────────────────────

#[test]
fn test_resource_type_balances_are_independent() {
    let (env, cid, _, player) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);

    client.harvest_resource(&player, &1u64, &0u32); // 100 stardust
    client.stake_for_yield(&player, &ResourceType::Stardust, &50i128, &LEDGERS_PER_DAY);

    advance_ledgers(&env, LEDGERS_PER_DAY * 365);
    let plasma = client.claim_yield(&player);

    // 50 × 5 % = 2.5 → integer 2 plasma
    assert_eq!(plasma, 2);
    // 50 liquid stardust untouched
    assert_eq!(client.get_balance(&player, &ResourceType::Stardust), 50);
    assert_eq!(client.get_balance(&player, &ResourceType::Plasma), 2);
    // Crystals entirely unaffected
    assert_eq!(client.get_balance(&player, &ResourceType::Crystals), 0);
}

// ─── Admin tests ──────────────────────────────────────────────────────────────

#[test]
fn test_update_daily_cap() {
    let (env, cid, _, _) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    client.update_daily_cap(&2_000i128);
    assert_eq!(client.get_config().unwrap().daily_harvest_cap, 2_000);
}

#[test]
fn test_update_apy() {
    let (env, cid, _, _) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    client.update_apy(&1_000u32); // 10 %
    assert_eq!(client.get_config().unwrap().apy_basis_points, 1_000);
}

#[test]
fn test_double_init_rejected() {
    let (env, cid, admin, _) = setup_env();
    let client = ResourceMinterClient::new(&env, &cid);
    let dummy = Address::generate(&env);
    let err = client.try_init(
        &admin,
        &dummy,
        &dummy,
        &500u32,
        &1_000i128,
        &LEDGERS_PER_DAY,
    );
    assert_eq!(err, Err(Ok(ResourceError::AlreadyInitialized)));
}
