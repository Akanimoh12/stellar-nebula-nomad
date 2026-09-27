#![cfg(test)]

use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
use soroban_sdk::{symbol_short, Address, Bytes, BytesN, Env, Symbol, Vec};
use stellar_nebula_nomad::{
    CellType, DynamicListError, NebulaLayout, NebulaNomadContract, NebulaNomadContractClient,
    PriceState, PricingConfig, PricingError, DEFAULT_MAX_DEVIATION_BPS,
    DEFAULT_MIN_COOLDOWN_SECS, DEFAULT_SMOOTHING_BPS, MAX_HISTORY_ENTRIES,
};

const DUST: Symbol = symbol_short!("dust");
const DARK: Symbol = symbol_short!("dark");
const EXOTIC: Symbol = symbol_short!("exotic");

/// Every asset id `harvest_resources` can credit, so a test can seed a price
/// for all of them and then find out which one the layout actually holds.
const HARVEST_ASSETS: [Symbol; 3] = [DUST, DARK, EXOTIC];

fn asset_for_cell(cell: CellType) -> Option<Symbol> {
    match cell {
        CellType::StellarDust => Some(DUST),
        CellType::DarkMatter => Some(DARK),
        CellType::ExoticMatter => Some(EXOTIC),
        _ => None,
    }
}

fn setup() -> (Env, NebulaNomadContractClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set(LedgerInfo {
        protocol_version: 22,
        sequence_number: 100,
        timestamp: 1_700_000_000,
        network_id: [0u8; 32],
        base_reserve: 10,
        min_temp_entry_ttl: 100,
        min_persistent_entry_ttl: 1000,
        max_entry_ttl: 10_000,
    });
    let contract_id = env.register(NebulaNomadContract, ());
    let client = NebulaNomadContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    (env, client, admin)
}

fn tick(env: &Env, secs: u64) {
    env.ledger().with_mut(|li| li.timestamp += secs);
}

/// Initialise the pricing engine with the shipped defaults.
fn init_pricing(client: &NebulaNomadContractClient, admin: &Address) {
    client.init_pricing_config(admin, &PricingConfig::default_issue_452());
}

/// Seed one observed price so the resource has an average to publish.
fn seed_price(
    client: &NebulaNomadContractClient,
    admin: &Address,
    asset: Symbol,
    price: i128,
) -> PriceState {
    client.observe_price(admin, &asset, &price)
}

/// Seed a price for all three harvest assets, so any layout is covered.
fn seed_all_prices(
    client: &NebulaNomadContractClient,
    admin: &Address,
    price: i128,
) {
    for asset in HARVEST_ASSETS {
        seed_price(client, admin, asset, price);
    }
}

fn mint_and_layout(
    env: &Env,
    client: &NebulaNomadContractClient,
    player: &Address,
) -> (u64, NebulaLayout) {
    let metadata = Bytes::from_slice(env, &[0u8; 4]);
    let ship = client.mint_ship(player, &symbol_short!("explorer"), &metadata);
    let seed = BytesN::from_array(env, &[42u8; 32]);
    let layout = client.generate_nebula_layout(&seed, player);
    (ship.id, layout)
}

/// The first harvestable asset actually present in a layout.
fn first_harvestable(layout: &NebulaLayout) -> Option<Symbol> {
    let cells: &Vec<stellar_nebula_nomad::NebulaCell> = &layout.cells;
    for i in 0..cells.len() {
        if let Some(asset) = asset_for_cell(cells.get(i).unwrap().cell_type.clone()) {
            return Some(asset);
        }
    }
    None
}

// ─── Configuration ────────────────────────────────────────────────────────────

#[test]
fn test_pricing_config_round_trips() {
    let (_env, client, admin) = setup();
    let config = PricingConfig::default_issue_452();
    client.init_pricing_config(&admin, &config);
    assert_eq!(client.get_pricing_config(), config);
}

#[test]
fn test_default_config_is_readable_before_initialization() {
    let (_env, client, _admin) = setup();
    // Reads must not require a config: the defaults apply immediately.
    assert_eq!(client.get_pricing_config(), PricingConfig::default_issue_452());
}

#[test]
fn test_init_pricing_config_rejects_double_init() {
    let (_env, client, admin) = setup();
    init_pricing(&client, &admin);
    let err = client
        .try_init_pricing_config(&admin, &PricingConfig::default_issue_452())
        .unwrap_err();
    assert_eq!(err, Ok(PricingError::InvalidConfig));
}

#[test]
fn test_pricing_requires_initialization() {
    let (_env, client, admin) = setup();
    let err = client.try_observe_price(&admin, &DUST, &100i128).unwrap_err();
    assert_eq!(err, Ok(PricingError::NotInitialized));
}

#[test]
fn test_set_pricing_config_rejects_non_admin() {
    let (_env, client, admin) = setup();
    let impostor = Address::generate(&_env);
    init_pricing(&client, &admin);
    let err = client
        .try_set_pricing_config(&impostor, &PricingConfig::default_issue_452())
        .unwrap_err();
    assert_eq!(err, Ok(PricingError::Unauthorized));
}

#[test]
fn test_observe_price_rejects_non_admin_source() {
    let (_env, client, admin) = setup();
    let impostor = Address::generate(&_env);
    init_pricing(&client, &admin);
    let err = client.try_observe_price(&impostor, &DUST, &100i128).unwrap_err();
    assert_eq!(err, Ok(PricingError::Unauthorized));
}

#[test]
fn test_set_pricing_config_rejects_implausible_values() {
    let (_env, client, admin) = setup();
    init_pricing(&client, &admin);

    let bad = PricingConfig {
        smoothing_bps: 0,
        ..PricingConfig::default_issue_452()
    };
    let err = client.try_set_pricing_config(&admin, &bad).unwrap_err();
    assert_eq!(err, Ok(PricingError::InvalidConfig));
}

#[test]
fn test_observe_price_rejects_non_positive_prices() {
    let (_env, client, admin) = setup();
    init_pricing(&client, &admin);
    let err = client.try_observe_price(&admin, &DUST, &0i128).unwrap_err();
    assert_eq!(err, Ok(PricingError::InvalidPrice));
}

// ─── Smoothing and volatility limits ──────────────────────────────────────────

#[test]
fn test_a_single_print_seeds_the_average() {
    let (_env, client, admin) = setup();
    init_pricing(&client, &admin);
    let state = seed_price(&client, &admin, DUST, 250);
    assert_eq!(state.sma, 250);
    assert_eq!(state.observation_count, 1);
}

#[test]
fn test_the_published_price_lags_the_raw_feed() {
    let (env, client, admin) = setup();
    init_pricing(&client, &admin);
    seed_price(&client, &admin, DUST, 100);

    tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
    // +30% is inside the 50% deviation cap, so it is accepted.
    client.observe_price(&admin, &DUST, &130i128);

    let price = client.get_dynamic_price(&DUST);
    // The feed says 130; the economy uses 106, because only 20% of the gap is
    // taken per observation.
    assert_eq!(price.sma, 106);
    assert_eq!(price.published, 106);
    assert!(price.published < 130, "published price must lag the raw feed");
}

#[test]
fn test_a_wild_print_cannot_move_the_published_price() {
    let (env, client, admin) = setup();
    init_pricing(&client, &admin);
    seed_price(&client, &admin, DUST, 100);

    tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
    let state = client.observe_price(&admin, &DUST, &100_000i128);

    // The print is refused, not applied — and the refusal is recorded.
    assert!(state.rejected_last);
    assert_eq!(state.sma, 100);

    let price = client.get_dynamic_price(&DUST);
    assert_eq!(price.sma, 100);
    assert_eq!(price.published, 100);

    // The rejection is durable, which is why it is not reported as an error:
    // a failed invocation would roll its own storage writes back.
    let stored = client.get_price_state(&DUST);
    assert_eq!(stored.rejected_prints, 1);
    assert_eq!(stored.observation_count, 1);
}

#[test]
fn test_a_rejected_print_does_not_become_a_history_point() {
    let (env, client, admin) = setup();
    init_pricing(&client, &admin);
    seed_price(&client, &admin, DUST, 100);
    // Seeding is itself an observation, so it is on record.
    assert_eq!(client.get_pricing_history(&DUST).len(), 1);

    tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
    client.observe_price(&admin, &DUST, &100_000i128);
    assert_eq!(client.get_pricing_history(&DUST).len(), 1);

    tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
    client.observe_price(&admin, &DUST, &110i128);
    assert_eq!(client.get_pricing_history(&DUST).len(), 2);
}

#[test]
fn test_repeated_manipulation_attempts_are_all_rejected() {
    let (env, client, admin) = setup();
    init_pricing(&client, &admin);
    seed_price(&client, &admin, DUST, 100);

    // Someone trying every plausible lie, in both directions, over time.
    for print in [1_000_000i128, 0, 1, 60, 400, 99] {
        tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
        let _ = client.try_observe_price(&admin, &DUST, &print);
    }

    let price = client.get_dynamic_price(&DUST);
    // 60 is 40% below 100, inside the cap, so that one was folded in; the rest
    // were not. The published price never left a narrow neighbourhood of 100.
    assert!(
        price.published >= 90 && price.published <= 110,
        "published price drifted to {} under manipulation",
        price.published
    );
    let state = client.get_price_state(&DUST);
    // 0 and the negative-adjacent prints are invalid outright, so they never
    // reach the deviation check; the rest are out of band.
    assert!(state.rejected_prints >= 3, "most prints should be rejected");
}

#[test]
fn test_the_cooldown_throttles_print_frequency() {
    let (env, client, admin) = setup();
    init_pricing(&client, &admin);
    seed_price(&client, &admin, DUST, 100);

    // Immediately after the seed print, the cooldown has not elapsed.
    let err = client.try_observe_price(&admin, &DUST, &110i128).unwrap_err();
    assert_eq!(err, Ok(PricingError::CooldownActive));

    tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
    client.observe_price(&admin, &DUST, &110i128);
}

#[test]
fn test_deviation_cap_is_configurable() {
    let (env, client, admin) = setup();
    client.init_pricing_config(
        &admin,
        &PricingConfig {
            max_deviation_bps: 1_000,
            ..PricingConfig::default_issue_452()
        },
    );
    seed_price(&client, &admin, DUST, 100);

    tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
    // +30%: fine under the 50% default, rejected under the tightened 10% cap.
    let state = client.observe_price(&admin, &DUST, &130i128);
    assert!(state.rejected_last);
    assert_eq!(state.sma, 100, "a rejected print must not move the average");
    assert_eq!(DEFAULT_MAX_DEVIATION_BPS, 5_000);
}

#[test]
fn test_smoothing_weight_is_configurable() {
    let (env, client, admin) = setup();
    // A 50% alpha halves the gap in one step instead of taking five.
    client.init_pricing_config(
        &admin,
        &PricingConfig {
            smoothing_bps: 5_000,
            ..PricingConfig::default_issue_452()
        },
    );
    seed_price(&client, &admin, DUST, 100);

    tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
    client.observe_price(&admin, &DUST, &120i128);
    assert_eq!(client.get_dynamic_price(&DUST).sma, 110);
    assert_eq!(DEFAULT_SMOOTHING_BPS, 2_000);
}

#[test]
fn test_the_published_price_is_clamped_to_the_volatility_band() {
    let (env, client, admin) = setup();
    // A 5% band: tighter than the 25% supply/demand nudge can ask for.
    client.init_pricing_config(
        &admin,
        &PricingConfig {
            base_volatility_band_bps: 500,
            max_volatility_band_bps: 500,
            supply_demand_adj_bps: 2_500,
            ..PricingConfig::default_issue_452()
        },
    );
    seed_price(&client, &admin, DUST, 100);

    // Severe undersupply: ratio 5000 => +40000bps, clamped to the +2500 nudge.
    client.observe_supply_demand(&admin, &DUST, &5_000i128, &1_000i128);

    let price = client.get_dynamic_price(&DUST);
    assert_eq!(price.supply_demand_bps, 2_500);
    // The nudge alone would ask for 125; the 5% band holds it at 105.
    assert_eq!(price.published, 105);
}

#[test]
fn test_the_band_widens_with_realized_volatility() {
    let (env, client, admin) = setup();
    client.init_pricing_config(
        &admin,
        &PricingConfig {
            min_cooldown_secs: 0,
            ..PricingConfig::default_issue_452()
        },
    );
    seed_price(&client, &admin, DUST, 100);

    // A quiet market.
    let quiet_band = client.get_dynamic_price(&DUST).band_bps;

    // Now a genuinely wobbly one: alternate the largest prints the 50% cap
    // allows, for long enough that the volatility EWMA climbs.
    for i in 0..16 {
        tick(&env, 1);
        let print = if i % 2 == 0 { 150 } else { 50 };
        let _ = client.try_observe_price(&admin, &DUST, &print);
    }

    let volatile = client.get_dynamic_price(&DUST);
    assert!(volatile.volatility_bps > 0, "volatility should register");
    assert!(
        volatile.band_bps > quiet_band,
        "band {} should widen beyond the quiet band {}",
        volatile.band_bps,
        quiet_band
    );
    // Never past the hard ceiling.
    assert!(volatile.band_bps <= 5_000);
}

#[test]
fn test_supply_and_demand_shift_the_price_in_the_right_direction() {
    let (_env, client, admin) = setup();
    init_pricing(&client, &admin);
    seed_price(&client, &admin, DUST, 100);

    client.observe_supply_demand(&admin, &DUST, &2_000i128, &1_000i128);
    let scarce = client.get_dynamic_price(&DUST);
    assert_eq!(scarce.supply_demand_bps, 2_500);
    assert_eq!(scarce.published, 125);

    client.observe_supply_demand(&admin, &DUST, &500i128, &1_000i128);
    let plentiful = client.get_dynamic_price(&DUST);
    assert_eq!(plentiful.supply_demand_bps, -2_500);
    assert_eq!(plentiful.published, 75);
}

#[test]
fn test_supply_demand_rejects_an_unpriced_resource() {
    let (_env, client, admin) = setup();
    init_pricing(&client, &admin);
    let err = client
        .try_observe_supply_demand(&admin, &DUST, &1_000i128, &1_000i128)
        .unwrap_err();
    assert_eq!(err, Ok(PricingError::ResourceNotFound));
}

// ─── Reads ────────────────────────────────────────────────────────────────────

#[test]
fn test_reads_on_an_unpriced_resource_report_not_found() {
    let (_env, client, _admin) = setup();
    assert_eq!(
        client.try_get_dynamic_price(&DUST).unwrap_err(),
        Ok(PricingError::ResourceNotFound)
    );
    assert_eq!(
        client.try_get_price_state(&DUST).unwrap_err(),
        Ok(PricingError::ResourceNotFound)
    );
    assert_eq!(client.get_pricing_history(&DUST).len(), 0);
}

#[test]
fn test_history_is_bounded() {
    let (env, client, admin) = setup();
    client.init_pricing_config(
        &admin,
        &PricingConfig {
            min_cooldown_secs: 0,
            ..PricingConfig::default_issue_452()
        },
    );
    seed_price(&client, &admin, DUST, 100);

    for _ in 0..(MAX_HISTORY_ENTRIES + 10) {
        tick(&env, 1);
        client.observe_price(&admin, &DUST, &101i128);
    }

    // The rolling window is capped, so a long-lived contract cannot grow an
    // unbounded history entry.
    assert_eq!(client.get_pricing_history(&DUST).len(), MAX_HISTORY_ENTRIES);
}

#[test]
fn test_state_survives_across_reads() {
    let (env, client, admin) = setup();
    init_pricing(&client, &admin);
    seed_price(&client, &admin, DUST, 100);
    tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
    client.observe_price(&admin, &DUST, &130i128);

    let state: PriceState = client.get_price_state(&DUST);
    assert_eq!(state.sma, 106);
    assert_eq!(state.last_accepted, 130);
    assert_eq!(state.observation_count, 2);
    assert_eq!(state.rejected_prints, 0);
    assert_eq!(state.resource, DUST);
}

// ─── DEX integration ──────────────────────────────────────────────────────────

#[test]
fn test_list_at_market_prices_the_offer_from_the_oracle() {
    let (env, client, admin) = setup();
    let player = Address::generate(&env);
    init_pricing(&client, &admin);
    seed_all_prices(&client, &admin, 77);

    let (ship_id, layout) = mint_and_layout(&env, &client, &player);
    let asset = first_harvestable(&layout).expect("layout should hold a resource");

    let (harvest, offer, price) = client.list_at_market(&player, &ship_id, &layout, &asset);
    assert_eq!(price, 77);
    // The offer carries the engine's price, not a number the caller typed.
    assert_eq!(offer.min_price, 77);
    assert!(offer.active);
    assert!(offer.amount > 0);
    assert_eq!(offer.asset_id, asset);
    // The offer escrows only the listed asset, which is a subset of the
    // harvest when the layout held more than one resource.
    assert!(harvest.total_harvested >= offer.amount);
}

#[test]
fn test_list_at_market_tracks_the_smoothed_price_not_the_raw_feed() {
    let (env, client, admin) = setup();
    let player = Address::generate(&env);
    init_pricing(&client, &admin);
    seed_all_prices(&client, &admin, 100);

    // The feed spikes, but only 20% of the gap is taken per observation.
    tick(&env, DEFAULT_MIN_COOLDOWN_SECS);
    for asset in HARVEST_ASSETS {
        client.observe_price(&admin, &asset, &150i128);
    }

    let (ship_id, layout) = mint_and_layout(&env, &client, &player);
    let asset = first_harvestable(&layout).expect("layout should hold a resource");

    let (_, offer, price) = client.list_at_market(&player, &ship_id, &layout, &asset);
    assert_eq!(price, 110, "100 + 20% of the 50 gap");
    assert_eq!(offer.min_price, 110);
    assert!(offer.min_price < 150, "the listing must not chase the raw spike");
}

#[test]
fn test_list_at_market_requires_an_observed_price() {
    let (env, client, _admin) = setup();
    let player = Address::generate(&env);
    let (ship_id, layout) = mint_and_layout(&env, &client, &player);
    let asset = first_harvestable(&layout).expect("layout should hold a resource");

    // No pricing config and no observations at all.
    let err = client
        .try_list_at_market(&player, &ship_id, &layout, &asset)
        .unwrap_err();
    assert_eq!(err, Ok(DynamicListError::PriceUnavailable));
}

/// Every harvestable asset actually present in a layout.
#[allow(dead_code)]
fn harvestable_assets(env: &Env, layout: &NebulaLayout) -> Vec<Symbol> {
    let cells: &Vec<stellar_nebula_nomad::NebulaCell> = &layout.cells;
    let mut found: Vec<Symbol> = Vec::new(env);
    for i in 0..cells.len() {
        if let Some(asset) = asset_for_cell(cells.get(i).unwrap().cell_type.clone()) {
            let mut already = false;
            for j in 0..found.len() {
                if found.get(j).unwrap() == asset {
                    already = true;
                }
            }
            if !already {
                found.push_back(asset);
            }
        }
    }
    found
}

#[test]
fn test_list_at_market_rejects_a_resource_absent_from_the_layout() {
    let (env, client, admin) = setup();
    let player = Address::generate(&env);
    init_pricing(&client, &admin);
    seed_all_prices(&client, &admin, 50);

    let (ship_id, layout) = mint_and_layout(&env, &client, &player);
    // A well-priced resource the harvest cannot yield. A generated layout holds
    // most of the resource cell types, so "an asset the layout lacks" is not
    // reliably constructible; a priced asset that is definitely not harvestable
    // is, and it exercises the same branch.
    let ghost = symbol_short!("ghost");
    seed_price(&client, &admin, ghost.clone(), 50);

    let err = client
        .try_list_at_market(&player, &ship_id, &layout, &ghost)
        .unwrap_err();
    assert_eq!(err, Ok(DynamicListError::HarvestFailed));
}

#[test]
fn test_list_at_market_honours_the_listing_cap() {
    let (env, client, admin) = setup();
    let player = Address::generate(&env);
    init_pricing(&client, &admin);
    seed_all_prices(&client, &admin, 40);

    // Five listings is the cap; the sixth must fail even with a live price.
    // `generate_nebula_layout` is itself rate limited, so let the window roll
    // between layouts — the cap under test is the listing cap, not that one.
    for _ in 0..5 {
        tick(&env, 61);
        let (ship_id, layout) = mint_and_layout(&env, &client, &player);
        let asset = first_harvestable(&layout).expect("layout should hold a resource");
        client.list_at_market(&player, &ship_id, &layout, &asset);
    }

    tick(&env, 61);
    let (ship_id, layout) = mint_and_layout(&env, &client, &player);
    let asset = first_harvestable(&layout).expect("layout should hold a resource");
    let err = client
        .try_list_at_market(&player, &ship_id, &layout, &asset)
        .unwrap_err();
    assert_eq!(err, Ok(DynamicListError::HarvestFailed));
}

#[test]
fn test_a_market_listing_can_still_be_cancelled() {
    let (env, client, admin) = setup();
    let player = Address::generate(&env);
    init_pricing(&client, &admin);
    seed_all_prices(&client, &admin, 60);

    let (ship_id, layout) = mint_and_layout(&env, &client, &player);
    let asset = first_harvestable(&layout).expect("layout should hold a resource");
    let (_, offer, _) = client.list_at_market(&player, &ship_id, &layout, &asset);

    let cancelled = client.cancel_listing(&player, &offer.offer_id);
    assert!(!cancelled.active);
}
