#![cfg(test)]

use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{symbol_short, Address, Env, String};
use stellar_nebula_nomad::alliance_manager::{
    contribute_to_treasury, found_alliance, get_alliance, get_alliance_treasury,
    get_member_contribution, get_player_alliance, join_alliance,
};
use stellar_nebula_nomad::guild_quests::{
    contribute_quest_progress, create_guild_quest, get_active_guild_quests,
};

fn make_env() -> Env {
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
    env
}

#[test]
fn test_guild_operations_workflow_e2e() {
    let env = make_env();
    let founder = Address::generate(&env);
    let member1 = Address::generate(&env);
    let member2 = Address::generate(&env);

    // Step 1: Founder creates new alliance
    let alliance_name = String::from_str(&env, "Cosmic Voyagers");
    let alliance_id = found_alliance(&env, founder.clone(), alliance_name).unwrap();
    assert_eq!(get_player_alliance(&env, founder.clone()), Some(alliance_id));

    // Step 2: Member 1 joins alliance
    join_alliance(&env, alliance_id, member1.clone()).unwrap();
    assert_eq!(get_player_alliance(&env, member1.clone()), Some(alliance_id));

    // Step 3: Member 2 joins alliance
    join_alliance(&env, alliance_id, member2.clone()).unwrap();
    assert_eq!(get_player_alliance(&env, member2.clone()), Some(alliance_id));

    // Step 4: Verify alliance roster
    let alliance = get_alliance(&env, alliance_id).unwrap();
    assert_eq!(alliance.members.len(), 3);

    // Step 5: Member 1 contributes to guild treasury
    contribute_to_treasury(&env, member1.clone(), 1_000).unwrap();
    assert_eq!(get_alliance_treasury(&env, alliance_id), 1_000);
    assert_eq!(get_member_contribution(&env, alliance_id, member1.clone()), 1_000);

    // Step 6: Create cooperative guild quest
    let quest_id = create_guild_quest(
        &env,
        founder.clone(),
        alliance_id,
        symbol_short!("scan"),
        10,
        500, // reward essence
        100, // reward xp
        3600,
    )
    .unwrap();
    assert_eq!(quest_id, 1);

    // Step 7: Verify active quests list
    let active_quests = get_active_guild_quests(&env, alliance_id);
    assert_eq!(active_quests.len(), 1);

    // Step 8: Member 1 & Member 2 contribute quest progress to complete quest
    contribute_quest_progress(&env, member1.clone(), symbol_short!("scan"), 6).unwrap();
    contribute_quest_progress(&env, member2.clone(), symbol_short!("scan"), 4).unwrap();

    // Step 9: Verify quest completed and treasury rewards distributed
    let quests_after = get_active_guild_quests(&env, alliance_id);
    assert_eq!(quests_after.len(), 0); // No longer active because completed

    // Step 10: Verify final treasury reflects contribution + quest reward
    assert_eq!(get_alliance_treasury(&env, alliance_id), 1_500);
}
