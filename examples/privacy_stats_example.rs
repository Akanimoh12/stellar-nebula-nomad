#![cfg(test)]

/// Example demonstrating privacy-preserving player stats usage
/// 
/// This example shows how to:
/// 1. Opt into privacy features
/// 2. Commit stats without revealing values
/// 3. Verify commitments with proofs
/// 4. Use batch operations for efficiency

use soroban_sdk::{symbol_short, testutils::Address as _, Address, BytesN, Env, Symbol, Vec};
use stellar_nebula_nomad::{
    batch_commit_stats, commit_private_stat, get_commitment, get_commitment_count,
    is_opted_in_privacy, opt_in_privacy, verify_private_stat, PrivacyError,
};

#[test]
fn example_basic_privacy_workflow() {
    // Setup environment and create a player
    let env = Env::default();
    env.mock_all_auths();
    let player = Address::generate(&env);

    println!("=== Basic Privacy Workflow ===\n");

    // Step 1: Check opt-in status (should be false initially)
    let opted_in = is_opted_in_privacy(env.clone(), player.clone());
    println!("1. Initial opt-in status: {}", opted_in);
    assert!(!opted_in);

    // Step 2: Opt into privacy features
    println!("2. Opting into privacy features...");
    opt_in_privacy(env.clone(), player.clone()).expect("Failed to opt in");
    
    let opted_in = is_opted_in_privacy(env.clone(), player.clone());
    println!("   Opt-in status after: {}", opted_in);
    assert!(opted_in);

    // Step 3: Commit a private stat (e.g., player score)
    println!("3. Committing private score stat...");
    let score_value = 1000i128;
    let score_commitment = commit_private_stat(
        env.clone(),
        player.clone(),
        symbol_short!("score"),
        score_value,
    )
    .expect("Failed to commit stat");
    
    println!("   Commitment hash: {:?}", score_commitment);
    println!("   (Raw score value {} is NOT stored on-chain)", score_value);

    // Step 4: Retrieve the commitment
    println!("4. Retrieving commitment from storage...");
    let stored_commitment = get_commitment(
        env.clone(),
        player.clone(),
        symbol_short!("score"),
    )
    .expect("Failed to get commitment");
    
    println!("   Player: {:?}", stored_commitment.player);
    println!("   Stat type: {:?}", stored_commitment.stat_type);
    println!("   Timestamp: {}", stored_commitment.timestamp);
    println!("   Verified: {}", stored_commitment.verified);

    // Step 5: Create a proof and verify
    println!("5. Creating and verifying proof...");
    
    // In a real implementation, this would be a proper ZK proof
    // For this example, we create a simple proof where first 32 bytes match commitment
    let mut proof_bytes = [0u8; 64];
    for i in 0..32 {
        proof_bytes[i] = score_commitment.get(i).unwrap();
    }
    // Fill remaining bytes with dummy proof data
    for i in 32..64 {
        proof_bytes[i] = (i % 256) as u8;
    }
    let proof = BytesN::from_array(&env, &proof_bytes);
    
    let is_valid = verify_private_stat(env.clone(), score_commitment, proof)
        .expect("Verification failed");
    
    println!("   Proof valid: {}", is_valid);
    assert!(is_valid);

    // Step 6: Check global commitment count
    let total_commitments = get_commitment_count(env.clone());
    println!("6. Total commitments in system: {}", total_commitments);
    assert_eq!(total_commitments, 1);

    println!("\n=== Workflow Complete ===");
}

#[test]
fn example_batch_commit_workflow() {
    let env = Env::default();
    env.mock_all_auths();
    let player = Address::generate(&env);

    println!("=== Batch Commit Workflow ===\n");

    // Opt in first
    opt_in_privacy(env.clone(), player.clone()).expect("Failed to opt in");
    println!("1. Player opted into privacy features");

    // Prepare multiple stats to commit
    let mut stat_types = Vec::new(&env);
    stat_types.push_back(symbol_short!("score"));
    stat_types.push_back(symbol_short!("kills"));
    stat_types.push_back(symbol_short!("deaths"));
    stat_types.push_back(symbol_short!("assists"));
    stat_types.push_back(symbol_short!("wins"));

    let mut values = Vec::new(&env);
    values.push_back(1000i128);  // score
    values.push_back(50i128);    // kills
    values.push_back(10i128);    // deaths
    values.push_back(25i128);    // assists
    values.push_back(15i128);    // wins

    println!("2. Committing {} stats in batch:", stat_types.len());
    println!("   - score: 1000");
    println!("   - kills: 50");
    println!("   - deaths: 10");
    println!("   - assists: 25");
    println!("   - wins: 15");

    // Batch commit all stats at once
    let commitments = batch_commit_stats(
        env.clone(),
        player.clone(),
        stat_types.clone(),
        values,
    )
    .expect("Failed to batch commit");

    println!("3. Batch commit successful!");
    println!("   Generated {} commitment hashes", commitments.len());

    // Verify all commitments were stored
    println!("4. Verifying all commitments are retrievable:");
    for i in 0..stat_types.len() {
        let stat_type = stat_types.get(i).unwrap();
        let commitment = get_commitment(env.clone(), player.clone(), stat_type)
            .expect("Failed to retrieve commitment");
        println!("   ✓ {} commitment found", stat_type.to_string());
    }

    let total = get_commitment_count(env.clone());
    println!("5. Total commitments in system: {}", total);
    assert_eq!(total, 5);

    println!("\n=== Batch Workflow Complete ===");
}

#[test]
fn example_multi_player_leaderboard() {
    let env = Env::default();
    env.mock_all_auths();

    println!("=== Multi-Player Leaderboard Example ===\n");

    // Create multiple players
    let player1 = Address::generate(&env);
    let player2 = Address::generate(&env);
    let player3 = Address::generate(&env);

    println!("1. Setting up 3 players for private leaderboard");

    // All players opt in
    opt_in_privacy(env.clone(), player1.clone()).unwrap();
    opt_in_privacy(env.clone(), player2.clone()).unwrap();
    opt_in_privacy(env.clone(), player3.clone()).unwrap();
    println!("   All players opted in");

    // Each player commits their score privately
    println!("2. Players committing their scores:");
    
    let score1 = 1500i128;
    let commitment1 = commit_private_stat(
        env.clone(),
        player1.clone(),
        symbol_short!("score"),
        score1,
    )
    .unwrap();
    println!("   Player 1: Score {} committed (hash: {:?})", score1, commitment1);

    let score2 = 2000i128;
    let commitment2 = commit_private_stat(
        env.clone(),
        player2.clone(),
        symbol_short!("score"),
        score2,
    )
    .unwrap();
    println!("   Player 2: Score {} committed (hash: {:?})", score2, commitment2);

    let score3 = 1200i128;
    let commitment3 = commit_private_stat(
        env.clone(),
        player3.clone(),
        symbol_short!("score"),
        score3,
    )
    .unwrap();
    println!("   Player 3: Score {} committed (hash: {:?})", score3, commitment3);

    // Verify commitments are different
    assert_ne!(commitment1, commitment2);
    assert_ne!(commitment2, commitment3);
    assert_ne!(commitment1, commitment3);
    println!("3. All commitments are unique ✓");

    // Simulate leaderboard verification
    println!("4. Leaderboard verifying commitments:");
    
    // Create proofs for each player
    let mut proof1_bytes = [0u8; 64];
    for i in 0..32 {
        proof1_bytes[i] = commitment1.get(i).unwrap();
    }
    let proof1 = BytesN::from_array(&env, &proof1_bytes);
    
    let valid1 = verify_private_stat(env.clone(), commitment1, proof1).unwrap();
    println!("   Player 1 proof: {} ✓", if valid1 { "VALID" } else { "INVALID" });

    let total_commitments = get_commitment_count(env.clone());
    println!("5. Total commitments in leaderboard: {}", total_commitments);
    assert_eq!(total_commitments, 3);

    println!("\n=== Leaderboard Example Complete ===");
    println!("Note: In production, use proper ZK proofs for ranking without revealing scores");
}

#[test]
fn example_error_handling() {
    let env = Env::default();
    env.mock_all_auths();
    let player = Address::generate(&env);

    println!("=== Error Handling Examples ===\n");

    // Example 1: Trying to commit without opt-in
    println!("1. Attempting to commit without opt-in:");
    let result = commit_private_stat(
        env.clone(),
        player.clone(),
        symbol_short!("score"),
        1000i128,
    );
    match result {
        Err(PrivacyError::NotOptedIn) => {
            println!("   ✓ Correctly rejected: Player not opted in");
        }
        _ => panic!("Expected NotOptedIn error"),
    }

    // Opt in for remaining examples
    opt_in_privacy(env.clone(), player.clone()).unwrap();

    // Example 2: Duplicate commitment
    println!("2. Attempting duplicate commitment:");
    commit_private_stat(
        env.clone(),
        player.clone(),
        symbol_short!("score"),
        1000i128,
    )
    .unwrap();
    
    let result = commit_private_stat(
        env.clone(),
        player.clone(),
        symbol_short!("score"),
        2000i128,
    );
    match result {
        Err(PrivacyError::CommitmentExists) => {
            println!("   ✓ Correctly rejected: Commitment already exists");
        }
        _ => panic!("Expected CommitmentExists error"),
    }

    // Example 3: Invalid proof
    println!("3. Attempting verification with invalid proof:");
    let commitment = commit_private_stat(
        env.clone(),
        player.clone(),
        symbol_short!("kills"),
        50i128,
    )
    .unwrap();
    
    let invalid_proof = BytesN::from_array(&env, &[0u8; 64]);
    let result = verify_private_stat(env.clone(), commitment, invalid_proof);
    match result {
        Err(PrivacyError::InvalidProof) => {
            println!("   ✓ Correctly rejected: Invalid proof");
        }
        _ => panic!("Expected InvalidProof error"),
    }

    // Example 4: Commitment not found
    println!("4. Attempting to retrieve non-existent commitment:");
    let result = get_commitment(
        env.clone(),
        player.clone(),
        symbol_short!("missing"),
    );
    match result {
        Err(PrivacyError::CommitmentNotFound) => {
            println!("   ✓ Correctly rejected: Commitment not found");
        }
        _ => panic!("Expected CommitmentNotFound error"),
    }

    println!("\n=== Error Handling Complete ===");
}

#[test]
fn example_gas_optimization() {
    let env = Env::default();
    env.mock_all_auths();
    let player = Address::generate(&env);

    println!("=== Gas Optimization Example ===\n");

    opt_in_privacy(env.clone(), player.clone()).unwrap();

    // Inefficient: Multiple individual commits
    println!("1. Inefficient approach (individual commits):");
    println!("   Committing 5 stats one by one...");
    // This would require 5 separate transactions in production
    
    // Efficient: Batch commit
    println!("2. Efficient approach (batch commit):");
    let mut stat_types = Vec::new(&env);
    let mut values = Vec::new(&env);
    
    for i in 0..5 {
        stat_types.push_back(Symbol::new(&env, &format!("stat{}", i)));
        values.push_back((i * 100) as i128);
    }
    
    println!("   Committing 5 stats in single batch operation...");
    let commitments = batch_commit_stats(
        env.clone(),
        player.clone(),
        stat_types,
        values,
    )
    .unwrap();
    
    println!("   ✓ Batch commit completed");
    println!("   Generated {} commitments", commitments.len());
    println!("\n   Benefits:");
    println!("   - Single authentication check");
    println!("   - Single opt-in verification");
    println!("   - Amortized storage costs");
    println!("   - Lower total gas consumption");

    println!("\n=== Gas Optimization Complete ===");
}
