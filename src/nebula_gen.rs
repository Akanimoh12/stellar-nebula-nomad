// ============================================================
// nebula_gen.rs — Merged: Input Validation + Lifecycle Management
//
// Resolved from:
//   • all-issues/combined-fixed  (fix/nebula-input-validation, Issue #170)
//   • main  (configurable TTL, admin cleanup, storage rent)
//
// Resolution decisions:
//   • Contract struct name: NebulaGen  (main's rename wins as trunk)
//   • Error enum: unified NebulaError; added InvalidShipId, InvalidRegionId,
//     AnomalyOutOfBounds from combined-fixed into main's set
//   • Input validation (ship_id, region_id, seed) kept in generate_validated_nebula_layout
//   • has_anomaly returns Result<bool, NebulaError> — preserves InvalidShipId
//     signalling while also surfacing expiry (LayoutNotFound)
//   • TTL / extend_ttl / admin sweep logic kept from main
//   • Both test suites merged and deduplicated
//   • Generation hot path optimised (Issue #438) — see "PRNG Engine"
// ============================================================

use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror, log, symbol_short,
    Address, BytesN, Env, Vec,
};

use crate::gas_optimized_compute::{
    derive_spread, expand_u64_to_bytes32, fold_seed_bytes, is_zero_bytes32, splitmix64,
    GOLDEN_GAMMA,
};
use crate::rate_limiter::{check_rate_limit, Operation, RateLimitError};

// ── Constants ────────────────────────────────────────────────

/// Maximum valid region ID.  Keeps storage bounded (Issue #170).
pub const MAX_REGION_ID: u64 = 1_000_000;
/// Minimum valid ship ID (must be > 0).
pub const MIN_SHIP_ID: u64 = 1;
/// Default number of anomalies generated per layout.
pub const DEFAULT_ANOMALY_COUNT: u32 = 16;
/// Default time-to-live for an active layout: 24 hours (in ledger seconds).
pub const DEFAULT_LAYOUT_TTL: u64 = 86_400;

// ── Error enum ───────────────────────────────────────────────

/// Unified contract error codes.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum NebulaError {
    NotInitialized      = 1,
    AlreadyInitialized  = 2,
    /// Seed is degenerate (all-zero bytes).
    InvalidSeed         = 3,
    /// Requested anomaly index is out of bounds for this layout.
    InvalidIndex        = 4,
    /// No active (non-expired) layout found for the given ship.
    LayoutNotFound      = 5,
    /// Requested nebula size is outside the configured [min_size, max_size] range.
    InvalidSize         = 6,
    /// Provided layout TTL is zero / invalid.
    InvalidTtl          = 7,
    /// ship_id must be greater than zero.
    InvalidShipId       = 8,
    /// region_id must be between 1 and MAX_REGION_ID (inclusive).
    InvalidRegionId     = 9,
    /// Anomaly index is out of bounds for this layout.
    AnomalyOutOfBounds  = 10,
    /// Caller exceeded the layout-generation rate limit (DoS prevention).
    RateLimitExceeded   = 11,
}

impl crate::error_standard::StandardContractError for NebulaError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::NotInitialized | Self::LayoutNotFound => (ErrorKind::NotFound, false),
            Self::AlreadyInitialized => (ErrorKind::Conflict, false),
            Self::InvalidSeed
            | Self::InvalidIndex
            | Self::InvalidSize
            | Self::InvalidTtl
            | Self::InvalidShipId
            | Self::InvalidRegionId
            | Self::AnomalyOutOfBounds => (ErrorKind::Validation, false),
            Self::RateLimitExceeded => (ErrorKind::ResourceLimit, true),
        };
        crate::error_standard::ErrorDescriptor {
            module: "nebula_gen",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

impl From<RateLimitError> for NebulaError {
    fn from(_: RateLimitError) -> Self {
        NebulaError::RateLimitExceeded
    }
}

// ── Data types ───────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceClass {
    Sparse,
    Moderate,
    Abundant,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnomalyType {
    DustCloud,
    IonStorm,
    CrystalFormation,
    PlasmaVent,
    DarkMatterPocket,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Anomaly {
    pub x:              u64,
    pub y:              u64,
    pub rarity:         u64,
    pub anomaly_type:   AnomalyType,
    pub resource_class: ResourceClass,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct NebulaLayout {
    pub ship_id:      u64,
    pub region_id:    u64,
    pub layout_hash:  BytesN<32>,
    pub anomalies:    Vec<Anomaly>,
    pub size:         u32,
    /// Ledger timestamp at which the layout was generated.
    pub generated_at: u64,
}

// ── Config ───────────────────────────────────────────────────

/// Configurable nebula generation parameters (updatable by admin).
#[contracttype]
#[derive(Clone)]
pub struct NebulaConfig {
    pub admin:        Address,
    /// Default anomalies per generated layout (clamped to [min_size, max_size]).
    pub default_size: u32,
    /// Absolute minimum anomalies per layout.
    pub min_size:     u32,
    /// Absolute maximum anomalies per layout.
    pub max_size:     u32,
    /// Lifetime of an active layout in seconds.
    pub layout_ttl:   u64,
}

// ── Storage Keys ─────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Config,
    /// Most-recent layout for a ship, keyed by ship_id.
    ActiveLayout(u64),
}

// ── PRNG Engine ──────────────────────────────────────────────
//
// Performance notes (Issue #438) — hotspots found while profiling
// `generate_validated_nebula_layout` with `env.cost_estimate().budget()`:
//   1. Seed extraction issued 32 `BytesN::get` host calls; it now makes a
//      single `to_array()` call and folds the bytes natively.
//   2. Anomalies were appended one `push_back` at a time. Each push clones
//      the host vector, so cost grew quadratically with layout size. They are
//      now built in fixed-size chunks with `Vec::from_array` + `append`.
//   3. `index * GOLDEN_GAMMA` was recomputed for every salt; it is now
//      computed once per anomaly.
//   4. The layout hash was assembled byte-by-byte; it is now built from
//      four little-endian lanes.
// The PRNG maths is unchanged, so layouts are bit-for-bit identical to the
// previous implementation. The tests below check this against a reference
// copy of the old algorithm.

// Salt constants for derive_spread()
const SALT_X: u64 = 0x9e37_79b9_7f4a_7c15;
const SALT_Y: u64 = 0x6c62_272e_07bb_0142;
const SALT_R: u64 = 0xbf58_476d_1ce4_e5b9;
const SALT_T: u64 = 0x94d0_49bb_1331_11eb;

/// Number of anomalies materialised per host `Vec` allocation.
const ANOMALY_CHUNK: usize = 8;

/// Expand a u64 into a 32-byte layout hash using four independent splitmix64 chains.
fn build_layout_hash(env: &Env, h: u64) -> BytesN<32> {
    BytesN::from_array(env, &expand_u64_to_bytes32(h))
}

/// Build the anomaly at `index` for the given entropy `master`.
#[inline]
fn make_anomaly(master: u64, index: u32) -> Anomaly {
    let spread = u64::from(index).wrapping_mul(GOLDEN_GAMMA);
    let x      = derive_spread(master, spread, SALT_X) % 1000;
    let y      = derive_spread(master, spread, SALT_Y) % 1000;
    let rarity = derive_spread(master, spread, SALT_R) % 101;
    let t      = derive_spread(master, spread, SALT_T);
    Anomaly {
        x,
        y,
        rarity,
        anomaly_type:   u64_to_anomaly_type(t),
        resource_class: rarity_to_class(rarity),
    }
}

/// Generate `size` anomalies. Full chunks of [`ANOMALY_CHUNK`] are built
/// with one host allocation each; any remainder is pushed individually.
fn generate_anomalies(env: &Env, master: u64, size: u32) -> Vec<Anomaly> {
    let mut anomalies: Vec<Anomaly> = Vec::new(env);
    let mut base = 0u32;
    #[allow(clippy::cast_possible_truncation)]
    let chunk_len = ANOMALY_CHUNK as u32;
    while base + chunk_len <= size {
        let chunk: [Anomaly; ANOMALY_CHUNK] = core::array::from_fn(|k| {
            #[allow(clippy::cast_possible_truncation)]
            let offset = k as u32;
            make_anomaly(master, base + offset)
        });
        if base == 0 {
            anomalies = Vec::from_array(env, chunk);
        } else {
            anomalies.extend_from_array(chunk);
        }
        base += chunk_len;
    }
    while base < size {
        anomalies.push_back(make_anomaly(master, base));
        base += 1;
    }
    anomalies
}

fn u64_to_anomaly_type(v: u64) -> AnomalyType {
    match v % 5 {
        0 => AnomalyType::DustCloud,
        1 => AnomalyType::IonStorm,
        2 => AnomalyType::CrystalFormation,
        3 => AnomalyType::PlasmaVent,
        _ => AnomalyType::DarkMatterPocket,
    }
}

fn rarity_to_class(rarity: u64) -> ResourceClass {
    if rarity <= 33 {
        ResourceClass::Sparse
    } else if rarity <= 66 {
        ResourceClass::Moderate
    } else {
        ResourceClass::Abundant
    }
}

/// Convert a logical TTL in seconds into ledgers (~5 s per ledger),
/// saturating at `u32::MAX` instead of silently truncating.
fn ttl_to_ledgers(ttl_seconds: u64) -> u32 {
    u32::try_from(ttl_seconds / 5).unwrap_or(u32::MAX)
}

/// Returns `true` when a layout generated at `generated_at` has outlived
/// `ttl` seconds. Saturating addition prevents overflow false-positives.
fn is_expired(now: u64, generated_at: u64, ttl: u64) -> bool {
    now > generated_at.saturating_add(ttl)
}

// ── Contract ─────────────────────────────────────────────────

#[contract]
pub struct NebulaGen;

#[contractimpl]
impl NebulaGen {
    // ── Initialisation ────────────────────────────────────────

    /// Initialise the nebula generator. Must be called once by the admin.
    ///
    /// # Parameters
    /// - `default_size` – anomalies per layout (clamped to [min_size, max_size])
    /// - `min_size` / `max_size` – hard bounds
    /// - `layout_ttl` – layout lifetime in seconds; pass `0` to use [`DEFAULT_LAYOUT_TTL`]
    pub fn init(
        env:          Env,
        admin:        Address,
        default_size: u32,
        min_size:     u32,
        max_size:     u32,
        layout_ttl:   u64,
    ) -> Result<(), NebulaError> {
        if env.storage().instance().has(&DataKey::Config) {
            return Err(NebulaError::AlreadyInitialized);
        }
        admin.require_auth();
        if min_size == 0 || min_size > max_size {
            return Err(NebulaError::InvalidSize);
        }
        let ttl     = if layout_ttl == 0 { DEFAULT_LAYOUT_TTL } else { layout_ttl };
        let clamped = default_size.max(min_size).min(max_size);
        env.storage().instance().set(
            &DataKey::Config,
            &NebulaConfig { admin, default_size: clamped, min_size, max_size, layout_ttl: ttl },
        );
        Ok(())
    }

    // ── Generation ────────────────────────────────────────────

    /// Generate a deterministic nebula layout for a given ship / region.
    ///
    /// # Validation (Issue #170)
    /// - `ship_id`   must be > 0
    /// - `region_id` must be in [1, MAX_REGION_ID]
    /// - `seed`      must not be all-zero bytes
    pub fn generate_validated_nebula_layout(
        env:       Env,
        caller:    Address,
        ship_id:   u64,
        region_id: u64,
        seed:      BytesN<32>,
    ) -> Result<NebulaLayout, NebulaError> {
        let config = Self::require_config(&env)?;

        // ── Require caller authentication ─────────────────────
        caller.require_auth();

        // ── Rate limit: layout generation is CPU + storage heavy ──
        check_rate_limit(&env, &caller, Operation::NebulaGeneration)?;

        // ── Input validation (Issue #170) ─────────────────────
        if ship_id < MIN_SHIP_ID {
            log!(&env, "[ERROR] NebulaGen: invalid ship_id={}", ship_id);
            return Err(NebulaError::InvalidShipId);
        }
        if region_id < 1 || region_id > MAX_REGION_ID {
            log!(&env, "[ERROR] NebulaGen: invalid region_id={}", region_id);
            return Err(NebulaError::InvalidRegionId);
        }
        // Seed must not be all-zero. One host call copies the seed into
        // native memory; validation and folding then run without host calls.
        let seed_bytes = seed.to_array();
        if is_zero_bytes32(&seed_bytes) {
            return Err(NebulaError::InvalidSeed);
        }
        let seed_u64 = fold_seed_bytes(&seed_bytes);

        // ── Build entropy master ──────────────────────────────
        let ledger_seq = env.ledger().sequence() as u64;
        let timestamp  = env.ledger().timestamp();

        let master: u64 = splitmix64(seed_u64)
            ^ splitmix64(ledger_seq)
            ^ splitmix64(timestamp)
            ^ splitmix64(ship_id)
            ^ splitmix64(region_id);

        // ── Generate anomalies ────────────────────────────────
        let size = config.default_size;
        let anomalies = generate_anomalies(&env, master, size);

        // ── Build layout hash ─────────────────────────────────
        let layout_hash = build_layout_hash(&env, master);

        let layout = NebulaLayout {
            ship_id,
            region_id,
            layout_hash: layout_hash.clone(),
            anomalies,
            size,
            generated_at: timestamp,
        };

        // ── Persist layout ────────────────────────────────────
        // Build the storage key once and reuse it for the write and the TTL bump.
        let key = DataKey::ActiveLayout(ship_id);
        let store = env.storage().persistent();
        store.set(&key, &layout);

        // Tie storage rent to the configured logical TTL (~5 s per ledger).
        let ttl_ledgers = ttl_to_ledgers(config.layout_ttl);
        store.extend_ttl(&key, ttl_ledgers, ttl_ledgers);

        // ── Emit event ────────────────────────────────────────
        env.events().publish(
            (symbol_short!("NebulaGen"), symbol_short!("generated")),
            (ship_id, layout_hash, size),
        );

        log!(&env, "[INFO] NebulaGen: generated layout | ship_id={} region_id={} size={}",
             ship_id, region_id, size);

        Ok(layout)
    }

    // ── Queries ───────────────────────────────────────────────

    /// Return a single anomaly by index from the active layout of `ship_id`.
    /// Expired layouts are cleaned up and treated as absent.
    pub fn query_anomaly(
        env:     Env,
        ship_id: u64,
        index:   u32,
    ) -> Result<Anomaly, NebulaError> {
        if ship_id < MIN_SHIP_ID {
            return Err(NebulaError::InvalidShipId);
        }
        let layout = Self::get_live_layout(&env, ship_id)
            .ok_or(NebulaError::LayoutNotFound)?;
        layout.anomalies.get(index).ok_or(NebulaError::InvalidIndex)
    }

    /// Return the full active layout for `ship_id`, or `None` if absent or expired.
    pub fn get_layout(env: Env, ship_id: u64) -> Option<NebulaLayout> {
        Self::get_live_layout(&env, ship_id)
    }

    /// Check whether `anomaly_index` is valid for `ship_id`'s active layout.
    /// Returns `Err(InvalidShipId)` for ship_id == 0, `Err(LayoutNotFound)` when
    /// no live layout exists, `Err(AnomalyOutOfBounds)` for an out-of-range index,
    /// and `Ok(true)` when the anomaly is present.
    pub fn has_anomaly(
        env:           Env,
        ship_id:       u64,
        anomaly_index: u32,
    ) -> Result<bool, NebulaError> {
        if ship_id < MIN_SHIP_ID {
            return Err(NebulaError::InvalidShipId);
        }
        let layout = Self::get_live_layout(&env, ship_id)
            .ok_or(NebulaError::LayoutNotFound)?;
        if anomaly_index >= layout.size {
            return Err(NebulaError::AnomalyOutOfBounds);
        }
        Ok(true)
    }

    // ── Admin operations ──────────────────────────────────────

    /// Update the active-layout TTL (seconds). Must be non-zero. Admin only.
    pub fn update_layout_ttl(env: Env, new_ttl: u64) -> Result<(), NebulaError> {
        let mut config = Self::require_config(&env)?;
        config.admin.require_auth();
        if new_ttl == 0 {
            return Err(NebulaError::InvalidTtl);
        }
        config.layout_ttl = new_ttl;
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    /// Remove the active layout for `ship_id` if it has expired. Admin only.
    /// Returns `true` when an expired layout was removed.
    pub fn clean_expired_layout(env: Env, ship_id: u64) -> Result<bool, NebulaError> {
        let config = Self::require_config(&env)?;
        config.admin.require_auth();
        let now = env.ledger().timestamp();
        Ok(Self::remove_if_expired(&env, &config, now, ship_id))
    }

    /// Sweep a batch of ship layouts, removing any that have expired. Admin only.
    /// Returns the number of layouts removed.
    pub fn clean_expired_layouts(env: Env, ship_ids: Vec<u64>) -> Result<u32, NebulaError> {
        let config = Self::require_config(&env)?;
        config.admin.require_auth();
        let now = env.ledger().timestamp();
        let mut removed = 0u32;
        for ship_id in ship_ids.iter() {
            if Self::remove_if_expired(&env, &config, now, ship_id) {
                removed += 1;
            }
        }
        Ok(removed)
    }

    // ── Internal helpers ──────────────────────────────────────

    fn require_config(env: &Env) -> Result<NebulaConfig, NebulaError> {
        env.storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(NebulaError::NotInitialized)
    }

    /// Fetch the active layout for `ship_id`, lazily removing and returning
    /// `None` if it has expired.
    ///
    /// The config is read only when a layout exists, and the storage key is
    /// built once for both the read and the removal.
    fn get_live_layout(env: &Env, ship_id: u64) -> Option<NebulaLayout> {
        let key = DataKey::ActiveLayout(ship_id);
        let store = env.storage().persistent();
        let layout: NebulaLayout = store.get(&key)?;

        let ttl = env
            .storage()
            .instance()
            .get::<DataKey, NebulaConfig>(&DataKey::Config)
            .map_or(DEFAULT_LAYOUT_TTL, |c| c.layout_ttl);

        if is_expired(env.ledger().timestamp(), layout.generated_at, ttl) {
            store.remove(&key);
            env.events().publish(
                (symbol_short!("neb_gen"), symbol_short!("expired")),
                ship_id,
            );
            return None;
        }
        Some(layout)
    }

    /// Remove the active layout for `ship_id` if expired under `config`.
    ///
    /// `now` is passed in so batch sweeps read the ledger timestamp once
    /// rather than once per ship.
    fn remove_if_expired(env: &Env, config: &NebulaConfig, now: u64, ship_id: u64) -> bool {
        let key = DataKey::ActiveLayout(ship_id);
        let store = env.storage().persistent();
        match store.get::<DataKey, NebulaLayout>(&key) {
            Some(l) if is_expired(now, l.generated_at, config.layout_ttl) => {
                store.remove(&key);
                env.events().publish(
                    (symbol_short!("neb_gen"), symbol_short!("cleaned")),
                    ship_id,
                );
                true
            }
            _ => false,
        }
    }
}

// ── Tests ────────────────────────────────────────────────────
//
// Merged from both branches:
//   • Input validation edge cases (Issue #170) — from all-issues/combined-fixed
//   • TTL expiry / admin lifecycle — from main

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, BytesN, Env, Vec,
    };

    const SHORT_TTL: u64 = 100; // seconds

    fn ledger_info(seq: u32, ts: u64) -> LedgerInfo {
        LedgerInfo {
            protocol_version:        22,
            sequence_number:         seq,
            timestamp:               ts,
            network_id:              [0u8; 32],
            base_reserve:            10,
            min_temp_entry_ttl:      16,
            min_persistent_entry_ttl: 16,
            max_entry_ttl:           100_000,
        }
    }

    fn setup() -> (Env, NebulaGenClient<'static>, Address) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(ledger_info(1, 1_000));
        let id     = env.register(NebulaGen, ());
        let client = NebulaGenClient::new(&env, &id);
        let admin  = Address::generate(&env);
        client.init(&admin, &5u32, &1u32, &10u32, &SHORT_TTL);
        (env, client, admin)
    }

    fn zero_seed(env: &Env) -> BytesN<32> {
        BytesN::from_array(env, &[0u8; 32])
    }

    fn valid_seed(env: &Env) -> BytesN<32> {
        BytesN::from_array(env, &[
            1,  2,  3,  4,  5,  6,  7,  8,
            9,  10, 11, 12, 13, 14, 15, 16,
            17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32,
        ])
    }

    fn gen_layout(env: &Env, client: &NebulaGenClient, ship_id: u64) {
        let caller = Address::generate(env);
        client.generate_validated_nebula_layout(&caller, &ship_id, &1u64, &valid_seed(env));
    }

    // ── ship_id validation (Issue #170) ──────────────────────

    #[test]
    fn test_ship_id_zero_rejected() {
        let (env, client, _) = setup();
        let caller = Address::generate(&env);
        let result = client.try_generate_validated_nebula_layout(
            &caller, &0u64, &1u64, &valid_seed(&env),
        );
        assert_eq!(result, Err(Ok(NebulaError::InvalidShipId)));
    }

    #[test]
    fn test_ship_id_one_accepted() {
        let (env, client, _) = setup();
        let caller = Address::generate(&env);
        assert!(client.try_generate_validated_nebula_layout(
            &caller, &1u64, &1u64, &valid_seed(&env),
        ).is_ok());
    }

    #[test]
    fn test_ship_id_max_u64_accepted() {
        let (env, client, _) = setup();
        let caller = Address::generate(&env);
        assert!(client.try_generate_validated_nebula_layout(
            &caller, &u64::MAX, &1u64, &valid_seed(&env),
        ).is_ok());
    }

    // ── region_id validation (Issue #170) ────────────────────

    #[test]
    fn test_region_id_zero_rejected() {
        let (env, client, _) = setup();
        let caller = Address::generate(&env);
        let result = client.try_generate_validated_nebula_layout(
            &caller, &1u64, &0u64, &valid_seed(&env),
        );
        assert_eq!(result, Err(Ok(NebulaError::InvalidRegionId)));
    }

    #[test]
    fn test_region_id_one_accepted() {
        let (env, client, _) = setup();
        let caller = Address::generate(&env);
        assert!(client.try_generate_validated_nebula_layout(
            &caller, &1u64, &1u64, &valid_seed(&env),
        ).is_ok());
    }

    #[test]
    fn test_region_id_max_accepted() {
        let (env, client, _) = setup();
        let caller = Address::generate(&env);
        assert!(client.try_generate_validated_nebula_layout(
            &caller, &1u64, &MAX_REGION_ID, &valid_seed(&env),
        ).is_ok());
    }

    #[test]
    fn test_region_id_exceeds_max_rejected() {
        let (env, client, _) = setup();
        let caller = Address::generate(&env);
        let result = client.try_generate_validated_nebula_layout(
            &caller, &1u64, &(MAX_REGION_ID + 1), &valid_seed(&env),
        );
        assert_eq!(result, Err(Ok(NebulaError::InvalidRegionId)));
    }

    #[test]
    fn test_region_id_u64_max_rejected() {
        let (env, client, _) = setup();
        let caller = Address::generate(&env);
        let result = client.try_generate_validated_nebula_layout(
            &caller, &1u64, &u64::MAX, &valid_seed(&env),
        );
        assert_eq!(result, Err(Ok(NebulaError::InvalidRegionId)));
    }

    // ── seed validation (Issue #170) ─────────────────────────

    #[test]
    fn test_all_zero_seed_rejected() {
        let (env, client, _) = setup();
        let caller = Address::generate(&env);
        let result = client.try_generate_validated_nebula_layout(
            &caller, &1u64, &1u64, &zero_seed(&env),
        );
        assert_eq!(result, Err(Ok(NebulaError::InvalidSeed)));
    }

    // ── combined invalid inputs ───────────────────────────────

    #[test]
    fn test_both_ids_invalid_ship_id_error_first() {
        // ship_id is checked before region_id
        let (env, client, _) = setup();
        let caller = Address::generate(&env);
        let result = client.try_generate_validated_nebula_layout(
            &caller, &0u64, &0u64, &valid_seed(&env),
        );
        assert_eq!(result, Err(Ok(NebulaError::InvalidShipId)));
    }

    // ── has_anomaly validation ────────────────────────────────

    #[test]
    fn test_has_anomaly_ship_id_zero_rejected() {
        let (_env, client, _) = setup();
        let result = client.try_has_anomaly(&0u64, &0u32);
        assert_eq!(result, Err(Ok(NebulaError::InvalidShipId)));
    }

    #[test]
    fn test_has_anomaly_layout_not_found() {
        let (_env, client, _) = setup();
        let result = client.try_has_anomaly(&99u64, &0u32);
        assert_eq!(result, Err(Ok(NebulaError::LayoutNotFound)));
    }

    #[test]
    fn test_has_anomaly_out_of_bounds() {
        let (env, client, _) = setup();
        gen_layout(&env, &client, 5);
        // default_size = 5, so index 5 is OOB
        let result = client.try_has_anomaly(&5u64, &5u32);
        assert_eq!(result, Err(Ok(NebulaError::AnomalyOutOfBounds)));
    }

    #[test]
    fn test_has_anomaly_valid() {
        let (env, client, _) = setup();
        gen_layout(&env, &client, 5);
        assert!(client.has_anomaly(&5u64, &0u32));
    }

    // ── Determinism ───────────────────────────────────────────

    #[test]
    fn test_same_inputs_produce_same_layout_hash() {
        let (env, client, _) = setup();
        let seed = valid_seed(&env);

        let caller1 = Address::generate(&env);
        let caller2 = Address::generate(&env);
        let l1 = client.generate_validated_nebula_layout(&caller1, &42u64, &100u64, &seed);
        let l2 = client.generate_validated_nebula_layout(&caller2, &42u64, &100u64, &seed);

        assert_eq!(l1.layout_hash, l2.layout_hash);
    }

    // ── Determinism vs. legacy algorithm (Issue #438) ─────────
    //
    // A copy of the pre-optimisation generator, kept here as the reference
    // the optimised hot path must match bit-for-bit.

    mod legacy {
        use super::super::{rarity_to_class, u64_to_anomaly_type, Anomaly};
        use soroban_sdk::{BytesN, Env, Vec};

        pub fn splitmix64(mut z: u64) -> u64 {
            z = z.wrapping_add(0x9e3779b97f4a7c15);
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            z ^ (z >> 31)
        }

        fn derive(seed: u64, index: u32, salt: u64) -> u64 {
            splitmix64(seed ^ splitmix64((index as u64).wrapping_mul(0x9e3779b97f4a7c15) ^ salt))
        }

        fn read_u64(seed: &BytesN<32>, offset: u32) -> u64 {
            let mut val: u64 = 0;
            for i in 0..8u32 {
                val |= (seed.get(offset + i).unwrap_or(0) as u64) << (i * 8);
            }
            val
        }

        pub fn extract_seed(raw: &BytesN<32>) -> u64 {
            read_u64(raw, 0) ^ read_u64(raw, 8) ^ read_u64(raw, 16) ^ read_u64(raw, 24)
        }

        pub fn anomalies(env: &Env, master: u64, size: u32) -> Vec<Anomaly> {
            let mut out = Vec::new(env);
            for i in 0..size {
                let rarity = derive(master, i, 0xbf58476d1ce4e5b9) % 101;
                out.push_back(Anomaly {
                    x: derive(master, i, 0x9e3779b97f4a7c15) % 1000,
                    y: derive(master, i, 0x6c62272e07bb0142) % 1000,
                    rarity,
                    anomaly_type: u64_to_anomaly_type(derive(master, i, 0x94d049bb133111eb)),
                    resource_class: rarity_to_class(rarity),
                });
            }
            out
        }

        pub fn layout_hash(env: &Env, h: u64) -> BytesN<32> {
            let parts = [
                splitmix64(h),
                splitmix64(h ^ 0xdeadcafe12345678),
                splitmix64(h.wrapping_add(0x12345678deadbeef)),
                splitmix64(h.wrapping_mul(0x0101010101010101).wrapping_add(1)),
            ];
            let mut arr = [0u8; 32];
            for (p, part) in parts.iter().enumerate() {
                arr[p * 8..p * 8 + 8].copy_from_slice(&part.to_le_bytes());
            }
            BytesN::from_array(env, &arr)
        }
    }

    fn setup_sized(size: u32) -> (Env, NebulaGenClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(ledger_info(1, 1_000));
        let id     = env.register(NebulaGen, ());
        let client = NebulaGenClient::new(&env, &id);
        let admin  = Address::generate(&env);
        client.init(&admin, &size, &1u32, &64u32, &SHORT_TTL);
        (env, client)
    }

    fn legacy_master(env: &Env, seed: &BytesN<32>, ship_id: u64, region_id: u64) -> u64 {
        legacy::splitmix64(legacy::extract_seed(seed))
            ^ legacy::splitmix64(env.ledger().sequence() as u64)
            ^ legacy::splitmix64(env.ledger().timestamp())
            ^ legacy::splitmix64(ship_id)
            ^ legacy::splitmix64(region_id)
    }

    #[test]
    fn optimised_generation_matches_legacy_output() {
        // Sizes cover: remainder only, exact chunk, chunk + remainder,
        // several chunks, and the configured maximum.
        for size in [1u32, 5, 8, 13, 16, 21, 64] {
            let (env, client) = setup_sized(size);
            let seed   = valid_seed(&env);
            let caller = Address::generate(&env);
            let layout = client.generate_validated_nebula_layout(&caller, &42u64, &7u64, &seed);

            let master = legacy_master(&env, &seed, 42, 7);
            assert_eq!(layout.size, size);
            assert_eq!(layout.anomalies, legacy::anomalies(&env, master, size));
            assert_eq!(layout.layout_hash, legacy::layout_hash(&env, master));
        }
    }

    #[test]
    fn optimised_seed_fold_matches_legacy_for_many_seeds() {
        let env = Env::default();
        for n in 1u8..=32 {
            let mut raw = [0u8; 32];
            for (i, b) in raw.iter_mut().enumerate() {
                *b = n.wrapping_mul(31).wrapping_add(i as u8).rotate_left(u32::from(n % 8));
            }
            let seed = BytesN::from_array(&env, &raw);
            assert_eq!(fold_seed_bytes(&seed.to_array()), legacy::extract_seed(&seed));
        }
    }

    #[test]
    fn generation_is_deterministic_across_envs() {
        let (env_a, client_a) = setup_sized(21);
        let (env_b, client_b) = setup_sized(21);
        let a = client_a.generate_validated_nebula_layout(
            &Address::generate(&env_a), &9u64, &3u64, &valid_seed(&env_a),
        );
        let b = client_b.generate_validated_nebula_layout(
            &Address::generate(&env_b), &9u64, &3u64, &valid_seed(&env_b),
        );
        assert_eq!(a.layout_hash.to_array(), b.layout_hash.to_array());
        assert_eq!(a.anomalies.len(), b.anomalies.len());
        for i in 0..a.anomalies.len() {
            assert_eq!(a.anomalies.get(i), b.anomalies.get(i));
        }
    }

    #[test]
    fn non_zero_seed_whose_lanes_cancel_is_accepted() {
        // Two identical 8-byte lanes XOR to zero. The seed is not all-zero,
        // so it must be accepted as documented.
        let (env, client, _) = setup();
        let mut raw = [0u8; 32];
        raw[0] = 0xAB;
        raw[8] = 0xAB;
        let seed = BytesN::from_array(&env, &raw);
        let caller = Address::generate(&env);
        assert!(client
            .try_generate_validated_nebula_layout(&caller, &1u64, &1u64, &seed)
            .is_ok());
    }

    #[test]
    fn generation_cpu_budget_within_target() {
        let (env, client) = setup_sized(64);
        let seed   = valid_seed(&env);
        let caller = Address::generate(&env);
        env.cost_estimate().budget().reset_default();
        client.generate_validated_nebula_layout(&caller, &1u64, &1u64, &seed);
        // Generous ceiling: guards against regressions to per-byte host calls
        // or per-element vector cloning.
        assert!(env.cost_estimate().budget().cpu_instruction_cost() < 10_000_000);
    }

    // ── TTL / lifecycle (from main) ───────────────────────────

    #[test]
    fn layout_available_before_expiry() {
        let (env, client, _) = setup();
        gen_layout(&env, &client, 7);
        assert!(client.get_layout(&7u64).is_some());
    }

    #[test]
    fn layout_auto_cleaned_after_expiry() {
        let (env, client, _) = setup();
        gen_layout(&env, &client, 7);
        env.ledger().set(ledger_info(2, 1_000 + SHORT_TTL + 1));
        assert!(client.get_layout(&7u64).is_none());
        // Entry is physically removed, not just hidden
        assert_eq!(
            client.try_has_anomaly(&7u64, &0u32),
            Err(Ok(NebulaError::LayoutNotFound))
        );
    }

    #[test]
    fn admin_can_clean_expired_layout() {
        let (env, client, _) = setup();
        gen_layout(&env, &client, 7);
        env.ledger().set(ledger_info(2, 1_000 + SHORT_TTL + 1));
        assert!(client.clean_expired_layout(&7u64));
        // Cleaning an already-removed ship reports false
        assert!(!client.clean_expired_layout(&7u64));
    }

    #[test]
    fn admin_batch_cleanup_counts_removed() {
        let (env, client, _) = setup();
        for ship in [10u64, 11, 12] {
            gen_layout(&env, &client, ship);
        }
        env.ledger().set(ledger_info(2, 1_000 + SHORT_TTL + 1));
        let mut ships = Vec::new(&env);
        ships.push_back(10u64);
        ships.push_back(11u64);
        ships.push_back(12u64);
        assert_eq!(client.clean_expired_layouts(&ships), 3u32);
    }

    #[test]
    fn admin_can_update_ttl() {
        let (env, client, _) = setup();
        gen_layout(&env, &client, 7);
        // Extend TTL well past elapsed time; layout must stay live
        client.update_layout_ttl(&1_000_000u64);
        env.ledger().set(ledger_info(2, 1_000 + SHORT_TTL + 1));
        assert!(client.get_layout(&7u64).is_some());
    }
}