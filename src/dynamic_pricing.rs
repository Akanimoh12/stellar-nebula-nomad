//! Dynamic resource pricing (Issue #452).
//!
//! A raw price feed is trivially spammable: whoever pays the most gas can print
//! any number they like, ten times in a row. This module exists so that the
//! price the rest of the economy reads is **not** the raw feed value.
//!
//! Three guards, layered:
//!
//! 1. **EMA smoothing** — the published price is an exponential moving average
//!    of accepted observations, so a single print moves the price by at most
//!    `smoothing_bps` of the gap. See [`ema_step`].
//! 2. **Deviation rejection** — a raw print that sits further than
//!    `max_deviation_bps` from the current average is *discarded*, not folded
//!    in. This is what makes a flash-loan-style print useless: the attacker
//!    pays gas to move a number nobody reads.
//! 3. **Volatility band** — the published price is clamped to the average
//!    plus or minus a band that widens with realized volatility (quiet market →
//!    tight band, wild market → wide band) but never past
//!    `max_volatility_band_bps`. Prices still move when they should; they just
//!    cannot teleport.
//!
//! A bounded supply/demand nudge is applied on top, reusing the parity
//! convention from [`crate::economics::balancer`] (ratio `1000` = balanced) so
//! scarce resources trend up without the price ever running away.
//!
//! [`dex_integration::list_at_market`] consumes [`dynamic_price`] so listings
//! can be priced automatically instead of by hand.

use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env, Symbol, Vec};

/// Basis-point denominator.
pub const BPS_DENOMINATOR: u32 = 10_000;

/// Default EMA weight for a new observation: 20% of the gap, i.e. roughly five
/// observations to close most of a move.
pub const DEFAULT_SMOOTHING_BPS: u32 = 2_000;

/// Default distance from the average at which a raw print is thrown away: ±50%.
pub const DEFAULT_MAX_DEVIATION_BPS: u32 = 5_000;

/// Default published-price band around the average when the market is quiet:
/// ±30%.
pub const DEFAULT_BASE_VOLATILITY_BAND_BPS: u32 = 3_000;

/// Hard ceiling on the published-price band, reached only at extreme
/// realized volatility: ±50%.
pub const MAX_VOLATILITY_BAND_BPS: u32 = 5_000;

/// Default minimum spacing between accepted observations for one resource.
pub const DEFAULT_MIN_COOLDOWN_SECS: u64 = 60;

/// Default cap on the proportional supply/demand nudge: ±25%.
pub const DEFAULT_SUPPLY_DEMAND_ADJ_BPS: u32 = 2_500;

/// Upper bound on the EMA weight. Above half the average tracks the raw feed
/// and smoothing stops doing any work.
pub const MAX_SMOOTHING_BPS: u32 = 5_000;

/// Rolling history length retained per resource.
pub const MAX_HISTORY_ENTRIES: u32 = 24;

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum PricingError {
    /// No pricing configuration has been stored yet.
    NotInitialized = 1,
    /// The caller is not the configured pricing admin.
    Unauthorized = 2,
    /// A non-positive price was supplied.
    InvalidPrice = 3,
    /// The submitted configuration is out of range.
    InvalidConfig = 4,
    /// No observation has been recorded for this resource yet.
    ResourceNotFound = 5,
    /// An observation arrived before the cooldown expired.
    CooldownActive = 6,
    /// Pricing arithmetic overflowed.
    ArithmeticOverflow = 7,
}

impl crate::error_standard::StandardContractError for PricingError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::NotInitialized | Self::Unauthorized => (ErrorKind::Authorization, false),
            Self::InvalidPrice | Self::InvalidConfig => (ErrorKind::Validation, false),
            Self::CooldownActive => (ErrorKind::Conflict, false),
            Self::ResourceNotFound => (ErrorKind::NotFound, false),
            Self::ArithmeticOverflow => (ErrorKind::ResourceLimit, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "dynamic_pricing",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// Tunable guards for the dynamic pricing engine (Issue #452).
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PricingConfig {
    /// EMA weight for a new observation, in basis points.
    pub smoothing_bps: u32,
    /// Prints further than this from the average are discarded.
    pub max_deviation_bps: u32,
    /// Published-price band around the average in a quiet market.
    pub base_volatility_band_bps: u32,
    /// Hard ceiling on the published-price band.
    pub max_volatility_band_bps: u32,
    /// Minimum spacing between accepted observations, in seconds.
    pub min_cooldown_secs: u64,
    /// Cap on the proportional supply/demand nudge, in basis points.
    pub supply_demand_adj_bps: u32,
}

impl PricingConfig {
    /// The shipped defaults.
    pub fn default_issue_452() -> Self {
        Self {
            smoothing_bps: DEFAULT_SMOOTHING_BPS,
            max_deviation_bps: DEFAULT_MAX_DEVIATION_BPS,
            base_volatility_band_bps: DEFAULT_BASE_VOLATILITY_BAND_BPS,
            max_volatility_band_bps: MAX_VOLATILITY_BAND_BPS,
            min_cooldown_secs: DEFAULT_MIN_COOLDOWN_SECS,
            supply_demand_adj_bps: DEFAULT_SUPPLY_DEMAND_ADJ_BPS,
        }
    }
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self::default_issue_452()
    }
}

/// Rolling state for one resource.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceState {
    pub resource: Symbol,
    /// Current exponential moving average — the anti-manipulation anchor.
    pub sma: i128,
    /// Most recent raw print that was accepted.
    pub last_accepted: i128,
    /// Ledger timestamp of the most recent accepted observation.
    pub last_update: u64,
    /// EWMA of absolute deviation from the average, in basis points.
    pub volatility_bps: u32,
    /// How many raw prints have been discarded as out-of-band.
    pub rejected_prints: u32,
    /// Whether the most recent call discarded its print.
    ///
    /// A rejected print is *not* an error: a failed Soroban invocation rolls
    /// back its storage writes, so returning `Err` here would also roll back
    /// the rejection counter and the record would never be kept. The call
    /// succeeds, the average is left alone, and the rejection is reported
    /// through this flag, [`PriceState::rejected_prints`], and a
    /// `pricing.rejected` event.
    pub rejected_last: bool,
    /// How many observations have been folded into the average.
    pub observation_count: u32,
    /// Signed supply/demand nudge in basis points; positive means scarce.
    pub supply_demand_bps: i128,
}

/// One entry of the rolling price history.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PricePoint {
    pub sma: i128,
    pub raw: i128,
    pub published: i128,
    pub volatility_bps: u32,
    pub timestamp: u64,
}

/// The price other systems should actually use, plus why.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DynamicPrice {
    pub resource: Symbol,
    /// The smoothed average.
    pub sma: i128,
    /// Realized volatility driving the band width.
    pub volatility_bps: u32,
    /// Band actually applied, after widening for volatility.
    pub band_bps: u32,
    /// Signed supply/demand nudge applied, in basis points.
    pub supply_demand_bps: i128,
    /// The clamped, nudged price. Feed this to the DEX.
    pub published: i128,
}

/// Instance and persistent storage keys.
///
/// The variant names are module-prefixed on purpose. A `#[contracttype]` unit
/// variant encodes to the instance key `["<VariantName>"]`, so a bare `Config`
/// or `Admin` collides with every other module in this contract that uses the
/// same name — `difficulty_curve::CurveKey::Config` and friends already do, and
/// writing one silently corrupts the other. Prefixing keeps this module out of
/// that collision.
#[contracttype]
pub enum PricingKey {
    PricingConfig,
    PricingAdmin,
    /// resource -> PriceState
    PricingState(Symbol),
    /// resource -> Vec<PricePoint>
    PricingHistory(Symbol),
}

// ── Integer helpers ───────────────────────────────────────────────────────────

/// One EMA step: `sma + alpha_bps * (raw - sma) / BPS_DENOMINATOR`.
///
/// Integer division floors, so for a small gap the step rounds to zero and the
/// average would never converge. When that happens the average still moves by
/// one unit *towards* `raw`, so repeated prints always make progress instead of
/// stalling.
pub fn ema_step(sma: i128, raw: i128, alpha_bps: u32) -> i128 {
    let delta = raw - sma;
    let scaled = delta
        .saturating_mul(alpha_bps as i128)
        .checked_div(BPS_DENOMINATOR as i128)
        .unwrap_or(0);
    let mut next = sma.saturating_add(scaled);
    if next == sma && delta != 0 {
        next = sma.saturating_add(if delta > 0 { 1 } else { -1 });
    }
    if next <= 0 {
        1
    } else {
        next
    }
}

/// Distance from `sma` to `raw` in basis points, saturating at `u32::MAX`.
pub fn deviation_bps(sma: i128, raw: i128) -> u32 {
    if sma <= 0 {
        return u32::MAX;
    }
    let diff = raw.saturating_sub(sma).abs();
    let scaled = diff.saturating_mul(BPS_DENOMINATOR as i128) / sma;
    if scaled >= u32::MAX as i128 {
        u32::MAX
    } else {
        scaled as u32
    }
}

/// One EWMA step on a basis-point sample, with the same one-unit nudge so
/// rising volatility is always reflected.
fn ewma_bps(prev: u32, sample: u32, alpha_bps: u32) -> u32 {
    let prev_i = prev as i64;
    let delta = sample as i64 - prev_i;
    let mut next =
        prev_i + (delta.saturating_mul(alpha_bps as i64) / BPS_DENOMINATOR as i64);
    if next == prev_i && delta != 0 {
        next = prev_i + if delta > 0 { 1 } else { -1 };
    }
    next.clamp(0, u32::MAX as i64) as u32
}

/// Clamp `value` to `[anchor * (1 - band), anchor * (1 + band)]`.
fn clamp_to_band(anchor: i128, value: i128, band_bps: u32) -> i128 {
    if anchor <= 0 {
        return value.max(1);
    }
    let band = anchor
        .saturating_mul(band_bps as i128)
        .checked_div(BPS_DENOMINATOR as i128)
        .unwrap_or(0);
    let lower = anchor.saturating_sub(band).max(1);
    let upper = anchor.saturating_add(band);
    value.clamp(lower, upper)
}

/// Widen the band from `base` toward `max` in proportion to realized volatility.
fn effective_band(config: &PricingConfig, volatility_bps: u32) -> u32 {
    let base = config.base_volatility_band_bps.min(config.max_volatility_band_bps);
    let headroom = config.max_volatility_band_bps.saturating_sub(base);
    if headroom == 0 {
        return base;
    }
    // Volatility at or above max_deviation fully uses the headroom.
    let scale = if config.max_deviation_bps == 0 {
        0
    } else {
        (volatility_bps.min(config.max_deviation_bps) as u64 * headroom as u64)
            / config.max_deviation_bps as u64
    } as u32;
    base.saturating_add(scale).min(config.max_volatility_band_bps)
}

// ── Configuration ─────────────────────────────────────────────────────────────

/// Seed the pricing admin and guards. Admin-only, once.
pub fn init_pricing_config(
    env: &Env,
    admin: &Address,
    config: PricingConfig,
) -> Result<(), PricingError> {
    if env.storage().instance().has(&PricingKey::PricingAdmin) {
        return Err(PricingError::InvalidConfig);
    }
    admin.require_auth();
    validate_config(&config)?;
    env.storage().instance().set(&PricingKey::PricingAdmin, admin);
    env.storage()
        .instance()
        .set(&PricingKey::PricingConfig, &config);
    Ok(())
}

/// Retune the guards. Admin-only.
pub fn set_pricing_config(
    env: &Env,
    admin: &Address,
    config: PricingConfig,
) -> Result<(), PricingError> {
    let stored: Address = env
        .storage()
        .instance()
        .get(&PricingKey::PricingAdmin)
        .ok_or(PricingError::NotInitialized)?;
    admin.require_auth();
    if *admin != stored {
        return Err(PricingError::Unauthorized);
    }
    validate_config(&config)?;
    env.storage()
        .instance()
        .set(&PricingKey::PricingConfig, &config);
    Ok(())
}

/// The active configuration, falling back to the shipped defaults.
pub fn get_pricing_config(env: &Env) -> PricingConfig {
    env.storage()
        .instance()
        .get(&PricingKey::PricingConfig)
        .unwrap_or_else(PricingConfig::default_issue_452)
}

fn validate_config(config: &PricingConfig) -> Result<(), PricingError> {
    if config.smoothing_bps == 0 || config.smoothing_bps > MAX_SMOOTHING_BPS {
        return Err(PricingError::InvalidConfig);
    }
    if config.max_deviation_bps == 0 || config.max_deviation_bps > BPS_DENOMINATOR * 2 {
        return Err(PricingError::InvalidConfig);
    }
    if config.base_volatility_band_bps == 0
        || config.base_volatility_band_bps > MAX_VOLATILITY_BAND_BPS
        || config.max_volatility_band_bps < config.base_volatility_band_bps
        || config.max_volatility_band_bps > MAX_VOLATILITY_BAND_BPS
    {
        return Err(PricingError::InvalidConfig);
    }
    if config.supply_demand_adj_bps > BPS_DENOMINATOR {
        return Err(PricingError::InvalidConfig);
    }
    Ok(())
}

// ── Observations ──────────────────────────────────────────────────────────────

/// Feed one raw price print into the engine.
///
/// The first print for a resource seeds the average. Later prints must clear
/// the cooldown and must sit inside `max_deviation_bps` of the average.
///
/// An out-of-band print is **not** an error. It is counted in
/// [`PriceState::rejected_prints`], flagged in [`PriceState::rejected_last`],
/// announced in a `pricing.rejected` event, and the average is left exactly
/// where it was — which is the entire point of the guard. (It also cannot be an
/// error: a failed invocation reverts its own storage writes, which would take
/// the rejection counter down with it.)
pub fn observe_price(
    env: &Env,
    source: &Address,
    resource: Symbol,
    raw_price: i128,
) -> Result<PriceState, PricingError> {
    let stored: Address = env
        .storage()
        .instance()
        .get(&PricingKey::PricingAdmin)
        .ok_or(PricingError::NotInitialized)?;
    source.require_auth();
    if *source != stored {
        return Err(PricingError::Unauthorized);
    }
    if raw_price <= 0 {
        return Err(PricingError::InvalidPrice);
    }

    let config = get_pricing_config(env);
    let now = env.ledger().timestamp();
    let key = PricingKey::PricingState(resource.clone());

    let previous: Option<PriceState> = env.storage().persistent().get(&key);

    let mut state = match previous {
        // Seed: no average yet, so nothing to smooth or reject against.
        None => PriceState {
            resource: resource.clone(),
            sma: raw_price,
            last_accepted: raw_price,
            last_update: now,
            volatility_bps: 0,
            rejected_prints: 0,
            rejected_last: false,
            observation_count: 1,
            supply_demand_bps: 0,
        },
        Some(mut prior) => {
            if now.saturating_sub(prior.last_update) < config.min_cooldown_secs {
                return Err(PricingError::CooldownActive);
            }
            let dev = deviation_bps(prior.sma, raw_price);
            if dev > config.max_deviation_bps {
                prior.rejected_prints = prior.rejected_prints.saturating_add(1);
                prior.rejected_last = true;
                env.storage().persistent().set(&key, &prior);
                env.events().publish(
                    (symbol_short!("pricing"), symbol_short!("rejected")),
                    (resource, dev),
                );
                return Ok(prior);
            }
            let sma = ema_step(prior.sma, raw_price, config.smoothing_bps);
            prior.sma = sma;
            prior.last_accepted = raw_price;
            prior.last_update = now;
            prior.volatility_bps = ewma_bps(prior.volatility_bps, dev, config.smoothing_bps);
            prior.observation_count = prior.observation_count.saturating_add(1);
            prior.rejected_last = false;
            prior
        }
    };

    let published = price_from_state(&config, &state).published;

    env.storage().persistent().set(&key, &state);
    push_history(env, &resource, PricePoint {
        sma: state.sma,
        raw: raw_price,
        published,
        volatility_bps: state.volatility_bps,
        timestamp: now,
    });

    env.events().publish(
        (symbol_short!("pricing"), symbol_short!("observed")),
        (resource, raw_price, state.sma, published),
    );

    Ok(state)
}

/// Record the supply/demand balance for a resource.
///
/// Uses the parity convention from [`crate::economics::balancer`]: a ratio of
/// `1000` is balanced, so a ratio of `500` is a −50% nudge (oversupply) and
/// `2000` is +100% (undersupply), both clamped to
/// `supply_demand_adj_bps`.
pub fn observe_supply_demand(
    env: &Env,
    source: &Address,
    resource: Symbol,
    supply: i128,
    demand: i128,
) -> Result<i128, PricingError> {
    let stored: Address = env
        .storage()
        .instance()
        .get(&PricingKey::PricingAdmin)
        .ok_or(PricingError::NotInitialized)?;
    source.require_auth();
    if *source != stored {
        return Err(PricingError::Unauthorized);
    }
    if supply < 0 || demand < 0 {
        return Err(PricingError::InvalidPrice);
    }

    let config = get_pricing_config(env);
    // Parity is a ratio scaled by 1000, so the fractional deviation is
    // (ratio - 1000) / 1000, i.e. (ratio - 1000) * 10 in basis points.
    let ratio = if demand > 0 {
        supply
            .checked_mul(1_000)
            .ok_or(PricingError::ArithmeticOverflow)?
            / demand
    } else {
        1_000
    };
    let raw_bps = ratio
        .saturating_sub(1_000)
        .saturating_mul(10);
    let clamped = raw_bps.clamp(
        -(config.supply_demand_adj_bps as i128),
        config.supply_demand_adj_bps as i128,
    );

    let key = PricingKey::PricingState(resource.clone());
    let mut state: PriceState = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(PricingError::ResourceNotFound)?;
    state.supply_demand_bps = clamped;
    env.storage().persistent().set(&key, &state);

    env.events().publish(
        (symbol_short!("pricing"), symbol_short!("sd")),
        (resource, ratio, clamped),
    );

    Ok(clamped)
}

fn clamp_supply_demand_bps(config: &PricingConfig, bps: i128) -> i128 {
    bps.clamp(
        -(config.supply_demand_adj_bps as i128),
        config.supply_demand_adj_bps as i128,
    )
}

fn push_history(env: &Env, resource: &Symbol, point: PricePoint) {
    let key = PricingKey::PricingHistory(resource.clone());
    let mut history: Vec<PricePoint> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    history.push_back(point);
    while history.len() > MAX_HISTORY_ENTRIES {
        history.pop_front();
    }
    env.storage().persistent().set(&key, &history);
}

// ── Reads ─────────────────────────────────────────────────────────────────────

/// Rolling state for a resource.
pub fn get_price_state(env: &Env, resource: Symbol) -> Result<PriceState, PricingError> {
    env.storage()
        .persistent()
        .get(&PricingKey::PricingState(resource))
        .ok_or(PricingError::ResourceNotFound)
}

/// The smoothed average, without the band or supply/demand applied.
pub fn get_sma(env: &Env, resource: Symbol) -> Result<i128, PricingError> {
    Ok(get_price_state(env, resource)?.sma)
}

/// Realized volatility in basis points.
pub fn get_volatility_bps(env: &Env, resource: Symbol) -> Result<u32, PricingError> {
    Ok(get_price_state(env, resource)?.volatility_bps)
}

/// Rolling price history, oldest first.
pub fn get_pricing_history(env: &Env, resource: Symbol) -> Vec<PricePoint> {
    env.storage()
        .persistent()
        .get(&PricingKey::PricingHistory(resource))
        .unwrap_or_else(|| Vec::new(env))
}

/// The price the economy should use: supply/demand nudge applied, then clamped
/// into the volatility band around the average.
pub fn dynamic_price(env: &Env, resource: Symbol) -> Result<DynamicPrice, PricingError> {
    let state = get_price_state(env, resource)?;
    let config = get_pricing_config(env);
    Ok(price_from_state(&config, &state))
}

/// Shared by [`dynamic_price`] and the DEX so a listing and a read can never
/// disagree about the price.
pub fn price_from_state(config: &PricingConfig, state: &PriceState) -> DynamicPrice {
    let band_bps = effective_band(config, state.volatility_bps);
    let nudge = clamp_supply_demand_bps(config, state.supply_demand_bps);
    let nudged = state
        .sma
        .saturating_add(
            nudge
                .saturating_mul(state.sma)
                .checked_div(BPS_DENOMINATOR as i128)
                .unwrap_or(0),
        )
        .max(1);
    let published = clamp_to_band(state.sma, nudged, band_bps);
    DynamicPrice {
        resource: state.resource.clone(),
        sma: state.sma,
        volatility_bps: state.volatility_bps,
        band_bps,
        supply_demand_bps: nudge,
        published,
    }
}

/// Listing price for the DEX, floored at 1 stroop.
pub fn listing_price(env: &Env, resource: Symbol) -> Result<i128, PricingError> {
    Ok(dynamic_price(env, resource)?.published.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::{contract, contractimpl};

    #[contract]
    struct Stub;
    #[contractimpl]
    impl Stub {}

    fn setup() -> (Env, Address, Address, Symbol) {
        let env = Env::default();
        env.ledger().set(LedgerInfo {
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
        let admin = Address::generate(&env);
        let resource = Symbol::new(&env, "ore");
        env.mock_all_auths();
        (env, id, admin, resource)
    }

    fn in_contract<R: FnOnce() -> T, T>(env: &Env, id: &Address, f: R) -> T {
        env.as_contract(id, f)
    }

    /// Advance the ledger past the default cooldown.
    fn tick(env: &Env, secs: u64) {
        env.ledger().with_mut(|li| li.timestamp += secs);
    }

    /// Write the admin and config keys directly.
    ///
    /// A frame may only be authorized once, so seeding through
    /// `init_pricing_config` would burn the frame's single auth and starve the
    /// call a test is actually about. Writing the keys keeps one auth per
    /// frame, which lets each test drive the function under test directly.
    fn seed_admin(env: &Env, id: &Address, admin: &Address, config: PricingConfig) {
        in_contract(env, id, || {
            env.storage().instance().set(&PricingKey::PricingAdmin, admin);
            env.storage().instance().set(&PricingKey::PricingConfig, &config);
        });
    }

    /// Write a resource's state directly, as if `first` had already been
    /// observed at the current ledger timestamp.
    fn seed_state(env: &Env, id: &Address, resource: &Symbol, first: i128) {
        let state = PriceState {
            resource: resource.clone(),
            sma: first,
            last_accepted: first,
            last_update: env.ledger().timestamp(),
            volatility_bps: 0,
            rejected_prints: 0,
            rejected_last: false,
            observation_count: 1,
            supply_demand_bps: 0,
        };
        in_contract(env, id, || {
            env.storage()
                .persistent()
                .set(&PricingKey::PricingState(resource.clone()), &state);
        });
    }

    /// Defaults plus seeded admin plus one already-observed price.
    fn seeded(env: &Env, id: &Address, admin: &Address, resource: &Symbol, first: i128) {
        seed_admin(env, id, admin, PricingConfig::default_issue_452());
        seed_state(env, id, resource, first);
    }

    /// Config for the volatility tests: a 10% deviation cap so a 50% print is
    /// still rejected, and a tight 3% band so the clamp is easy to assert.
    fn tight_config() -> PricingConfig {
        PricingConfig {
            smoothing_bps: 2_000,
            max_deviation_bps: 1_000,
            base_volatility_band_bps: 3_000,
            max_volatility_band_bps: 3_000,
            min_cooldown_secs: 0,
            supply_demand_adj_bps: 0,
        }
    }

    // ── Pure integer helpers ───────────────────────────────────────────────

    #[test]
    fn ema_step_moves_twenty_percent_of_the_gap_by_default() {
        // sma 100, raw 200, alpha 20% => 100 + 20 = 120.
        assert_eq!(ema_step(100, 200, DEFAULT_SMOOTHING_BPS), 120);
    }

    #[test]
    fn ema_step_is_symmetric_around_the_average() {
        let up = ema_step(100, 200, DEFAULT_SMOOTHING_BPS) - 100;
        let down = 200 - ema_step(200, 100, DEFAULT_SMOOTHING_BPS);
        assert_eq!(up, down);
        assert_eq!(up, 20);
    }

    #[test]
    fn ema_step_nudges_by_one_instead_of_stalling_on_a_tiny_gap() {
        // 20% of a 1-unit gap floors to zero; without the nudge the average
        // would never move and repeated prints would do nothing.
        assert_eq!(ema_step(100, 101, DEFAULT_SMOOTHING_BPS), 101);
        assert_eq!(ema_step(100, 99, DEFAULT_SMOOTHING_BPS), 99);
    }

    #[test]
    fn ema_step_never_reaches_zero() {
        // A 100% alpha on a collapse must still leave a positive price.
        assert_eq!(ema_step(3, 1, BPS_DENOMINATOR), 1);
        assert!(ema_step(1, 1, BPS_DENOMINATOR) >= 1);
        assert!(ema_step(2, 1, BPS_DENOMINATOR) >= 1);
    }

    #[test]
    fn deviation_bps_measures_distance_from_the_average() {
        assert_eq!(deviation_bps(100, 100), 0);
        assert_eq!(deviation_bps(100, 150), 5_000);
        assert_eq!(deviation_bps(100, 50), 5_000);
        assert_eq!(deviation_bps(100, 110), 1_000);
    }

    #[test]
    fn deviation_bps_saturates_instead_of_overflowing() {
        // A near-zero average must not overflow on a huge print.
        assert_eq!(deviation_bps(1, 1_000_000), u32::MAX);
        assert_eq!(deviation_bps(0, 100), u32::MAX);
    }

    // ── Configuration ──────────────────────────────────────────────────────

    #[test]
    fn default_config_matches_issue_452_values() {
        let c = PricingConfig::default_issue_452();
        assert_eq!(c.smoothing_bps, 2_000);
        assert_eq!(c.max_deviation_bps, 5_000);
        assert_eq!(c.base_volatility_band_bps, 3_000);
        assert_eq!(c.max_volatility_band_bps, 5_000);
        assert_eq!(c.min_cooldown_secs, 60);
        assert_eq!(c.supply_demand_adj_bps, 2_500);
        assert_eq!(c, PricingConfig::default());
    }

    #[test]
    fn config_validation_rejects_implausible_values() {
        let base = PricingConfig::default_issue_452();

        // Alpha of zero would freeze the average.
        let mut c = base;
        c.smoothing_bps = 0;
        assert!(validate_config(&c).is_err());

        // Alpha above half stops smoothing anything.
        let mut c = base;
        c.smoothing_bps = MAX_SMOOTHING_BPS + 1;
        assert!(validate_config(&c).is_err());

        // A zero deviation cap would reject every print after the first.
        let mut c = base;
        c.max_deviation_bps = 0;
        assert!(validate_config(&c).is_err());

        // The band must be non-zero and the max must not sit below the base.
        let mut c = base;
        c.base_volatility_band_bps = 0;
        assert!(validate_config(&c).is_err());
        let mut c = base;
        c.max_volatility_band_bps = c.base_volatility_band_bps - 1;
        assert!(validate_config(&c).is_err());
        let mut c = base;
        c.max_volatility_band_bps = MAX_VOLATILITY_BAND_BPS + 1;
        assert!(validate_config(&c).is_err());
    }

    #[test]
    fn init_stores_the_config() {
        let (env, id, admin, _resource) = setup();
        let config = PricingConfig::default_issue_452();
        in_contract(&env, &id, || {
            init_pricing_config(&env, &admin, config).unwrap();
        });
        in_contract(&env, &id, || {
            assert_eq!(get_pricing_config(&env), config);
        });
    }

    #[test]
    fn init_rejects_double_init() {
        let (env, id, admin, _resource) = setup();
        seed_admin(&env, &id, &admin, PricingConfig::default_issue_452());
        in_contract(&env, &id, || {
            assert_eq!(
                init_pricing_config(&env, &admin, PricingConfig::default_issue_452()),
                Err(PricingError::InvalidConfig)
            );
        });
    }

    #[test]
    fn observe_rejects_a_non_admin_source() {
        let (env, id, admin, resource) = setup();
        let impostor = Address::generate(&env);
        seeded(&env, &id, &admin, &resource, 100);
        in_contract(&env, &id, || {
            assert_eq!(
                observe_price(&env, &impostor, resource.clone(), 120),
                Err(PricingError::Unauthorized)
            );
        });
    }

    #[test]
    fn set_config_rejects_a_non_admin_caller() {
        let (env, id, admin, _resource) = setup();
        let impostor = Address::generate(&env);
        seed_admin(&env, &id, &admin, PricingConfig::default_issue_452());
        in_contract(&env, &id, || {
            assert_eq!(
                set_pricing_config(&env, &impostor, PricingConfig::default_issue_452()),
                Err(PricingError::Unauthorized)
            );
        });
    }

    #[test]
    fn set_config_applies_to_subsequent_observations() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        in_contract(&env, &id, || {
            set_pricing_config(&env, &admin, tight_config()).unwrap();
        });
        tick(&env, 1);
        in_contract(&env, &id, || {
            observe_price(&env, &admin, resource.clone(), 110).unwrap();
        });
        in_contract(&env, &id, || {
            assert_eq!(get_sma(&env, resource).unwrap(), 102);
        });
    }

    #[test]
    fn operations_require_initialization() {
        let (env, id, admin, resource) = setup();
        in_contract(&env, &id, || {
            assert_eq!(
                observe_price(&env, &admin, resource.clone(), 100),
                Err(PricingError::NotInitialized)
            );
        });
        in_contract(&env, &id, || {
            assert_eq!(
                set_pricing_config(&env, &admin, PricingConfig::default_issue_452()),
                Err(PricingError::NotInitialized)
            );
        });
        // Reads still work without a config: the defaults apply.
        in_contract(&env, &id, || {
            assert_eq!(get_pricing_config(&env), PricingConfig::default_issue_452());
        });
    }

    // ── Observations ───────────────────────────────────────────────────────

    #[test]
    fn a_first_print_seeds_the_average_verbatim() {
        let (env, id, admin, resource) = setup();
        seed_admin(&env, &id, &admin, PricingConfig::default_issue_452());
        in_contract(&env, &id, || {
            observe_price(&env, &admin, resource.clone(), 250).unwrap();
        });
        in_contract(&env, &id, || {
            let state = get_price_state(&env, resource.clone()).unwrap();
            assert_eq!(state.sma, 250);
            assert_eq!(state.last_accepted, 250);
            assert_eq!(state.observation_count, 1);
            assert_eq!(state.volatility_bps, 0);
            assert_eq!(state.rejected_prints, 0);
        });
    }

    #[test]
    fn an_observation_smooths_towards_the_print() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
        in_contract(&env, &id, || {
            observe_price(&env, &admin, resource.clone(), 130).unwrap();
        });
        in_contract(&env, &id, || {
            // 100 + 20% of the 30 gap. The full move towards 130 is not taken.
            assert_eq!(get_sma(&env, resource.clone()).unwrap(), 106);
        });
    }

    #[test]
    fn a_print_beyond_the_cap_is_not_smoothed_in_either_direction() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
        in_contract(&env, &id, || {
            // +10000bps, double the cap: discarded, average untouched.
            let state = observe_price(&env, &admin, resource.clone(), 200).unwrap();
            assert!(state.rejected_last);
            assert_eq!(state.sma, 100);
        });
        tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
        in_contract(&env, &id, || {
            // -5000bps, exactly the cap: in band.
            let state = observe_price(&env, &admin, resource.clone(), 50).unwrap();
            assert!(!state.rejected_last);
            // 100 - 20% of the 50 gap.
            assert_eq!(state.sma, 90);
        });
    }

    #[test]
    fn a_wild_print_is_rejected_and_leaves_the_average_alone() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
        in_contract(&env, &id, || {
            // +10000bps is double the 5000bps cap.
            let state = observe_price(&env, &admin, resource.clone(), 1_000).unwrap();
            assert!(state.rejected_last);
            assert_eq!(state.rejected_prints, 1);
        });
        // The whole point: the average did not move for the attacker, and the
        // rejection survived the call.
        in_contract(&env, &id, || {
            let state = get_price_state(&env, resource.clone()).unwrap();
            assert_eq!(state.sma, 100);
            assert_eq!(state.last_accepted, 100);
            assert_eq!(state.observation_count, 1);
            assert_eq!(state.rejected_prints, 1);
            assert!(state.rejected_last);
        });
    }

    #[test]
    fn an_accepted_print_clears_the_rejection_flag() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
        in_contract(&env, &id, || {
            observe_price(&env, &admin, resource.clone(), 1_000).unwrap();
        });
        tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
        in_contract(&env, &id, || {
            let state = observe_price(&env, &admin, resource.clone(), 130).unwrap();
            assert!(!state.rejected_last);
            // The earlier rejection is still counted, just no longer the latest.
            assert_eq!(state.rejected_prints, 1);
        });
    }

    #[test]
    fn a_rejected_print_leaves_the_price_history_alone() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
        in_contract(&env, &id, || {
            observe_price(&env, &admin, resource.clone(), 1_000).unwrap();
        });
        in_contract(&env, &id, || {
            // A print that was thrown away is not a price point.
            assert_eq!(get_pricing_history(&env, resource).len(), 0);
        });
    }

    #[test]
    fn a_print_exactly_at_the_cap_is_accepted() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
        in_contract(&env, &id, || {
            // +50% is exactly max_deviation_bps, so it is in band.
            observe_price(&env, &admin, resource.clone(), 150).unwrap();
        });
        in_contract(&env, &id, || {
            assert_eq!(get_sma(&env, resource.clone()).unwrap(), 110);
        });
    }

    #[test]
    fn rejected_prints_are_counted() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        for _ in 0..3 {
            tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
            in_contract(&env, &id, || {
                assert!(observe_price(&env, &admin, resource.clone(), 100_000)
                    .unwrap()
                    .rejected_last);
            });
        }
        in_contract(&env, &id, || {
            let state = get_price_state(&env, resource.clone()).unwrap();
            assert_eq!(state.rejected_prints, 3);
            assert_eq!(state.sma, 100);
            assert_eq!(state.observation_count, 1);
        });
    }

    #[test]
    fn the_cooldown_blocks_rapid_successive_prints() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        tick(&env, DEFAULT_MIN_COOLDOWN_SECS - 1);
        in_contract(&env, &id, || {
            assert_eq!(
                observe_price(&env, &admin, resource.clone(), 105),
                Err(PricingError::CooldownActive)
            );
        });
    }

    #[test]
    fn non_positive_prices_are_rejected() {
        let (env, id, admin, resource) = setup();
        seed_admin(&env, &id, &admin, PricingConfig::default_issue_452());
        in_contract(&env, &id, || {
            assert_eq!(
                observe_price(&env, &admin, resource.clone(), 0),
                Err(PricingError::InvalidPrice)
            );
        });
        tick(&env, 1);
        in_contract(&env, &id, || {
            assert_eq!(
                observe_price(&env, &admin, resource.clone(), -5),
                Err(PricingError::InvalidPrice)
            );
        });
    }

    // ── Published price ────────────────────────────────────────────────────

    #[test]
    fn a_flat_market_publishes_the_average_unchanged() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        in_contract(&env, &id, || {
            let price = dynamic_price(&env, resource.clone()).unwrap();
            assert_eq!(price.sma, 100);
            assert_eq!(price.published, 100);
            assert_eq!(price.volatility_bps, 0);
            assert_eq!(price.supply_demand_bps, 0);
            // A quiet market uses the base band, not the hard cap.
            assert_eq!(price.band_bps, DEFAULT_BASE_VOLATILITY_BAND_BPS);
        });
    }

    #[test]
    fn a_reported_spike_cannot_teleport_the_published_price() {
        let (env, id, admin, resource) = setup();
        seed_admin(&env, &id, &admin, tight_config());
        seed_state(&env, &id, &resource, 100);
        in_contract(&env, &id, || {
            // Inside the 10% deviation cap, so it is folded in...
            observe_price(&env, &admin, resource.clone(), 110).unwrap();
        });
        in_contract(&env, &id, || {
            let price = dynamic_price(&env, resource.clone()).unwrap();
            // ...but the average only crept to 102, and the published price
            // stays within 3% of it — nowhere near the 110 print.
            assert_eq!(price.sma, 102);
            assert!(price.published <= 105, "published {} escaped the band", price.published);
            assert!(price.published >= 99);
        });
    }

    #[test]
    fn the_band_clamps_the_nudged_price_too() {
        let (env, id, admin, resource) = setup();
        // A 10% band is tighter than the 25% undersupply nudge, so the band
        // has the last word.
        seed_admin(
            &env,
            &id,
            &admin,
            PricingConfig {
                supply_demand_adj_bps: 2_500,
                base_volatility_band_bps: 1_000,
                max_volatility_band_bps: 1_000,
                ..PricingConfig::default_issue_452()
            },
        );
        seed_state(&env, &id, &resource, 100);
        in_contract(&env, &id, || {
            // ratio 3000 => +20000bps raw, clamped to +2500.
            let bps =
                observe_supply_demand(&env, &admin, resource.clone(), 3_000, 1_000).unwrap();
            assert_eq!(bps, 2_500);
        });
        in_contract(&env, &id, || {
            let price = dynamic_price(&env, resource.clone()).unwrap();
            assert_eq!(price.supply_demand_bps, 2_500);
            assert_eq!(price.band_bps, 1_000);
            // The nudge alone would ask for 125; the 10% band caps it at 110.
            assert_eq!(price.published, 110);
        });
    }

    #[test]
    fn volatility_widens_the_band_up_to_the_hard_cap() {
        let config = PricingConfig::default_issue_452();
        // No volatility -> base band.
        assert_eq!(effective_band(&config, 0), 3_000);
        // Half the deviation cap -> halfway through the headroom.
        assert_eq!(effective_band(&config, 2_500), 4_000);
        // At or beyond the cap -> the hard ceiling, never past it.
        assert_eq!(effective_band(&config, 5_000), 5_000);
        assert_eq!(effective_band(&config, 900_000), 5_000);
    }

    #[test]
    fn realized_volatility_rises_with_a_wobbly_market() {
        let (env, id, admin, resource) = setup();
        seed_admin(&env, &id, &admin, tight_config());
        seed_state(&env, &id, &resource, 100);
        // Repeatedly print the most the 10% cap allows, in both directions, so
        // the EWMA of deviation climbs off zero.
        for i in 0..12 {
            in_contract(&env, &id, || {
                let raw = if i % 2 == 0 { 110 } else { 90 };
                let _ = observe_price(&env, &admin, resource.clone(), raw);
            });
        }
        in_contract(&env, &id, || {
            let vol = get_volatility_bps(&env, resource.clone()).unwrap();
            assert!(vol > 0, "a wobbly market should register volatility");
            // Never unbounded: the EWMA is driven by capped samples.
            assert!(vol <= 1_000);
        });
    }

    // ── Supply / demand ────────────────────────────────────────────────────

    #[test]
    fn undersupply_pushes_the_published_price_up() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        in_contract(&env, &id, || {
            // ratio 2000 => undersupply => +10000bps raw, clamped to +2500.
            let bps =
                observe_supply_demand(&env, &admin, resource.clone(), 2_000, 1_000).unwrap();
            assert_eq!(bps, 2_500);
        });
        in_contract(&env, &id, || {
            let price = dynamic_price(&env, resource.clone()).unwrap();
            assert_eq!(price.supply_demand_bps, 2_500);
            // 100 nudged +25%, inside the 30% band.
            assert_eq!(price.published, 125);
        });
    }

    #[test]
    fn oversupply_pushes_the_published_price_down() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        in_contract(&env, &id, || {
            // ratio 500 => oversupply => -5000bps raw, clamped to -2500.
            let bps =
                observe_supply_demand(&env, &admin, resource.clone(), 500, 1_000).unwrap();
            assert_eq!(bps, -2_500);
        });
        in_contract(&env, &id, || {
            // 100 nudged -25%, inside the 30% band.
            assert_eq!(dynamic_price(&env, resource.clone()).unwrap().published, 75);
        });
    }

    #[test]
    fn balanced_supply_leaves_the_price_alone() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        in_contract(&env, &id, || {
            let bps =
                observe_supply_demand(&env, &admin, resource.clone(), 1_000, 1_000).unwrap();
            assert_eq!(bps, 0);
        });
        in_contract(&env, &id, || {
            assert_eq!(dynamic_price(&env, resource.clone()).unwrap().published, 100);
        });
    }

    #[test]
    fn zero_demand_reads_as_balanced() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        in_contract(&env, &id, || {
            let bps =
                observe_supply_demand(&env, &admin, resource.clone(), 5_000, 0).unwrap();
            assert_eq!(bps, 0);
        });
    }

    #[test]
    fn supply_demand_rejects_negatives_and_unseen_resources() {
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        in_contract(&env, &id, || {
            assert_eq!(
                observe_supply_demand(&env, &admin, resource.clone(), -1, 1),
                Err(PricingError::InvalidPrice)
            );
        });
        tick(&env, 1);
        in_contract(&env, &id, || {
            assert_eq!(
                observe_supply_demand(&env, &admin, Symbol::new(&env, "unseen"), 1, 1),
                Err(PricingError::ResourceNotFound)
            );
        });
    }

    // ── Reads ───────────────────────────────────────────────────────────────

    #[test]
    fn reads_on_an_unseen_resource_report_not_found() {
        let (env, id, _admin, _resource) = setup();
        in_contract(&env, &id, || {
            let unseen = Symbol::new(&env, "unobtanium");
            assert_eq!(
                get_price_state(&env, unseen.clone()),
                Err(PricingError::ResourceNotFound)
            );
            assert_eq!(get_sma(&env, unseen.clone()), Err(PricingError::ResourceNotFound));
            assert_eq!(
                dynamic_price(&env, unseen.clone()),
                Err(PricingError::ResourceNotFound)
            );
            assert_eq!(
                listing_price(&env, unseen.clone()),
                Err(PricingError::ResourceNotFound)
            );
            assert_eq!(get_pricing_history(&env, unseen).len(), 0);
        });
    }

    #[test]
    fn history_is_capped_and_ordered() {
        let (env, id, admin, resource) = setup();
        seed_admin(&env, &id, &admin, PricingConfig::default_issue_452());
        seed_state(&env, &id, &resource, 100);
        let total = MAX_HISTORY_ENTRIES + 8;
        for i in 0..total {
            tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
            in_contract(&env, &id, || {
                // Small increments keep every print inside the deviation cap.
                let raw = 100 + (i % 5) as i128;
                observe_price(&env, &admin, resource.clone(), raw).unwrap();
            });
        }
        in_contract(&env, &id, || {
            let history = get_pricing_history(&env, resource.clone());
            assert_eq!(history.len(), MAX_HISTORY_ENTRIES);
            // Oldest first, so entry 0 predates the final entry.
            let first = history.get(0).unwrap();
            let last = history.get(history.len() - 1).unwrap();
            assert!(first.timestamp <= last.timestamp);
            // Seeded state plus every accepted print.
            assert_eq!(
                get_price_state(&env, resource).unwrap().observation_count,
                total + 1
            );
        });
    }

    #[test]
    fn price_from_state_agrees_with_dynamic_price() {
        // The DEX and a direct read must never disagree.
        let (env, id, admin, resource) = setup();
        seeded(&env, &id, &admin, &resource, 100);
        tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
        in_contract(&env, &id, || {
            observe_price(&env, &admin, resource.clone(), 130).unwrap();
        });
        tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
        in_contract(&env, &id, || {
            // ratio 1800 => +8000bps raw, clamped to +2500.
            let bps =
                observe_supply_demand(&env, &admin, resource.clone(), 1_800, 1_000).unwrap();
            assert_eq!(bps, 2_500);
        });
        in_contract(&env, &id, || {
            let state = get_price_state(&env, resource.clone()).unwrap();
            let config = get_pricing_config(&env);
            assert_eq!(state.sma, 106);
            assert_eq!(
                price_from_state(&config, &state).published,
                dynamic_price(&env, resource.clone()).unwrap().published
            );
            // 106 nudged +25% = 132, inside the 30% band.
            assert_eq!(listing_price(&env, resource.clone()).unwrap(), 132);
        });
    }
}

