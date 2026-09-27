#![cfg(test)]

use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{symbol_short, vec, Address, Env};
use stellar_nebula_nomad::access_control::{
    approve_proposal, create_proposal, execute_proposal, get_proposal, has_permission,
    init_multisig, init_roles, ProposalOperation,
};

fn setup_env() -> (Env, Address, Address, Address) {
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
    let signer1 = Address::generate(&env);
    let signer2 = Address::generate(&env);
    (env, admin, signer1, signer2)
}

#[test]
fn test_governance_multisig_proposal_flow_e2e() {
    let (env, admin, signer1, signer2) = setup_env();

    // Step 1: Initialize roles and multisig governance
    init_roles(&env, admin.clone()).unwrap();

    let signers = vec![&env, admin.clone(), signer1.clone(), signer2.clone()];
    let timelock_secs = 3600u64;
    init_multisig(&env, admin.clone(), signers, 2, timelock_secs).unwrap();

    // Step 2: Proposer creates a proposal to grant a new permission
    let role = symbol_short!("council");
    let action = symbol_short!("enact");
    let op = ProposalOperation::GrantPermission(role.clone(), action.clone());

    let proposal_id = create_proposal(&env, admin.clone(), op).unwrap();
    let proposal = get_proposal(&env, proposal_id).unwrap();
    assert_eq!(proposal.approval_count, 1); // Proposer automatically approves
    assert!(!proposal.executed);

    // Step 3: Second signer approves proposal, reaching quorum (2 of 3)
    approve_proposal(&env, signer1.clone(), proposal_id).unwrap();
    let proposal_approved = get_proposal(&env, proposal_id).unwrap();
    assert_eq!(proposal_approved.approval_count, 2);

    // Step 4: Verify execution fails before timelock elapses
    let early_exec = execute_proposal(&env, admin.clone(), proposal_id);
    assert!(early_exec.is_err());

    // Step 5: Advance ledger timestamp past timelock
    let current_time = env.ledger().timestamp();
    env.ledger().set(LedgerInfo {
        protocol_version: 22,
        sequence_number: 110,
        timestamp: current_time + timelock_secs + 1,
        network_id: [0u8; 32],
        base_reserve: 10,
        min_temp_entry_ttl: 100,
        min_persistent_entry_ttl: 1_000,
        max_entry_ttl: 10_000,
    });

    // Step 6: Execute proposal and verify on-chain state update
    execute_proposal(&env, admin.clone(), proposal_id).unwrap();
    let final_proposal = get_proposal(&env, proposal_id).unwrap();
    assert!(final_proposal.executed);
    assert!(has_permission(&env, &role, &action));
}
