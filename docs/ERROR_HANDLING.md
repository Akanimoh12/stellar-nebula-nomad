# Error Handling Guidelines

## Overview

This document describes the error handling strategy across the stellar-nebula-nomad contract suite. All modules use `#[contracterror]` enums with the `StandardContractError` trait for consistent error classification, context propagation, and client handling.

## Error Architecture

### Error Standards

Every contract error implements `StandardContractError`, which provides:

```rust
pub trait StandardContractError {
    fn descriptor(self) -> ErrorDescriptor;
}
```

The descriptor includes:
- **module**: Namespace for error identity (e.g., "batch", "nebula_gen")
- **code**: Stable u32 code in the contract ABI
- **kind**: Semantic class (Validation, Authorization, NotFound, Conflict, ResourceLimit, Internal)
- **retryable**: Whether retrying without changes may succeed

### Error Classification

#### Validation Errors (non-retryable)
- Invalid input parameters, out-of-range values, malformed data
- Examples: InvalidShipId, InvalidRegionId, BatchLimitExceeded

#### Authorization Errors (non-retryable)
- Missing permissions, unauthorized caller, invalid signatures
- Examples: AdminRequired, Unauthorized

#### NotFound Errors (non-retryable)
- Resource does not exist, layout expired, player data missing
- Examples: LayoutNotFound, ShipNotFound

#### Conflict Errors (non-retryable)
- State contradicts operation, already initialized, race condition
- Examples: AlreadyInitialized, OperationInProgress

#### ResourceLimit Errors (retryable)
- Gas exhausted, rate limit exceeded, batch size exceeded
- Examples: GasLimitExceeded, RateLimitExceeded

#### Internal Errors (retryable)
- Unexpected state corruption, partial completion, rollback required
- Examples: OperationFailed, InternalInconsistency

## Error Context Requirements

Each error must include sufficient context for debugging:
1. **Resource Identity**: Which ship, player, or resource caused the error
2. **Value Context**: The invalid value, limit, or constraint violated
3. **Operation Context**: What was being attempted when the error occurred

## Module-Specific Errors

### nebula_gen::NebulaError
- `InvalidShipId`: ship_id must be > 0
- `InvalidRegionId`: region_id must be in [1, MAX_REGION_ID]
- `RateLimitExceeded`: Layout generation rate limit exceeded (retryable)

### batch_processor::BatchError
- `BatchLimitExceeded`: Batch size > MAX_BATCH_SIZE
- `GasLimitExceeded`: Estimated gas exceeds budget (retryable)
- `ShipNotFound`: Ship ID not in player's fleet

### resource_minter::MinterError
- `InvalidAmount`: Amount exceeds limits or is zero
- `InsufficientResources`: Player lacks required resources
- `RateLimitExceeded`: Minting rate limit exceeded (retryable)

### ship_upgrade::ShipUpgradeError
- `InvalidShipId`: Ship not found or invalid ID
- `InvalidUpgradeLevel`: Upgrade level out of range
- `MaxLevelReached`: Cannot upgrade beyond maximum
- `RateLimitExceeded`: Upgrade rate limit exceeded (retryable)

## Testing Error Conditions

Each module must include tests for error paths:

```rust
#[test]
fn test_invalid_ship_id_rejected() {
    let result = generate_nebula(INVALID_SHIP_ID, region_id, seed);
    assert!(matches!(result, Err(NebulaError::InvalidShipId)));
}

#[test]
fn test_rate_limit_exceeded_is_retryable() {
    let descriptor = NebulaError::RateLimitExceeded.descriptor();
    assert_eq!(descriptor.kind, ErrorKind::ResourceLimit);
    assert!(descriptor.retryable);
}
```

## Gas Considerations

Error handling is cheap:
- Error enum variants: ~50-100 gas each
- Error descriptor construction: ~200 gas
- Error propagation: negligible overhead

Errors should be preferred over panics because panics abort entire transactions with no recovery path.
