use crate::rate_limiter::{check_rate_limit, Operation, RateLimitError};
use crate::resource_minter::ResourceKey;
use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env, Map, Symbol, Vec};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of upgrade modules installable on a single ship.
pub const MAX_MODULES: u32 = 5;
/// Maximum cumulative mass before an upgrade is rejected.
pub const MAX_MASS: u32 = 100;
/// Maximum upgrades allowed in a single batch transaction.
pub const MAX_BATCH_UPGRADES: u32 = 2;

// ── Upgrade cost curve (Issue #454) ───────────────────────────────────────────

/// Basis-point denominator used by every rate in this module.
pub const BPS_DENOMINATOR: u32 = 10_000;

/// Default compounding markup applied per already-installed module.
///
/// The rebalanced curve (Issue #454) is **exponential**, not linear: each
/// successive module costs `base * (1 + 60%)^installed_modules`. A linear
/// alternative was rejected because it under-prices the back half of a hull's
/// build-out, which is exactly where a ship's power spike concentrates.
///
/// With `growth_bps = 6000` and a base cost of 100 the five installable
/// modules cost `100, 160, 256, 409, 654` — 1579 total, versus 500 under the
/// old flat schedule. That is a 3.16x sink for a full build-out, and it makes
/// the last module a genuine goal rather than an afterthought.
pub const DEFAULT_GROWTH_BPS: u32 = 6_000;

/// Hard ceiling on a single module's effective cost, in resource units.
///
/// Bounds the curve so a misconfigured `growth_bps` (or a future bump to
/// [`MAX_MODULES`]) can never make an upgrade unaffordable to the point of
/// bricking progression.
pub const DEFAULT_MAX_COST: u32 = 100_000;

/// Tuning knobs for the upgrade cost curve (Issue #454).
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpgradeEconomy {
    /// Compounding markup in basis points applied per installed module.
    /// `6000` = +60% per module. `0` restores the legacy flat schedule.
    pub growth_bps: u32,
    /// Ceiling applied to any single module's effective cost.
    /// `0` disables the cap.
    pub max_cost: u32,
}

impl UpgradeEconomy {
    /// The rebalanced default curve shipped with Issue #454.
    pub fn default_rebalanced() -> Self {
        Self {
            growth_bps: DEFAULT_GROWTH_BPS,
            max_cost: DEFAULT_MAX_COST,
        }
    }
}

impl Default for UpgradeEconomy {
    fn default() -> Self {
        Self::default_rebalanced()
    }
}

/// Apply the exponential upgrade cost curve to a blueprint's base cost.
///
/// `installed_modules` is the tier being priced: the *next* module to install
/// on a ship that already carries `installed_modules` modules. Tier 0 is
/// deliberately unscaled so onboarding is never taxed.
///
/// Compounding is computed iteratively in `u64` and each step is floored by
/// integer division, which biases the result at most one unit *below* the
/// exact curve — the sink is never overstated. The iteration count is clamped
/// to [`MAX_MODULES`] so a corrupted or future tier value cannot turn this into
/// an unbounded loop, and the result is clamped to `max_cost`.
pub fn scaled_upgrade_cost(
    base_cost: u32,
    installed_modules: u32,
    growth_bps: u32,
    max_cost: u32,
) -> u32 {
    if base_cost == 0 {
        return 0;
    }
    let cap: u64 = if max_cost == 0 {
        u64::from(u32::MAX)
    } else {
        u64::from(max_cost)
    };
    // A zero-growth curve reproduces the legacy flat schedule exactly.
    let multiplier = u64::from(u32::from(BPS_DENOMINATOR.saturating_add(growth_bps)).max(1));
    let mut cost = u64::from(base_cost);
    for _ in 0..installed_modules.min(MAX_MODULES) {
        cost = cost.saturating_mul(multiplier) / u64::from(BPS_DENOMINATOR);
        if cost >= cap {
            return u32::try_from(cap).unwrap_or(u32::MAX);
        }
    }
    if cost >= cap {
        u32::try_from(cap).unwrap_or(u32::MAX)
    } else {
        u32::try_from(cost).unwrap_or(u32::MAX)
    }
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ShipUpgradeError {
    NotInitialized     = 200,
    AlreadyInitialized = 201,
    InsufficientResources = 202,
    UnknownComponent   = 203,
    /// Invariant violated: module cap or mass limit exceeded.
    InvariantViolation = 204,
    BatchTooLarge      = 205,
    /// Ship ID must be greater than zero.
    InvalidShipId      = 206,
    /// A batch must contain at least one component.
    EmptyBatch         = 207,
    /// The blueprint map must contain at least one component.
    InvalidBlueprint   = 208,
    /// Caller exceeded the ship-upgrade rate limit (DoS prevention).
    RateLimitExceeded  = 209,
    /// The submitted cost curve is invalid (Issue #454).
    InvalidEconomy     = 210,
}

impl crate::error_standard::StandardContractError for ShipUpgradeError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::NotInitialized | Self::UnknownComponent => (ErrorKind::NotFound, false),
            Self::AlreadyInitialized => (ErrorKind::Conflict, false),
            Self::InsufficientResources | Self::BatchTooLarge => (ErrorKind::ResourceLimit, false),
            Self::InvariantViolation => (ErrorKind::Internal, false),
            Self::InvalidShipId | Self::EmptyBatch | Self::InvalidBlueprint => {
                (ErrorKind::Validation, false)
            }
            Self::RateLimitExceeded => (ErrorKind::ResourceLimit, true),
            Self::InvalidEconomy => (ErrorKind::Validation, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "ship_upgrade",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

impl From<RateLimitError> for ShipUpgradeError {
    fn from(_: RateLimitError) -> Self {
        ShipUpgradeError::RateLimitExceeded
    }
}

// ── Data Types ────────────────────────────────────────────────────────────────

/// Live on-chain upgrade stats for a ship.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ShipState {
    pub ship_id: u64,
    /// Number of installed upgrade modules.
    pub module_count: u32,
    /// Cumulative mass of all installed components.
    pub total_mass: u32,
    /// Total scanner power bonus from upgrades.
    pub scanner_bonus: u32,
    /// Total hull strength bonus from upgrades.
    pub hull_bonus: u32,
    /// Bonus to passive energy regeneration rate (Issue #190).
    pub regen_bonus: u32,
}

/// Per-component upgrade blueprint: cost and stat bonuses.
#[contracttype]
#[derive(Clone, Debug)]
pub struct UpgradeBlueprint {
    /// Asset symbol of the resource to burn (matches `ResourceKey::ResourceBalance` asset_id).
    pub asset_id: Symbol,
    /// Amount of that resource to consume per upgrade.
    pub resource_cost: u32,
    /// Mass added by this component (contributes to `total_mass`).
    pub mass: u32,
    /// Scanner power bonus granted by this component.
    pub scanner_bonus: u32,
    /// Hull strength bonus granted by this component.
    pub hull_bonus: u32,
    /// Bonus to passive energy regeneration rate (Issue #190).
    pub regen_bonus: u32,
}

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
enum UpgradeDataKey {
    /// Admin address for protected init call.
    Admin,
    /// Map<Symbol, UpgradeBlueprint> of all registered component blueprints.
    Config,
    /// Current upgrade stats for a ship keyed by ship_id.
    ShipState(u64),
    /// Active cost curve (Issue #454). Absent means "use the default".
    Economy,
    /// Cumulative resource units burned by upgrades across all ships (Issue #454).
    TotalUpgradedSpend,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the upgrade subsystem with a component blueprint map.
/// Admin-only; reverts with `AlreadyInitialized` if called again.
pub fn init_upgrade_config(
    env: &Env,
    admin: &Address,
    blueprints: Map<Symbol, UpgradeBlueprint>,
) -> Result<(), ShipUpgradeError> {
    if env.storage().instance().has(&UpgradeDataKey::Admin) {
        return Err(ShipUpgradeError::AlreadyInitialized);
    }
    admin.require_auth();
    if blueprints.is_empty() {
        return Err(ShipUpgradeError::InvalidBlueprint);
    }
    env.storage().instance().set(&UpgradeDataKey::Admin, admin);
    env.storage().instance().set(&UpgradeDataKey::Config, &blueprints);
    Ok(())
}

/// Apply a single component upgrade to `ship_id`.
///
/// Steps:
/// 1. Require `player` authorisation and enforce the per-address rate limit.
/// 2. Look up `component` in the blueprint config.
/// 3. Burn `blueprint.resource_cost` from the player's harvested resource balance.
/// 4. Compute new `ShipState` using saturating arithmetic (overflow-safe).
/// 5. Validate invariants (module cap, mass limit).
/// 6. Persist updated state.
/// 7. Emit `ShipUpgraded` event with before/after stats.
pub fn apply_upgrade(
    env: &Env,
    player: &Address,
    ship_id: u64,
    component: Symbol,
) -> Result<ShipState, ShipUpgradeError> {
    player.require_auth();
    check_rate_limit(env, player, Operation::ShipUpgrade)?;
    apply_upgrade_inner(env, player, ship_id, component)
}

/// Inner upgrade logic — no auth check; called from `apply_upgrade` (single)
/// and `batch_upgrade` (which does one top-level auth then calls this).
fn apply_upgrade_inner(
    env: &Env,
    player: &Address,
    ship_id: u64,
    component: Symbol,
) -> Result<ShipState, ShipUpgradeError> {
    if ship_id == 0 {
        return Err(ShipUpgradeError::InvalidShipId);
    }

    let blueprints: Map<Symbol, UpgradeBlueprint> = env
        .storage()
        .instance()
        .get(&UpgradeDataKey::Config)
        .ok_or(ShipUpgradeError::NotInitialized)?;

    let blueprint = blueprints
        .get(component.clone())
        .ok_or(ShipUpgradeError::UnknownComponent)?;

    // Price the module off the ship's current tier, not the blueprint's flat
    // base cost (Issue #454). This is the sink that makes the back half of a
    // build-out expensive.
    let installed: u32 = env
        .storage()
        .persistent()
        .get(&UpgradeDataKey::ShipState(ship_id))
        .map(|s: ShipState| s.module_count)
        .unwrap_or(0);
    let economy = get_upgrade_economy(env);
    let upgrade_cost = scaled_upgrade_cost(
        blueprint.resource_cost,
        installed,
        economy.growth_bps,
        economy.max_cost,
    );

    // Burn resource: deduct from ResourceMinter's balance for this player + asset.
    let res_key = ResourceKey::ResourceBalance(player.clone(), blueprint.asset_id.clone());
    let balance: u32 = env.storage().instance().get(&res_key).unwrap_or(0);
    if balance < upgrade_cost {
        return Err(ShipUpgradeError::InsufficientResources);
    }
    env.storage()
        .instance()
        .set(&res_key, &(balance - upgrade_cost));

    // Track cumulative sink volume for economy dashboards (Issue #454).
    let total_spend: u32 = env
        .storage()
        .instance()
        .get(&UpgradeDataKey::TotalUpgradedSpend)
        .unwrap_or(0);
    env.storage()
        .instance()
        .set(
            &UpgradeDataKey::TotalUpgradedSpend,
            &total_spend.saturating_add(upgrade_cost),
        );

    // Load before-state (default to zero stats if first upgrade for this ship).
    let before: ShipState = env
        .storage()
        .persistent()
        .get(&UpgradeDataKey::ShipState(ship_id))
        .unwrap_or(ShipState {
            ship_id,
            module_count: 0,
            total_mass: 0,
            scanner_bonus: 0,
            hull_bonus: 0,
            regen_bonus: 0,
        });

    // Build after-state with saturating arithmetic to prevent overflow exploits.
    let after = ShipState {
        ship_id,
        module_count: before.module_count.saturating_add(1),
        total_mass:   before.total_mass.saturating_add(blueprint.mass),
        scanner_bonus: before.scanner_bonus.saturating_add(blueprint.scanner_bonus),
        hull_bonus:    before.hull_bonus.saturating_add(blueprint.hull_bonus),
        regen_bonus:   before.regen_bonus.saturating_add(blueprint.regen_bonus),
    };

    // Validate invariants before committing — revert if violated.
    validate_invariants(&after)?;

    // Persist updated state.
    env.storage()
        .persistent()
        .set(&UpgradeDataKey::ShipState(ship_id), &after);

    // Emit ShipUpgraded event carrying before/after snapshots and component name.
    env.events().publish(
        (symbol_short!("ship_upg"), ship_id, player.clone()),
        (before.clone(), after.clone(), component),
    );

    Ok(after)
}

/// Apply up to `MAX_BATCH_UPGRADES` (2) component upgrades in one transaction.
/// Requires player auth once at the top level; inner calls skip re-auth.
/// The whole batch counts as one call against the rate limit.
/// Fails atomically: if any upgrade fails the entire batch is reverted.
pub fn batch_upgrade(
    env: &Env,
    player: &Address,
    ship_id: u64,
    components: Vec<Symbol>,
) -> Result<Vec<ShipState>, ShipUpgradeError> {
    if components.len() > MAX_BATCH_UPGRADES {
        return Err(ShipUpgradeError::BatchTooLarge);
    }
    if components.is_empty() {
        return Err(ShipUpgradeError::EmptyBatch);
    }

    player.require_auth();
    check_rate_limit(env, player, Operation::ShipUpgrade)?;

    let mut results: Vec<ShipState> = soroban_sdk::vec![env];
    for component in components.iter() {
        let state = apply_upgrade_inner(env, player, ship_id, component)?;
        results.push_back(state);
    }
    Ok(results)
}

/// Validate that a `ShipState` respects all hard invariants.
///
/// Invariants:
/// - `module_count` ≤ `MAX_MODULES` (5)
/// - `total_mass`   ≤ `MAX_MASS`    (100)
///
/// Returns `Err(InvariantViolation)` on the first breach.
pub fn validate_invariants(ship: &ShipState) -> Result<(), ShipUpgradeError> {
    if ship.module_count > MAX_MODULES {
        return Err(ShipUpgradeError::InvariantViolation);
    }
    if ship.total_mass > MAX_MASS {
        return Err(ShipUpgradeError::InvariantViolation);
    }
    Ok(())
}

/// Read the current upgrade state of `ship_id`. Returns `None` if no upgrades
/// have been applied yet.
pub fn get_ship_state(env: &Env, ship_id: u64) -> Option<ShipState> {
    env.storage()
        .persistent()
        .get(&UpgradeDataKey::ShipState(ship_id))
}

/// Read the registered blueprint map. Returns `None` if not yet initialised.
pub fn get_upgrade_config(env: &Env) -> Option<Map<Symbol, UpgradeBlueprint>> {
    env.storage().instance().get(&UpgradeDataKey::Config)
}

/// Read the active upgrade cost curve (Issue #454).
///
/// Returns the rebalanced default when the admin has never overridden it, so
/// callers never have to special-case "not configured".
pub fn get_upgrade_economy(env: &Env) -> UpgradeEconomy {
    env.storage()
        .instance()
        .get(&UpgradeDataKey::Economy)
        .unwrap_or_else(UpgradeEconomy::default_rebalanced)
}

/// Override the upgrade cost curve. Admin-only (Issue #454).
///
/// Setting `growth_bps` to 0 restores the legacy flat schedule, which is the
/// supported way to A/B the curve without redeploying.
pub fn set_upgrade_economy(
    env: &Env,
    admin: &Address,
    economy: UpgradeEconomy,
) -> Result<(), ShipUpgradeError> {
    let stored: Address = env
        .storage()
        .instance()
        .get(&UpgradeDataKey::Admin)
        .ok_or(ShipUpgradeError::NotInitialized)?;
    admin.require_auth();
    if *admin != stored {
        return Err(ShipUpgradeError::NotInitialized);
    }
    if economy.growth_bps > 100_000 {
        return Err(ShipUpgradeError::InvalidEconomy);
    }
    env.storage()
        .instance()
        .set(&UpgradeDataKey::Economy, &economy);
    Ok(())
}

/// Quote what installing `component` on `ship_id` costs right now (Issue #454).
///
/// Pure view — does not mutate state or charge the player. Frontends should
/// call this to render a price before the player commits.
pub fn quote_upgrade_cost(env: &Env, ship_id: u64, component: Symbol) -> Result<u32, ShipUpgradeError> {
    if ship_id == 0 {
        return Err(ShipUpgradeError::InvalidShipId);
    }
    let blueprints: Map<Symbol, UpgradeBlueprint> = env
        .storage()
        .instance()
        .get(&UpgradeDataKey::Config)
        .ok_or(ShipUpgradeError::NotInitialized)?;
    let blueprint = blueprints
        .get(component)
        .ok_or(ShipUpgradeError::UnknownComponent)?;
    let installed: u32 = env
        .storage()
        .persistent()
        .get(&UpgradeDataKey::ShipState(ship_id))
        .map(|s: ShipState| s.module_count)
        .unwrap_or(0);
    let economy = get_upgrade_economy(env);
    Ok(scaled_upgrade_cost(
        blueprint.resource_cost,
        installed,
        economy.growth_bps,
        economy.max_cost,
    ))
}

/// Total resource units burned by upgrades across all ships (Issue #454).
///
/// This is the headline sink number for the resource-sinks dashboard: every
/// unit here left circulation permanently.
pub fn get_total_upgrade_spend(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&UpgradeDataKey::TotalUpgradedSpend)
        .unwrap_or(0)
}

/// Apply a regeneration upgrade effect to the ship's energy manager.
/// This is called after a successful upgrade that has regen_bonus > 0.
/// It increases the passive regeneration rate in energy_manager.
/// Admin-only: the upgrade admin set in `init_upgrade_config` must authorise.
pub fn apply_regen_upgrade(
    env: &Env,
    ship_id: u64,
    bonus: u32,
) -> Result<(), ShipUpgradeError> {
    let admin: Address = env
        .storage()
        .instance()
        .get(&UpgradeDataKey::Admin)
        .ok_or(ShipUpgradeError::NotInitialized)?;
    admin.require_auth();

    if bonus == 0 {
        return Ok(());
    }

    let ship_state: ShipState = env
        .storage()
        .persistent()
        .get(&UpgradeDataKey::ShipState(ship_id))
        .ok_or(ShipUpgradeError::NotInitialized)?;

    let new_regen = ship_state.regen_bonus;
    // Store the regen bonus so energy_manager can read it
    env.storage()
        .persistent()
        .set(&UpgradeDataKey::ShipState(ship_id), &ShipState {
            regen_bonus: new_regen,
            ..ship_state
        });

    env.events().publish(
        (symbol_short!("ship_upg"), symbol_short!("regen")),
        (ship_id, new_regen),
    );

    Ok(())
}

// ── Upgrade cost curve tests (Issue #454) ────────────────────────────────────
#[cfg(test)]
mod economy_tests {
    use super::*;
    use crate::error_standard::{ErrorKind, StandardContractError};
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{contract, contractimpl};

    #[contract]
    struct Stub;
    #[contractimpl]
    impl Stub {}

    /// Contract-instance storage is only addressable from inside a contract
    /// context, so every ledger read/write below is funnelled through this.
    fn in_contract<R: FnOnce() -> T, T>(env: &Env, id: &Address, f: R) -> T {
        env.as_contract(id, f)
    }

    fn setup() -> (Env, Address, Address, Address) {
        let env = Env::default();
        let id = env.register(Stub, ());
        let admin = Address::generate(&env);
        let player = Address::generate(&env);
        env.mock_all_auths();
        (env, id, admin, player)
    }

    /// Install a single-component blueprint set plus the upgrade admin.
    fn seed_blueprint(env: &Env, id: &Address, admin: &Address, base: u32) {
        let mut blueprints = Map::new(env);
        blueprints.set(
            symbol_short!("scanner"),
            UpgradeBlueprint {
                asset_id: symbol_short!("dust"),
                resource_cost: base,
                mass: 10,
                scanner_bonus: 5,
                hull_bonus: 0,
                regen_bonus: 0,
            },
        );
        let admin = admin.clone();
        in_contract(env, id, || {
            env.storage()
                .instance()
                .set(&UpgradeDataKey::Config, &blueprints);
            env.storage()
                .instance()
                .set(&UpgradeDataKey::Admin, &admin);
        });
    }

    fn seed_dust(env: &Env, id: &Address, player: &Address, amount: u32) {
        let key = ResourceKey::ResourceBalance(player.clone(), symbol_short!("dust"));
        let amount = amount;
        in_contract(env, id, || {
            env.storage().instance().set(&key, &amount);
        });
    }

    fn read_dust(env: &Env, id: &Address, player: &Address) -> u32 {
        let key = ResourceKey::ResourceBalance(player.clone(), symbol_short!("dust"));
        in_contract(env, id, || {
            env.storage().instance().get(&key).unwrap_or(0)
        })
    }

    // ── Pure curve arithmetic ───────────────────────────────────────────────

    #[test]
    fn tier_zero_is_unscaled() {
        // The first module costs exactly the blueprint price: onboarding is
        // never taxed.
        assert_eq!(
            scaled_upgrade_cost(100, 0, DEFAULT_GROWTH_BPS, DEFAULT_MAX_COST),
            100
        );
    }

    #[test]
    fn curve_compounds_by_growth_bps() {
        // 100 -> 160 -> 256 -> 409 -> 654 at +60% per module.
        assert_eq!(scaled_upgrade_cost(100, 1, 6_000, DEFAULT_MAX_COST), 160);
        assert_eq!(scaled_upgrade_cost(100, 2, 6_000, DEFAULT_MAX_COST), 256);
        assert_eq!(scaled_upgrade_cost(100, 3, 6_000, DEFAULT_MAX_COST), 409);
        assert_eq!(scaled_upgrade_cost(100, 4, 6_000, DEFAULT_MAX_COST), 654);
    }

    #[test]
    fn full_buildout_sinks_far_more_than_the_flat_schedule() {
        let flat: u32 = (0..MAX_MODULES).map(|_| 100).sum();
        let scaled: u32 = (0..MAX_MODULES)
            .map(|tier| scaled_upgrade_cost(100, tier, DEFAULT_GROWTH_BPS, DEFAULT_MAX_COST))
            .sum();
        assert_eq!(flat, 500);
        assert_eq!(scaled, 1_579);
        assert!(scaled > flat * 3, "rebalanced curve must be a >3x sink");
    }

    #[test]
    fn zero_growth_reproduces_legacy_flat_schedule() {
        for tier in 0..=MAX_MODULES {
            assert_eq!(scaled_upgrade_cost(100, tier, 0, DEFAULT_MAX_COST), 100);
        }
    }

    #[test]
    fn cost_is_capped_by_max_cost() {
        assert_eq!(scaled_upgrade_cost(100, MAX_MODULES, 100_000, 1_000), 1_000);
        // A zero cap disables the ceiling entirely.
        assert!(scaled_upgrade_cost(100, MAX_MODULES, 100_000, 0) > 1_000);
    }

    #[test]
    fn tier_is_clamped_to_max_modules() {
        // An absurd tier must neither loop unboundedly nor overshoot the
        // curve's natural value at MAX_MODULES.
        assert_eq!(
            scaled_upgrade_cost(100, MAX_MODULES, DEFAULT_GROWTH_BPS, 0),
            scaled_upgrade_cost(100, u32::MAX, DEFAULT_GROWTH_BPS, 0)
        );
    }

    #[test]
    fn zero_base_cost_stays_free() {
        assert_eq!(
            scaled_upgrade_cost(0, 4, DEFAULT_GROWTH_BPS, DEFAULT_MAX_COST),
            0
        );
    }

    #[test]
    fn curve_never_overflows() {
        // Extreme inputs must not panic under `overflow-checks = true`.
        let _ = scaled_upgrade_cost(u32::MAX, MAX_MODULES, 100_000, 0);
        let _ = scaled_upgrade_cost(u32::MAX, u32::MAX, 100_000, 0);
        let _ = scaled_upgrade_cost(u32::MAX, 4, u32::MAX, 0);
    }

    // ── On-chain wiring ─────────────────────────────────────────────────────

    #[test]
    fn default_economy_matches_shipped_constants() {
        let econ = UpgradeEconomy::default_rebalanced();
        assert_eq!(econ.growth_bps, DEFAULT_GROWTH_BPS);
        assert_eq!(econ.max_cost, DEFAULT_MAX_COST);
        assert_eq!(UpgradeEconomy::default(), econ);
    }

    #[test]
    fn unset_economy_reads_as_rebalanced_default() {
        let (env, id, _admin, _player) = setup();
        assert_eq!(
            in_contract(&env, &id, || get_upgrade_economy(&env)),
            UpgradeEconomy::default_rebalanced()
        );
    }

    #[test]
    fn total_spend_starts_at_zero() {
        let (env, id, _admin, _player) = setup();
        assert_eq!(in_contract(&env, &id, || get_total_upgrade_spend(&env)), 0);
    }

    #[test]
    fn quote_tracks_the_ships_tier() {
        let (env, id, admin, player) = setup();
        seed_blueprint(&env, &id, &admin, 100);

        // A ship with no upgrades installed is tier 0.
        assert_eq!(
            in_contract(&env, &id, || quote_upgrade_cost(&env, 1, symbol_short!("scanner"))),
            Ok(100)
        );

        seed_dust(&env, &id, &player, 10_000);
        in_contract(&env, &id, || {
            apply_upgrade(&env, &player, 1, symbol_short!("scanner")).unwrap();
        });
        assert_eq!(
            in_contract(&env, &id, || quote_upgrade_cost(&env, 1, symbol_short!("scanner"))),
            Ok(160)
        );
    }

    #[test]
    fn quote_rejects_bad_ship_id_and_unknown_component() {
        let (env, id, admin, _player) = setup();
        seed_blueprint(&env, &id, &admin, 100);

        assert_eq!(
            in_contract(&env, &id, || quote_upgrade_cost(&env, 1, symbol_short!("warp"))),
            Err(ShipUpgradeError::UnknownComponent)
        );
        assert_eq!(
            in_contract(&env, &id, || quote_upgrade_cost(&env, 0, symbol_short!("scanner"))),
            Err(ShipUpgradeError::InvalidShipId)
        );
    }

    #[test]
    fn quote_before_initialisation_is_not_initialized() {
        let (env, id, _admin, _player) = setup();
        assert_eq!(
            in_contract(&env, &id, || quote_upgrade_cost(&env, 1, symbol_short!("scanner"))),
            Err(ShipUpgradeError::NotInitialized)
        );
    }

    #[test]
    fn upgrade_burns_the_scaled_cost_not_the_base_cost() {
        let (env, id, admin, player) = setup();
        seed_blueprint(&env, &id, &admin, 100);
        seed_dust(&env, &id, &player, 1_000);

        // Tier 0 costs 100, tier 1 costs 160 => 260 burned, not the flat 200.
        // Each call gets its own frame: `mock_all_auths` authorises a single
        // `require_auth` per frame, so two upgrades cannot share one.
        in_contract(&env, &id, || {
            apply_upgrade(&env, &player, 42, symbol_short!("scanner")).unwrap();
        });
        in_contract(&env, &id, || {
            apply_upgrade(&env, &player, 42, symbol_short!("scanner")).unwrap();
        });

        assert_eq!(read_dust(&env, &id, &player), 1_000 - 260);
        assert_eq!(in_contract(&env, &id, || get_total_upgrade_spend(&env)), 260);
    }

    #[test]
    fn growth_of_zero_restores_flat_pricing_on_chain() {
        let (env, id, admin, player) = setup();
        seed_blueprint(&env, &id, &admin, 100);

        in_contract(&env, &id, || {
            set_upgrade_economy(
                &env,
                &admin,
                UpgradeEconomy {
                    growth_bps: 0,
                    max_cost: DEFAULT_MAX_COST,
                },
            )
            .unwrap();
        });
        assert_eq!(
            in_contract(&env, &id, || get_upgrade_economy(&env).growth_bps),
            0
        );

        seed_dust(&env, &id, &player, 1_000);
        in_contract(&env, &id, || {
            apply_upgrade(&env, &player, 7, symbol_short!("scanner")).unwrap();
        });
        in_contract(&env, &id, || {
            apply_upgrade(&env, &player, 7, symbol_short!("scanner")).unwrap();
        });
        // Flat schedule: 2 x 100.
        assert_eq!(read_dust(&env, &id, &player), 800);
    }

    #[test]
    fn set_economy_rejects_absurd_growth() {
        let (env, id, admin, _player) = setup();
        seed_blueprint(&env, &id, &admin, 100);

        assert_eq!(
            in_contract(&env, &id, || {
                set_upgrade_economy(
                    &env,
                    &admin,
                    UpgradeEconomy {
                        growth_bps: 100_001,
                        max_cost: 1,
                    },
                )
            }),
            Err(ShipUpgradeError::InvalidEconomy)
        );
    }

    #[test]
    fn set_economy_rejects_non_admin() {
        let (env, id, admin, _player) = setup();
        let intruder = Address::generate(&env);
        seed_blueprint(&env, &id, &admin, 100);

        assert_eq!(
            in_contract(&env, &id, || {
                set_upgrade_economy(
                    &env,
                    &intruder,
                    UpgradeEconomy {
                        growth_bps: 100,
                        max_cost: 1,
                    },
                )
            }),
            Err(ShipUpgradeError::NotInitialized)
        );
    }

    #[test]
    fn set_economy_requires_initialisation() {
        let (env, id, admin, _player) = setup();
        assert_eq!(
            in_contract(&env, &id, || {
                set_upgrade_economy(&env, &admin, UpgradeEconomy::default_rebalanced())
            }),
            Err(ShipUpgradeError::NotInitialized)
        );
    }

    #[test]
    fn invalid_economy_is_a_validation_error() {
        let d = ShipUpgradeError::InvalidEconomy.descriptor();
        assert_eq!(d.module, "ship_upgrade");
        assert_eq!(d.code, 210);
        assert_eq!(d.kind, ErrorKind::Validation);
        assert!(!d.retryable);
    }
}
