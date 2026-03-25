use crate::ship_nft::{DataKey as ShipDataKey, ShipNft};
use crate::{CellType, NebulaLayout};
use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env, Symbol, Vec};

pub type AssetId = Symbol;

const ASSET_STELLAR_DUST: Symbol = symbol_short!("dust");
const ASSET_ASTEROID_ORE: Symbol = symbol_short!("ore");
const ASSET_GAS_UNITS: Symbol = symbol_short!("gas");
const ASSET_DARK_MATTER: Symbol = symbol_short!("dark");
const ASSET_EXOTIC_MATTER: Symbol = symbol_short!("exotic");
const ASSET_WORMHOLE_CORE: Symbol = symbol_short!("worm");

#[derive(Clone)]
#[contracttype]
pub enum ResourceKey {
    ResourceCounter,
    HarvestCounter,
    DexOfferCounter,
    ResourceBalance(Address, AssetId),
    DexOffer(u64),
}

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum HarvestError {
    ShipNotFound = 1,
    EmptyHarvest = 2,
    InvalidPrice = 3,
    AssetNotHarvested = 4,
    PriceOverflow = 5,
    DexFailure = 6,
}

/// Resource data structure for in-game tradeable resources.
#[derive(Clone)]
#[contracttype]
pub struct Resource {
    pub id: u64,
    pub owner: Address,
    pub resource_type: u32,
    pub quantity: u32,
}

/// A single harvested resource entry.
#[derive(Clone)]
#[contracttype]
pub struct HarvestedResource {
    pub asset_id: AssetId,
    pub amount: u32,
}

/// Result of a harvest operation.
#[derive(Clone)]
#[contracttype]
pub struct HarvestResult {
    pub ship_id: u64,
    pub resources: Vec<HarvestedResource>,
    pub total_harvested: u32,
}

/// DEX offer for auto-listing harvested resources.
#[derive(Clone)]
#[contracttype]
pub struct DexOffer {
    pub offer_id: u64,
    pub asset_id: AssetId,
    pub amount: u32,
    pub min_price: i128,
    pub active: bool,
}

/// Map a CellType to its corresponding asset symbol.
fn cell_type_to_asset(cell_type: &CellType) -> Option<AssetId> {
    match cell_type {
        CellType::StellarDust => Some(ASSET_STELLAR_DUST),
        CellType::Asteroid => Some(ASSET_ASTEROID_ORE),
        CellType::GasCloud => Some(ASSET_GAS_UNITS),
        CellType::DarkMatter => Some(ASSET_DARK_MATTER),
        CellType::ExoticMatter => Some(ASSET_EXOTIC_MATTER),
        CellType::Wormhole => Some(ASSET_WORMHOLE_CORE),
        _ => None,
    }
}

#[allow(dead_code)]
fn next_harvest_id(env: &Env) -> u64 {
    let current: u64 = env
        .storage()
        .instance()
        .get(&ResourceKey::HarvestCounter)
        .unwrap_or(0);
    let next = current + 1;
    env.storage()
        .instance()
        .set(&ResourceKey::HarvestCounter, &next);
    next
}

fn next_dex_offer_id(env: &Env) -> u64 {
    let current: u64 = env
        .storage()
        .instance()
        .get(&ResourceKey::DexOfferCounter)
        .unwrap_or(0);
    let next = current + 1;
    env.storage()
        .instance()
        .set(&ResourceKey::DexOfferCounter, &next);
    next
}

/// Gas-optimized single-invocation harvest that scans a layout and
/// collects resources from non-empty cells.
pub fn harvest_resources(
    env: &Env,
    ship_id: u64,
    layout: &NebulaLayout,
) -> Result<HarvestResult, HarvestError> {
    // Verify the ship exists
    let _ship: ShipNft = env
        .storage()
        .persistent()
        .get(&ShipDataKey::Ship(ship_id))
        .ok_or(HarvestError::ShipNotFound)?;

    let mut resources = Vec::new(env);
    let mut total_harvested: u32 = 0;

    // Scan layout cells and harvest resources
    for i in 0..layout.cells.len() {
        if let Some(cell) = layout.cells.get(i) {
            if let Some(asset_id) = cell_type_to_asset(&cell.cell_type) {
                let amount = cell.energy;
                if amount > 0 {
                    resources.push_back(HarvestedResource {
                        asset_id: asset_id.clone(),
                        amount,
                    });
                    total_harvested += amount;

                    // Update balance
                    let key = ResourceKey::ResourceBalance(_ship.owner.clone(), asset_id.clone());
                    let balance: u32 = env.storage().instance().get(&key).unwrap_or(0);
                    env.storage().instance().set(&key, &(balance + amount));
                }
            }
        }
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

/// Create an AMM-listing hook for a harvested resource.
pub fn auto_list_on_dex(
    env: &Env,
    resource: &AssetId,
    min_price: i128,
) -> Result<DexOffer, HarvestError> {
    if min_price <= 0 {
        return Err(HarvestError::InvalidPrice);
    }

    let offer_id = next_dex_offer_id(env);
    let offer = DexOffer {
        offer_id,
        asset_id: resource.clone(),
        amount: 0,
        min_price,
        active: true,
    };

    env.storage()
        .instance()
        .set(&ResourceKey::DexOffer(offer_id), &offer);

    env.events().publish(
        (symbol_short!("dex"), symbol_short!("listed")),
        (offer_id, resource.clone(), min_price),
    );

    Ok(offer)
}
