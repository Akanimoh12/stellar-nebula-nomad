//! Chaos: partial writes and state consistency (Issue #480).
//!
//! A transaction that fails part-way must roll back atomically, and the
//! deterministic layout generator must produce identical, self-consistent
//! results so corrupted layouts can be detected by recomputation.

use soroban_sdk::{testutils::Address as _, Address, BytesN, Env};
use stellar_nebula_nomad::{NebulaNomadContract, NebulaNomadContractClient};

fn setup(env: &Env) -> NebulaNomadContractClient<'_> {
    env.mock_all_auths();
    let id = env.register(NebulaNomadContract, ());
    NebulaNomadContractClient::new(env, &id)
}

fn seed(env: &Env) -> BytesN<32> {
    let mut raw = [0x11u8; 32];
    raw[7] = 0x22;
    BytesN::from_array(env, &raw)
}

#[test]
fn failed_transaction_leaves_no_partial_state() {
    let env = Env::default();
    let client = setup(&env);
    let player = Address::generate(&env);
    let s = seed(&env);
    for _ in 0..5 {
        client.scan_nebula(&s, &player);
    }
    let before = client.get_global_stats();

    // Rate limit panics mid-scan; analytics must not be half-updated.
    assert!(client.try_scan_nebula(&s, &player).is_err());
    let after = client.get_global_stats();
    assert_eq!(before.total_scans, after.total_scans);
    assert_eq!(before.total_essence_accrued, after.total_essence_accrued);
}

#[test]
fn layout_is_self_consistent_and_detects_tampering() {
    let env = Env::default();
    let client = setup(&env);
    let player = Address::generate(&env);
    let layout = client.generate_nebula_layout(&seed(&env), &player);

    let sum: u32 = layout.cells.iter().map(|c| c.energy).sum();
    assert_eq!(sum, layout.total_energy);
    assert_eq!(layout.cells.len(), layout.width * layout.height);

    // A tampered copy no longer matches its recomputed energy total.
    let mut tampered = layout.clone();
    tampered.total_energy += 1;
    let sum: u32 = tampered.cells.iter().map(|c| c.energy).sum();
    assert_ne!(sum, tampered.total_energy);
}
