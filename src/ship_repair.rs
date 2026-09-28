//! Paid ship repair — a resource sink (Issue #453).
//!
//! [`crate::ship_nft::repair_ship`] restores a hull for free. That is fine for
//! gameplay but it is a pure faucet: durability is the currency players spend
//! risk on, and giving it back at no cost removes any reason to hold hull in
//! reserve. This module adds a *priced* repair path that permanently destroys
//! harvested resources, so damage has an ongoing economic cost.
//!
//! Two sinks live here:
//!
//! * [`repair_ship`] — proportional repair, priced per durability point.
//! * [`repair_ship`] with `emergency = true` — the "field patch" special
//!   action, which repairs more per call but carries a surcharge on top.
//!
//! Burned volume is accumulated in [`get_total_repair_burn`] so the sink can
//! be measured rather than assumed.

use crate::rate_limiter::{check_rate_limit, Operation, RateLimitError};
use crate::resource_minter::ResourceKey;
use crate::ship_nft::{self, DataKey, ShipError, ShipNft};
use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env, Symbol};

use crate::ensure_auth;

// ── Defaults ──────────────────────────────────────────────────────────────────

/// Basis-point denominator for surcharge arithmetic.
pub const BPS_DENOMINATOR: u32 = 10_000;

/// Resource units burned per point of durability restored.
pub const DEFAULT_COST_PER_POINT: u32 = 2;

/// Surcharge, in basis points, applied by the emergency field-patch action.
/// `5000` = +50% on top of the proportional price.
pub const DEFAULT_EMERGENCY_SURCHARGE_BPS: u32 = 5_000;

/// Ceiling on durability restored by a single call.
///
/// Bounds both the DoS surface of the write and the size of any one sink
/// event, so a player cannot sink an unbounded amount in a single
/// transaction.
pub const DEFAULT_MAX_REPAIR_PER_CALL: u32 = 50;

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ShipRepairError {
    /// No ship with the given ID exists.
    ShipNotFound = 1,
    /// The caller does not own the ship being repaired.
    NotOwner = 2,
    /// The player cannot cover the repair price.
    InsufficientResources = 3,
    /// The ship is already at full durability — nothing to sink.
    ShipAlreadyFull = 4,
    /// The submitted repair configuration is invalid.
    InvalidConfig = 5,
    /// Repair pricing arithmetic overflowed.
    CostOverflow = 6,
    /// Caller exceeded the ship-repair rate limit (DoS prevention).
    RateLimitExceeded = 7,
}

impl crate::error_standard::StandardContractError for ShipRepairError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::ShipNotFound => (ErrorKind::NotFound, false),
            Self::NotOwner => (ErrorKind::Authorization, false),
            Self::InsufficientResources => (ErrorKind::ResourceLimit, false),
            Self::ShipAlreadyFull => (ErrorKind::Conflict, false),
            Self::InvalidConfig | Self::CostOverflow => (ErrorKind::Validation, false),
            Self::RateLimitExceeded => (ErrorKind::ResourceLimit, true),
        };
        crate::error_standard::ErrorDescriptor {
            module: "ship_repair",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

impl From<RateLimitError> for ShipRepairError {
    fn from(_: RateLimitError) -> Self {
        ShipRepairError::RateLimitExceeded
    }
}

impl From<ShipError> for ShipRepairError {
    fn from(err: ShipError) -> Self {
        match err {
            ShipError::ShipNotFound => ShipRepairError::ShipNotFound,
            ShipError::NotOwner => ShipRepairError::NotOwner,
            _ => ShipRepairError::CostOverflow,
        }
    }
}

// ── Data types ────────────────────────────────────────────────────────────────

/// Tunable pricing for the repair sink (Issue #453).
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RepairConfig {
    /// Resource units burned per durability point restored.
    pub cost_per_point: u32,
    /// Surcharge in basis points applied by the emergency field-patch action.
    pub emergency_surcharge_bps: u32,
    /// Ceiling on durability restored by one call.
    pub max_repair_per_call: u32,
}

impl RepairConfig {
    /// Balanced defaults shipped with Issue #453.
    pub fn default_rebalanced() -> Self {
        Self {
            cost_per_point: DEFAULT_COST_PER_POINT,
            emergency_surcharge_bps: DEFAULT_EMERGENCY_SURCHARGE_BPS,
            max_repair_per_call: DEFAULT_MAX_REPAIR_PER_CALL,
        }
    }
}

impl Default for RepairConfig {
    fn default() -> Self {
        Self::default_rebalanced()
    }
}

/// Price of a repair, quoted before any state is touched.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepairQuote {
    /// Ship the quote is for.
    pub ship_id: u64,
    /// Resource that would be burned.
    pub asset_id: Symbol,
    /// Durability currently missing.
    pub missing: u32,
    /// Durability the call would actually restore (capped).
    pub restore_points: u32,
    /// Resource units the call would burn.
    pub resource_cost: u32,
    /// Whether the emergency surcharge applies.
    pub emergency: bool,
}

/// Receipt returned by a completed repair.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepairReceipt {
    pub ship_id: u64,
    pub asset_id: Symbol,
    pub durability_before: u32,
    pub durability_after: u32,
    /// Resource units permanently destroyed by this call.
    pub resource_burned: u32,
    /// Whether the emergency surcharge was applied.
    pub emergency: bool,
}

// ── Storage keys ──────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
enum RepairKey {
    /// Address allowed to retune repair pricing.
    Admin,
    /// Active pricing. Absent means "use the default".
    Config,
    /// Cumulative resource units burned by repairs (the sink total).
    TotalBurned,
}

// ── Pricing ───────────────────────────────────────────────────────────────────

/// Price a repair for `restore_points` durability.
///
/// The emergency action pays the proportional price plus a basis-point
/// surcharge. Integer division floors the result, so the sink is never
/// overstated by rounding.
pub fn repair_cost(restore_points: u32, config: &RepairConfig, emergency: bool) -> u32 {
    let base = u64::from(restore_points).saturating_mul(u64::from(config.cost_per_point));
    if !emergency {
        return u32::try_from(base).unwrap_or(u32::MAX);
    }
    let multiplier = u64::from(BPS_DENOMINATOR.saturating_add(config.emergency_surcharge_bps));
    let surcharged = base.saturating_mul(multiplier) / u64::from(BPS_DENOMINATOR);
    u32::try_from(surcharged).unwrap_or(u32::MAX)
}

// ── Configuration ─────────────────────────────────────────────────────────────

/// Read the active repair pricing. Falls back to the balanced default so
/// callers never have to special-case "not configured".
pub fn get_repair_config(env: &Env) -> RepairConfig {
    env.storage()
        .instance()
        .get(&RepairKey::Config)
        .unwrap_or_else(RepairConfig::default_rebalanced)
}

/// Seed the repair sink admin so pricing can be retuned later. Admin-only,
/// once. Optional: the sink works on defaults without it.
pub fn init_repair_config(env: &Env, admin: &Address, config: RepairConfig) -> Result<(), ShipRepairError> {
    if env.storage().instance().has(&RepairKey::Admin) {
        return Err(ShipRepairError::InvalidConfig);
    }
    admin.require_auth();
    validate_config(&config)?;
    env.storage().instance().set(&RepairKey::Admin, admin);
    env.storage().instance().set(&RepairKey::Config, &config);
    Ok(())
}

/// Retune repair pricing. Admin-only.
pub fn set_repair_config(
    env: &Env,
    admin: &Address,
    config: RepairConfig,
) -> Result<(), ShipRepairError> {
    let stored: Address = env
        .storage()
        .instance()
        .get(&RepairKey::Admin)
        .ok_or(ShipRepairError::NotOwner)?;
    admin.require_auth();
    if *admin != stored {
        return Err(ShipRepairError::NotOwner);
    }
    validate_config(&config)?;
    env.storage().instance().set(&RepairKey::Config, &config);
    Ok(())
}

fn validate_config(config: &RepairConfig) -> Result<(), ShipRepairError> {
    if config.cost_per_point == 0 || config.max_repair_per_call == 0 {
        return Err(ShipRepairError::InvalidConfig);
    }
    if config.emergency_surcharge_bps > u32::from(BPS_DENOMINATOR) * 10 {
        return Err(ShipRepairError::InvalidConfig);
    }
    Ok(())
}

// ── Quoting ───────────────────────────────────────────────────────────────────

/// Price a repair without mutating anything.
///
/// `emergency = true` prices the field-patch special action, which repairs up
/// to `max_repair_per_call` durability at a surcharge.
pub fn quote_repair(
    env: &Env,
    owner: &Address,
    ship_id: u64,
    asset_id: Symbol,
    emergency: bool,
) -> Result<RepairQuote, ShipRepairError> {
    let ship = ship_nft::get_ship(env, ship_id)?;
    if ship.owner != *owner {
        return Err(ShipRepairError::NotOwner);
    }

    let config = get_repair_config(env);
    let missing = ship.max_durability.saturating_sub(ship.durability);
    let restore_points = missing.min(config.max_repair_per_call);

    Ok(RepairQuote {
        ship_id,
        asset_id,
        missing,
        restore_points,
        resource_cost: repair_cost(restore_points, &config, emergency),
        emergency,
    })
}

// ── The sink ──────────────────────────────────────────────────────────────────

/// Repair `ship_id`, burning resources proportionally to the durability
/// restored.
///
/// With `emergency = true` this is the field-patch special action: it repairs
/// up to `max_repair_per_call` durability in one call and burns a surcharge on
/// top, so players have a reason to pay above the base rate when a run is
/// live.
///
/// Burned units leave the player's harvested balance permanently — that is
/// the sink. The restored durability and the burn happen in the same
/// transaction, so a partial failure reverts both.
pub fn repair_ship(
    env: &Env,
    player: &Address,
    ship_id: u64,
    asset_id: Symbol,
    emergency: bool,
) -> Result<RepairReceipt, ShipRepairError> {
    ensure_auth!(player);
    check_rate_limit(env, player, Operation::ShipRepair)?;

    let ship: ShipNft = ship_nft::get_ship(env, ship_id)?;
    if ship.owner != *player {
        return Err(ShipRepairError::NotOwner);
    }

    let config = get_repair_config(env);
    let missing = ship.max_durability.saturating_sub(ship.durability);
    if missing == 0 {
        return Err(ShipRepairError::ShipAlreadyFull);
    }
    let restore_points = missing.min(config.max_repair_per_call);
    let cost = repair_cost(restore_points, &config, emergency);

    // Burn first: if the player cannot pay, nothing is repaired.
    if cost > 0 {
        let key = ResourceKey::ResourceBalance(player.clone(), asset_id.clone());
        let balance: u32 = env.storage().instance().get(&key).unwrap_or(0);
        if balance < cost {
            return Err(ShipRepairError::InsufficientResources);
        }
        env.storage().instance().set(&key, &(balance - cost));
    }

    let durability_before = ship.durability;
    let durability_after = durability_before.saturating_add(restore_points);
    let mut repaired = ship.clone();
    repaired.durability = durability_after;
    env.storage()
        .persistent()
        .set(&DataKey::Ship(ship_id), &repaired);

    let total: u32 = env
        .storage()
        .instance()
        .get(&RepairKey::TotalBurned)
        .unwrap_or(0);
    env.storage()
        .instance()
        .set(&RepairKey::TotalBurned, &total.saturating_add(cost));

    env.events().publish(
        (symbol_short!("ship_rep"), symbol_short!("paid")),
        (ship_id, player.clone(), asset_id.clone(), cost, emergency),
    );
    env.events().publish(
        (symbol_short!("ship_rep"), symbol_short!("durbl")),
        (durability_before, durability_after),
    );

    Ok(RepairReceipt {
        ship_id,
        asset_id,
        durability_before,
        durability_after,
        resource_burned: cost,
        emergency,
    })
}

/// Cumulative resource units destroyed by repairs across all ships.
///
/// This is the headline number for the resource-sinks dashboard: every unit
/// counted here left circulation permanently.
pub fn get_total_repair_burn(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&RepairKey::TotalBurned)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error_standard::{ErrorKind, StandardContractError};
    use soroban_sdk::testutils::{Address as _, Ledger};
    use soroban_sdk::{contract, contractimpl};

    #[contract]
    struct Stub;
    #[contractimpl]
    impl Stub {}

    /// Returns the env, the contract id, and one player who both owns the
    /// seeded ship and pays for repairs. Tests that need a second party mint
    /// an address inline.
    fn setup() -> (Env, Address, Address) {
        let env = Env::default();
        env.ledger().set(soroban_sdk::testutils::LedgerInfo {
            protocol_version: 22,
            sequence_number: 100,
            timestamp: 1_000_000,
            network_id: [0u8; 32],
            base_reserve: 10,
            min_temp_entry_ttl: 100,
            min_persistent_entry_ttl: 100,
            max_entry_ttl: 1000,
        });
        let id = env.register(Stub, ());
        let player = Address::generate(&env);
        env.mock_all_auths();
        (env, id, player)
    }

    fn in_contract<R: FnOnce() -> T, T>(env: &Env, id: &Address, f: R) -> T {
        env.as_contract(id, f)
    }

    /// Store a ship with the given durability directly, avoiding a full mint.
    fn seed_ship(env: &Env, id: &Address, ship_id: u64, owner: &Address, durability: u32) {
        let ship = ShipNft {
            id: ship_id,
            owner: owner.clone(),
            ship_type: symbol_short!("explorer"),
            hull: 100,
            scanner_power: 50,
            durability,
            max_durability: 100,
            metadata: soroban_sdk::Bytes::new(env),
            metadata_uri: soroban_sdk::Bytes::new(env),
        };
        in_contract(env, id, || {
            env.storage().persistent().set(&DataKey::Ship(ship_id), &ship);
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

    fn read_durability(env: &Env, id: &Address, ship_id: u64) -> u32 {
        in_contract(env, id, || {
            ship_nft::get_ship(env, ship_id).map(|s| s.durability).unwrap_or(0)
        })
    }

    // ── Pure pricing ────────────────────────────────────────────────────────

    #[test]
    fn cost_is_proportional_to_points_restored() {
        let cfg = RepairConfig::default_rebalanced();
        assert_eq!(repair_cost(0, &cfg, false), 0);
        assert_eq!(repair_cost(1, &cfg, false), 2);
        assert_eq!(repair_cost(10, &cfg, false), 20);
        assert_eq!(repair_cost(50, &cfg, false), 100);
    }

    #[test]
    fn emergency_action_adds_a_fifty_percent_surcharge() {
        let cfg = RepairConfig::default_rebalanced();
        // 10 points: 20 base -> 30 with the +50% field-patch surcharge.
        assert_eq!(repair_cost(10, &cfg, true), 30);
        assert!(repair_cost(10, &cfg, true) > repair_cost(10, &cfg, false));
    }

    #[test]
    fn zero_surcharge_emergency_matches_base_price() {
        let cfg = RepairConfig {
            cost_per_point: 2,
            emergency_surcharge_bps: 0,
            max_repair_per_call: 50,
        };
        assert_eq!(repair_cost(10, &cfg, true), repair_cost(10, &cfg, false));
    }

    #[test]
    fn pricing_never_overflows() {
        let cfg = RepairConfig {
            cost_per_point: u32::MAX,
            emergency_surcharge_bps: u32::MAX,
            max_repair_per_call: 50,
        };
        // Must saturate rather than panic under `overflow-checks = true`.
        let _ = repair_cost(u32::MAX, &cfg, false);
        let _ = repair_cost(u32::MAX, &cfg, true);
    }

    // ── The sink ────────────────────────────────────────────────────────────

    #[test]
    fn repair_restores_durability_and_burns_resources() {
        let (env, id, player) = setup();
        seed_ship(&env, &id, 1, &player, 60); // 40 missing, under the per-call cap
        seed_dust(&env, &id, &player, 1_000);

        let receipt =
            in_contract(&env, &id, || repair_ship(&env, &player, 1, symbol_short!("dust"), false).unwrap());

        assert_eq!(receipt.durability_before, 60);
        assert_eq!(receipt.durability_after, 100);
        assert_eq!(receipt.resource_burned, 80); // 40 points x 2
        assert_eq!(read_durability(&env, &id, 1), 100);
        assert_eq!(read_dust(&env, &id, &player), 1_000 - 80);
        assert_eq!(in_contract(&env, &id, || get_total_repair_burn(&env)), 80);
    }

    #[test]
    fn repair_is_capped_per_call() {
        let (env, id, player) = setup();
        seed_ship(&env, &id, 1, &player, 0); // 100 missing, cap is 50
        seed_dust(&env, &id, &player, 10_000);

        let receipt =
            in_contract(&env, &id, || repair_ship(&env, &player, 1, symbol_short!("dust"), false).unwrap());

        assert_eq!(receipt.durability_after, DEFAULT_MAX_REPAIR_PER_CALL);
        assert_eq!(receipt.resource_burned, DEFAULT_MAX_REPAIR_PER_CALL * 2);
    }

    #[test]
    fn emergency_field_patch_burns_more_for_the_same_repair() {
        let (env, id, player) = setup();
        seed_ship(&env, &id, 1, &player, 60);
        seed_dust(&env, &id, &player, 10_000);

        let receipt =
            in_contract(&env, &id, || repair_ship(&env, &player, 1, symbol_short!("dust"), true).unwrap());

        assert!(receipt.emergency);
        assert_eq!(receipt.durability_after, 100);
        // 40 points x 2 = 80 base, +50% = 120.
        assert_eq!(receipt.resource_burned, 120);
    }

    #[test]
    fn repair_on_a_full_hull_is_rejected() {
        let (env, id, player) = setup();
        seed_ship(&env, &id, 1, &player, 100);
        seed_dust(&env, &id, &player, 10_000);

        assert_eq!(
            in_contract(&env, &id, || repair_ship(&env, &player, 1, symbol_short!("dust"), false)),
            Err(ShipRepairError::ShipAlreadyFull)
        );
        // Nothing was burned — a no-op repair must not act as a free sink.
        assert_eq!(read_dust(&env, &id, &player), 10_000);
        assert_eq!(in_contract(&env, &id, || get_total_repair_burn(&env)), 0);
    }

    #[test]
    fn repair_without_enough_resources_fails_and_restores_nothing() {
        let (env, id, player) = setup();
        seed_ship(&env, &id, 1, &player, 60);
        seed_dust(&env, &id, &player, 10); // needs 80

        assert_eq!(
            in_contract(&env, &id, || repair_ship(&env, &player, 1, symbol_short!("dust"), false)),
            Err(ShipRepairError::InsufficientResources)
        );
        assert_eq!(read_durability(&env, &id, 1), 60, "hull must be untouched");
        assert_eq!(read_dust(&env, &id, &player), 10);
        assert_eq!(in_contract(&env, &id, || get_total_repair_burn(&env)), 0);
    }

    #[test]
    fn non_owner_cannot_repair_someone_elses_ship() {
        let (env, id, owner) = setup();
        let intruder = Address::generate(&env);
        seed_ship(&env, &id, 1, &owner, 60);
        seed_dust(&env, &id, &intruder, 10_000);

        assert_eq!(
            in_contract(&env, &id, || {
                repair_ship(&env, &intruder, 1, symbol_short!("dust"), false)
            }),
            Err(ShipRepairError::NotOwner)
        );
        assert_eq!(read_dust(&env, &id, &intruder), 10_000);
        assert_eq!(read_durability(&env, &id, 1), 60);
    }

    #[test]
    fn repairing_an_unknown_ship_is_not_found() {
        let (env, id, player) = setup();
        assert_eq!(
            in_contract(&env, &id, || repair_ship(&env, &player, 99, symbol_short!("dust"), false)),
            Err(ShipRepairError::ShipNotFound)
        );
    }

    #[test]
    fn repeated_repairs_accumulate_sink_volume() {
        let (env, id, player) = setup();
        seed_ship(&env, &id, 1, &player, 0); // 100 missing, 50 per call
        seed_dust(&env, &id, &player, 10_000);

        in_contract(&env, &id, || {
            repair_ship(&env, &player, 1, symbol_short!("dust"), false).unwrap()
        });
        in_contract(&env, &id, || {
            repair_ship(&env, &player, 1, symbol_short!("dust"), false).unwrap()
        });

        assert_eq!(read_durability(&env, &id, 1), 100);
        assert_eq!(in_contract(&env, &id, || get_total_repair_burn(&env)), 200);
        assert_eq!(read_dust(&env, &id, &player), 10_000 - 200);
    }

    #[test]
    fn repair_is_rate_limited() {
        let (env, id, player) = setup();
        seed_ship(&env, &id, 1, &player, 0);
        seed_dust(&env, &id, &player, 1_000_000);

        // Cap each call at 30 points so the hull survives the full 3-call
        // budget and the rate limit — not the hull — is what rejects call 4.
        in_contract(&env, &id, || {
            init_repair_config(
                &env,
                &player,
                RepairConfig {
                    cost_per_point: 1,
                    emergency_surcharge_bps: 0,
                    max_repair_per_call: 30,
                },
            )
            .unwrap();
        });

        for _ in 0..3 {
            in_contract(&env, &id, || {
                repair_ship(&env, &player, 1, symbol_short!("dust"), false).unwrap()
            });
        }
        assert_eq!(read_durability(&env, &id, 1), 90);

        assert_eq!(
            in_contract(&env, &id, || repair_ship(&env, &player, 1, symbol_short!("dust"), false)),
            Err(ShipRepairError::RateLimitExceeded)
        );
    }

    #[test]
    fn total_burn_starts_at_zero() {
        let (env, id, _player) = setup();
        assert_eq!(in_contract(&env, &id, || get_total_repair_burn(&env)), 0);
    }

    // ── Quoting ─────────────────────────────────────────────────────────────

    #[test]
    fn quote_matches_what_repair_actually_charges() {
        let (env, id, player) = setup();
        seed_ship(&env, &id, 1, &player, 60);
        seed_dust(&env, &id, &player, 1_000);

        let quote = in_contract(&env, &id, || {
            quote_repair(&env, &player, 1, symbol_short!("dust"), false).unwrap()
        });
        assert_eq!(quote.missing, 40);
        assert_eq!(quote.restore_points, 40);
        assert_eq!(quote.resource_cost, 80);

        let receipt =
            in_contract(&env, &id, || repair_ship(&env, &player, 1, symbol_short!("dust"), false).unwrap());
        assert_eq!(receipt.resource_burned, quote.resource_cost);
    }

    #[test]
    fn quote_caps_restore_points() {
        let (env, id, player) = setup();
        seed_ship(&env, &id, 1, &player, 0);

        let quote = in_contract(&env, &id, || {
            quote_repair(&env, &player, 1, symbol_short!("dust"), true).unwrap()
        });
        assert_eq!(quote.missing, 100);
        assert_eq!(quote.restore_points, DEFAULT_MAX_REPAIR_PER_CALL);
        assert!(quote.emergency);
    }

    #[test]
    fn quote_rejects_non_owner_and_unknown_ship() {
        let (env, id, owner) = setup();
        let intruder = Address::generate(&env);
        seed_ship(&env, &id, 1, &owner, 60);

        assert_eq!(
            in_contract(&env, &id, || {
                quote_repair(&env, &intruder, 1, symbol_short!("dust"), false)
            }),
            Err(ShipRepairError::NotOwner)
        );
        assert_eq!(
            in_contract(&env, &id, || {
                quote_repair(&env, &owner, 42, symbol_short!("dust"), false)
            }),
            Err(ShipRepairError::ShipNotFound)
        );
    }

    // ── Configuration ───────────────────────────────────────────────────────

    #[test]
    fn default_config_is_used_when_unset() {
        let (env, id, _player) = setup();
        assert_eq!(
            in_contract(&env, &id, || get_repair_config(&env)),
            RepairConfig::default_rebalanced()
        );
        assert_eq!(RepairConfig::default(), RepairConfig::default_rebalanced());
    }

    #[test]
    fn admin_can_retune_pricing() {
        let (env, id, player) = setup();
        let admin = Address::generate(&env);
        in_contract(&env, &id, || {
            init_repair_config(&env, &admin, RepairConfig::default_rebalanced()).unwrap();
        });

        // Double init is rejected.
        assert_eq!(
            in_contract(&env, &id, || {
                init_repair_config(&env, &admin, RepairConfig::default_rebalanced())
            }),
            Err(ShipRepairError::InvalidConfig)
        );

        in_contract(&env, &id, || {
            set_repair_config(
                &env,
                &admin,
                RepairConfig {
                    cost_per_point: 5,
                    emergency_surcharge_bps: 0,
                    max_repair_per_call: 10,
                },
            )
            .unwrap();
        });
        assert_eq!(in_contract(&env, &id, || get_repair_config(&env).cost_per_point), 5);

        // 10 missing points x 5 = 50, and the surcharge is now zero.
        seed_ship(&env, &id, 1, &player, 90);
        seed_dust(&env, &id, &player, 1_000);
        let receipt =
            in_contract(&env, &id, || repair_ship(&env, &player, 1, symbol_short!("dust"), true).unwrap());
        assert_eq!(receipt.resource_burned, 50);
    }

    #[test]
    fn set_config_rejects_non_admin() {
        let (env, id, _player) = setup();
        let admin = Address::generate(&env);
        let intruder = Address::generate(&env);
        in_contract(&env, &id, || {
            init_repair_config(&env, &admin, RepairConfig::default_rebalanced()).unwrap();
        });
        assert_eq!(
            in_contract(&env, &id, || {
                set_repair_config(&env, &intruder, RepairConfig::default_rebalanced())
            }),
            Err(ShipRepairError::NotOwner)
        );
    }

    #[test]
    fn set_config_requires_an_admin() {
        let (env, id, player) = setup();
        assert_eq!(
            in_contract(&env, &id, || {
                set_repair_config(&env, &player, RepairConfig::default_rebalanced())
            }),
            Err(ShipRepairError::NotOwner)
        );
    }

    #[test]
    fn config_validation_rejects_nonsense() {
        let (env, id, _player) = setup();
        let admin = Address::generate(&env);
        for bad in [
            RepairConfig { cost_per_point: 0, emergency_surcharge_bps: 0, max_repair_per_call: 10 },
            RepairConfig { cost_per_point: 1, emergency_surcharge_bps: 0, max_repair_per_call: 0 },
            RepairConfig {
                cost_per_point: 1,
                emergency_surcharge_bps: 1_000_000,
                max_repair_per_call: 10,
            },
        ] {
            assert_eq!(
                in_contract(&env, &id, || init_repair_config(&env, &admin, bad)),
                Err(ShipRepairError::InvalidConfig)
            );
        }
    }

    // ── Error metadata ──────────────────────────────────────────────────────

    #[test]
    fn rate_limit_error_is_retryable() {
        let d = ShipRepairError::RateLimitExceeded.descriptor();
        assert_eq!(d.module, "ship_repair");
        assert_eq!(d.code, 7);
        assert_eq!(d.kind, ErrorKind::ResourceLimit);
        assert!(d.retryable);
    }

    #[test]
    fn not_owner_is_an_authorization_error() {
        let d = ShipRepairError::NotOwner.descriptor();
        assert_eq!(d.kind, ErrorKind::Authorization);
        assert!(!d.retryable);
    }

    #[test]
    fn ship_error_conversion_maps_the_cases_we_rely_on() {
        assert_eq!(
            ShipRepairError::from(ShipError::ShipNotFound),
            ShipRepairError::ShipNotFound
        );
        assert_eq!(
            ShipRepairError::from(ShipError::NotOwner),
            ShipRepairError::NotOwner
        );
    }

    #[test]
    fn rate_limit_error_conversion_maps_to_the_repair_variant() {
        assert_eq!(
            ShipRepairError::from(RateLimitError::RateLimitExceeded),
            ShipRepairError::RateLimitExceeded
        );
    }

    #[test]
    fn ship_repair_rate_limit_operation_is_registered() {
        // Guards the rate-limiter match arm against silently losing a variant.
        assert_eq!(format!("{:?}", Operation::ShipRepair), "ShipRepair");
    }
}

