#![cfg(test)]

use soroban_sdk::{symbol_short, testutils::Address as _, Address, BytesN, Env, Symbol, Vec};
use stellar_nebula_nomad::{
    batch_commit_stats, commit_private_stat, get_commitment, get_commitment_count, is_opted_in_privacy,
    opt_in_privacy, reset_privacy_burst_counter, verify_private_stat, PrivacyError,
    MAX_COMMITMENTS_PER_TX,
};

fn setup_test_env() -> (Env, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let player = Address::generate(&env);
    (env, player)
}

#[test]
fn test_opt_in_privacy() {
    let (env, player) = setup_test_env();

    // Initially not opted in
    assert!(!is_opted_in_privacy(env.clone(), player.clone()));

    // Opt in
    let result = opt_in_privacy(env.clone(), player.clone());
    assert!(result.is_ok());

    // Now opted in
    assert!(is_opted_in_privacy(env.clone(), player.clone()));
}

#[test]
fn test_commit_private_stat_requires_opt_in() {
    let (env, player) = setup_test_env();

    let stat_type = symbol_short!("score");
    let value = 1000i128;

    // Should fail without opt-in
    let result = commit_private_stat(env.clone(), player.clone(), stat_type.clone(), value);
    assert_eq!(result, Err(PrivacyError::NotOptedIn));
}

#[test]
fn test_commit_private_stat_success() {
    let (env, player) = setup_test_env();

    // Opt in first
    opt_in_privacy(env.clone(), player.clone()).unwrap();

    let stat_type = symbol_short!("score");
    let value = 1000i128;

    // Commit stat
    let result = commit_private_stat(env.clone(), player.clone(), stat_type.clone(), value);
    assert!(result.is_ok());

    let commitment_hash = result.unwrap();
    assert_eq!(commitment_hash.len(), 32);

    // Verify commitment was stored
    let commitment = get_commitment(env.clone(), player.clone(), stat_type.clone());
    assert!(commitment.is_ok());

    let stored = commitment.unwrap();
    assert_eq!(stored.player, player);
    assert_eq!(stored.stat_type, stat_type);
    assert_eq!(stored.commitment_hash, commitment_hash);
    assert!(!stored.verified);

    // Check counter incremented
    assert_eq!(get_commitment_count(env.clone()), 1);
}

#[test]
fn test_commit_duplicate_stat_fails() {
    let (env, player) = setup_test_env();

    opt_in_privacy(env.clone(), player.clone()).unwrap();

    let stat_type = symbol_short!("score");
    let value = 1000i128;

    // First commit succeeds
    let result1 = commit_private_stat(env.clone(), player.clone(), stat_type.clone(), value);
    assert!(result1.is_ok());

    // Second commit with same stat_type should fail
    let result2 = commit_private_stat(env.clone(), player.clone(), stat_type.clone(), value + 100);
    assert_eq!(result2, Err(PrivacyError::CommitmentExists));
}

#[test]
fn test_verify_private_stat_valid_proof() {
    let (env, player) = setup_test_env();

    opt_in_privacy(env.clone(), player.clone()).unwrap();

    let stat_type = symbol_short!("kills");
    let value = 42i128;

    let commitment_hash = commit_private_stat(env.clone(), player.clone(), stat_type, value).unwrap();

    // Create a valid proof (first 32 bytes match commitment, rest can be anything)
    let mut proof_bytes = [0u8; 64];
    for i in 0..32 {
        proof_bytes[i] = commitment_hash.get(i).unwrap();
    }
    // Fill remaining bytes with dummy data
    for i in 32..64 {
        proof_bytes[i] = (i % 256) as u8;
    }
    let proof = BytesN::from_array(&env, &proof_bytes);

    // Verify should succeed
    let result = verify_private_stat(env.clone(), commitment_hash, proof);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), true);
}

#[test]
fn test_verify_private_stat_invalid_proof() {
    let (env, player) = setup_test_env();

    opt_in_privacy(env.clone(), player.clone()).unwrap();

    let stat_type = symbol_short!("deaths");
    let value = 5i128;

    let commitment_hash = commit_private_stat(env.clone(), player.clone(), stat_type, value).unwrap();

    // Create an invalid proof (doesn't match commitment)
    let invalid_proof = BytesN::from_array(&env, &[0u8; 64]);

    // Verify should fail
    let result = verify_private_stat(env.clone(), commitment_hash, invalid_proof);
    assert_eq!(result, Err(PrivacyError::InvalidProof));
}

#[test]
fn test_batch_commit_stats() {
    let (env, player) = setup_test_env();

    opt_in_privacy(env.clone(), player.clone()).unwrap();

    let mut stat_types = Vec::new(&env);
    stat_types.push_back(symbol_short!("score"));
    stat_types.push_back(symbol_short!("kills"));
    stat_types.push_back(symbol_short!("deaths"));

    let mut values = Vec::new(&env);
    values.push_back(1000i128);
    values.push_back(50i128);
    values.push_back(10i128);

    // Batch commit
    let result = batch_commit_stats(env.clone(), player.clone(), stat_types.clone(), values);
    assert!(result.is_ok());

    let commitments = result.unwrap();
    assert_eq!(commitments.len(), 3);

    // Verify all commitments were stored
    for i in 0..3 {
        let stat_type = stat_types.get(i).unwrap();
        let commitment = get_commitment(env.clone(), player.clone(), stat_type);
        assert!(commitment.is_ok());
    }

    // Check counter
    assert_eq!(get_commitment_count(env.clone()), 3);
}

#[test]
fn test_batch_commit_exceeds_limit() {
    let (env, player) = setup_test_env();

    opt_in_privacy(env.clone(), player.clone()).unwrap();

    // Create more than MAX_COMMITMENTS_PER_TX stats
    let mut stat_types = Vec::new(&env);
    let mut values = Vec::new(&env);

    for i in 0..(MAX_COMMITMENTS_PER_TX + 1) {
        stat_types.push_back(Symbol::new(&env, &format!("stat{}", i)));
        values.push_back(i as i128);
    }

    // Should fail due to burst limit
    let result = batch_commit_stats(env.clone(), player.clone(), stat_types, values);
    assert_eq!(result, Err(PrivacyError::BurstLimitExceeded));
}

#[test]
fn test_batch_commit_mismatched_lengths() {
    let (env, player) = setup_test_env();

    opt_in_privacy(env.clone(), player.clone()).unwrap();

    let mut stat_types = Vec::new(&env);
    stat_types.push_back(symbol_short!("score"));
    stat_types.push_back(symbol_short!("kills"));

    let mut values = Vec::new(&env);
    values.push_back(1000i128);
    // Missing second value

    // Should fail due to mismatched lengths
    let result = batch_commit_stats(env.clone(), player.clone(), stat_types, values);
    assert_eq!(result, Err(PrivacyError::InvalidProof));
}

#[test]
fn test_burst_limit_enforcement() {
    let (env, player) = setup_test_env();

    opt_in_privacy(env.clone(), player.clone()).unwrap();

    // Commit up to the limit
    for i in 0..MAX_COMMITMENTS_PER_TX {
        let stat_type = Symbol::new(&env, &format!("stat{}", i));
        let result = commit_private_stat(env.clone(), player.clone(), stat_type, i as i128);
        assert!(result.is_ok());
    }

    // Next commit should fail
    let stat_type = symbol_short!("extra");
    let result = commit_private_stat(env.clone(), player.clone(), stat_type, 999i128);
    assert_eq!(result, Err(PrivacyError::BurstLimitExceeded));

    // Reset burst counter
    reset_privacy_burst_counter(env.clone());

    // Now should work again
    let stat_type = symbol_short!("extra");
    let result = commit_private_stat(env.clone(), player.clone(), stat_type, 999i128);
    assert!(result.is_ok());
}

#[test]
fn test_get_commitment_not_found() {
    let (env, player) = setup_test_env();

    let stat_type = symbol_short!("missing");

    let result = get_commitment(env.clone(), player.clone(), stat_type);
    assert_eq!(result, Err(PrivacyError::CommitmentNotFound));
}

#[test]
fn test_multiple_players_independent_commitments() {
    let env = Env::default();
    env.mock_all_auths();

    let player1 = Address::generate(&env);
    let player2 = Address::generate(&env);

    // Both opt in
    opt_in_privacy(env.clone(), player1.clone()).unwrap();
    opt_in_privacy(env.clone(), player2.clone()).unwrap();

    let stat_type = symbol_short!("score");

    // Both commit same stat type with different values
    let hash1 = commit_private_stat(env.clone(), player1.clone(), stat_type.clone(), 1000i128).unwrap();
    let hash2 = commit_private_stat(env.clone(), player2.clone(), stat_type.clone(), 2000i128).unwrap();

    // Hashes should be different (different players/timestamps)
    assert_ne!(hash1, hash2);

    // Both commitments should be retrievable
    let commitment1 = get_commitment(env.clone(), player1.clone(), stat_type.clone()).unwrap();
    let commitment2 = get_commitment(env.clone(), player2.clone(), stat_type.clone()).unwrap();

    assert_eq!(commitment1.player, player1);
    assert_eq!(commitment2.player, player2);
    assert_eq!(commitment1.commitment_hash, hash1);
    assert_eq!(commitment2.commitment_hash, hash2);

    // Total count should be 2
    assert_eq!(get_commitment_count(env.clone()), 2);
}

#[test]
fn test_commitment_timestamp_recorded() {
    let (env, player) = setup_test_env();

    opt_in_privacy(env.clone(), player.clone()).unwrap();

    let stat_type = symbol_short!("time");
    let value = 123i128;

    commit_private_stat(env.clone(), player.clone(), stat_type.clone(), value).unwrap();

    let commitment = get_commitment(env.clone(), player.clone(), stat_type).unwrap();

    // Timestamp should be set and reasonable
    assert!(commitment.timestamp > 0);
}

#[test]
fn test_different_stat_types_same_player() {
    let (env, player) = setup_test_env();

    opt_in_privacy(env.clone(), player.clone()).unwrap();

    // Commit multiple different stat types
    let stats = vec![
        (symbol_short!("score"), 1000i128),
        (symbol_short!("kills"), 50i128),
        (symbol_short!("deaths"), 10i128),
        (symbol_short!("assists"), 25i128),
    ];

    for (stat_type, value) in stats.iter() {
        let result = commit_private_stat(env.clone(), player.clone(), stat_type.clone(), *value);
        assert!(result.is_ok());
    }

    // All should be retrievable
    for (stat_type, _) in stats.iter() {
        let commitment = get_commitment(env.clone(), player.clone(), stat_type.clone());
        assert!(commitment.is_ok());
    }

    assert_eq!(get_commitment_count(env.clone()), 4);
}
