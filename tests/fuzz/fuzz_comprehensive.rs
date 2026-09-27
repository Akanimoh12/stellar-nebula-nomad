//! Comprehensive edge-case fuzz coverage for the main contract surface.
//!
//! Run directly with:
//! `cargo test --test fuzz_comprehensive`

#![cfg(test)]

use proptest::prelude::*;
use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{symbol_short, Address, Bytes, Env, Vec};
use stellar_nebula_nomad::{
    DifficultyError, NavError, NebulaNomadContract, NebulaNomadContractClient, ProfileError,
    RouteEdge, ShipError, MAX_CONNECTIONS_PER_BATCH,
};

fn setup() -> (Env, NebulaNomadContractClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        protocol_version: 22,
        sequence_number: 100,
        timestamp: 1_700_000_000,
        network_id: [0u8; 32],
        base_reserve: 10,
        min_temp_entry_ttl: 100,
        min_persistent_entry_ttl: 1_000,
        max_entry_ttl: 10_000,
    });
    let contract_id = env.register(NebulaNomadContract, ());
    let client = NebulaNomadContractClient::new(&env, &contract_id);
    let player = Address::generate(&env);
    (env, client, player)
}

proptest! {
    #[test]
    fn prop_batch_mint_accepts_only_supported_batch_sizes(count in 0u32..=6u32) {
        let (env, client, player) = setup();
        let metadata = Bytes::from_array(&env, &[7u8; 8]);
        let mut ship_types = Vec::new(&env);
        for i in 0..count {
            ship_types.push_back(match i % 3 {
                0 => symbol_short!("fighter"),
                1 => symbol_short!("miner"),
                _ => symbol_short!("scout"),
            });
        }

        let result = client.try_batch_mint_ships(&player, &ship_types, &metadata);
        if count > 3 {
            prop_assert!(matches!(result, Err(Ok(ShipError::BatchLimitExceeded))));
        } else {
            let ships = result.expect("host call must complete").expect("valid batch must mint");
            prop_assert_eq!(ships.len(), count);
        }
    }

    #[test]
    fn prop_difficulty_boundaries_never_panic(level in any::<u32>()) {
        let (_, client, _) = setup();
        let result = client.try_calculate_difficulty(&level);
        if level == 0 || level > 100 {
            prop_assert!(matches!(result, Err(Ok(DifficultyError::InvalidLevel))));
        } else {
            let difficulty = result.expect("host call must complete").expect("valid level must succeed");
            let weights = difficulty.rarity_weights;
            prop_assert_eq!(
                weights.common + weights.uncommon + weights.rare + weights.epic + weights.legendary,
                100
            );
        }
    }

    #[test]
    fn prop_navigation_batch_limit_is_enforced(edge_count in 0u32..=25u32) {
        let (env, client, admin) = setup();
        client.initialize_nav_graph(&admin);
        let mut edges = Vec::new(&env);
        for i in 0..edge_count {
            edges.push_back(RouteEdge {
                from: i as u64 + 1,
                to: i as u64 + 2,
                fuel_cost: i + 1,
                hazard_level: i % 101,
            });
        }
        let result = client.try_add_nebula_connections_batch(&admin, &edges);
        if edge_count > MAX_CONNECTIONS_PER_BATCH {
            prop_assert!(matches!(result, Err(Ok(NavError::BatchTooLarge))));
        } else {
            prop_assert!(matches!(result, Ok(Ok(n)) if n == edge_count));
        }
    }

    #[test]
    fn prop_profile_progress_updates_are_additive(scan_count in 0u32..=250u32, essence in 0i128..=1_000_000i128) {
        let (_, client, player) = setup();
        let profile_id = client.initialize_profile(&player);
        client.update_progress(&player, &profile_id, &scan_count, &essence).unwrap();
        let profile = client.get_profile(&profile_id).unwrap();
        prop_assert_eq!(profile.total_scans, scan_count);
        prop_assert_eq!(profile.essence_earned, essence);
    }
}

#[test]
fn zero_essence_and_zero_scans_are_valid_noop_progress() {
    let (_, client, player) = setup();
    let profile_id = client.initialize_profile(&player);
    assert!(client
        .try_update_progress(&player, &profile_id, &0u32, &0i128)
        .is_ok());
}

#[test]
fn negative_essence_is_rejected() {
    let (_, client, player) = setup();
    let profile_id = client.initialize_profile(&player);
    let result = client.try_update_progress(&player, &profile_id, &1u32, &-1i128);
    assert!(matches!(result, Err(Ok(ProfileError::Unauthorized))));
}
