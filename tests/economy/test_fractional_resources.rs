#![cfg(test)]

use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{Address, Env, Symbol, Vec};
use stellar_nebula_nomad::{
    FractionalError, MAX_FRACTIONS_PER_TX,
    NebulaNomadContract, NebulaNomadContractClient,
};

fn setup_env() -> (Env, NebulaNomadContractClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        protocol_version: 22,
        sequence_number: 100,
        timestamp: 1_700_000_000,
        network_id: [0u8; 32],
        base_reserve: 10,
        min_temp_entry_ttl: 100,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 10_000,
    });
    let contract_id = env.register(NebulaNomadContract, ());
    let client = NebulaNomadContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    (env, client, admin)
}

// ─── Initialization Tests ─────────────────────────────────────────────────

#[test]
fn test_initialize_fractional() {
    let (_env, client, admin) = setup_env();
    client.initialize_fractional(&admin);
}

// ─── Fractionalization Tests ────────────────────────────────────────────────

#[test]
fn test_fractionalize_resource_success() {
    let (env, client, owner) = setup_env();
    
    // Initialize first
    client.initialize_fractional(&owner);
    
    // Fractionalize 100 units into 10 shares
    let resource_type = Symbol::new(&env, "stellar_dust");
    let ids = client.fractionalize_resource(
        &owner,
        &resource_type,
        &100,
        &10,
    );
    
    assert_eq!(ids.len(), 10);
    
    // Verify shares exist
    for i in 0..ids.len() {
        let share_id = ids.get(i).unwrap();
        let share = client.get_share(&share_id);
        assert!(share.is_some());
        
        let share = share.unwrap();
        assert_eq!(share.owner, owner);
        assert_eq!(share.resource_type, resource_type);
        assert_eq!(share.amount, 10); // 100 / 10 = 10 per share
        assert_eq!(share.total_shares, 10);
    }
}

#[test]
fn test_fractionalize_resource_invalid_share_count() {
    let (env, client, owner) = setup_env();
    client.initialize_fractional(&owner);
    
    let resource_type = Symbol::new(&env, "stellar_dust");
    
    // 0 shares should fail
    let result = client.try_fractionalize_resource(&owner, &resource_type, &100, &0);
    assert!(result.is_err());
    
    // More than MAX_FRACTIONS_PER_TX should fail
    let result = client.try_fractionalize_resource(
        &owner,
        &resource_type,
        &100,
        &(MAX_FRACTIONS_PER_TX + 1),
    );
    assert!(result.is_err());
}

#[test]
fn test_fractionalize_resource_share_too_small() {
    let (env, client, owner) = setup_env();
    client.initialize_fractional(&owner);
    
    let resource_type = Symbol::new(&env, "stellar_dust");
    
    // 100 units into 101 shares = less than 1 per share
    let result = client.try_fractionalize_resource(&owner, &resource_type, &100, &101);
    assert!(result.is_err());
}

#[test]
fn test_fractionalize_resource_remainder() {
    let (env, client, owner) = setup_env();
    client.initialize_fractional(&owner);
    
    let resource_type = Symbol::new(&env, "stellar_dust");
    
    // 100 units into 3 shares = 33.33... per share (remainder)
    let result = client.try_fractionalize_resource(&owner, &resource_type, &100, &3);
    assert!(result.is_err());
}

// ─── Merge Tests ───────────────────────────────────────────────────────────

#[test]
fn test_merge_fractions_success() {
    let (env, client, owner) = setup_env();
    client.initialize_fractional(&owner);
    
    let resource_type = Symbol::new(&env, "stellar_dust");
    let share_ids = client.fractionalize_resource(&owner, &resource_type, &100, &10);
    
    // Get first 5 shares to merge
    let mut merge_ids = Vec::new(&env);
    for i in 0..5 {
        merge_ids.push_back(share_ids.get(i).unwrap());
    }
    
    let merged_amount = client.merge_fractions(&owner, &merge_ids);
    assert_eq!(merged_amount, 50); // 5 shares * 10 each = 50
}

#[test]
fn test_merge_fractions_empty() {
    let (env, client, owner) = setup_env();
    client.initialize_fractional(&owner);
    
    let empty_ids = Vec::new(&env);
    let result = client.try_merge_fractions(&owner, &empty_ids);
    assert!(result.is_err());
}

#[test]
fn test_merge_fractions_not_owner() {
    let (env, client, owner) = setup_env();
    let not_owner = Address::generate(&env);
    client.initialize_fractional(&owner);
    
    let resource_type = Symbol::new(&env, "stellar_dust");
    let share_ids = client.fractionalize_resource(&owner, &resource_type, &100, &10);
    
    // Try to merge as non-owner
    let mut merge_ids = Vec::new(&env);
    merge_ids.push_back(share_ids.get(0).unwrap());
    
    let result = client.try_merge_fractions(&not_owner, &merge_ids);
    assert!(result.is_err());
}

// ─── Transfer Tests ────────────────────────────────────────────────────────

#[test]
fn test_transfer_share_success() {
    let (env, client, owner) = setup_env();
    let recipient = Address::generate(&env);
    client.initialize_fractional(&owner);
    
    let resource_type = Symbol::new(&env, "stellar_dust");
    let share_ids = client.fractionalize_resource(&owner, &resource_type, &100, &10);
    let share_id = share_ids.get(0).unwrap();
    
    // Transfer first share
    let share = client.transfer_share(&owner, &recipient, &share_id);
    assert_eq!(share.owner, recipient);
    
    // Verify owner lost the share
    let owner_shares = client.get_owner_shares(&owner);
    assert_eq!(owner_shares.len(), 9);
    
    // Verify recipient got the share
    let recipient_shares = client.get_owner_shares(&recipient);
    assert_eq!(recipient_shares.len(), 1);
    assert!(client.is_share_owner(&recipient, &share_id));
}

#[test]
fn test_transfer_share_not_owner() {
    let (env, client, owner) = setup_env();
    let not_owner = Address::generate(&env);
    let recipient = Address::generate(&env);
    client.initialize_fractional(&owner);
    
    let resource_type = Symbol::new(&env, "stellar_dust");
    let share_ids = client.fractionalize_resource(&owner, &resource_type, &100, &10);
    let share_id = share_ids.get(0).unwrap();
    
    // Try to transfer as non-owner
    let result = client.try_transfer_share(&not_owner, &recipient, &share_id);
    assert!(result.is_err());
}

// ─── View Function Tests ───────────────────────────────────────────────────

#[test]
fn test_get_owner_shares() {
    let (env, client, owner) = setup_env();
    client.initialize_fractional(&owner);
    
    // Initially empty
    let shares = client.get_owner_shares(&owner);
    assert_eq!(shares.len(), 0);
    
    // After fractionalization
    let resource_type = Symbol::new(&env, "stellar_dust");
    let share_ids = client.fractionalize_resource(&owner, &resource_type, &100, &5);
    
    let shares = client.get_owner_shares(&owner);
    assert_eq!(shares.len(), 5);
    
    for i in 0..shares.len() {
        assert_eq!(shares.get(i).unwrap(), share_ids.get(i).unwrap());
    }
}

#[test]
fn test_get_total_shares() {
    let (env, client, owner) = setup_env();
    client.initialize_fractional(&owner);
    
    assert_eq!(client.get_total_shares(), 0);
    
    let resource_type = Symbol::new(&env, "stellar_dust");
    client.fractionalize_resource(&owner, &resource_type, &100, &5);
    
    assert_eq!(client.get_total_shares(), 6);
}

#[test]
fn test_get_original_resource() {
    let (env, client, owner) = setup_env();
    client.initialize_fractional(&owner);
    
    let resource_type = Symbol::new(&env, "stellar_dust");
    client.fractionalize_resource(&owner, &resource_type, &100, &5);
    
    let original = client.get_original_resource(&resource_type);
    assert!(original.is_some());
    
    let original = original.unwrap();
    assert_eq!(original.owner, owner);
    assert_eq!(original.total_amount, 100);
    assert!(original.is_fractionalized);
}

// ─── Admin/Config Tests ───────────────────────────────────────────────────────

#[test]
fn test_update_fractional_config() {
    let (_env, client, admin) = setup_env();
    client.initialize_fractional(&admin);
    
    let config = client.update_fractional_config(&admin, &5, &25);
    assert_eq!(config.min_share_size, 5);
    assert_eq!(config.max_fractions_per_tx, 25);
}

#[test]
fn test_update_fractional_config_unauthorized() {
    let (env, client, admin) = setup_env();
    let not_admin = Address::generate(&env);
    client.initialize_fractional(&admin);
    
    let result = client.try_update_fractional_config(&not_admin, &5, &25);
    assert!(result.is_err());
}

// ─── Integration Tests ─────────────────────────────────────────────────────

#[test]
fn test_fractional_lifecycle() {
    let (env, client, owner) = setup_env();
    let buyer = Address::generate(&env);
    client.initialize_fractional(&owner);
    
    let resource_type = Symbol::new(&env, "stellar_dust");
    
    // 1. Fractionalize resource
    let share_ids = client.fractionalize_resource(&owner, &resource_type, &100, &10);
    
    // 2. Transfer some shares to buyer
    for i in 0..3 {
        let share_id = share_ids.get(i).unwrap();
        client.transfer_share(&owner, &buyer, &share_id);
    }
    
    // 3. Verify balances
    assert_eq!(client.get_owner_shares(&owner).len(), 7);
    assert_eq!(client.get_owner_shares(&buyer).len(), 3);
    
    // 4. Buyer merges their shares
    let buyer_shares = client.get_owner_shares(&buyer);
    let merged_amount = client.merge_fractions(&buyer, &buyer_shares);
    assert_eq!(merged_amount, 30); // 3 shares * 10 each
    
    // 5. Verify buyer no longer has shares
    assert_eq!(client.get_owner_shares(&buyer).len(), 0);
}

// ─── Constants Tests ─────────────────────────────────────────────────────────

#[test]
fn test_constants() {
    assert_eq!(MAX_FRACTIONS_PER_TX, 50);
}
