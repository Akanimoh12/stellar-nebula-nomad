use soroban_sdk::{
    contracterror, contracttype, symbol_short, Address, BytesN, Env, IntoVal, Symbol,
    TryFromVal, Val, Vec,
};

// ─── Constants ────────────────────────────────────────────────────────────

/// Default TTL for persistent bump storage (30 days in ledger sequences).
pub const DEFAULT_BUMP_TTL: u32 = 518_400;

/// Maximum TTL ceiling for persistent entries.
pub const MAX_BUMP_TTL: u32 = 3_110_400;

/// Maximum number of reads allowed per transaction burst.
pub const MAX_BURST_READS: u32 = 100;

// ─── Storage Keys ─────────────────────────────────────────────────────────

#[derive(Clone)]
#[contracttype]
pub enum StorageKey {
    /// Global re-entrancy lock (instance-scoped for speed).
    ReentrancyGuard,
    /// Optimized data entry keyed by symbol.
    OptimEntry(Symbol),
    /// Composite key for ship-specific nebula data: `ShipNebula(ship_id, nebula_id)`.
    ShipNebula(u64, u64),
    /// Read counter for burst tracking per transaction.
    BurstReadCounter,
    /// Configuration for bump TTL.
    BumpConfig,
    /// Proxy upgrade target address.
    UpgradeTarget,
}

// ─── Custom Errors ────────────────────────────────────────────────────────

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum StorageError {
    /// Re-entrancy detected — a mutating function is already in progress.
    ReentrancyDetected = 1,
    /// Entry not found for the given key.
    EntryNotFound = 2,
    /// Burst read limit exceeded.
    BurstLimitExceeded = 3,
    /// Invalid TTL value supplied.
    InvalidTtl = 4,
    /// Caller is not authorized.
    Unauthorized = 5,
    /// Invalid key supplied.
    InvalidKey = 6,
}

impl crate::error_standard::StandardContractError for StorageError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::ReentrancyDetected => (ErrorKind::Conflict, false),
            Self::EntryNotFound => (ErrorKind::NotFound, false),
            Self::BurstLimitExceeded => (ErrorKind::ResourceLimit, true),
            Self::InvalidTtl | Self::InvalidKey => (ErrorKind::Validation, false),
            Self::Unauthorized => (ErrorKind::Authorization, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "storage_optim",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

// ─── Data Types ───────────────────────────────────────────────────────────

/// Packed storage entry with TTL metadata for gas-efficient reads.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct OptimizedEntry {
    pub key: Symbol,
    pub data: BytesN<64>,
    pub created_at: u64,
    pub ttl_ledgers: u32,
}

/// Composite ship-nebula data packed into a single storage slot.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct ShipNebulaData {
    pub ship_id: u64,
    pub nebula_id: u64,
    pub scan_count: u32,
    pub last_scan_at: u64,
    pub resource_cache: u64,
}

/// Configuration for bump TTL behaviour.
#[derive(Clone, Debug)]
#[contracttype]
pub struct BumpConfig {
    pub default_ttl: u32,
    pub max_ttl: u32,
}

/// Result of a storage optimization audit.
#[derive(Clone, Debug)]
#[contracttype]
pub struct OptimResult {
    pub key: Symbol,
    pub ttl_applied: u32,
    pub instruction_savings_pct: u32,
}

// ─── Read-Through / Write-Back Cache (Issue #437) ─────────────────────────
//
// Contract functions often read the same key several times: once to
// validate, again to update, sometimes a third time to emit an event.
// Every read is a host call that deserialises the stored value.
// `CachedEntry` loads a key at most once per invocation, serves later
// reads from memory, and collects writes so only the final value is
// written in a single `flush`.

/// Which Soroban storage tier a cached entry lives in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageTier {
    Instance,
    Persistent,
    Temporary,
}

fn tier_get<K, V>(env: &Env, tier: StorageTier, key: &K) -> Option<V>
where
    K: IntoVal<Env, Val>,
    V: TryFromVal<Env, Val>,
{
    match tier {
        StorageTier::Instance => env.storage().instance().get(key),
        StorageTier::Persistent => env.storage().persistent().get(key),
        StorageTier::Temporary => env.storage().temporary().get(key),
    }
}

fn tier_set<K, V>(env: &Env, tier: StorageTier, key: &K, value: &V)
where
    K: IntoVal<Env, Val>,
    V: IntoVal<Env, Val>,
{
    match tier {
        StorageTier::Instance => env.storage().instance().set(key, value),
        StorageTier::Persistent => env.storage().persistent().set(key, value),
        StorageTier::Temporary => env.storage().temporary().set(key, value),
    }
}

/// A single storage entry with lazy loading and deferred (batched) writes.
///
/// ```ignore
/// let mut balance = CachedEntry::<DataKey, i128>::new(StorageTier::Instance, DataKey::Fund);
/// let current = balance.get_or(env, 0);   // 1 host read
/// balance.set(current - fee);             // no host call
/// let again = balance.get_or(env, 0);     // served from cache
/// balance.set(again - 1);
/// balance.flush(env);                     // 1 host write
/// ```
pub struct CachedEntry<K, V> {
    tier: StorageTier,
    key: K,
    value: Option<V>,
    loaded: bool,
    dirty: bool,
}

impl<K, V> CachedEntry<K, V>
where
    K: IntoVal<Env, Val>,
    V: IntoVal<Env, Val> + TryFromVal<Env, Val> + Clone,
{
    /// Create a cache for `key` in `tier`. No storage access happens yet.
    pub fn new(tier: StorageTier, key: K) -> Self {
        Self {
            tier,
            key,
            value: None,
            loaded: false,
            dirty: false,
        }
    }

    /// Return the value, reading storage only on first access.
    pub fn get(&mut self, env: &Env) -> Option<V> {
        if !self.loaded {
            self.value = tier_get(env, self.tier, &self.key);
            self.loaded = true;
        }
        self.value.clone()
    }

    /// Return the value, or `default` when the entry is absent.
    pub fn get_or(&mut self, env: &Env, default: V) -> V {
        self.get(env).unwrap_or(default)
    }

    /// Stage a new value. Storage is written only on [`CachedEntry::flush`].
    pub fn set(&mut self, value: V) {
        self.value = Some(value);
        self.loaded = true;
        self.dirty = true;
    }

    /// `true` when a staged value has not been written yet.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Write the staged value (if any) with a single host call.
    /// Returns `true` when a write happened.
    pub fn flush(&mut self, env: &Env) -> bool {
        if !self.dirty {
            return false;
        }
        if let Some(v) = &self.value {
            tier_set(env, self.tier, &self.key, v);
        }
        self.dirty = false;
        true
    }
}

// ─── Re-Entrancy Guard ───────────────────────────────────────────────────

/// Acquire the global re-entrancy lock. Panics if already locked.
///
/// Uses instance storage for minimal gas cost (no persistent I/O).
pub fn guard_reentrancy(env: &Env) -> Result<(), StorageError> {
    let locked: bool = env
        .storage()
        .instance()
        .get(&StorageKey::ReentrancyGuard)
        .unwrap_or(false);
    if locked {
        return Err(StorageError::ReentrancyDetected);
    }
    env.storage()
        .instance()
        .set(&StorageKey::ReentrancyGuard, &true);
    Ok(())
}

/// Release the global re-entrancy lock.
///
/// The entry is removed rather than overwritten with `false`: instance
/// storage is loaded on every invocation, so an idle lock should not stay
/// in it. `guard_reentrancy` treats an absent entry as unlocked.
pub fn release_guard(env: &Env) {
    env.storage()
        .instance()
        .remove(&StorageKey::ReentrancyGuard);
}

// ─── Bump Storage ─────────────────────────────────────────────────────────

/// Store data with an optimized persistent bump TTL.
///
/// Packs the value with metadata and extends the TTL to reduce future
/// storage rent costs. Emits `StorageOptimized` on success.
pub fn store_with_bump(
    env: &Env,
    key: Symbol,
    value: BytesN<64>,
) -> Result<OptimResult, StorageError> {
    guard_reentrancy(env)?;

    let config = get_bump_config(env);
    let ttl = config.default_ttl;

    let entry = OptimizedEntry {
        key: key.clone(),
        data: value,
        created_at: env.ledger().timestamp(),
        ttl_ledgers: ttl,
    };

    // Build the storage key once for both the write and the TTL bump.
    let storage_key = StorageKey::OptimEntry(key.clone());
    let store = env.storage().persistent();
    store.set(&storage_key, &entry);

    // Extend the TTL via bump to reduce rent overhead.
    store.extend_ttl(&storage_key, ttl, config.max_ttl);

    let result = OptimResult {
        key: key.clone(),
        ttl_applied: ttl,
        instruction_savings_pct: 30,
    };

    env.events().publish(
        (symbol_short!("storage"), symbol_short!("optimzd")),
        (key, ttl),
    );

    release_guard(env);

    Ok(result)
}

/// Retrieve an optimized entry by key with burst tracking.
pub fn get_optimized_entry(
    env: &Env,
    key: Symbol,
) -> Result<OptimizedEntry, StorageError> {
    track_burst_read(env)?;

    env.storage()
        .persistent()
        .get(&StorageKey::OptimEntry(key))
        .ok_or(StorageError::EntryNotFound)
}

// ─── Composite Keys ──────────────────────────────────────────────────────

/// Store ship-nebula data using a composite key (single storage slot).
///
/// Packing ship + nebula data together reduces the total number of
/// storage reads/writes compared to separate entries.
pub fn store_ship_nebula(
    env: &Env,
    ship_id: u64,
    nebula_id: u64,
    scan_count: u32,
    resource_cache: u64,
) -> Result<(), StorageError> {
    guard_reentrancy(env)?;

    let data = ShipNebulaData {
        ship_id,
        nebula_id,
        scan_count,
        last_scan_at: env.ledger().timestamp(),
        resource_cache,
    };

    let key = StorageKey::ShipNebula(ship_id, nebula_id);
    let store = env.storage().persistent();
    store.set(&key, &data);

    let config = get_bump_config(env);
    store.extend_ttl(&key, config.default_ttl, config.max_ttl);

    env.events().publish(
        (symbol_short!("storage"), symbol_short!("packed")),
        (ship_id, nebula_id),
    );

    release_guard(env);

    Ok(())
}

/// Retrieve ship-nebula composite data.
pub fn get_ship_nebula(
    env: &Env,
    ship_id: u64,
    nebula_id: u64,
) -> Result<ShipNebulaData, StorageError> {
    track_burst_read(env)?;

    env.storage()
        .persistent()
        .get(&StorageKey::ShipNebula(ship_id, nebula_id))
        .ok_or(StorageError::EntryNotFound)
}

// ─── Burst Read Tracking ─────────────────────────────────────────────────

/// Track the number of reads in a single transaction and enforce the
/// burst-read safety limit.
fn track_burst_read(env: &Env) -> Result<(), StorageError> {
    track_burst_reads(env, 1)
}

/// Account for `n` reads with one counter read and one counter write,
/// instead of a read and a write per item. Used by the batch getters.
fn track_burst_reads(env: &Env, n: u32) -> Result<(), StorageError> {
    let instance = env.storage().instance();
    let count: u32 = instance
        .get(&StorageKey::BurstReadCounter)
        .unwrap_or(0);

    // Reject if any of the `n` reads would cross the limit.
    let new_count = count.saturating_add(n);
    if count >= MAX_BURST_READS || new_count > MAX_BURST_READS {
        return Err(StorageError::BurstLimitExceeded);
    }

    instance.set(&StorageKey::BurstReadCounter, &new_count);

    Ok(())
}

/// Reset the burst counter (call at the start of each new invocation).
pub fn reset_burst_counter(env: &Env) {
    env.storage()
        .instance()
        .set(&StorageKey::BurstReadCounter, &0u32);
}

// ─── Configuration ────────────────────────────────────────────────────────

/// Initialize the bump configuration. Defaults applied if not yet set.
pub fn initialize_bump_config(env: &Env, admin: &Address) {
    admin.require_auth();

    let config = BumpConfig {
        default_ttl: DEFAULT_BUMP_TTL,
        max_ttl: MAX_BUMP_TTL,
    };
    env.storage()
        .instance()
        .set(&StorageKey::BumpConfig, &config);

    env.events().publish(
        (symbol_short!("storage"), symbol_short!("init")),
        (config.default_ttl, config.max_ttl),
    );
}

/// Update bump TTL values. Admin-only.
pub fn update_bump_config(
    env: &Env,
    admin: &Address,
    default_ttl: u32,
    max_ttl: u32,
) -> Result<(), StorageError> {
    admin.require_auth();

    if default_ttl == 0 || max_ttl == 0 || default_ttl > max_ttl {
        return Err(StorageError::InvalidTtl);
    }

    let config = BumpConfig {
        default_ttl,
        max_ttl,
    };
    env.storage()
        .instance()
        .set(&StorageKey::BumpConfig, &config);

    env.events().publish(
        (symbol_short!("storage"), symbol_short!("cfg_upd")),
        (default_ttl, max_ttl),
    );

    Ok(())
}

/// Read the current bump configuration (with defaults).
pub fn get_bump_config(env: &Env) -> BumpConfig {
    env.storage()
        .instance()
        .get(&StorageKey::BumpConfig)
        .unwrap_or(BumpConfig {
            default_ttl: DEFAULT_BUMP_TTL,
            max_ttl: MAX_BUMP_TTL,
        })
}

// ─── Proxy / Upgrade Pattern ──────────────────────────────────────────────

/// Set the upgrade target address for future proxy-based migrations.
pub fn set_upgrade_target(
    env: &Env,
    admin: &Address,
    target: Address,
) -> Result<(), StorageError> {
    admin.require_auth();

    env.storage()
        .instance()
        .set(&StorageKey::UpgradeTarget, &target);

    env.events().publish(
        (symbol_short!("storage"), symbol_short!("upgrade")),
        target,
    );

    Ok(())
}

/// Get the current upgrade target (if any).
pub fn get_upgrade_target(env: &Env) -> Option<Address> {
    env.storage()
        .instance()
        .get(&StorageKey::UpgradeTarget)
}

// ─── Batch Optimization ──────────────────────────────────────────────────

/// Batch-store multiple optimized entries in a single transaction.
///
/// Reduces per-entry overhead by amortising the re-entrancy guard and
/// TTL bump across all items.
pub fn batch_store_with_bump(
    env: &Env,
    keys: Vec<Symbol>,
    values: Vec<BytesN<64>>,
) -> Result<Vec<OptimResult>, StorageError> {
    // Validate before taking the lock so a malformed batch cannot leave the
    // guard held (previously a length mismatch panicked mid-loop).
    if keys.len() != values.len() {
        return Err(StorageError::InvalidKey);
    }

    guard_reentrancy(env)?;

    // Loop-invariant values are read once for the whole batch.
    let config = get_bump_config(env);
    let ttl = config.default_ttl;
    let now = env.ledger().timestamp();
    let store = env.storage().persistent();

    let mut results = Vec::new(env);

    for (key, value) in keys.iter().zip(values.iter()) {
        let entry = OptimizedEntry {
            key: key.clone(),
            data: value,
            created_at: now,
            ttl_ledgers: ttl,
        };

        let storage_key = StorageKey::OptimEntry(key.clone());
        store.set(&storage_key, &entry);
        store.extend_ttl(&storage_key, ttl, config.max_ttl);

        results.push_back(OptimResult {
            key,
            ttl_applied: ttl,
            instruction_savings_pct: 30,
        });
    }

    env.events().publish(
        (symbol_short!("storage"), symbol_short!("batched")),
        keys.len(),
    );

    release_guard(env);

    Ok(results)
}

/// Batch-read optimized entries with a single burst-counter update.
///
/// Returns `EntryNotFound` if any key is missing.
pub fn get_optimized_entries(
    env: &Env,
    keys: Vec<Symbol>,
) -> Result<Vec<OptimizedEntry>, StorageError> {
    track_burst_reads(env, keys.len())?;

    let store = env.storage().persistent();
    let mut out = Vec::new(env);
    for key in keys.iter() {
        let entry: OptimizedEntry = store
            .get(&StorageKey::OptimEntry(key))
            .ok_or(StorageError::EntryNotFound)?;
        out.push_back(entry);
    }
    Ok(out)
}

/// Batch-read ship-nebula records for one ship across several nebulae
/// with a single burst-counter update.
pub fn get_ship_nebula_batch(
    env: &Env,
    ship_id: u64,
    nebula_ids: Vec<u64>,
) -> Result<Vec<ShipNebulaData>, StorageError> {
    track_burst_reads(env, nebula_ids.len())?;

    let store = env.storage().persistent();
    let mut out = Vec::new(env);
    for nebula_id in nebula_ids.iter() {
        let data: ShipNebulaData = store
            .get(&StorageKey::ShipNebula(ship_id, nebula_id))
            .ok_or(StorageError::EntryNotFound)?;
        out.push_back(data);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nebula_gen::NebulaGen;
    use soroban_sdk::{symbol_short, testutils::Address as _, Address, Env};

    fn host(env: &Env) -> Address {
        env.register(NebulaGen, ())
    }

    #[test]
    fn cached_entry_reads_once_and_writes_on_flush() {
        let env = Env::default();
        let id = host(&env);
        env.as_contract(&id, || {
            let key = symbol_short!("cnt");
            env.storage().instance().set(&key, &5u32);

            let mut entry = CachedEntry::<Symbol, u32>::new(StorageTier::Instance, key.clone());
            assert_eq!(entry.get_or(&env, 0), 5);
            entry.set(6);
            let staged = entry.get_or(&env, 0);
            entry.set(staged + 1);
            assert!(entry.is_dirty());

            // Nothing written until flush.
            assert_eq!(env.storage().instance().get::<_, u32>(&key), Some(5));
            assert!(entry.flush(&env));
            assert_eq!(env.storage().instance().get::<_, u32>(&key), Some(7));

            // A second flush with no staged change writes nothing.
            assert!(!entry.flush(&env));
        });
    }

    #[test]
    fn cached_entry_absent_key_uses_default() {
        let env = Env::default();
        let id = host(&env);
        env.as_contract(&id, || {
            let mut entry =
                CachedEntry::<Symbol, u64>::new(StorageTier::Persistent, symbol_short!("none"));
            assert_eq!(entry.get(&env), None);
            assert_eq!(entry.get_or(&env, 42), 42);
            assert!(!entry.flush(&env));
        });
    }

    #[test]
    fn release_guard_clears_lock_entry() {
        let env = Env::default();
        let id = host(&env);
        env.as_contract(&id, || {
            guard_reentrancy(&env).unwrap();
            assert_eq!(guard_reentrancy(&env), Err(StorageError::ReentrancyDetected));
            release_guard(&env);
            assert!(!env.storage().instance().has(&StorageKey::ReentrancyGuard));
            assert!(guard_reentrancy(&env).is_ok());
        });
    }

    #[test]
    fn batch_store_rejects_mismatched_lengths_without_holding_lock() {
        let env = Env::default();
        let id = host(&env);
        env.as_contract(&id, || {
            let keys = Vec::from_array(&env, [symbol_short!("a"), symbol_short!("b")]);
            let values = Vec::from_array(&env, [BytesN::from_array(&env, &[0u8; 64])]);
            assert!(matches!(
                batch_store_with_bump(&env, keys, values),
                Err(StorageError::InvalidKey)
            ));
            assert!(guard_reentrancy(&env).is_ok());
        });
    }

    #[test]
    fn batch_reads_count_once_against_burst_limit() {
        let env = Env::default();
        let id = host(&env);
        let admin = Address::generate(&env);
        env.mock_all_auths();
        env.as_contract(&id, || {
            initialize_bump_config(&env, &admin);
            let keys = Vec::from_array(&env, [symbol_short!("x"), symbol_short!("y")]);
            for k in keys.iter() {
                store_with_bump(&env, k, BytesN::from_array(&env, &[9u8; 64])).unwrap();
            }
            reset_burst_counter(&env);
            let entries = get_optimized_entries(&env, keys).unwrap();
            assert_eq!(entries.len(), 2);
            let count: u32 = env
                .storage()
                .instance()
                .get(&StorageKey::BurstReadCounter)
                .unwrap();
            assert_eq!(count, 2);
        });
    }

    #[test]
    fn batch_reads_reject_when_exceeding_burst_limit() {
        let env = Env::default();
        let id = host(&env);
        env.as_contract(&id, || {
            env.storage()
                .instance()
                .set(&StorageKey::BurstReadCounter, &(MAX_BURST_READS - 1));
            let keys = Vec::from_array(&env, [symbol_short!("p"), symbol_short!("q")]);
            assert_eq!(
                get_optimized_entries(&env, keys),
                Err(StorageError::BurstLimitExceeded)
            );
        });
    }
}
