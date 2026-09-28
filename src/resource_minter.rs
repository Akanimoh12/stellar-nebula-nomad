// ============================================================
// resource_minter.rs — Fix #175: Rate-limited resource minting
// Branch: security/rate-limiting
// ============================================================
//
// Changes vs baseline:
//   • Import and call check_rate_limit(Operation::ResourceMinting)
//     at the top of mint_resource() before any state mutation.
//   • RateLimitHit events are emitted inside check_rate_limit.

use crate::nebula_explorer::{CellType, NebulaLayout};
use crate::nebula_gen::{NebulaError as NebulaGenError, NebulaGen};
use crate::rate_limiter::{check_rate_limit, Operation, RateLimitError};
use crate::reentrancy_guard::{with_guard, ReentrancyError};
use crate::economics::anti_whale::{process_anti_whale_action, AntiWhaleError};
use soroban_sdk::{contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, Symbol, Vec};

pub type AssetId = ResourceType;

#[contracttype]
#[derive(Clone)]
pub enum ResourceKey {
    ResourceBalance(Address, Symbol),
    /// Monotonic counter backing [`DexOffer::offer_id`] allocation.
    DexOfferCounter,
    /// DEX offer record by ID.
    DexOffer(u64),
}

pub fn resource_type_to_symbol(rt: &ResourceType) -> Symbol {
    match rt {
        ResourceType::StellarDust => symbol_short!("stdust"),
        ResourceType::DarkMatter => symbol_short!("drmatt"),
        ResourceType::ExoticMatter => symbol_short!("exomat"),
    }
}

// ── Resource types ────────────────────────────────────────────
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceType {
    StellarDust,
    DarkMatter,
    ExoticMatter,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceRecord {
    pub owner: Address,
    pub resource_type: ResourceType,
    pub amount: u64,
    pub minted_at: u64,
}

/// Compact form of [`ResourceRecord`]: `amount` and `minted_at` share one
/// `u128` slot (see `storage_optim::pack_u64x2`). New storage only; existing
/// `ResourceRecord` entries are untouched for backward compatibility.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackedResourceRecord {
    pub owner: Address,
    pub resource_type: ResourceType,
    pub packed: u128,
}

impl From<&ResourceRecord> for PackedResourceRecord {
    fn from(r: &ResourceRecord) -> Self {
        Self {
            owner: r.owner.clone(),
            resource_type: r.resource_type.clone(),
            packed: crate::storage_optim::pack_u64x2(r.amount, r.minted_at),
        }
    }
}

impl From<&PackedResourceRecord> for ResourceRecord {
    fn from(p: &PackedResourceRecord) -> Self {
        let (amount, minted_at) = crate::storage_optim::unpack_u64x2(p.packed);
        Self {
            owner: p.owner.clone(),
            resource_type: p.resource_type.clone(),
            amount,
            minted_at,
        }
    }
}

#[contracttype]
pub enum MinterKey {
    Balance(Address, ResourceType),
    TotalSupply(ResourceType),
    /// Cumulative amount ever minted, never decremented. Together with the
    /// burn counters in `token_burning` this gives the deflation rate
    /// (Issue #281): `burned / ever_minted`.
    TotalMinted(ResourceType),
}

// ── Error ─────────────────────────────────────────────────────
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MinterError {
    /// Amount must be > 0.
    InvalidAmount = 200,
    /// Caller exceeded the minting rate limit (DoS prevention).
    RateLimitExceeded = 201,
    /// No nebula layout found for this ship (must scan first).
    NoLayoutForShip = 202,
    /// The specified anomaly index does not contain a resource.
    NoResourceAtAnomaly = 203,
    /// A checked arithmetic operation overflowed (Issue #239).
    ArithmeticOverflow = 204,
    /// The account holds less than the requested debit amount (Issue #281).
    InsufficientBalance = 205,
    /// Requested amount exceeds anti-whale daily operation cap (Issue #455).
    DailyCapExceeded = 206,
    /// A guarded section was re-entered (Issue #472).
    Reentrancy = 207,
}

impl From<AntiWhaleError> for MinterError {
    fn from(err: AntiWhaleError) -> Self {
        match err {
            AntiWhaleError::DailyCapExceeded => MinterError::DailyCapExceeded,
            AntiWhaleError::ArithmeticOverflow => MinterError::ArithmeticOverflow,
            AntiWhaleError::InvalidAmount => MinterError::InvalidAmount,
        }
    }
}

impl crate::error_standard::StandardContractError for MinterError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::InvalidAmount => (ErrorKind::Validation, false),
            Self::RateLimitExceeded => (ErrorKind::ResourceLimit, true),
            Self::NoLayoutForShip | Self::NoResourceAtAnomaly => (ErrorKind::NotFound, false),
            Self::ArithmeticOverflow | Self::InsufficientBalance | Self::DailyCapExceeded => {
                (ErrorKind::ResourceLimit, false)
            }
            Self::Reentrancy => (ErrorKind::Conflict, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "resource_minter",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

impl From<ReentrancyError> for MinterError {
    fn from(_: ReentrancyError) -> Self {
        MinterError::Reentrancy
    }
}

impl From<RateLimitError> for MinterError {
    fn from(_: RateLimitError) -> Self {
        MinterError::RateLimitExceeded
    }
}

// ── Contract ─────────────────────────────────────────────────
#[contract]
pub struct ResourceMinterContract;

#[contractimpl]
impl ResourceMinterContract {
    /// Mint `amount` units of `resource_type` for `caller`.
    ///
    /// Rate-limited to prevent spam (Issue #175).
    ///
    /// # Reentrancy
    /// Runs under the global reentrancy guard (Issue #472). Balance, supply
    /// and mint counters are all written before the `minted` event is
    /// published, so a nested call can never observe a half-applied mint.
    pub fn mint_resource(
        env: &Env,
        caller: Address,
        ship_id: u64,
        anomaly_index: u32,
        resource_type: ResourceType,
        amount: u64,
    ) -> Result<ResourceRecord, MinterError> {
        // ── Auth ───────────────────────────────────────────────
        caller.require_auth();

        with_guard(env, || {
            mint_resource_unguarded(env, caller, ship_id, anomaly_index, resource_type, amount)
        })
    }

    /// Query the balance of `owner` for `resource_type`.
    pub fn balance(env: &Env, owner: Address, resource_type: ResourceType) -> u64 {
        env.storage()
            .persistent()
            .get(&MinterKey::Balance(owner, resource_type))
            .unwrap_or(0)
    }

    /// Total supply of a given resource type.
    pub fn total_supply(env: &Env, resource_type: ResourceType) -> u64 {
        env.storage()
            .persistent()
            .get(&MinterKey::TotalSupply(resource_type))
            .unwrap_or(0)
    }

    /// Cumulative amount ever minted for `resource_type`, ignoring burns.
    pub fn total_minted(env: &Env, resource_type: ResourceType) -> u64 {
        self::total_minted(env, &resource_type)
    }
}

/// Unguarded body of [`ResourceMinterContract::mint_resource`]; callers must
/// already hold the reentrancy lock.
fn mint_resource_unguarded(
    env: &Env,
    caller: Address,
    ship_id: u64,
    anomaly_index: u32,
    resource_type: ResourceType,
    amount: u64,
) -> Result<ResourceRecord, MinterError> {
    // ── Rate limit check (Issue #175) ──────────────────────
    check_rate_limit(env, &caller, Operation::ResourceMinting).map_err(MinterError::from)?;

    // ── Basic validation ───────────────────────────────────
    if amount == 0 {
        return Err(MinterError::InvalidAmount);
    }

    // ── Confirm anomaly exists for this ship ───────────────
    NebulaGen::has_anomaly(env.clone(), ship_id, anomaly_index).map_err(|e| match e {
        NebulaGenError::LayoutNotFound => MinterError::NoLayoutForShip,
        NebulaGenError::AnomalyOutOfBounds => MinterError::NoResourceAtAnomaly,
        _ => MinterError::NoLayoutForShip,
    })?;

    // ── Anti-Whale check (Issue #455) ─────────────────────
    let (effective_amount, _progressive_fee) = process_anti_whale_action(env, &caller, amount)?;

    // ── Update balances (checked: Issue #239) ──────────────
    let balance_key = MinterKey::Balance(caller.clone(), resource_type.clone());
    let current: u64 = env.storage().persistent().get(&balance_key).unwrap_or(0);
    let new_balance = current
        .checked_add(effective_amount)
        .ok_or(MinterError::ArithmeticOverflow)?;
    env.storage().persistent().set(&balance_key, &new_balance);

    let supply_key = MinterKey::TotalSupply(resource_type.clone());
    let supply: u64 = env.storage().persistent().get(&supply_key).unwrap_or(0);
    let new_supply = supply
        .checked_add(effective_amount)
        .ok_or(MinterError::ArithmeticOverflow)?;
    env.storage().persistent().set(&supply_key, &new_supply);

    // ── Cumulative mint counter (Issue #281) ───────────────
    // Unlike TotalSupply this is monotonic — burning reduces supply but
    // never the historical mint total, which is the denominator of the
    // deflation rate.
    let minted_key = MinterKey::TotalMinted(resource_type.clone());
    let minted: u64 = env.storage().persistent().get(&minted_key).unwrap_or(0);
    let new_minted = minted
        .checked_add(effective_amount)
        .ok_or(MinterError::ArithmeticOverflow)?;
    env.storage().persistent().set(&minted_key, &new_minted);

    let record = ResourceRecord {
        owner: caller.clone(),
        resource_type: resource_type.clone(),
        amount: effective_amount,
        minted_at: env.ledger().timestamp(),
    };

    // ── Emit event ─────────────────────────────────────────
    env.events().publish(
        (symbol_short!("Minter"), symbol_short!("minted")),
        (caller, resource_type, effective_amount),
    );

    Ok(record)
}

// ─────────────────────────────────────────────────────────────
// Supply-reducing primitives (Issue #281)
// ─────────────────────────────────────────────────────────────
//
// `token_burning` is the only intended caller. Keeping these here rather than
// in the burning module means the balance and supply ledgers stay owned by a
// single module, so every credit and debit goes through the same checked
// arithmetic.

/// Read a holder's balance without going through the contract entrypoint.
pub fn balance_of(env: &Env, owner: &Address, resource_type: &ResourceType) -> u64 {
    env.storage()
        .persistent()
        .get(&MinterKey::Balance(owner.clone(), resource_type.clone()))
        .unwrap_or(0)
}

/// Cumulative amount ever minted for `resource_type`.
pub fn total_minted(env: &Env, resource_type: &ResourceType) -> u64 {
    env.storage()
        .persistent()
        .get(&MinterKey::TotalMinted(resource_type.clone()))
        .unwrap_or(0)
}

/// Current circulating supply for `resource_type`.
pub fn circulating_supply(env: &Env, resource_type: &ResourceType) -> u64 {
    env.storage()
        .persistent()
        .get(&MinterKey::TotalSupply(resource_type.clone()))
        .unwrap_or(0)
}

/// Debit `amount` from `owner`'s balance.
///
/// Does **not** require auth — the caller is responsible for having authorized
/// the holder. Returns the new balance, or `InsufficientBalance` if the debit
/// would go negative (checked, never wrapping).
pub fn debit_balance(
    env: &Env,
    owner: &Address,
    resource_type: &ResourceType,
    amount: u64,
) -> Result<u64, MinterError> {
    if amount == 0 {
        return Err(MinterError::InvalidAmount);
    }

    let key = MinterKey::Balance(owner.clone(), resource_type.clone());
    let current: u64 = env.storage().persistent().get(&key).unwrap_or(0);
    let new_balance = current
        .checked_sub(amount)
        .ok_or(MinterError::InsufficientBalance)?;
    env.storage().persistent().set(&key, &new_balance);

    Ok(new_balance)
}

/// Reduce the circulating supply of `resource_type` by `amount`.
///
/// Returns the new supply. Underflow is treated as `InsufficientBalance`: it
/// would mean the supply ledger disagreed with the sum of balances, and
/// silently saturating there would hide the inconsistency.
pub fn reduce_supply(
    env: &Env,
    resource_type: &ResourceType,
    amount: u64,
) -> Result<u64, MinterError> {
    if amount == 0 {
        return Err(MinterError::InvalidAmount);
    }

    let key = MinterKey::TotalSupply(resource_type.clone());
    let supply: u64 = env.storage().persistent().get(&key).unwrap_or(0);
    let new_supply = supply
        .checked_sub(amount)
        .ok_or(MinterError::InsufficientBalance)?;
    env.storage().persistent().set(&key, &new_supply);

    Ok(new_supply)
}

/// Move `amount` between two holders without touching supply counters.
///
/// A transfer is not a mint or a burn, so neither `TotalSupply` nor
/// `TotalMinted` changes — only the two balances do.
pub fn move_balance(
    env: &Env,
    from: &Address,
    to: &Address,
    resource_type: &ResourceType,
    amount: u64,
) -> Result<u64, MinterError> {
    if amount == 0 {
        return Err(MinterError::InvalidAmount);
    }
    if from == to {
        return Ok(balance_of(env, from, resource_type));
    }

    let from_key = MinterKey::Balance(from.clone(), resource_type.clone());
    let from_balance: u64 = env.storage().persistent().get(&from_key).unwrap_or(0);
    let remaining = from_balance
        .checked_sub(amount)
        .ok_or(MinterError::InsufficientBalance)?;

    let to_key = MinterKey::Balance(to.clone(), resource_type.clone());
    let to_balance: u64 = env.storage().persistent().get(&to_key).unwrap_or(0);
    let credited = to_balance
        .checked_add(amount)
        .ok_or(MinterError::ArithmeticOverflow)?;

    env.storage().persistent().set(&from_key, &remaining);
    env.storage().persistent().set(&to_key, &credited);

    Ok(credited)
}

/// Credit `amount` to `owner` without auth or rate limiting.
///
/// Test- and reward-path helper: used by the burn tests to seed balances and by
/// reward subsystems that grant resources they have already authorized.
pub fn credit_balance(
    env: &Env,
    owner: &Address,
    resource_type: &ResourceType,
    amount: u64,
) -> Result<u64, MinterError> {
    if amount == 0 {
        return Err(MinterError::InvalidAmount);
    }

    let key = MinterKey::Balance(owner.clone(), resource_type.clone());
    let current: u64 = env.storage().persistent().get(&key).unwrap_or(0);
    let new_balance = current
        .checked_add(amount)
        .ok_or(MinterError::ArithmeticOverflow)?;
    env.storage().persistent().set(&key, &new_balance);

    let supply_key = MinterKey::TotalSupply(resource_type.clone());
    let supply: u64 = env.storage().persistent().get(&supply_key).unwrap_or(0);
    env.storage().persistent().set(
        &supply_key,
        &supply
            .checked_add(amount)
            .ok_or(MinterError::ArithmeticOverflow)?,
    );

    let minted_key = MinterKey::TotalMinted(resource_type.clone());
    let minted: u64 = env.storage().persistent().get(&minted_key).unwrap_or(0);
    env.storage().persistent().set(
        &minted_key,
        &minted
            .checked_add(amount)
            .ok_or(MinterError::ArithmeticOverflow)?,
    );

    Ok(new_balance)
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────
// ─────────────────────────────────────────────────────────────────────────────
// Harvest + DEX auto-listing
// ─────────────────────────────────────────────────────────────────────────────
//
// `harvest_resources` converts a scanned nebula layout into on-chain resource
// balances, and `auto_list_on_dex` turns those balances into DEX offers. Both
// write to the same `ResourceKey::ResourceBalance(owner, asset)` ledger that
// `gifting_system` and `ship_upgrade` already use, so a harvested unit is
// immediately spendable on gifts and upgrades without a conversion step.
//
// This is deliberately separate from the `MinterKey` ledger behind
// `mint_resource`: harvesting is layout-derived and not rate-limited (the layout
// is the cost), whereas minting is caller-chosen and rate-limited.

/// Asset symbol for a harvested cell type, or `None` for non-resource cells.
///
/// `CellType::Empty` and `CellType::Star` carry no harvestable resource.
fn cell_type_to_asset(cell_type: &CellType) -> Option<Symbol> {
    match cell_type {
        CellType::StellarDust => Some(symbol_short!("dust")),
        CellType::Asteroid => Some(symbol_short!("ore")),
        CellType::GasCloud => Some(symbol_short!("gas")),
        CellType::DarkMatter => Some(symbol_short!("dark")),
        CellType::ExoticMatter => Some(symbol_short!("exotic")),
        CellType::Wormhole => Some(symbol_short!("worm")),
        CellType::Empty | CellType::Star => None,
    }
}

/// A single asset yielded by one harvest.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarvestedResource {
    /// Asset symbol credited to the ship's owner.
    pub asset_id: Symbol,
    /// Units credited.
    pub amount: u32,
}

/// Outcome of a harvest pass over a layout.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarvestResult {
    /// Ship the harvest was performed with.
    pub ship_id: u64,
    /// Per-asset breakdown, in layout cell order.
    pub resources: Vec<HarvestedResource>,
    /// Sum of every entry in `resources`.
    pub total_harvested: u32,
}

/// A live or cancelled DEX listing.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DexOffer {
    /// Unique, monotonically increasing offer ID.
    pub offer_id: u64,
    /// Seller whose `amount` is held in escrow. Only this address may cancel
    /// the offer, and the refund is always credited back to them.
    pub seller: Address,
    /// Asset symbol being sold.
    pub asset_id: Symbol,
    /// Units escrowed from the seller at listing time.
    pub amount: u32,
    /// Seller's floor price per unit. Always `> 0`.
    pub min_price: i128,
    /// `false` once cancelled.
    pub active: bool,
}

/// Errors from the harvest and DEX-listing paths.
///
/// Discriminants are part of the contract's public ABI — append new variants
/// rather than renumbering existing ones.
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum HarvestError {
    /// No ship with the given ID exists.
    ShipNotFound = 1,
    /// The layout yielded no resources (all cells empty or non-resource).
    EmptyHarvest = 2,
    /// `min_price` was zero or negative.
    InvalidPrice = 3,
    /// The requested asset was not present in this harvest.
    AssetNotHarvested = 4,
    /// A checked arithmetic operation overflowed (Issue #239).
    PriceOverflow = 5,
    /// Generic DEX failure: unknown offer, already cancelled, or rate-limited.
    DexFailure = 6,
    /// Seller does not hold enough of `resource` to cover the listing.
    InsufficientBalance = 7,
    /// A guarded section was re-entered (Issue #472).
    Reentrancy = 8,
}

impl crate::error_standard::StandardContractError for HarvestError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::ShipNotFound | Self::AssetNotHarvested => (ErrorKind::NotFound, false),
            Self::EmptyHarvest | Self::InvalidPrice => (ErrorKind::Validation, false),
            Self::PriceOverflow | Self::DexFailure => (ErrorKind::Internal, false),
            Self::InsufficientBalance => (ErrorKind::ResourceLimit, false),
            Self::Reentrancy => (ErrorKind::Conflict, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "resource_minter",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

impl From<ReentrancyError> for HarvestError {
    fn from(_: ReentrancyError) -> Self {
        HarvestError::Reentrancy
    }
}

/// Allocate the next DEX offer ID.
///
/// Shared with [`crate::dex_integration`] so both listing paths draw from one
/// counter and IDs can never collide.
pub(crate) fn next_dex_offer_id(env: &Env) -> Result<u64, HarvestError> {
    let current: u64 = env
        .storage()
        .instance()
        .get(&ResourceKey::DexOfferCounter)
        .unwrap_or(0);
    let next = current.checked_add(1).ok_or(HarvestError::PriceOverflow)?;
    env.storage()
        .instance()
        .set(&ResourceKey::DexOfferCounter, &next);
    Ok(next)
}

/// Read a holder's harvest balance for `asset`.
pub fn resource_balance(env: &Env, owner: &Address, asset: &Symbol) -> u32 {
    env.storage()
        .instance()
        .get(&ResourceKey::ResourceBalance(owner.clone(), asset.clone()))
        .unwrap_or(0)
}

/// Credit `amount` of `asset` to `owner`, rejecting overflow (Issue #239).
pub fn credit_resource_balance(
    env: &Env,
    owner: &Address,
    asset: &Symbol,
    amount: u32,
) -> Result<u32, HarvestError> {
    let key = ResourceKey::ResourceBalance(owner.clone(), asset.clone());
    let current: u32 = env.storage().instance().get(&key).unwrap_or(0);
    let next = current
        .checked_add(amount)
        .ok_or(HarvestError::PriceOverflow)?;
    env.storage().instance().set(&key, &next);
    Ok(next)
}

/// Harvest every resource-bearing cell in `layout` and credit the ship's owner.
///
/// The ship is resolved from `ship_id` rather than trusted from the caller, so
/// harvested units always land with the current NFT owner. Auth is not required:
/// the layout is the caller-supplied cost, and the reward is bounded by the
/// layout's own contents — a caller can only ever pay gas to credit a ship owner
/// (who may be somebody else).
///
/// Returns [`HarvestError::ShipNotFound`] if `ship_id` does not exist and
/// [`HarvestError::EmptyHarvest`] if the layout yields nothing.
///
/// # Errors
/// See [`HarvestError`].
///
/// # Reentrancy
/// Runs under the global reentrancy guard (Issue #472): every check and state
/// effect completes while the lock is held, following checks-effects-
/// interactions, so a nested call into any guarded entry point while this one
/// is in flight is rejected with a `Reentrancy` error.
pub fn harvest_resources(
    env: &Env,
    ship_id: u64,
    layout: &NebulaLayout,
) -> Result<HarvestResult, HarvestError> {
    with_guard(env, || harvest_resources_unguarded(env, ship_id, layout))
}

/// Unguarded body of [`harvest_resources`]; callers must already hold the
/// reentrancy lock (e.g. another guarded entry point composing it).
pub(crate) fn harvest_resources_unguarded(
    env: &Env,
    ship_id: u64,
    layout: &NebulaLayout,
) -> Result<HarvestResult, HarvestError> {
    let ship = crate::ship_nft::get_ship(env, ship_id).map_err(|_| HarvestError::ShipNotFound)?;

    let mut resources: Vec<HarvestedResource> = Vec::new(env);
    let mut total_harvested: u32 = 0;

    for i in 0..layout.cells.len() {
        let Some(cell) = layout.cells.get(i) else {
            continue;
        };
        let Some(asset_id) = cell_type_to_asset(&cell.cell_type) else {
            continue;
        };
        if cell.energy == 0 {
            continue;
        }

        resources.push_back(HarvestedResource {
            asset_id: asset_id.clone(),
            amount: cell.energy,
        });
        total_harvested = total_harvested
            .checked_add(cell.energy)
            .ok_or(HarvestError::PriceOverflow)?;

        credit_resource_balance(env, &ship.owner, &asset_id, cell.energy)?;
    }

    if total_harvested == 0 {
        return Err(HarvestError::EmptyHarvest);
    }

    env.events().publish(
        (symbol_short!("harvest"), symbol_short!("done")),
        (ship_id, total_harvested),
    );

    Ok(HarvestResult {
        ship_id,
        resources,
        total_harvested,
    })
}

/// List the caller's entire balance of `resource` on the DEX.
///
/// Escrows the full current balance as `amount` and debits it from
/// `ResourceKey::ResourceBalance`, so a listed offer cannot be double-sold.
/// Cancelling via [`crate::dex_integration::cancel_listing`] releases the escrow.
///
/// `min_price` must be positive. [`HarvestError::InsufficientBalance`] is
/// returned when the caller holds nothing of `resource`.
///
/// # Errors
/// See [`HarvestError`].
///
/// # Reentrancy
/// Runs under the global reentrancy guard (Issue #472): every check and state
/// effect completes while the lock is held, following checks-effects-
/// interactions, so a nested call into any guarded entry point while this one
/// is in flight is rejected with a `Reentrancy` error.
pub fn auto_list_on_dex(
    env: &Env,
    player: &Address,
    resource: &Symbol,
    min_price: i128,
) -> Result<DexOffer, HarvestError> {
    with_guard(env, || {
        auto_list_on_dex_unguarded(env, player, resource, min_price)
    })
}

/// Unguarded body of [`auto_list_on_dex`]; callers must already hold the
/// reentrancy lock (e.g. another guarded entry point composing it).
fn auto_list_on_dex_unguarded(
    env: &Env,
    player: &Address,
    resource: &Symbol,
    min_price: i128,
) -> Result<DexOffer, HarvestError> {
    player.require_auth();

    if min_price <= 0 {
        return Err(HarvestError::InvalidPrice);
    }

    let amount = resource_balance(env, player, resource);
    if amount == 0 {
        return Err(HarvestError::InsufficientBalance);
    }

    // Escrow: clear the balance this offer is about to sell. The `0u32` suffix
    // matters — an unsuffixed `0` would infer as `i32` and write a value the
    // `u32` reads in `resource_balance` cannot convert back from.
    let balance_key = ResourceKey::ResourceBalance(player.clone(), resource.clone());
    env.storage().instance().set(&balance_key, &0u32);

    let offer_id = next_dex_offer_id(env)?;
    let offer = DexOffer {
        offer_id,
        seller: player.clone(),
        asset_id: resource.clone(),
        amount,
        min_price,
        active: true,
    };

    env.storage()
        .instance()
        .set(&ResourceKey::DexOffer(offer_id), &offer);

    env.events().publish(
        (symbol_short!("dex"), symbol_short!("listed")),
        (
            offer_id,
            player.clone(),
            resource.clone(),
            amount,
            min_price,
        ),
    );

    Ok(offer)
}

/// Read a DEX offer by ID.
pub fn get_dex_offer(env: &Env, offer_id: u64) -> Option<DexOffer> {
    env.storage()
        .instance()
        .get(&ResourceKey::DexOffer(offer_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Address, Env};

    fn make_env() -> Env {
        let env = Env::default();
        env.mock_all_auths();
        env
    }

    fn in_contract<T>(env: &Env, f: impl FnOnce() -> T) -> T {
        let contract = env.register(ResourceMinterContract, ());
        env.as_contract(&contract, f)
    }

    // ── Arithmetic safety (Issue #239) ──────────────────────────
    //
    // `mint_resource` credits balances via `current.checked_add(amount)`
    // (see above). These property tests pin down that operation's
    // overflow behavior directly, independent of the full mint flow.
    mod arithmetic_safety {
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn checked_credit_never_wraps(current in any::<u64>(), amount in any::<u64>()) {
                match current.checked_add(amount) {
                    Some(sum) => {
                        prop_assert!(sum >= current);
                        prop_assert!(sum >= amount);
                    }
                    None => {
                        // Only reports overflow when the true (unbounded) sum
                        // would actually exceed u64::MAX — never a false positive.
                        prop_assert!(current as u128 + amount as u128 > u64::MAX as u128);
                    }
                }
            }
        }

        #[test]
        fn checked_credit_detects_overflow_at_max_balance() {
            assert_eq!(u64::MAX.checked_add(1), None);
            assert_eq!((u64::MAX - 1).checked_add(1), Some(u64::MAX));
        }
    }

    #[test]
    fn test_mint_zero_amount_rejected() {
        let env = make_env();
        let caller = Address::generate(&env);
        let result = in_contract(&env, || {
            ResourceMinterContract::mint_resource(
                &env,
                caller,
                1,
                0,
                ResourceType::StellarDust,
                0,
            )
        });
        assert_eq!(result, Err(MinterError::InvalidAmount));
    }

    #[test]
    fn test_mint_rejected_while_guard_held() {
        // Simulates a callback re-entering mint_resource while an earlier
        // guarded invocation is still in flight (Issue #472).
        let env = make_env();
        let caller = Address::generate(&env);
        let contract = env.register(ResourceMinterContract, ());

        env.as_contract(&contract, || {
            crate::reentrancy_guard::acquire(&env).expect("lock should be free");
            let result = ResourceMinterContract::mint_resource(
                &env,
                caller.clone(),
                1,
                0,
                ResourceType::StellarDust,
                5,
            );
            assert_eq!(result, Err(MinterError::Reentrancy));
            crate::reentrancy_guard::release(&env);

            // The rejected call minted nothing.
            assert_eq!(balance_of(&env, &caller, &ResourceType::StellarDust), 0);
            assert_eq!(circulating_supply(&env, &ResourceType::StellarDust), 0);
        });

        // With the lock released the call proceeds past the guard to validation.
        env.as_contract(&contract, || {
            let result = ResourceMinterContract::mint_resource(
                &env,
                caller.clone(),
                1,
                0,
                ResourceType::StellarDust,
                0,
            );
            assert_eq!(result, Err(MinterError::InvalidAmount));
        });
    }

    #[test]
    fn test_rate_limit_enforced_on_minting() {
        let env = make_env();
        let caller = Address::generate(&env);
        let contract = env.register(ResourceMinterContract, ());

        // Use up the default ResourceMinting limit (10 / 60 s).
        for _ in 0..10 {
            env.as_contract(&contract, || {
                let _ = ResourceMinterContract::mint_resource(
                    &env,
                    caller.clone(),
                    1,
                    0,
                    ResourceType::StellarDust,
                    1,
                );
            });
        }
        let result = env.as_contract(&contract, || {
            let result = ResourceMinterContract::mint_resource(
                &env,
                caller,
                1,
                0,
                ResourceType::StellarDust,
                1,
            );
            result
        });
        assert_eq!(result, Err(MinterError::RateLimitExceeded));
    }

    // ── Supply-reducing primitives (Issue #281) ─────────────────
    //
    // These touch the balance and supply ledgers, so — unlike the pure
    // arithmetic tests above — they must run inside a contract invocation.
    mod supply_primitives {
        use super::*;
        use soroban_sdk::contractimpl;

        #[contract]
        struct Stub;
        #[contractimpl]
        impl Stub {}

        fn in_contract<T>(f: impl FnOnce(&Env) -> T) -> T {
            let env = make_env();
            let contract = env.register(Stub, ());
            env.as_contract(&contract, || f(&env))
        }

        #[test]
        fn credit_updates_balance_supply_and_mint_total() {
            in_contract(|env| {
                let holder = Address::generate(env);
                let rt = ResourceType::DarkMatter;

                assert_eq!(credit_balance(env, &holder, &rt, 400).unwrap(), 400);
                assert_eq!(balance_of(env, &holder, &rt), 400);
                assert_eq!(circulating_supply(env, &rt), 400);
                assert_eq!(total_minted(env, &rt), 400);
            });
        }

        #[test]
        fn debit_reduces_balance_but_not_the_mint_total() {
            in_contract(|env| {
                let holder = Address::generate(env);
                let rt = ResourceType::StellarDust;
                credit_balance(env, &holder, &rt, 100).unwrap();

                assert_eq!(debit_balance(env, &holder, &rt, 40).unwrap(), 60);
                assert_eq!(reduce_supply(env, &rt, 40).unwrap(), 60);
                assert_eq!(
                    total_minted(env, &rt),
                    100,
                    "the historical mint total must be monotonic"
                );
            });
        }

        #[test]
        fn debit_beyond_balance_is_rejected_without_wrapping() {
            in_contract(|env| {
                let holder = Address::generate(env);
                let rt = ResourceType::StellarDust;
                credit_balance(env, &holder, &rt, 10).unwrap();

                assert_eq!(
                    debit_balance(env, &holder, &rt, 11),
                    Err(MinterError::InsufficientBalance)
                );
                assert_eq!(balance_of(env, &holder, &rt), 10, "balance is unchanged");
            });
        }

        #[test]
        fn reduce_supply_beyond_supply_is_rejected() {
            in_contract(|env| {
                assert_eq!(
                    reduce_supply(env, &ResourceType::ExoticMatter, 1),
                    Err(MinterError::InsufficientBalance)
                );
            });
        }

        #[test]
        fn zero_amount_transfers_are_rejected() {
            in_contract(|env| {
                let holder = Address::generate(env);
                let rt = ResourceType::StellarDust;

                assert_eq!(
                    credit_balance(env, &holder, &rt, 0),
                    Err(MinterError::InvalidAmount)
                );
                assert_eq!(
                    debit_balance(env, &holder, &rt, 0),
                    Err(MinterError::InvalidAmount)
                );
            });
        }

        #[test]
        fn credit_detects_balance_overflow() {
            in_contract(|env| {
                let holder = Address::generate(env);
                let rt = ResourceType::StellarDust;
                credit_balance(env, &holder, &rt, u64::MAX).unwrap();

                assert_eq!(
                    credit_balance(env, &holder, &rt, 1),
                    Err(MinterError::ArithmeticOverflow)
                );
            });
        }

        #[test]
        fn balances_are_tracked_per_resource_type() {
            in_contract(|env| {
                let holder = Address::generate(env);
                credit_balance(env, &holder, &ResourceType::StellarDust, 10).unwrap();
                credit_balance(env, &holder, &ResourceType::DarkMatter, 20).unwrap();

                assert_eq!(balance_of(env, &holder, &ResourceType::StellarDust), 10);
                assert_eq!(balance_of(env, &holder, &ResourceType::DarkMatter), 20);
                assert_eq!(balance_of(env, &holder, &ResourceType::ExoticMatter), 0);
            });
        }

        #[test]
        fn move_balance_shifts_holdings_without_changing_supply() {
            in_contract(|env| {
                let from = Address::generate(env);
                let to = Address::generate(env);
                let rt = ResourceType::DarkMatter;
                credit_balance(env, &from, &rt, 500).unwrap();

                assert_eq!(move_balance(env, &from, &to, &rt, 200).unwrap(), 200);
                assert_eq!(balance_of(env, &from, &rt), 300);
                assert_eq!(balance_of(env, &to, &rt), 200);
                assert_eq!(circulating_supply(env, &rt), 500, "a move is not a mint");
                assert_eq!(total_minted(env, &rt), 500);
            });
        }

        #[test]
        fn move_balance_rejects_an_underfunded_sender() {
            in_contract(|env| {
                let from = Address::generate(env);
                let to = Address::generate(env);
                let rt = ResourceType::DarkMatter;
                credit_balance(env, &from, &rt, 10).unwrap();

                assert_eq!(
                    move_balance(env, &from, &to, &rt, 11),
                    Err(MinterError::InsufficientBalance)
                );
                assert_eq!(balance_of(env, &from, &rt), 10);
                assert_eq!(balance_of(env, &to, &rt), 0);
            });
        }

        #[test]
        fn move_balance_to_self_is_a_no_op() {
            in_contract(|env| {
                let holder = Address::generate(env);
                let rt = ResourceType::DarkMatter;
                credit_balance(env, &holder, &rt, 100).unwrap();

                assert_eq!(move_balance(env, &holder, &holder, &rt, 50).unwrap(), 100);
                assert_eq!(balance_of(env, &holder, &rt), 100);
            });
        }
    }

    // ── Harvest + DEX auto-listing ─────────────────────────────────
    mod harvest_and_dex {
        use super::*;
        use crate::nebula_explorer::{CellType, NebulaCell};
        use soroban_sdk::contractimpl;
        use soroban_sdk::testutils::{Events as _, Ledger, LedgerInfo};
        use soroban_sdk::Address as _;

        #[contract]
        struct Stub;

        #[contractimpl]
        impl Stub {}

        /// Run `f` as a single contract invocation.
        ///
        /// `mock_all_auths` rejects a second `require_auth()` for the same
        /// address within one invocation, so a test that legitimately performs
        /// several authorized contract calls must use a fresh `as_contract` block
        /// per call (via [`Contract`]) rather than nesting them here.
        fn in_contract<T>(f: impl FnOnce(&Env) -> T) -> T {
            let env = make_env();
            let contract = env.register(Stub, ());
            env.as_contract(&contract, || f(&env))
        }

        /// A registered contract whose invocations can be driven one at a time.
        struct Contract {
            env: Env,
            id: soroban_sdk::Address,
        }

        impl Contract {
            fn new() -> Self {
                let env = make_env();
                let id = env.register(Stub, ());
                Self { env, id }
            }

            fn env(&self) -> &Env {
                &self.env
            }

            /// Perform one contract invocation.
            fn invoke<T>(&self, f: impl FnOnce(&Env) -> T) -> T {
                self.env.as_contract(&self.id, || f(&self.env))
            }
        }

        /// Build a single-cell layout with the given cell type and energy.
        fn layout_with(env: &Env, cell_type: CellType, energy: u32) -> NebulaLayout {
            let mut cells = Vec::new(env);
            cells.push_back(NebulaCell {
                x: 0,
                y: 0,
                cell_type,
                energy,
            });
            NebulaLayout {
                width: 1,
                height: 1,
                cells,
                seed: soroban_sdk::BytesN::from_array(env, &[7u8; 32]),
                timestamp: 0,
                total_energy: energy,
            }
        }

        /// `Events::all()` reads back the host's ledger event log, so a ledger
        /// must exist before any event assertion.
        fn seed_ledger(env: &Env) {
            env.ledger().set(LedgerInfo {
                protocol_version: 22,
                sequence_number: 0,
                timestamp: 0,
                network_id: [0u8; 32],
                base_reserve: 10,
                min_temp_entry_ttl: 100,
                min_persistent_entry_ttl: 100,
                max_entry_ttl: 1_000,
            });
        }

        /// Register a ship owned by `owner` and return its ID.
        fn ship_for(env: &Env, owner: &Address) -> u64 {
            ship_nft::mint_ship(
                env,
                owner,
                &soroban_sdk::symbol_short!("explorer"),
                &soroban_sdk::Bytes::new(env),
            )
            .unwrap()
            .id
        }

        // ── cell_type_to_asset ───────────────────────────────────

        #[test]
        fn resource_cells_map_to_an_asset() {
            for ct in [
                CellType::StellarDust,
                CellType::Asteroid,
                CellType::GasCloud,
                CellType::DarkMatter,
                CellType::ExoticMatter,
                CellType::Wormhole,
            ] {
                assert!(
                    cell_type_to_asset(&ct).is_some(),
                    "{ct:?} should be harvestable"
                );
            }
        }

        #[test]
        fn empty_and_star_cells_map_to_no_asset() {
            assert!(cell_type_to_asset(&CellType::Empty).is_none());
            assert!(cell_type_to_asset(&CellType::Star).is_none());
        }

        // ── harvest_resources ────────────────────────────────────

        #[test]
        fn harvest_credits_the_ship_owner() {
            in_contract(|env| {
                let owner = Address::generate(env);
                let ship_id = ship_for(env, &owner);
                let layout = layout_with(env, CellType::StellarDust, 40);

                let result = harvest_resources(env, ship_id, &layout).unwrap();

                assert_eq!(result.ship_id, ship_id);
                assert_eq!(result.total_harvested, 40);
                assert_eq!(result.resources.len(), 1);
                assert_eq!(
                    resource_balance(env, &owner, &soroban_sdk::symbol_short!("dust")),
                    40
                );
            });
        }

        #[test]
        fn harvest_credits_the_current_owner_after_transfer() {
            let c = Contract::new();
            let env = c.env();
            let from = Address::generate(env);
            let to = Address::generate(env);

            let ship_id = c.invoke(|env| ship_for(env, &from));
            c.invoke(|env| ship_nft::transfer_ship(env, ship_id, &from, &to).unwrap());

            let layout = layout_with(env, CellType::DarkMatter, 12);
            c.invoke(|env| harvest_resources(env, ship_id, &layout).unwrap());

            // The new owner is credited, not the one who minted the ship.
            c.invoke(|env| {
                assert_eq!(
                    resource_balance(env, &to, &soroban_sdk::symbol_short!("dark")),
                    12
                );
                assert_eq!(
                    resource_balance(env, &from, &soroban_sdk::symbol_short!("dark")),
                    0
                );
            });
        }

        #[test]
        fn harvest_rejects_an_unknown_ship() {
            in_contract(|env| {
                let layout = layout_with(env, CellType::Asteroid, 5);
                assert_eq!(
                    harvest_resources(env, 9_999, &layout),
                    Err(HarvestError::ShipNotFound)
                );
            });
        }

        #[test]
        fn harvest_rejects_a_layout_with_no_resources() {
            in_contract(|env| {
                let owner = Address::generate(env);
                let ship_id = ship_for(env, &owner);
                let layout = layout_with(env, CellType::Empty, 100);

                assert_eq!(
                    harvest_resources(env, ship_id, &layout),
                    Err(HarvestError::EmptyHarvest)
                );
            });
        }

        #[test]
        fn harvest_ignores_zero_energy_cells() {
            in_contract(|env| {
                let owner = Address::generate(env);
                let ship_id = ship_for(env, &owner);
                let layout = layout_with(env, CellType::ExoticMatter, 0);

                assert_eq!(
                    harvest_resources(env, ship_id, &layout),
                    Err(HarvestError::EmptyHarvest)
                );
            });
        }

        #[test]
        fn harvest_accumulates_across_calls() {
            in_contract(|env| {
                let owner = Address::generate(env);
                let ship_id = ship_for(env, &owner);
                let layout = layout_with(env, CellType::GasCloud, 3);

                harvest_resources(env, ship_id, &layout).unwrap();
                harvest_resources(env, ship_id, &layout).unwrap();

                assert_eq!(
                    resource_balance(env, &owner, &soroban_sdk::symbol_short!("gas")),
                    6
                );
            });
        }

        #[test]
        fn harvest_separates_assets_by_type() {
            in_contract(|env| {
                let owner = Address::generate(env);
                let ship_id = ship_for(env, &owner);

                let mut cells = Vec::new(env);
                cells.push_back(NebulaCell {
                    x: 0,
                    y: 0,
                    cell_type: CellType::StellarDust,
                    energy: 10,
                });
                cells.push_back(NebulaCell {
                    x: 1,
                    y: 0,
                    cell_type: CellType::DarkMatter,
                    energy: 25,
                });
                // Non-resource cell must be skipped without affecting the total.
                cells.push_back(NebulaCell {
                    x: 2,
                    y: 0,
                    cell_type: CellType::Star,
                    energy: 500,
                });
                let layout = NebulaLayout {
                    width: 3,
                    height: 1,
                    cells,
                    seed: soroban_sdk::BytesN::from_array(env, &[1u8; 32]),
                    timestamp: 0,
                    total_energy: 535,
                };

                let result = harvest_resources(env, ship_id, &layout).unwrap();

                assert_eq!(result.resources.len(), 2);
                assert_eq!(result.total_harvested, 35);
                assert_eq!(
                    resource_balance(env, &owner, &soroban_sdk::symbol_short!("dust")),
                    10
                );
                assert_eq!(
                    resource_balance(env, &owner, &soroban_sdk::symbol_short!("dark")),
                    25
                );
            });
        }

        #[test]
        fn harvest_emits_an_event() {
            in_contract(|env| {
                seed_ledger(env);
                let owner = Address::generate(env);
                let ship_id = ship_for(env, &owner);
                let layout = layout_with(env, CellType::Wormhole, 8);
                let before = env.events().all().len();

                harvest_resources(env, ship_id, &layout).unwrap();

                assert_eq!(env.events().all().len(), before + 1);
            });
        }

        // ── auto_list_on_dex ─────────────────────────────────────

        #[test]
        fn auto_list_creates_an_active_offer() {
            in_contract(|env| {
                let seller = Address::generate(env);
                let asset = soroban_sdk::symbol_short!("dust");
                credit_resource_balance(env, &seller, &asset, 60).unwrap();

                let offer = auto_list_on_dex(env, &seller, &asset, 25).unwrap();

                assert_eq!(offer.offer_id, 1);
                assert_eq!(offer.asset_id, asset);
                assert_eq!(offer.amount, 60);
                assert_eq!(offer.min_price, 25);
                assert!(offer.active);
            });
        }

        #[test]
        fn auto_list_escrows_the_listed_balance() {
            in_contract(|env| {
                let seller = Address::generate(env);
                let asset = soroban_sdk::symbol_short!("dust");
                credit_resource_balance(env, &seller, &asset, 60).unwrap();

                auto_list_on_dex(env, &seller, &asset, 25).unwrap();

                // Escrowed, so it cannot also be spent on a gift or burned.
                assert_eq!(resource_balance(env, &seller, &asset), 0);
            });
        }

        #[test]
        fn auto_list_rejects_a_non_positive_price() {
            let c = Contract::new();
            let env = c.env();
            let asset = soroban_sdk::symbol_short!("dust");
            let seller = Address::generate(env);
            c.invoke(|env| credit_resource_balance(env, &seller, &asset, 10).unwrap());

            assert_eq!(
                c.invoke(|env| auto_list_on_dex(env, &seller, &asset, 0)),
                Err(HarvestError::InvalidPrice)
            );
            assert_eq!(
                c.invoke(|env| auto_list_on_dex(env, &seller, &asset, -1)),
                Err(HarvestError::InvalidPrice)
            );

            // Nothing was escrowed by a rejected listing.
            c.invoke(|env| assert_eq!(resource_balance(env, &seller, &asset), 10));
        }

        #[test]
        fn auto_list_rejects_an_empty_balance() {
            in_contract(|env| {
                let seller = Address::generate(env);
                let asset = soroban_sdk::symbol_short!("dust");

                assert_eq!(
                    auto_list_on_dex(env, &seller, &asset, 5),
                    Err(HarvestError::InsufficientBalance)
                );
            });
        }

        #[test]
        fn offer_ids_are_unique_and_monotonic() {
            in_contract(|env| {
                let a = Address::generate(env);
                let b = Address::generate(env);
                let asset = soroban_sdk::symbol_short!("dust");
                credit_resource_balance(env, &a, &asset, 10).unwrap();
                credit_resource_balance(env, &b, &asset, 10).unwrap();

                let first = auto_list_on_dex(env, &a, &asset, 1).unwrap();
                let second = auto_list_on_dex(env, &b, &asset, 1).unwrap();

                assert_eq!(first.offer_id, 1);
                assert_eq!(second.offer_id, 2);
                assert_ne!(first.offer_id, second.offer_id);
            });
        }

        #[test]
        fn get_dex_offer_round_trips() {
            in_contract(|env| {
                let seller = Address::generate(env);
                let asset = soroban_sdk::symbol_short!("dark");
                credit_resource_balance(env, &seller, &asset, 7).unwrap();

                let offer = auto_list_on_dex(env, &seller, &asset, 3).unwrap();

                assert_eq!(get_dex_offer(env, offer.offer_id), Some(offer));
                assert_eq!(get_dex_offer(env, 4_242), None);
            });
        }

        #[test]
        fn auto_list_emits_an_event() {
            in_contract(|env| {
                seed_ledger(env);
                let seller = Address::generate(env);
                let asset = soroban_sdk::symbol_short!("dust");
                credit_resource_balance(env, &seller, &asset, 4).unwrap();
                let before = env.events().all().len();

                auto_list_on_dex(env, &seller, &asset, 2).unwrap();

                assert_eq!(env.events().all().len(), before + 1);
            });
        }

        // ── Reentrancy protection (Issue #472) ──────────────────

        #[test]
        fn harvest_is_rejected_while_guard_held() {
            let c = Contract::new();
            let env = c.env();
            let owner = Address::generate(env);
            let ship_id = c.invoke(|env| ship_for(env, &owner));
            let layout = layout_with(env, CellType::StellarDust, 40);
            let dust = soroban_sdk::symbol_short!("dust");

            c.invoke(|env| {
                crate::reentrancy_guard::acquire(env).expect("lock should be free");
                assert_eq!(
                    harvest_resources(env, ship_id, &layout),
                    Err(HarvestError::Reentrancy)
                );
                crate::reentrancy_guard::release(env);
                // The rejected harvest credited nothing.
                assert_eq!(resource_balance(env, &owner, &dust), 0);
            });

            c.invoke(|env| {
                harvest_resources(env, ship_id, &layout).unwrap();
                assert_eq!(resource_balance(env, &owner, &dust), 40);
                assert!(!crate::reentrancy_guard::is_locked(env));
            });
        }

        #[test]
        fn auto_list_is_rejected_while_guard_held() {
            in_contract(|env| {
                let seller = Address::generate(env);
                let asset = soroban_sdk::symbol_short!("dust");
                credit_resource_balance(env, &seller, &asset, 60).unwrap();

                crate::reentrancy_guard::acquire(env).expect("lock should be free");
                assert_eq!(
                    auto_list_on_dex(env, &seller, &asset, 25),
                    Err(HarvestError::Reentrancy)
                );
                crate::reentrancy_guard::release(env);

                // No escrow was taken and no offer was created.
                assert_eq!(resource_balance(env, &seller, &asset), 60);
                assert_eq!(get_dex_offer(env, 1), None);

                let offer = auto_list_on_dex(env, &seller, &asset, 25).unwrap();
                assert_eq!(offer.amount, 60);
            });
        }

        #[test]
        fn harvest_and_list_rejects_reentry_but_composes_its_own_harvest() {
            let c = Contract::new();
            let env = c.env();
            let owner = Address::generate(env);
            let ship_id = c.invoke(|env| ship_for(env, &owner));
            let layout = layout_with(env, CellType::StellarDust, 30);
            let asset = soroban_sdk::symbol_short!("dust");

            c.invoke(|env| {
                crate::reentrancy_guard::acquire(env).expect("lock should be free");
                assert_eq!(
                    crate::dex_integration::harvest_and_list(
                        env, &owner, ship_id, &layout, &asset, 5
                    )
                    .map(|(_, offer)| offer.offer_id),
                    Err(HarvestError::Reentrancy)
                );
                crate::reentrancy_guard::release(env);
                assert_eq!(resource_balance(env, &owner, &asset), 0);
            });

            // Unlocked, the harvest leg runs inside the same guard without
            // tripping it, and the lock is released afterwards.
            let (harvest, offer) = c.invoke(|env| {
                crate::dex_integration::harvest_and_list(env, &owner, ship_id, &layout, &asset, 5)
                    .unwrap()
            });
            assert_eq!(harvest.total_harvested, 30);
            assert_eq!(offer.amount, 30);
            c.invoke(|env| assert!(!crate::reentrancy_guard::is_locked(env)));
        }

        #[test]
        fn cancel_listing_is_rejected_while_guard_held() {
            let c = Contract::new();
            let env = c.env();
            let seller = Address::generate(env);
            let asset = soroban_sdk::symbol_short!("dust");
            let offer = c.invoke(|env| {
                credit_resource_balance(env, &seller, &asset, 9).unwrap();
                auto_list_on_dex(env, &seller, &asset, 4).unwrap()
            });

            c.invoke(|env| {
                crate::reentrancy_guard::acquire(env).expect("lock should be free");
                assert_eq!(
                    crate::dex_integration::cancel_listing(env, &seller, offer.offer_id),
                    Err(HarvestError::Reentrancy)
                );
                crate::reentrancy_guard::release(env);
                // Offer is still live and the escrow was not refunded.
                assert!(get_dex_offer(env, offer.offer_id).unwrap().active);
                assert_eq!(resource_balance(env, &seller, &asset), 0);
            });

            c.invoke(|env| {
                crate::dex_integration::cancel_listing(env, &seller, offer.offer_id).unwrap();
                assert_eq!(resource_balance(env, &seller, &asset), 9);
            });
        }

        // ── credit overflow (Issue #239) ────────────────────────

        #[test]
        fn crediting_past_u32_max_overflows_instead_of_wrapping() {
            in_contract(|env| {
                let owner = Address::generate(env);
                let asset = soroban_sdk::symbol_short!("dust");
                credit_resource_balance(env, &owner, &asset, u32::MAX).unwrap();

                assert_eq!(
                    credit_resource_balance(env, &owner, &asset, 1),
                    Err(HarvestError::PriceOverflow)
                );
                // Balance must be untouched after the rejected credit.
                assert_eq!(resource_balance(env, &owner, &asset), u32::MAX);
            });
        }
    }
}

#[cfg(test)]
mod packed_record_tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn packed_resource_record_round_trips() {
        let env = Env::default();
        let rec = ResourceRecord {
            owner: Address::generate(&env),
            resource_type: ResourceType::DarkMatter,
            amount: u64::MAX,
            minted_at: 1_700_000_000,
        };
        let packed = PackedResourceRecord::from(&rec);
        assert_eq!(packed.packed, (1_700_000_000u128 << 64) | u64::MAX as u128);
        assert_eq!(ResourceRecord::from(&packed), rec);
    }
}
