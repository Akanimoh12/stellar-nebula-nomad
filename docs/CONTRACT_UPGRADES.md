# Contract Upgrade Procedures & Security Guidelines

This document details the architecture, pre-upgrade verification, step-by-step upgrading process, and rollback procedures for Soroban smart contracts within the `stellar-nebula-nomad` ecosystem.

---

## 1. Overview & Architecture

Contracts in `stellar-nebula-nomad` use Soroban's native contract upgrading mechanisms combined with state versioning defined in `src/migration_framework.rs`.

### Key Security Design Principles
- **WASM Hash Verification**: All contract upgrades update the contract code hash via Soroban's `env.deployer().update_current_contract_wasm(new_wasm_hash)`.
- **Role-Based Access Control (RBAC)**: Only authorized admin addresses (`admin.require_auth()`) can trigger upgrade and migration functions.
- **Atomic State Versioning**: Every contract maintains a `CurrentSchemaVersion` state key to ensure binary logic and storage schemas stay in sync.

---

## 2. Pre-Upgrade Checklist

Before initiating any production upgrade, complete the following mandatory verification steps:

- [ ] **Contract Compilation & Verification**
  - [ ] Build release WASM binary: `cargo build --target wasm32-unknown-unknown --release`
  - [ ] Optimize WASM size using `soroban contract optimize --wasm target/wasm32-unknown-unknown/release/stellar_nebula_nomad.wasm`
  - [ ] Calculate new WASM SHA-256 hash: `sha256sum target/wasm32-unknown-unknown/release/stellar_nebula_nomad.optimized.wasm`
- [ ] **Dry-Run & Validation**
  - [ ] Execute `dry_run_migration()` with sample state payloads to verify gas usage and data schema compatibility.
  - [ ] Verify `is_backward_compatible()` returns `true` for target version transitions.
- [ ] **State Snapshot & Rollback Checkpoint**
  - [ ] Export current state snapshot using `state_snapshot::create_snapshot()`.
  - [ ] Verify rollback checkpoint storage allocation (`MigrationKey::RollbackCheckpoint`).
- [ ] **Governance & Multi-Sig Signoff**
  - [ ] Prepare transaction authorization payloads for admin signatures.
  - [ ] Confirm emergency pause controls are operational via `emergency_controls.rs`.

---

## 3. Step-by-Step Upgrade Procedure

### Step 1: Install New WASM Code
Upload the optimized WASM byte code to the Soroban network:
```bash
soroban contract install \
  --wasm target/wasm32-unknown-unknown/release/stellar_nebula_nomad.optimized.wasm \
  --source admin-identity \
  --network mainnet
```
Note down the returned `<NEW_WASM_HASH>`.

### Step 2: Plan the Migration
Invoke `plan_migration` via the migration framework to initialize the upgrade metadata record:
```rust
let record = migration_framework::plan_migration(
    &env,
    &admin,
    current_version,
    target_version,
    symbol_short!("upg_v2"),
)?;
```

### Step 3: Run Dry-Run Verification
Execute `dry_run_migration` with sample records to validate schema constraints without committing storage changes:
```rust
let report = migration_framework::dry_run_migration(
    &env,
    &admin,
    record.id,
    sample_records,
)?;
assert!(report.would_succeed, "Dry-run validation failed!");
```

### Step 4: Execute Code Upgrade & Batch State Migration
Update the contract WASM code and run batch migration steps:
```rust
// 1. Update WASM code on chain
env.deployer().update_current_contract_wasm(new_wasm_hash);

// 2. Process state migration batches (max 100 records per batch)
let batch_state = migration_framework::execute_migration_batch(
    &env,
    &admin,
    record.id,
    batch_index,
    total_batches,
    batch_data,
)?;
```

### Step 5: Finalize Migration & Update Version
Record completion in migration history and advance schema version:
```rust
migration_framework::record_migration_completion(
    &env,
    record.id,
    from_version,
    to_version,
    total_records_processed,
);
```

---

## 4. Rollback Procedures

If an anomaly, state mismatch, or unexpected behavior occurs post-upgrade, execute the automated rollback path immediately:

1. **Trigger Emergency Pause**:
   ```rust
   emergency_controls::pause_contract(&env, &admin);
   ```

2. **Invoke Rollback Migration**:
   Call `rollback_migration` to revert state modifications from the saved checkpoint (`MigrationKey::RollbackCheckpoint`):
   ```rust
   migration_framework::rollback_migration(&env, &admin, migration_id)?;
   ```

3. **Revert Contract WASM**:
   Update contract WASM back to the previous verified WASM hash:
   ```rust
   env.deployer().update_current_contract_wasm(previous_wasm_hash);
   ```

4. **Verify State Consistency & Unpause**:
   Audit storage keys and unpause contract operations.

---

## 5. Security & Access Control

- All upgrade entry points require explicit authorization (`admin.require_auth()`).
- Unauthenticated upgrade attempts emit audit events and fail with `MigrationError::Unauthorized`.
- Incompatible migrations must be explicitly flagged using `mark_incompatible()`.
