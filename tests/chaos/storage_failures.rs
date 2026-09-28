//! Chaos: storage failures and missing data (Issue #480).
//!
//! Soroban storage cannot be made to error on demand, so a "failed" read is
//! modelled as a missing or expired entry. Reads must degrade to safe
//! defaults, and a panicking write must leave no partial state behind.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, BytesN, Env,
};
use stellar_nebula_nomad::{NebulaNomadContract, NebulaNomadContractClient};

fn setup(env: &Env) -> NebulaNomadContractClient<'_> {
    env.mock_all_auths();
    let id = env.register(NebulaNomadContract, ());
    NebulaNomadContractClient::new(env, &id)
}

#[test]
fn missing_stats_degrade_to_zero() {
    let env = Env::default();
    let client = setup(&env);
    let stats = client.get_global_stats();
    assert_eq!(stats.total_scans, 0);
    assert_eq!(stats.ships_minted, 0);
    assert_eq!(stats.total_essence_accrued, 0);
}

#[test]
fn expired_temporary_rate_limit_state_recovers() {
    let env = Env::default();
    let client = setup(&env);
    let player = Address::generate(&env);
    let mut raw = [1u8; 32];
    raw[5] = 2;
    let seed = BytesN::from_array(&env, &raw);

    for _ in 0..5 {
        client.scan_nebula(&seed, &player);
    }
    assert!(client.try_scan_nebula(&seed, &player).is_err());

    // Rate-limit window state "lost" (window expired): the player recovers.
    env.ledger().with_mut(|l| l.timestamp += 61);
    assert!(client.try_scan_nebula(&seed, &player).is_ok());
    assert_eq!(client.get_global_stats().total_scans, 6);
}
