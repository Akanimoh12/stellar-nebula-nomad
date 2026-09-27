//! Chaos: cross-contract "network" failures (Issue #480).
//!
//! A call to a contract that is not (yet) deployed stands in for a timed-out
//! or unreachable peer. The caller retries with exponential backoff; once the
//! peer "recovers" (is deployed) the call succeeds and state is consistent.

use soroban_sdk::{testutils::Address as _, Address, BytesN, Env};
use stellar_nebula_nomad::{NebulaNomadContract, NebulaNomadContractClient};

/// Retry `op` up to `max_attempts` times, doubling the delay each time.
/// Returns the result and the delays that were waited.
fn retry_with_backoff<T>(
    max_attempts: u32,
    base_delay: u64,
    mut op: impl FnMut(u32) -> Option<T>,
) -> (Option<T>, std::vec::Vec<u64>) {
    let mut delays = std::vec::Vec::new();
    for attempt in 0..max_attempts {
        if let Some(v) = op(attempt) {
            return (Some(v), delays);
        }
        delays.push(base_delay << attempt);
    }
    (None, delays)
}

fn seed(env: &Env) -> BytesN<32> {
    let mut raw = [3u8; 32];
    raw[0] = 9;
    BytesN::from_array(env, &raw)
}

#[test]
fn unreachable_contract_call_fails_without_side_effects() {
    let env = Env::default();
    env.mock_all_auths();
    let peer = Address::generate(&env);
    let client = NebulaNomadContractClient::new(&env, &peer);
    let player = Address::generate(&env);
    assert!(client.try_scan_nebula(&seed(&env), &player).is_err());
}

#[test]
fn retry_with_exponential_backoff_recovers_after_outage() {
    let env = Env::default();
    env.mock_all_auths();
    let peer = Address::generate(&env);
    let client = NebulaNomadContractClient::new(&env, &peer);
    let player = Address::generate(&env);
    let s = seed(&env);

    let (result, delays) = retry_with_backoff(5, 100, |attempt| {
        if attempt == 2 {
            // Network recovers: the peer comes online.
            env.register_at(&peer, NebulaNomadContract, ());
        }
        client
            .try_scan_nebula(&s, &player)
            .ok()
            .and_then(|r| r.ok())
    });

    assert!(result.is_some());
    assert_eq!(delays, std::vec![100, 200]);
    // Exactly one scan landed despite the failed attempts.
    assert_eq!(client.get_global_stats().total_scans, 1);
}

#[test]
fn backoff_gives_up_after_max_attempts() {
    let (result, delays) = retry_with_backoff::<()>(4, 50, |_| None);
    assert!(result.is_none());
    assert_eq!(delays, std::vec![50, 100, 200, 400]);
}
