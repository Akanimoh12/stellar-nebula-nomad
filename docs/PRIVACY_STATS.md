# Privacy-Preserving Player Stats

## Overview

The Privacy Stats module enables players to share statistics anonymously using zero-knowledge style commitments. This allows participation in leaderboards and competitions without revealing raw performance data.

## Features

### Zero-Knowledge Style Commitments

- Players commit to stat values without revealing the actual numbers
- Commitments are cryptographically hashed and stored on-chain
- Verification can occur without exposing underlying data

### Opt-In System

- Privacy features are entirely optional
- Players must explicitly opt in before using privacy commitments
- Non-opted-in players can still use traditional stat tracking

### Burst Protection

- Maximum 10 commitments per transaction to prevent spam
- Burst counter can be reset between transactions
- Protects network resources and storage costs

## Architecture

### Storage Keys

- `Commitment(Address, Symbol)` - Stores commitment data per player and stat type
- `OptIn(Address)` - Tracks which players have opted into privacy features
- `CommitmentCount` - Global counter of all commitments
- `BurstCounter` - Rate limiting counter for current transaction

### Data Structures

#### StatCommitment

```rust
pub struct StatCommitment {
    pub player: Address,
    pub stat_type: Symbol,
    pub commitment_hash: BytesN<32>,
    pub timestamp: u64,
    pub verified: bool,
}
```

### Error Types

- `NotOptedIn` - Player must opt in before using privacy features
- `InvalidProof` - Provided proof doesn't match commitment
- `CommitmentNotFound` - No commitment exists for the given player/stat
- `BurstLimitExceeded` - Too many commitments in single transaction
- `CommitmentExists` - Duplicate commitment for same stat type

## API Reference

### Setup Functions

#### `opt_in_privacy(player: Address) -> Result<(), PrivacyError>`

Enable privacy features for a player. Must be called before any commitments can be made.

**Events:** Emits `PrivateStatOptIn` event

**Example:**

```rust
opt_in_privacy(env, player_address)?;
```

#### `is_opted_in_privacy(player: Address) -> bool`

Check if a player has opted into privacy features.

**Pure function** - No state changes

### Commitment Functions

#### `commit_private_stat(player: Address, stat_type: Symbol, value: i128) -> Result<BytesN<32>, PrivacyError>`

Create a cryptographic commitment to a stat value without revealing it.

**Requirements:**

- Player must be opted in
- Stat type must not already have a commitment
- Must not exceed burst limit (10 per tx)

**Returns:** 32-byte commitment hash

**Events:** Emits `PrivateStatCommitted` event

**Example:**

```rust
let commitment = commit_private_stat(
    env,
    player,
    symbol_short!("score"),
    1000i128
)?;
```

#### `batch_commit_stats(player: Address, stat_types: Vec<Symbol>, values: Vec<i128>) -> Result<Vec<BytesN<32>>, PrivacyError>`

Commit multiple stats in a single transaction (up to 10).

**Requirements:**

- Player must be opted in
- stat_types and values must have same length
- Total count must not exceed MAX_COMMITMENTS_PER_TX (10)
- No duplicate stat types

**Returns:** Vector of commitment hashes

**Example:**

```rust
let mut types = Vec::new(&env);
types.push_back(symbol_short!("score"));
types.push_back(symbol_short!("kills"));

let mut values = Vec::new(&env);
values.push_back(1000i128);
values.push_back(50i128);

let commitments = batch_commit_stats(env, player, types, values)?;
```

### Verification Functions

#### `verify_private_stat(commitment: BytesN<32>, proof: BytesN<64>) -> Result<bool, PrivacyError>`

Verify a commitment using a zero-knowledge proof without revealing the underlying data.

**Pure function** - No authentication required

**Returns:** `true` if proof is valid

**Events:** Emits `PrivateStatVerified` event

**Example:**

```rust
let is_valid = verify_private_stat(env, commitment_hash, proof)?;
```

### Query Functions

#### `get_commitment(player: Address, stat_type: Symbol) -> Result<StatCommitment, PrivacyError>`

Retrieve a stored commitment for a specific player and stat type.

**Example:**

```rust
let commitment = get_commitment(env, player, symbol_short!("score"))?;
```

#### `get_commitment_count() -> u64`

Get the total number of commitments made across all players.

**Pure function**

### Utility Functions

#### `reset_privacy_burst_counter()`

Reset the burst counter for a new transaction. Should be called at the start of each transaction that will make commitments.

## Integration with Leaderboards

The privacy system is designed to integrate with existing leaderboard functionality:

1. **Optional Participation**: Players can choose to use privacy commitments or traditional stats
2. **Verification**: Leaderboard systems can verify commitments without seeing raw values
3. **Ranking**: Commitments can be compared using zero-knowledge proofs
4. **Transparency**: All commitments are on-chain and auditable

## Security Considerations

### Current Implementation

- Uses SHA-256 hashing for commitments
- Simple proof verification (first 32 bytes match)
- Suitable for demonstration and testing

### Production Recommendations

For production deployment, consider upgrading to:

- **Proper ZK-SNARKs**: Use libraries like Groth16 or PLONK
- **Pedersen Commitments**: Homomorphic properties for range proofs
- **Bulletproofs**: Efficient range proofs without trusted setup
- **Recursive SNARKs**: For complex stat aggregations

### Attack Vectors

- **Replay Attacks**: Mitigated by including timestamp in commitment
- **Duplicate Commitments**: Prevented by checking for existing commitments
- **Spam**: Protected by burst limits (10 per tx)
- **Front-running**: Commitments are binding once submitted

## Future Enhancements

### Planned Features

1. **Full ZK-SNARK Integration**: Replace simple hashing with proper zero-knowledge proofs
2. **Range Proofs**: Prove stats are within valid ranges without revealing exact values
3. **Aggregation**: Combine multiple commitments for team/alliance stats
4. **Revocation**: Allow players to update or revoke commitments
5. **Selective Disclosure**: Reveal specific stats while keeping others private

### Upgrade Path

The module is designed with upgradeability in mind:

- Storage keys are versioned
- Proof format is extensible (64 bytes allows for future proof types)
- Error types can be extended without breaking existing code

## Testing

Comprehensive test suite covers:

- Opt-in functionality
- Commitment creation and storage
- Duplicate prevention
- Burst limit enforcement
- Proof verification (valid and invalid)
- Batch operations
- Multi-player scenarios
- Edge cases and error conditions

Run tests:

```bash
cargo test --test test_privacy_stats
```

## Gas Optimization

### Single Commitment

- Opt-in: ~1 storage write
- Commit: ~2 storage writes + 1 hash operation
- Verify: Pure function, minimal gas

### Batch Commitment

- More efficient than individual commits
- Amortizes authentication overhead
- Single burst check for all operations

### Best Practices

1. Use batch operations when committing multiple stats
2. Reset burst counter at transaction start
3. Opt in once, commit many times
4. Cache commitment hashes off-chain for verification

## Examples

### Basic Usage

```rust
// 1. Opt in to privacy features
opt_in_privacy(env.clone(), player.clone())?;

// 2. Commit a stat
let commitment = commit_private_stat(
    env.clone(),
    player.clone(),
    symbol_short!("score"),
    1000i128
)?;

// 3. Later, verify the commitment
let proof = generate_proof(1000i128); // Off-chain proof generation
let is_valid = verify_private_stat(env.clone(), commitment, proof)?;
```

### Batch Usage

```rust
// Opt in first
opt_in_privacy(env.clone(), player.clone())?;

// Prepare multiple stats
let mut types = Vec::new(&env);
types.push_back(symbol_short!("score"));
types.push_back(symbol_short!("kills"));
types.push_back(symbol_short!("deaths"));

let mut values = Vec::new(&env);
values.push_back(1000i128);
values.push_back(50i128);
values.push_back(10i128);

// Commit all at once
let commitments = batch_commit_stats(
    env.clone(),
    player.clone(),
    types,
    values
)?;
```

### Leaderboard Integration

```rust
// Player commits their score privately
let score_commitment = commit_private_stat(
    env.clone(),
    player.clone(),
    symbol_short!("score"),
    player_score
)?;

// Leaderboard can verify without seeing the score
let proof = player_generates_proof_offchain(player_score);
let is_valid = verify_private_stat(
    env.clone(),
    score_commitment,
    proof
)?;

// Leaderboard accepts the commitment if valid
if is_valid {
    add_to_leaderboard(player, score_commitment);
}
```

## Constants

- `MAX_COMMITMENTS_PER_TX`: 10 - Maximum commitments per transaction

## Events

### PrivateStatOptIn

Emitted when a player opts into privacy features.

- Topics: `("privacy", "optin")`
- Data: `player: Address`

### PrivateStatCommitted

Emitted when a stat commitment is created.

- Topics: `("privacy", "commit")`
- Data: `(player: Address, stat_type: Symbol, commitment_hash: BytesN<32>)`

### PrivateStatVerified

Emitted when a commitment is successfully verified.

- Topics: `("privacy", "verify")`
- Data: `(commitment: BytesN<32>, valid: bool)`

## Responsible Data Handling

### Privacy Principles

1. **Opt-In Only**: No player is forced to use privacy features
2. **No Raw Data Storage**: Only commitments are stored on-chain
3. **Transparent Verification**: Anyone can verify commitments
4. **Player Control**: Players decide what to commit and when

### Compliance

- GDPR-friendly: No personal data stored
- Right to be forgotten: Commitments can be designed to expire
- Data minimization: Only hashes stored, not raw values
- Purpose limitation: Stats used only for game mechanics

## Support

For questions or issues:

- Check the test suite for usage examples
- Review the inline code documentation
- Consult the main ARCHITECTURE.md for system overview
