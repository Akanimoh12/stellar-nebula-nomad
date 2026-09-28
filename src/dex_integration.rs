//! Decentralized-exchange harvest and swap integration.
//!
//! The harvest and offer types live in [`crate::resource_minter`] — this module
//! owns the *composition* paths (harvest-then-list, and cancellation) on top of
//! them, and is re-exported here so callers can keep importing a single module.

use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env, Symbol};

use crate::reentrancy_guard::with_guard;
use crate::resource_minter::{
    get_dex_offer, harvest_resources_unguarded, next_dex_offer_id, ResourceKey,
};

// Re-exported so callers can depend on this module alone.
pub use crate::resource_minter::{DexOffer, HarvestError, HarvestResult};

/// Cap on listings a single player may create, to bound offer-spam.
const MAX_LISTINGS_PER_SESSION: u32 = 5;

#[contracttype]
#[derive(Clone)]
pub enum DexKey {
    /// Number of listings created by a player.
    SessionListings(Address),
}

/// Harvest resources from a layout and immediately list one asset on the DEX.
///
/// Combines [`crate::resource_minter::harvest_resources`] with DEX offer
/// creation in a single call: the caller avoids paying for two transactions, and the listed amount is
/// exactly what the harvest yielded (never more, so an offer can never be
/// oversold against the seller's balance).
///
/// Limited to [`MAX_LISTINGS_PER_SESSION`] listings per player.
///
/// # Errors
/// - [`HarvestError::InvalidPrice`] if `min_price <= 0`.
/// - [`HarvestError::DexFailure`] if the player already hit the listing cap.
/// - [`HarvestError::AssetNotHarvested`] if `resource` was not in the harvest.
/// - plus any error from [`crate::resource_minter::harvest_resources`].
///
/// # Reentrancy
/// Runs under the global reentrancy guard (Issue #472): every check and state
/// effect completes while the lock is held, following checks-effects-
/// interactions, so a nested call into any guarded entry point while this one
/// is in flight is rejected with a `Reentrancy` error.
pub fn harvest_and_list(
    env: &Env,
    player: &Address,
    ship_id: u64,
    layout: &crate::nebula_explorer::NebulaLayout,
    resource: &Symbol,
    min_price: i128,
) -> Result<(HarvestResult, DexOffer), HarvestError> {
    with_guard(env, || {
        harvest_and_list_unguarded(env, player, ship_id, layout, resource, min_price)
    })
}

/// Unguarded body of [`harvest_and_list`]; callers must already hold the
/// reentrancy lock (e.g. another guarded entry point composing it).
fn harvest_and_list_unguarded(
    env: &Env,
    player: &Address,
    ship_id: u64,
    layout: &crate::nebula_explorer::NebulaLayout,
    resource: &Symbol,
    min_price: i128,
) -> Result<(HarvestResult, DexOffer), HarvestError> {
    player.require_auth();

    if min_price <= 0 {
        return Err(HarvestError::InvalidPrice);
    }

    let session_key = DexKey::SessionListings(player.clone());
    let current_listings: u32 = env.storage().instance().get(&session_key).unwrap_or(0);
    if current_listings >= MAX_LISTINGS_PER_SESSION {
        return Err(HarvestError::DexFailure);
    }

    // Composes the unguarded harvest: this function already holds the lock,
    // and re-acquiring it would reject our own nested call.
    let harvest_result = harvest_resources_unguarded(env, ship_id, layout)?;

    let mut listed_amount: u32 = 0;
    for i in 0..harvest_result.resources.len() {
        if let Some(hr) = harvest_result.resources.get(i) {
            if hr.asset_id == *resource {
                listed_amount = listed_amount
                    .checked_add(hr.amount)
                    .ok_or(HarvestError::PriceOverflow)?;
            }
        }
    }

    if listed_amount == 0 {
        return Err(HarvestError::AssetNotHarvested);
    }

    // The harvest already credited `player`, so escrow exactly the portion being
    // sold and leave the remainder liquid.
    let balance_key = ResourceKey::ResourceBalance(player.clone(), resource.clone());
    let balance: u32 = env.storage().instance().get(&balance_key).unwrap_or(0);
    if balance < listed_amount {
        return Err(HarvestError::InsufficientBalance);
    }
    env.storage()
        .instance()
        .set(&balance_key, &(balance - listed_amount));

    let offer_id = next_dex_offer_id(env)?;
    let offer = DexOffer {
        offer_id,
        seller: player.clone(),
        asset_id: resource.clone(),
        amount: listed_amount,
        min_price,
        active: true,
    };
    env.storage()
        .instance()
        .set(&ResourceKey::DexOffer(offer_id), &offer);

    env.storage()
        .instance()
        .set(&session_key, &(current_listings + 1));

    env.events().publish(
        (symbol_short!("dex"), symbol_short!("listed")),
        (
            offer_id,
            player.clone(),
            resource.clone(),
            listed_amount,
            min_price,
        ),
    );

    Ok((harvest_result, offer))
}

/// Cancel an active DEX listing. Only the offer's escrow holder may cancel.
///
/// Refunds the escrowed `amount` back to the seller. Cancelling an already
/// cancelled, unknown, or someone else's offer returns
/// [`HarvestError::DexFailure`].
///
/// Without the `seller` check below this would be a fund-theft bug: the escrow
/// refund is credited to `caller`, so any address could cancel any live offer
/// and collect the seller's escrowed units.
///
/// # Reentrancy
/// Runs under the global reentrancy guard (Issue #472): every check and state
/// effect completes while the lock is held, following checks-effects-
/// interactions, so a nested call into any guarded entry point while this one
/// is in flight is rejected with a `Reentrancy` error.
pub fn cancel_listing(env: &Env, owner: &Address, offer_id: u64) -> Result<DexOffer, HarvestError> {
    with_guard(env, || cancel_listing_unguarded(env, owner, offer_id))
}

/// Unguarded body of [`cancel_listing`]; callers must already hold the
/// reentrancy lock (e.g. another guarded entry point composing it).
fn cancel_listing_unguarded(
    env: &Env,
    owner: &Address,
    offer_id: u64,
) -> Result<DexOffer, HarvestError> {
    owner.require_auth();

    let mut offer: DexOffer = get_dex_offer(env, offer_id).ok_or(HarvestError::DexFailure)?;

    if !offer.active || offer.seller != *owner {
        return Err(HarvestError::DexFailure);
    }

    offer.active = false;
    env.storage()
        .instance()
        .set(&ResourceKey::DexOffer(offer_id), &offer);

    // Release the escrow.
    let balance_key = ResourceKey::ResourceBalance(owner.clone(), offer.asset_id.clone());
    let balance: u32 = env.storage().instance().get(&balance_key).unwrap_or(0);
    let refunded = balance
        .checked_add(offer.amount)
        .ok_or(HarvestError::PriceOverflow)?;
    env.storage().instance().set(&balance_key, &refunded);

    env.events().publish(
        (symbol_short!("dex"), symbol_short!("canceld")),
        (offer_id, owner.clone()),
    );

    Ok(offer)
}

/// Read a DEX offer by ID.
pub fn get_offer(env: &Env, offer_id: u64) -> Option<DexOffer> {
    get_dex_offer(env, offer_id)
}

// ── Dynamic pricing (Issue #452) ──────────────────────────────────────────────

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum DynamicListError {
    /// The harvest or the listing leg failed. The underlying
    /// [`HarvestError`] code is published in the `dlist` event so it stays
    /// diagnosable on-chain.
    HarvestFailed = 1,
    /// No dynamic price exists yet for this resource — nothing has been
    /// observed, so there is nothing to list against.
    PriceUnavailable = 2,
    /// The dynamic price resolved to a non-positive value.
    InvalidPrice = 3,
}

impl From<HarvestError> for DynamicListError {
    fn from(_: HarvestError) -> Self {
        DynamicListError::HarvestFailed
    }
}

impl From<crate::dynamic_pricing::PricingError> for DynamicListError {
    fn from(err: crate::dynamic_pricing::PricingError) -> Self {
        match err {
            crate::dynamic_pricing::PricingError::InvalidPrice => DynamicListError::InvalidPrice,
            _ => DynamicListError::PriceUnavailable,
        }
    }
}

impl crate::error_standard::StandardContractError for DynamicListError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::HarvestFailed => (ErrorKind::Conflict, false),
            Self::PriceUnavailable => (ErrorKind::NotFound, true),
            Self::InvalidPrice => (ErrorKind::Validation, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "dex_integration",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

/// Harvest a resource and list it at the current dynamic market price.
///
/// Same escrow and listing-cap rules as [`harvest_and_list`]; the only
/// difference is that `min_price` comes from
/// [`crate::dynamic_pricing::listing_price`] — the EMA-smoothed, volatility-
/// clamped price — instead of a number the caller types in. That is the point:
/// a hand-typed price is how a listing gets stuck far below market after a
/// spike, and a spammed price is how someone drains the book.
///
/// A player who wants to beat the market can still use [`harvest_and_list`]
/// with an explicit price; this path is the default.
///
/// # Errors
/// - [`DynamicListError::PriceUnavailable`] if the resource has no observed
///   price yet.
/// - [`DynamicListError::HarvestFailed`] for anything [`harvest_and_list`]
///   would reject.
pub fn list_at_market(
    env: &Env,
    player: &Address,
    ship_id: u64,
    layout: &crate::nebula_explorer::NebulaLayout,
    resource: &Symbol,
) -> Result<(HarvestResult, DexOffer, i128), DynamicListError> {
    let price = crate::dynamic_pricing::listing_price(env, resource.clone())?;
    if price <= 0 {
        return Err(DynamicListError::InvalidPrice);
    }

    let result = harvest_and_list(env, player, ship_id, layout, resource, price)
        .map_err(|err| {
            env.events().publish(
                (symbol_short!("dlist"), symbol_short!("failed")),
                (resource.clone(), err as u32),
            );
            DynamicListError::HarvestFailed
        })?;

    Ok((result.0, result.1, price))
}

