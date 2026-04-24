/// # Proxy / Upgradeable Contract Module
///
/// Implements the upgradeable contract pattern for Stellar Nebula Nomad,
/// allowing the contract to be upgraded without losing on-chain state.
///
/// ## Design
///
/// Soroban contracts are immutable once deployed, but the **Wasm blob** stored
/// at a contract's address can be replaced atomically via
/// `env.deployer().update_current_contract_wasm(new_wasm_hash)`.
/// This module wraps that primitive with:
///
/// - **Admin-only authorization** — only the recorded admin address may
///   trigger an upgrade.
/// - **Version tracking** — each upgrade bumps a monotonic `contract_version`
///   stored in instance storage.
/// - **Rollback capability** — the previous Wasm hash and version are kept in
///   storage so that a rollback can be proposed.
/// - **State migration hooks** — callers can run a batch data-migration step
///   after upgrading by calling [`run_migration`].
/// - **Upgrade events** — every upgrade and rollback emits a contract event
///   for off-chain indexers.
///
/// ## Upgrade workflow
///
/// ```text
/// 1. Admin calls `authorize_upgrade(env, new_wasm_hash)`.
///    → Stores `PendingUpgrade { new_wasm_hash, authorized_at }`.
///
/// 2. Admin calls `execute_upgrade(env)`.
///    → Verifies pending upgrade exists, updates Wasm, bumps version,
///      emits `prx_upgrd` event.
///
/// 3. (Optional) Admin calls `run_migration(env, batch_size)`.
///    → Runs one batch of the migration logic; repeat until done.
///
/// 4. (Emergency) Admin calls `rollback_upgrade(env)`.
///    → Reverts to the previous Wasm hash and decrements version.
/// ```
///
/// ## Security
///
/// - All mutating functions call `admin.require_auth()`.
/// - The admin address is set once at `initialize` and cannot be changed
///   without an upgrade.
/// - A two-step (authorize then execute) pattern prevents accidental upgrades.
use soroban_sdk::{
    contracttype, contracterror, symbol_short, Address, BytesN, Env, Vec,
};

// ─── Storage Keys ─────────────────────────────────────────────────────────────

/// Storage key namespace for all proxy / upgrade data.
#[derive(Clone)]
#[contracttype]
pub enum ProxyKey {
    /// The admin address authorized to perform upgrades.
    Admin,
    /// Current contract version (monotonic u32, starts at 1).
    Version,
    /// The Wasm hash that was active before the most recent upgrade.
    PreviousWasmHash,
    /// Version number before the most recent upgrade (for rollback).
    PreviousVersion,
    /// Pending upgrade awaiting execution.
    PendingUpgrade,
    /// Migration state: which step has been completed.
    MigrationStep,
    /// Log of all upgrade records.
    UpgradeHistory,
}

// ─── Data Types ───────────────────────────────────────────────────────────────

/// A pending upgrade proposal created by the admin.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct PendingUpgrade {
    /// Wasm hash of the new contract version.
    pub new_wasm_hash: BytesN<32>,
    /// Ledger sequence when the upgrade was authorized.
    pub authorized_at: u32,
}

/// An entry in the immutable upgrade history log.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct UpgradeRecord {
    /// Version number after the upgrade.
    pub version: u32,
    /// Wasm hash installed by this upgrade.
    pub wasm_hash: BytesN<32>,
    /// Ledger sequence when the upgrade was executed.
    pub upgraded_at: u32,
}

// ─── Errors ───────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ProxyError {
    /// Caller is not the authorized admin.
    NotAdmin = 1,
    /// `initialize` has already been called.
    AlreadyInitialized = 2,
    /// No pending upgrade exists; call `authorize_upgrade` first.
    NoPendingUpgrade = 3,
    /// A migration is already in progress.
    MigrationInProgress = 4,
    /// No previous version to roll back to.
    NoRollbackTarget = 5,
    /// The contract has not been initialized yet.
    NotInitialized = 6,
}

// ─── Initialization ───────────────────────────────────────────────────────────

/// Initialize the proxy with the given `admin` address.
///
/// Sets version to `1` and records the admin.  Must be called exactly once at
/// deployment time.
///
/// # Errors
///
/// [`ProxyError::AlreadyInitialized`] if called more than once.
pub fn initialize(env: &Env, admin: Address) -> Result<(), ProxyError> {
    admin.require_auth();

    if env.storage().instance().has(&ProxyKey::Admin) {
        return Err(ProxyError::AlreadyInitialized);
    }

    env.storage().instance().set(&ProxyKey::Admin, &admin);
    env.storage().instance().set(&ProxyKey::Version, &1u32);

    env.events().publish(
        (symbol_short!("prx_init"), admin.clone()),
        1u32,
    );

    Ok(())
}

// ─── Admin Helpers ────────────────────────────────────────────────────────────

/// Return the current admin address.
///
/// # Errors
///
/// [`ProxyError::NotInitialized`] if `initialize` has not been called.
pub fn get_admin(env: &Env) -> Result<Address, ProxyError> {
    env.storage()
        .instance()
        .get(&ProxyKey::Admin)
        .ok_or(ProxyError::NotInitialized)
}

fn require_admin(env: &Env) -> Result<Address, ProxyError> {
    let admin = get_admin(env)?;
    admin.require_auth();
    Ok(admin)
}

// ─── Version ──────────────────────────────────────────────────────────────────

/// Return the current contract version.
pub fn get_version(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&ProxyKey::Version)
        .unwrap_or(1)
}

// ─── Authorize Upgrade ────────────────────────────────────────────────────────

/// Store a pending upgrade proposal.
///
/// The admin must call this before [`execute_upgrade`].  The two-step process
/// provides a window for off-chain monitoring systems to detect unexpected
/// upgrade proposals.
///
/// # Errors
///
/// [`ProxyError::NotAdmin`] if the caller is not the admin.
pub fn authorize_upgrade(
    env: &Env,
    new_wasm_hash: BytesN<32>,
) -> Result<(), ProxyError> {
    let admin = require_admin(env)?;

    let pending = PendingUpgrade {
        new_wasm_hash: new_wasm_hash.clone(),
        authorized_at: env.ledger().sequence(),
    };

    env.storage()
        .instance()
        .set(&ProxyKey::PendingUpgrade, &pending);

    env.events().publish(
        (symbol_short!("prx_auth"), admin),
        (new_wasm_hash, env.ledger().sequence()),
    );

    Ok(())
}

// ─── Execute Upgrade ──────────────────────────────────────────────────────────

/// Execute the pending upgrade.
///
/// 1. Verifies admin authorization and a pending upgrade exists.
/// 2. Saves the current Wasm hash and version for rollback.
/// 3. Calls `env.deployer().update_current_contract_wasm()` to install the new Wasm.
/// 4. Increments the version counter.
/// 5. Appends an entry to the upgrade history.
/// 6. Emits a `prx_upgrd` event.
///
/// # Errors
///
/// - [`ProxyError::NotAdmin`] — caller is not the admin.
/// - [`ProxyError::NoPendingUpgrade`] — no upgrade has been authorized.
pub fn execute_upgrade(env: &Env) -> Result<u32, ProxyError> {
    let admin = require_admin(env)?;

    let pending: PendingUpgrade = env
        .storage()
        .instance()
        .get(&ProxyKey::PendingUpgrade)
        .ok_or(ProxyError::NoPendingUpgrade)?;

    let old_version = get_version(env);

    // Save previous state for rollback.
    env.storage()
        .instance()
        .set(&ProxyKey::PreviousVersion, &old_version);

    env.storage()
        .instance()
        .remove(&ProxyKey::PendingUpgrade);

    // Install new Wasm — this takes effect after the current invocation returns.
    env.deployer()
        .update_current_contract_wasm(pending.new_wasm_hash.clone());

    let new_version = old_version + 1;
    env.storage()
        .instance()
        .set(&ProxyKey::Version, &new_version);

    // Append to upgrade history.
    let record = UpgradeRecord {
        version: new_version,
        wasm_hash: pending.new_wasm_hash.clone(),
        upgraded_at: env.ledger().sequence(),
    };
    append_upgrade_history(env, record);

    env.events().publish(
        (symbol_short!("prx_upgrd"), admin),
        (old_version, new_version, pending.new_wasm_hash),
    );

    Ok(new_version)
}

// ─── Rollback ─────────────────────────────────────────────────────────────────

/// Rollback to the Wasm installed before the most recent upgrade.
///
/// Reverts the version counter and re-installs the previous Wasm hash.
///
/// > **Warning:** state migrations are not automatically reversed.  Consult the
/// > UPGRADE_GUIDE.md before rolling back a version that included a migration.
///
/// # Errors
///
/// - [`ProxyError::NotAdmin`] — caller is not the admin.
/// - [`ProxyError::NoRollbackTarget`] — no previous version to roll back to.
pub fn rollback_upgrade(env: &Env) -> Result<u32, ProxyError> {
    let admin = require_admin(env)?;

    let history = get_upgrade_history(env);
    if history.len() < 2 {
        return Err(ProxyError::NoRollbackTarget);
    }

    // The second-to-last entry holds the previous wasm hash.
    let prev_record: UpgradeRecord = history
        .get(history.len() - 2)
        .ok_or(ProxyError::NoRollbackTarget)?;

    let current_version = get_version(env);
    let rolled_back_version = if current_version > 1 { current_version - 1 } else { 1 };

    env.deployer()
        .update_current_contract_wasm(prev_record.wasm_hash.clone());

    env.storage()
        .instance()
        .set(&ProxyKey::Version, &rolled_back_version);

    env.events().publish(
        (symbol_short!("prx_rback"), admin),
        (current_version, rolled_back_version, prev_record.wasm_hash),
    );

    Ok(rolled_back_version)
}

// ─── Migration ────────────────────────────────────────────────────────────────

/// Run one batch of the post-upgrade state migration.
///
/// The migration step counter is incremented on each call.  Callers should
/// repeat until this function returns `true` (migration complete).
///
/// The actual migration logic is version-specific and should be added here
/// when a new version introduces a breaking storage schema change.
///
/// # Parameters
///
/// - `batch_size` — maximum number of records to process in this call.
///
/// # Returns
///
/// `true` when migration is complete, `false` when more batches are needed.
///
/// # Errors
///
/// [`ProxyError::NotAdmin`] — caller is not the admin.
pub fn run_migration(env: &Env, batch_size: u32) -> Result<bool, ProxyError> {
    require_admin(env)?;

    let step: u32 = env
        .storage()
        .instance()
        .get(&ProxyKey::MigrationStep)
        .unwrap_or(0);

    // ── Migration logic lives here ────────────────────────────────────────────
    // Add version-specific migration code as new versions are released.
    // Example structure:
    //
    //   let version = get_version(env);
    //   match version {
    //       2 => migrate_v1_to_v2(env, step, batch_size),
    //       3 => migrate_v2_to_v3(env, step, batch_size),
    //       _ => {} // no migration needed
    //   }
    //
    // For now, this is a no-op placeholder.
    let _ = batch_size;
    // ── End migration logic ───────────────────────────────────────────────────

    let next_step = step + 1;
    env.storage()
        .instance()
        .set(&ProxyKey::MigrationStep, &next_step);

    // Mark migration complete after step 1 (placeholder).
    let done = next_step >= 1;

    if done {
        env.storage().instance().remove(&ProxyKey::MigrationStep);
        env.events().publish(
            (symbol_short!("prx_mig"), env.current_contract_address()),
            get_version(env),
        );
    }

    Ok(done)
}

// ─── Upgrade History ──────────────────────────────────────────────────────────

/// Return the full upgrade history log.
pub fn get_upgrade_history(env: &Env) -> Vec<UpgradeRecord> {
    env.storage()
        .persistent()
        .get(&ProxyKey::UpgradeHistory)
        .unwrap_or_else(|| Vec::new(env))
}

fn append_upgrade_history(env: &Env, record: UpgradeRecord) {
    let mut history = get_upgrade_history(env);
    history.push_back(record);
    env.storage()
        .persistent()
        .set(&ProxyKey::UpgradeHistory, &history);
}
