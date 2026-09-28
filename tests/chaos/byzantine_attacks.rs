//! Chaos: Byzantine / malicious callers (Issue #480).
//!
//! Malicious actors call without authorization or replay requests to farm
//! scans. Calls must be rejected without mutating state.

use soroban_sdk::{testutils::Address as _, Address, BytesN, Env};
use stellar_nebula_nomad::{NebulaNomadContract, NebulaNomadContractClient};

fn seed(env: &Env) -> BytesN<32> {
    let mut raw = [0xABu8; 32];
    raw[31] = 0xCD;
    BytesN::from_array(env, &raw)
}

#[test]
fn unauthorized_scan_is_rejected_without_state_change() {
    let env = Env::default(); // no mock_all_auths: attacker cannot sign for victim
    let id = env.register(NebulaNomadContract, ());
    let client = NebulaNomadContractClient::new(&env, &id);
    let victim = Address::generate(&env);

    assert!(client.try_scan_nebula(&seed(&env), &victim).is_err());
    assert_eq!(client.get_global_stats().total_scans, 0);
}

#[test]
fn replay_flood_is_throttled() {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(NebulaNomadContract, ());
    let client = NebulaNomadContractClient::new(&env, &id);
    let attacker = Address::generate(&env);
    let s = seed(&env);

    let rejected = (0..50)
        .filter(|_| client.try_scan_nebula(&s, &attacker).is_err())
        .count();
    assert_eq!(rejected, 45);
    assert_eq!(client.get_global_stats().total_scans, 5);
}
