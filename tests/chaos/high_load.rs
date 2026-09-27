//! Chaos: high transaction volume (Issue #480).
//!
//! Fires 1000+ scan transactions from many players and checks that rate
//! limiting throttles hot callers while every accepted scan is counted once.

use soroban_sdk::{testutils::Address as _, Address, BytesN, Env};
use stellar_nebula_nomad::{NebulaNomadContract, NebulaNomadContractClient};

const PLAYERS: u32 = 200;
const TX_PER_PLAYER: u32 = 6; // default limit is 5 scans / minute
const TOTAL_TX: u32 = PLAYERS * TX_PER_PLAYER;

#[test]
fn rate_limiting_holds_under_1000_plus_transactions() {
    let env = Env::default();
    env.mock_all_auths();
    env.cost_estimate().budget().reset_unlimited();
    let id = env.register(NebulaNomadContract, ());
    let client = NebulaNomadContractClient::new(&env, &id);

    let mut accepted = 0u64;
    let mut throttled = 0u64;
    for p in 0..PLAYERS {
        let player = Address::generate(&env);
        for t in 0..TX_PER_PLAYER {
            let mut raw = [0u8; 32];
            raw[..4].copy_from_slice(&p.to_be_bytes());
            raw[4] = t as u8 + 1;
            let seed = BytesN::from_array(&env, &raw);
            match client.try_scan_nebula(&seed, &player) {
                Ok(Ok(_)) => accepted += 1,
                _ => throttled += 1,
            }
        }
    }

    assert!(TOTAL_TX >= 1000);
    assert_eq!(accepted + throttled, TOTAL_TX as u64);
    // The 6th call per player in the same window is rejected.
    assert_eq!(throttled, PLAYERS as u64);
    // Invariant: no lost or double-counted scans.
    assert_eq!(client.get_global_stats().total_scans, accepted);
}

#[test]
fn throttled_player_does_not_starve_others() {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register(NebulaNomadContract, ());
    let client = NebulaNomadContractClient::new(&env, &id);
    let mut raw = [4u8; 32];
    raw[1] = 5;
    let seed = BytesN::from_array(&env, &raw);

    let hot = Address::generate(&env);
    for _ in 0..5 {
        client.scan_nebula(&seed, &hot);
    }
    assert!(client.try_scan_nebula(&seed, &hot).is_err());

    // A queued, well-behaved player is still served.
    let other = Address::generate(&env);
    assert!(client.try_scan_nebula(&seed, &other).is_ok());
}
