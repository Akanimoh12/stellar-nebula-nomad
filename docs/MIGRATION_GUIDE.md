# Data Migration & Schema Evolution Guide

This guide provides technical instructions for executing data schema migrations in `stellar-nebula-nomad` contracts using `src/migration_framework.rs`.

---

## 1. Schema Migration Framework Overview

`migration_framework.rs` manages breaking schema changes and state evolution.

### Key Components
- **`MigrationRecord`**: Tracks migration status (`pending`, `in_progress`, `completed`, `failed`, `rolled_back`), from/to version tags, timestamp, and SHA-256 state checksums.
- **`BatchMigrationState`**: Monitors batch execution progress (`batch_id`, `batch_index`, `total_batches`, `records_processed`).
- **`MigrationKey`**: Storage key enumeration for schema versioning, batch state, rollback checkpoints, and compatibility flags.

---

## 2. Migration Script Documentation & Automation Patterns

### Batch Processing Constraints
- **Maximum Batch Size**: `MAX_MIGRATION_BATCH = 100` records per transaction batch to prevent transaction resource/gas limit overflow.
- **History Retention**: `MAX_MIGRATION_HISTORY = 50` records retained on-chain.

### Example Migration Execution Script (Rust / CLI Integration)

```rust
use soroban_sdk::{Env, Address, Vec, Bytes, symbol_short};
use stellar_nebula_nomad::migration_framework::{
    initialize_migrations, plan_migration, dry_run_migration,
    execute_migration_batch, record_migration_completion, MAX_MIGRATION_BATCH,
};

pub fn run_schema_v1_to_v2_migration(
    env: &Env,
    admin: &Address,
    dataset: Vec<Bytes>,
) -> Result<(), u32> {
    // 1. Plan Migration
    let migration = plan_migration(env, admin, 1, 2, symbol_short!("v1_to_v2"))
        .map_err(|e| e as u32)?;

    // 2. Perform Dry-Run Validation
    let dry_run = dry_run_migration(env, admin, migration.id, dataset.clone())
        .map_err(|e| e as u32)?;

    if !dry_run.would_succeed {
        return Err(3); // Validation failure
    }

    // 3. Process Data in Batches
    let total_records = dataset.len();
    let total_batches = (total_records + MAX_MIGRATION_BATCH - 1) / MAX_MIGRATION_BATCH;

    for batch_idx in 0..total_batches {
        let start = batch_idx * MAX_MIGRATION_BATCH;
        let end = (start + MAX_MIGRATION_BATCH).min(total_records);
        let mut batch_chunk = Vec::new(env);
        for i in start..end {
            batch_chunk.push_back(dataset.get(i).unwrap());
        }

        execute_migration_batch(
            env,
            admin,
            migration.id,
            batch_idx,
            total_batches,
            batch_chunk,
        ).map_err(|e| e as u32)?;
    }

    // 4. Mark Completed
    record_migration_completion(env, migration.id, 1, 2, total_records);

    Ok(())
}
```

---

## 3. Backward Compatibility Management

When introducing a non-breaking schema evolution, maintain dual-read capability. For breaking changes:

1. Flag version incompatibility using `mark_incompatible`:
   ```rust
   migration_framework::mark_incompatible(&env, &admin, 1, 3)?;
   ```
2. Verify version safety before executing queries:
   ```rust
   let compatible = migration_framework::is_backward_compatible(&env, 1, 3);
   if !compatible {
       // Enforce full migration requirement
   }
   ```

---

## 4. Verification & Testing Checklist

- [ ] Execute `cargo test --test migration_tests` to verify batch state transitions.
- [ ] Confirm events are emitted for `(migration, init)`, `(migration, planned)`, `(migration, batch_ok)`, and `(migration, completed)`.
- [ ] Ensure checksum validation passes across all migrated state keys.
