#![cfg(test)]

use soroban_sdk::{
    symbol_short, testutils::Address as _, Address, Bytes, BytesN, Env, Vec,
};
use stellar_nebula_nomad::{NebulaNomadContract, NebulaNomadContractClient};

fn setup() -> (Env, NebulaNomadContractClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, NebulaNomadContract);
    let client = NebulaNomadContractClient::new(&env, &contract_id);
    let player = Address::generate(&env);
    (env, client, player)
}

fn bytes_to_string(bytes: &Bytes) -> std::string::String {
    let mut buf = [0u8; 512];
    let len = bytes.len() as usize;
    bytes.copy_into_slice(&mut buf[..len]);
    std::str::from_utf8(&buf[..len]).unwrap().to_string()
}

#[test]
fn test_progressive_difficulty_curve() {
    let (env, client, admin) = setup();

    client.adjust_curve_parameter(&admin, &symbol_short!("base"), &100i128);
    client.adjust_curve_parameter(&admin, &symbol_short!("grow"), &1_020_000i128);

    let mut previous_score = 0i128;
    for level in 1..=200 {
        let curve = client.calculate_progressive_difficulty(&level);
        assert_eq!(curve.player_level, level);
        assert!(curve.curve_score >= previous_score);
        assert!(curve.difficulty_multiplier >= 100);
        assert!(curve.scan_multiplier >= 100);
        previous_score = curve.curve_score;
    }

    let seed = BytesN::from_array(&env, &[9u8; 32]);
    let baseline = client.generate_nebula_layout(&seed, &admin);

    let metadata = Bytes::new(&env);
    client.mint_ship(&admin, &symbol_short!("explorer"), &metadata);

    let boosted = client.generate_nebula_layout(&seed, &admin);
    assert!(boosted.total_energy >= baseline.total_energy);
}

#[test]
fn test_health_monitor_summary() {
    let (env, client, _player) = setup();

    let mut metrics = Vec::new(&env);
    metrics.push_back(stellar_nebula_nomad::HealthMetricInput {
        metric: symbol_short!("gas"),
        value: 10,
    });
    metrics.push_back(stellar_nebula_nomad::HealthMetricInput {
        metric: symbol_short!("stor"),
        value: 5,
    });
    metrics.push_back(stellar_nebula_nomad::HealthMetricInput {
        metric: symbol_short!("gas"),
        value: 20,
    });

    let recorded = client.record_contract_health_batch(&metrics);
    assert_eq!(recorded.len(), 3);

    let summary = client.get_health_summary();
    assert_eq!(summary.total_samples, 3);
    assert_eq!(summary.metrics.len(), 2);
    assert_eq!(summary.metrics.get(0).unwrap().metric, symbol_short!("gas"));
    assert_eq!(summary.metrics.get(0).unwrap().total, 30);
}

#[test]
fn test_achievement_unlock_flow() {
    let (env, client, player) = setup();
    let profile_id = client.initialize_profile(&player);
    let metadata = Bytes::new(&env);
    client.mint_ship(&player, &symbol_short!("explorer"), &metadata);
    client.update_progress(&player, &profile_id, &500u32, &20_000i128);

    let single = client.unlock_achievement(&player, &1u64);
    assert_eq!(single.achievement_id, 1);

    let mut ids = Vec::new(&env);
    ids.push_back(6);
    ids.push_back(10);
    ids.push_back(2);
    ids.push_back(3);
    ids.push_back(4);

    let batch = client.batch_unlock_achievements(&player, &ids);
    assert_eq!(batch.len(), 5);

    let progress = client.check_achievement_progress(&player);
    assert_eq!(progress.len(), 20);
    assert!(progress.get(0).unwrap().unlocked);
    assert!(progress.get(5).unwrap().eligible);

    let count = client.get_player_achievement_count(&player);
    assert_eq!(count, 6);

    let badges = client.get_player_badges(&player);
    assert_eq!(badges.len(), 6);
}

#[test]
fn test_data_export_pagination() {
    let (env, client, player_a) = setup();
    let player_b = Address::generate(&env);

    let profile_a = client.initialize_profile(&player_a);
    let profile_b = client.initialize_profile(&player_b);
    client.set_export_opt_in(&player_a, &true);
    client.set_export_opt_in(&player_b, &true);
    client.set_export_compression(&player_b, &true);

    let metadata = Bytes::new(&env);
    client.mint_ship(&player_a, &symbol_short!("explorer"), &metadata);
    client.mint_ship(&player_b, &symbol_short!("hauler"), &metadata);
    client.update_progress(&player_a, &profile_a, &12u32, &1_200i128);
    client.update_progress(&player_b, &profile_b, &24u32, &2_400i128);

    let payload = client.export_player_data(&player_a);
    let payload_text = bytes_to_string(&payload);
    assert!(payload_text.contains("profile_id,scans,essence,compressed,last_updated"));

    let page_one = client.batch_export_players(&1u32);
    assert_eq!(page_one.len(), 1);
    assert!(!page_one.get(0).unwrap().compressed);

    let page_two = client.batch_export_players(&1u32);
    assert_eq!(page_two.len(), 1);
    assert!(page_two.get(0).unwrap().compressed);

    let session = client.get_export_session();
    assert_eq!(session.total_exports, 2);
}
