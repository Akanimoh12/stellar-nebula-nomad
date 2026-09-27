#![cfg(test)]

use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{symbol_short, vec, Address, Env};
use stellar_nebula_nomad::access_control::{
    check_permission, grant_role, init_roles, is_emergency_mode, set_emergency_mode,
    set_emergency_role,
};
use stellar_nebula_nomad::emergency_controls::{
    execute_unpause, initialize_admins, is_paused, pause_contract, require_not_paused,
    schedule_unpause,
};

fn setup_env() -> (Env, Address, Address) {
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
    let admin = Address::generate(&env);
    let responder = Address::generate(&env);
    (env, admin, responder)
}

#[test]
fn test_emergency_pause_and_recovery_workflow_e2e() {
    let (env, admin, responder) = setup_env();

    // Step 1: Initialize emergency controllers and access control
    let admins = vec![&env, admin.clone()];
    initialize_admins(&env, admins).unwrap();
    init_roles(&env, admin.clone()).unwrap();

    // Verify system begins unpaused
    assert!(!is_paused(&env));
    assert!(require_not_paused(&env).is_ok());

    // Step 2: Trigger emergency pause
    pause_contract(&env, &admin).unwrap();
    assert!(is_paused(&env));
    assert!(require_not_paused(&env).is_err());

    // Step 3: Configure emergency responder role
    let emergency_role = symbol_short!("guardian");
    grant_role(&env, admin.clone(), emergency_role.clone(), responder.clone(), None).unwrap();
    set_emergency_role(&env, admin.clone(), emergency_role.clone(), true).unwrap();

    // Step 4: Toggle emergency mode on RBAC
    set_emergency_mode(&env, admin.clone(), true).unwrap();
    assert!(is_emergency_mode(&env));

    // Emergency responder can bypass normal permissions during emergency mode
    assert!(check_permission(&env, &responder, &symbol_short!("evacuate")).is_ok());

    // Step 5: Schedule unpause with security delay
    let unpause_time = schedule_unpause(&env, &admin).unwrap();

    // Advance time past unpause schedule
    env.ledger().set(LedgerInfo {
        protocol_version: 22,
        sequence_number: 120,
        timestamp: unpause_time + 1,
        network_id: [0u8; 32],
        base_reserve: 10,
        min_temp_entry_ttl: 100,
        min_persistent_entry_ttl: 1_000,
        max_entry_ttl: 10_000,
    });

    // Step 6: Execute unpause and return to normal operational state
    execute_unpause(&env, &admin).unwrap();
    assert!(!is_paused(&env));
    assert!(require_not_paused(&env).is_ok());

    set_emergency_mode(&env, admin, false).unwrap();
    assert!(!is_emergency_mode(&env));
}
