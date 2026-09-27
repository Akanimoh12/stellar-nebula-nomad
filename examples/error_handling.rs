//! # Example: Handling contract errors from Rust
//!
//! Companion to `docs/ERROR_CODES.md`.
//!
//! Shows:
//!   * the `try_*` client and its nested `Result`,
//!   * turning an error into `(module, code)` for logs and telemetry,
//!   * a retry policy by error category (retry, fix input, or give up).
//!
//! Run:
//! ```text
//! cargo run --example error_handling
//! ```

use soroban_sdk::{symbol_short, testutils::Address as _, Address, Bytes, BytesN, Env};
use stellar_nebula_nomad::nebula_gen::{NebulaError, NebulaGen, NebulaGenClient};
use stellar_nebula_nomad::{NebulaNomadContract, NebulaNomadContractClient, ShipError};

/// What the caller should do next. Mirrors the categories in
/// `docs/ERROR_CODES.md` §3.
#[derive(Debug)]
enum Action {
    /// The request itself is wrong: fix the input and do not retry as-is.
    FixInput,
    /// A prerequisite is missing: run it first, then retry.
    RunPrerequisite(&'static str),
    /// A rate limit was hit: back off, then retry the same request.
    BackOffAndRetry,
    /// Wrong signer or permission.
    NeedsAuthorization,
    /// Operator or setup problem.
    ReportToOperator,
}

fn classify_nebula(err: NebulaError) -> Action {
    match err {
        NebulaError::InvalidSeed
        | NebulaError::InvalidShipId
        | NebulaError::InvalidRegionId
        | NebulaError::InvalidIndex
        | NebulaError::AnomalyOutOfBounds
        | NebulaError::InvalidSize
        | NebulaError::InvalidTtl => Action::FixInput,
        NebulaError::LayoutNotFound => Action::RunPrerequisite("generate_validated_nebula_layout"),
        NebulaError::RateLimitExceeded => Action::BackOffAndRetry,
        NebulaError::NotInitialized | NebulaError::AlreadyInitialized => Action::ReportToOperator,
    }
}

fn classify_ship(err: &ShipError) -> Action {
    match err {
        ShipError::NotOwner => Action::NeedsAuthorization,
        ShipError::ShipNotFound => Action::RunPrerequisite("mint_ship"),
        ShipError::ReentrancyDetected | ShipError::ShipAlreadyExists => Action::ReportToOperator,
        _ => Action::FixInput,
    }
}

fn main() {
    let env = Env::default();
    env.mock_all_auths();
    let pilot = Address::generate(&env);

    // ── 1. Anatomy of a try_* result ─────────────────────────────────────
    //
    //   Ok(Ok(value))    success
    //   Ok(Err(conv))    returned value could not be decoded (ABI mismatch)
    //   Err(Ok(e))       contract returned `Err(e)`: a documented error code
    //   Err(Err(host))   host-level failure (panic, budget, auth)
    let gen_id = env.register(NebulaGen, ());
    let nebula = NebulaGenClient::new(&env, &gen_id);

    // Calling before `init` -> NotInitialized (#1)
    match nebula.try_get_layout(&1u64) {
        Ok(Ok(None)) => println!("No layout yet (reads do not require init)"),
        other => println!("get_layout: {other:?}"),
    }
    let seed = BytesN::from_array(&env, &[3u8; 32]);
    match nebula.try_generate_validated_nebula_layout(&pilot, &1u64, &1u64, &seed) {
        Err(Ok(e)) => println!(
            "[nebula_gen #{:>2}] {e:?} -> {:?}",
            e as u32,
            classify_nebula(e)
        ),
        other => println!("unexpected: {other:?}"),
    }

    // ── 2. Recover from a prerequisite error ─────────────────────────────
    let admin = Address::generate(&env);
    nebula.init(&admin, &8u32, &1u32, &32u32, &0u64);

    let ship_id = 5u64;
    let anomaly = match nebula.try_query_anomaly(&ship_id, &0u32) {
        Ok(Ok(a)) => a,
        Err(Ok(e)) => {
            let action = classify_nebula(e);
            println!("[nebula_gen #{:>2}] {e:?} -> {action:?}", e as u32);
            if let Action::RunPrerequisite(step) = action {
                println!("  running prerequisite `{step}` then retrying once");
            }
            // Run the prerequisite, then retry once.
            nebula.generate_validated_nebula_layout(&pilot, &ship_id, &1u64, &seed);
            nebula.query_anomaly(&ship_id, &0u32)
        }
        other => panic!("unrecoverable: {other:?}"),
    };
    println!("Recovered: anomaly at ({}, {})", anomaly.x, anomaly.y);

    // ── 3. Same numeric code, different module ───────────────────────────
    // `#3` is InvalidSeed in nebula_gen but NotOwner in ship_nft. Always
    // decode a code using the module you called.
    let game = NebulaNomadContractClient::new(&env, &env.register(NebulaNomadContract, ()));
    let ship = game.mint_ship(&pilot, &symbol_short!("fighter"), &Bytes::new(&env));
    let thief = Address::generate(&env);
    if let Err(Ok(e)) = game.try_transfer_ship(&ship.id, &thief, &pilot) {
        println!(
            "[ship_nft   #{:>2}] {e:?} -> {:?}",
            e.clone() as u32,
            classify_ship(&e)
        );
    }
    let zero = BytesN::from_array(&env, &[0u8; 32]);
    if let Err(Ok(e)) = nebula.try_generate_validated_nebula_layout(&pilot, &1u64, &1u64, &zero) {
        println!("[nebula_gen #{:>2}] {e:?} -> {:?}", e as u32, classify_nebula(e));
    }
}
