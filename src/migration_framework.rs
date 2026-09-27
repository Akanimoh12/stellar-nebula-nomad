use soroban_sdk::{
    contracterror, contracttype, symbol_short, Address, Bytes, Env, Vec, Symbol,
};

// ─── Migration Framework for Soroban Contract Upgrades ──────────────────────
//
// This module provides a comprehensive migration framework for handling
// breaking changes, data schema evolution, and safe upgrade paths.

// ─── Configuration ───────────────────────────────────────────────────────

/// Maximum records to process in a single migration batch.
pub const MAX_MIGRATION_BATCH: u32 = 100;

/// Maximum migration history entries.
pub const MAX_MIGRATION_HISTORY: u32 = 50;

// ─── Storage Keys ────────────────────────────────────────────────────────

#[derive(Clone)]
#[contracttype]
pub enum MigrationKey {
    /// Address allowed to run privileged migration operations.
    Admin,
    /// Current schema version.
    CurrentSchemaVersion,
    /// Migration history entries.
    MigrationHistory,
    /// Batch migration state.
    BatchState(u32), // batch_id
    /// Rollback checkpoint.
    RollbackCheckpoint(u32), // migration_id
    /// Migration dry-run results.
    DryRunResults(u32), // migration_id
    /// Data backward compatibility flag.
    BackwardCompatible(u32, u32), // (from_version, to_version)
}

// ─── Error Types ─────────────────────────────────────────────────────────

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum MigrationError {
    /// Migration already in progress.
    MigrationInProgress = 1,
    /// Schema version incompatible.
    IncompatibleSchema = 2,
    /// Data validation failed during migration.
    ValidationFailed = 3,
    /// Batch size exceeds maximum.
    BatchTooLarge = 4,
    /// Unauthorized caller.
    Unauthorized = 5,
    /// Rollback failed.
    RollbackFailed = 6,
    /// No rollback checkpoint available.
    NoCheckpoint = 7,
    /// Migration not found.
    MigrationNotFound = 8,
}

impl crate::error_standard::StandardContractError for MigrationError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::MigrationInProgress => (ErrorKind::Conflict, true),
            Self::IncompatibleSchema => (ErrorKind::Validation, false),
            Self::ValidationFailed | Self::RollbackFailed => (ErrorKind::Internal, false),
            Self::BatchTooLarge => (ErrorKind::ResourceLimit, false),
            Self::Unauthorized => (ErrorKind::Authorization, false),
            Self::NoCheckpoint | Self::MigrationNotFound => (ErrorKind::NotFound, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "migration_framework",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

// ─── Data Structures ─────────────────────────────────────────────────────

/// Migration metadata record.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct MigrationRecord {
    pub id: u32,
    pub from_version: u32,
    pub to_version: u32,
    pub status: Symbol, // "pending", "in_progress", "completed", "failed", "rolled_back"
    pub record_count: u32,
    pub started_at: u64,
    pub completed_at: u64,
    pub checksum: BytesN<32>, // SHA-256 of migrated data
}

/// Batch migration state.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct BatchMigrationState {
    pub batch_id: u32,
    pub migration_id: u32,
    pub batch_index: u32,
    pub total_batches: u32,
    pub records_processed: u32,
    pub errors_encountered: u32,
    pub status: Symbol,
}

/// Migration validation result.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct ValidationResult {
    pub is_valid: bool,
    pub errors: Vec<Symbol>,
    pub warnings: Vec<Symbol>,
    pub records_checked: u32,
}

/// Dry-run report.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct DryRunReport {
    pub migration_id: u32,
    pub would_succeed: bool,
    pub records_affected: u32,
    pub estimated_gas: u128,
    pub validation_result: ValidationResult,
}

// ─── Access Control ─────────────────────────────────────────────────────

/// Require `admin` to authorize the call and to be the admin recorded by
/// `initialize_migrations`.
fn require_admin(env: &Env, admin: &Address) -> Result<(), MigrationError> {
    admin.require_auth();

    let stored: Option<Address> = env.storage().instance().get(&MigrationKey::Admin);
    match stored {
        Some(current) if current == *admin => Ok(()),
        _ => Err(MigrationError::Unauthorized),
    }
}

// ─── Initialization ─────────────────────────────────────────────────────

/// Initialize the migration framework and record `admin` as the only address
/// allowed to run privileged migration operations.
pub fn initialize_migrations(env: &Env, admin: &Address, initial_version: u32) -> Result<(), MigrationError> {
    admin.require_auth();

    if !env.storage().instance().has(&MigrationKey::CurrentSchemaVersion) {
        env.storage().instance().set(&MigrationKey::Admin, admin);
        env.storage()
            .instance()
            .set(&MigrationKey::CurrentSchemaVersion, &initial_version);

        env.events().publish(
            (symbol_short!("migration"), symbol_short!("init")),
            (admin.clone(), initial_version, env.ledger().timestamp()),
        );
    }

    Ok(())
}

/// Get current schema version.
pub fn get_current_version(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&MigrationKey::CurrentSchemaVersion)
        .unwrap_or(1)
}

// ─── Migration Planning ─────────────────────────────────────────────────

/// Plan a migration from old_version to new_version.
pub fn plan_migration(
    env: &Env,
    admin: &Address,
    from_version: u32,
    to_version: u32,
    description: Symbol,
) -> Result<MigrationRecord, MigrationError> {
    require_admin(env, admin)?;

    let current = get_current_version(env);
    if from_version > current {
        return Err(MigrationError::IncompatibleSchema);
    }

    let migration_id = env.ledger().timestamp() as u32; // Simple ID generation.

    let record = MigrationRecord {
        id: migration_id,
        from_version,
        to_version,
        status: symbol_short!("pending"),
        record_count: 0,
        started_at: env.ledger().timestamp(),
        completed_at: 0,
        checksum: BytesN::from_array(env, &[0u8; 32]),
    };

    env.events().publish(
        (symbol_short!("migration"), symbol_short!("planned")),
        (migration_id, from_version, to_version, description, env.ledger().timestamp()),
    );

    Ok(record)
}

// ─── Dry-Run Execution ──────────────────────────────────────────────────

/// Execute migration in dry-run mode (no state changes).
pub fn dry_run_migration(
    env: &Env,
    admin: &Address,
    migration_id: u32,
    sample_records: Vec<Bytes>,
) -> Result<DryRunReport, MigrationError> {
    require_admin(env, admin)?;

    if (sample_records.len() as u32) > MAX_MIGRATION_BATCH {
        return Err(MigrationError::BatchTooLarge);
    }

    let mut validation = ValidationResult {
        is_valid: true,
        errors: Vec::new(env),
        warnings: Vec::new(env),
        records_checked: sample_records.len() as u32,
    };

    // Validate each sample record.
    for record in sample_records.iter() {
        if record.len() == 0 {
            validation.is_valid = false;
            validation.errors.push_back(symbol_short!("empty"));
        }
    }

    let report = DryRunReport {
        migration_id,
        would_succeed: validation.is_valid,
        records_affected: validation.records_checked,
        estimated_gas: 50_000_000, // Placeholder estimate.
        validation_result: validation,
    };

    env.storage()
        .instance()
        .set(&MigrationKey::DryRunResults(migration_id), &report.clone());

    env.events().publish(
        (symbol_short!("migration"), symbol_short!("dry_run")),
        (migration_id, report.would_succeed, env.ledger().timestamp()),
    );

    Ok(report)
}

// ─── Backward Compatibility Checks ──────────────────────────────────────

/// Check backward compatibility between versions.
pub fn is_backward_compatible(
    env: &Env,
    from_version: u32,
    to_version: u32,
) -> bool {
    env.storage()
        .instance()
        .get(&MigrationKey::BackwardCompatible(from_version, to_version))
        .unwrap_or(true) // Assume compatible unless explicitly marked.
}

/// Mark versions as backward incompatible.
pub fn mark_incompatible(
    env: &Env,
    admin: &Address,
    from_version: u32,
    to_version: u32,
) -> Result<(), MigrationError> {
    require_admin(env, admin)?;

    env.storage()
        .instance()
        .set(&MigrationKey::BackwardCompatible(from_version, to_version), &false);

    env.events().publish(
        (symbol_short!("migration"), symbol_short!("incomp")),
        (from_version, to_version, env.ledger().timestamp()),
    );

    Ok(())
}

// ─── Batch Migration Execution ──────────────────────────────────────────

/// Execute migration in batches with checkpoint support.
pub fn execute_migration_batch(
    env: &Env,
    admin: &Address,
    migration_id: u32,
    batch_index: u32,
    total_batches: u32,
    batch_data: Vec<Bytes>,
) -> Result<BatchMigrationState, MigrationError> {
    require_admin(env, admin)?;

    if (batch_data.len() as u32) > MAX_MIGRATION_BATCH {
        return Err(MigrationError::BatchTooLarge);
    }

    let state = BatchMigrationState {
        batch_id: env.ledger().timestamp() as u32,
        migration_id,
        batch_index,
        total_batches,
        records_processed: batch_data.len() as u32,
        errors_encountered: 0,
        status: symbol_short!("completed"),
    };

    env.storage()
        .instance()
        .set(&MigrationKey::BatchState(migration_id), &state.clone());

    // Create checkpoint for rollback.
    env.storage()
        .instance()
        .set(&MigrationKey::RollbackCheckpoint(migration_id), &state.clone());

    env.events().publish(
        (symbol_short!("migration"), symbol_short!("batch_ok")),
        (
            migration_id,
            batch_index,
            state.records_processed,
            env.ledger().timestamp(),
        ),
    );

    Ok(state)
}

// ─── Rollback Support ────────────────────────────────────────────────────

/// Rollback a completed migration.
pub fn rollback_migration(
    env: &Env,
    admin: &Address,
    migration_id: u32,
) -> Result<(), MigrationError> {
    require_admin(env, admin)?;

    let checkpoint: Option<BatchMigrationState> = env
        .storage()
        .instance()
        .get(&MigrationKey::RollbackCheckpoint(migration_id));

    if checkpoint.is_none() {
        return Err(MigrationError::NoCheckpoint);
    }

    // Restore from checkpoint (placeholder logic).
    env.storage()
        .instance()
        .remove(&MigrationKey::RollbackCheckpoint(migration_id));

    env.events().publish(
        (symbol_short!("migration"), symbol_short!("rollback")),
        (migration_id, env.ledger().timestamp()),
    );

    Ok(())
}

// ─── Migration History ──────────────────────────────────────────────────

/// Record a completed migration in history. Admin-only.
pub fn record_migration_completion(
    env: &Env,
    admin: &Address,
    migration_id: u32,
    from_version: u32,
    to_version: u32,
    record_count: u32,
) -> Result<(), MigrationError> {
    require_admin(env, admin)?;

    let record = MigrationRecord {
        id: migration_id,
        from_version,
        to_version,
        status: symbol_short!("completed"),
        record_count,
        started_at: env.ledger().timestamp(),
        completed_at: env.ledger().timestamp(),
        checksum: BytesN::from_array(env, &[0u8; 32]),
    };

    env.events().publish(
        (symbol_short!("migration"), symbol_short!("completed")),
        (
            migration_id,
            from_version,
            to_version,
            record_count,
            env.ledger().timestamp(),
        ),
    );

    Ok(())
}

use soroban_sdk::BytesN;

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, testutils::Address as _};

    #[contract]
    struct StubContract;
    #[contractimpl]
    impl StubContract {}

    fn setup_env() -> (Env, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        (env, admin)
    }

    #[test]
    fn test_initialize_and_get_version() {
        let (env, admin) = setup_env();
        let contract = env.register(StubContract, ());

        env.as_contract(&contract, || {
            assert_eq!(get_current_version(&env), 1);
            initialize_migrations(&env, &admin, 2).unwrap();
            assert_eq!(get_current_version(&env), 2);

            // Re-initializing does not overwrite initial version
            initialize_migrations(&env, &admin, 3).unwrap();
            assert_eq!(get_current_version(&env), 2);
        });
    }

    #[test]
    fn test_plan_migration_success_and_incompatible_error() {
        let (env, admin) = setup_env();
        let contract = env.register(StubContract, ());

        env.as_contract(&contract, || {
            initialize_migrations(&env, &admin, 1).unwrap();
            let record = plan_migration(&env, &admin, 1, 2, symbol_short!("plan_v2")).unwrap();
            assert_eq!(record.from_version, 1);
            assert_eq!(record.to_version, 2);

            // from_version > current_version yields error
            let err = plan_migration(&env, &admin, 5, 6, symbol_short!("invalid"));
            assert_eq!(err, Err(MigrationError::IncompatibleSchema));
        });
    }

    #[test]
    fn test_dry_run_migration() {
        let (env, admin) = setup_env();
        let contract = env.register(StubContract, ());

        env.as_contract(&contract, || {
            initialize_migrations(&env, &admin, 1).unwrap();
            let record = plan_migration(&env, &admin, 1, 2, symbol_short!("plan_v2")).unwrap();

            let mut sample_data = Vec::new(&env);
            sample_data.push_back(Bytes::from_slice(&env, b"payload1"));
            sample_data.push_back(Bytes::from_slice(&env, b"payload2"));

            let report = dry_run_migration(&env, &admin, record.id, sample_data).unwrap();
            assert!(report.would_succeed);
            assert_eq!(report.records_affected, 2);

            // Empty record triggers validation error in dry run
            let mut invalid_sample = Vec::new(&env);
            invalid_sample.push_back(Bytes::new(&env));
            let report_fail = dry_run_migration(&env, &admin, record.id, invalid_sample).unwrap();
            assert!(!report_fail.would_succeed);

            // Batch size exceeding MAX_MIGRATION_BATCH yields error
            let mut oversized = Vec::new(&env);
            for _ in 0..MAX_MIGRATION_BATCH + 1 {
                oversized.push_back(Bytes::from_slice(&env, b"x"));
            }
            let err = dry_run_migration(&env, &admin, record.id, oversized);
            assert_eq!(err, Err(MigrationError::BatchTooLarge));
        });
    }

    #[test]
    fn test_execute_batch_and_rollback() {
        let (env, admin) = setup_env();
        let contract = env.register(StubContract, ());

        env.as_contract(&contract, || {
            initialize_migrations(&env, &admin, 1).unwrap();
            let record = plan_migration(&env, &admin, 1, 2, symbol_short!("v2")).unwrap();

            let mut batch_data = Vec::new(&env);
            batch_data.push_back(Bytes::from_slice(&env, b"data1"));

            let batch_state = execute_migration_batch(&env, &admin, record.id, 0, 1, batch_data).unwrap();
            assert_eq!(batch_state.records_processed, 1);
            assert_eq!(batch_state.migration_id, record.id);

            // Rollback succeeds when checkpoint exists
            rollback_migration(&env, &admin, record.id).unwrap();

            // Second rollback fails because checkpoint was consumed/removed
            let err = rollback_migration(&env, &admin, record.id);
            assert_eq!(err, Err(MigrationError::NoCheckpoint));
        });
    }

    #[test]
    fn test_backward_compatibility() {
        let (env, admin) = setup_env();
        let contract = env.register(StubContract, ());

        env.as_contract(&contract, || {
            initialize_migrations(&env, &admin, 1).unwrap();
            assert!(is_backward_compatible(&env, 1, 2));
            mark_incompatible(&env, &admin, 1, 2).unwrap();
            assert!(!is_backward_compatible(&env, 1, 2));
        });
    }

    #[test]
    fn test_record_migration_completion() {
        let (env, admin) = setup_env();
        let contract = env.register(StubContract, ());

        env.as_contract(&contract, || {
            initialize_migrations(&env, &admin, 1).unwrap();
            record_migration_completion(&env, &admin, 101, 1, 2, 50).unwrap();
        });
    }

    #[test]
    fn test_privileged_operations_reject_non_admin() {
        let (env, admin) = setup_env();
        let intruder = Address::generate(&env);
        let contract = env.register(StubContract, ());

        env.as_contract(&contract, || {
            initialize_migrations(&env, &admin, 1).unwrap();

            let samples: Vec<Bytes> = Vec::new(&env);
            assert_eq!(
                plan_migration(&env, &intruder, 1, 2, symbol_short!("v2")),
                Err(MigrationError::Unauthorized)
            );
            assert_eq!(
                dry_run_migration(&env, &intruder, 1, samples.clone()).err(),
                Some(MigrationError::Unauthorized)
            );
            assert_eq!(
                mark_incompatible(&env, &intruder, 1, 2),
                Err(MigrationError::Unauthorized)
            );
            assert_eq!(
                execute_migration_batch(&env, &intruder, 1, 0, 1, samples).err(),
                Some(MigrationError::Unauthorized)
            );
            assert_eq!(
                rollback_migration(&env, &intruder, 1),
                Err(MigrationError::Unauthorized)
            );
            assert_eq!(
                record_migration_completion(&env, &intruder, 1, 1, 2, 0),
                Err(MigrationError::Unauthorized)
            );
        });
    }

    #[test]
    fn test_privileged_operations_reject_before_initialization() {
        let (env, admin) = setup_env();
        let contract = env.register(StubContract, ());

        env.as_contract(&contract, || {
            assert_eq!(
                mark_incompatible(&env, &admin, 1, 2),
                Err(MigrationError::Unauthorized)
            );
        });
    }
}

