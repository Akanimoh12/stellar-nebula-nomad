# Error Code Reference

This is the reference for every contract error in Stellar Nebula Nomad: what
each code means, what usually causes it, and how a client should handle it.

- **Scope:** every `#[contracterror]` enum under `src/`. That is 115 enums
  with 766 codes, including a few modules not yet wired into `src/lib.rs`
  (marked ⚠️ below).
- **Related docs:** [ERROR_HANDLING.md](ERROR_HANDLING.md) covers how to
  *add* errors and the `StandardContractError` descriptor convention. This
  document is the lookup table.

> **Maintenance:** codes are part of the public ABI. When you add a variant,
> add it to the matching table here. Never renumber or reuse an existing
> code (see [ERROR_HANDLING.md § Adding an error](ERROR_HANDLING.md#adding-an-error)).

---

## Contents

1. [How errors reach a client](#1-how-errors-reach-a-client)
2. [Codes are only unique per module](#2-codes-are-only-unique-per-module)
3. [Error categories and default handling](#3-error-categories-and-default-handling)
4. [Most common errors: causes and fixes](#4-most-common-errors-causes-and-fixes)
5. [Client error handling examples](#5-client-error-handling-examples)
6. [Module index](#6-module-index)
7. [Full reference by module](#7-full-reference-by-module)

---

## 1. How errors reach a client

Each module declares its errors as a `#[repr(u32)]` enum:

```rust
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum NebulaError {
    NotInitialized = 1,
    // ...
    LayoutNotFound = 5,
}
```

When a contract function returns `Err(NebulaError::LayoutNotFound)`, the
transaction fails and the Soroban host reports it as:

```text
HostError: Error(Contract, #5)
```

Only the **number** crosses the host boundary. The enum name does not. How
it shows up depends on the caller:

| Caller | What you see |
|--------|--------------|
| Rust tests / other contracts (`try_*` client) | `Err(Ok(NebulaError::LayoutNotFound))` |
| Soroban CLI (`stellar contract invoke`) | `Error(Contract, #5)` in the diagnostic output |
| JS/TS (`@stellar/stellar-sdk` simulation) | `simulation.error` string containing `Error(Contract, #5)` |
| Transaction result (submitted tx) | `txFailed` with the contract error in the result meta / diagnostic events |

A contract that **panics** (e.g. `unwrap()` on `None`, overflow with
`overflow-checks = true`) does not produce `Error(Contract, #N)`. You get a
host error such as `Error(WasmVm, InvalidAction)` instead. Treat those as
[Internal](#3-error-categories-and-default-handling) and report them.

## 2. Codes are only unique per module

Many modules start numbering at `1`, so **`#1` means something different in
each module**. For example:

| Code | `nebula_gen::NebulaError` | `gas_sponsor::SponsorError` | `ship_nft::ShipError` |
|-----:|---------------------------|-----------------------------|-----------------------|
| 1 | `NotInitialized` | `AlreadySponsored` | `ShipAlreadyExists` |
| 3 | `InvalidSeed` | `InsufficientFunds` | `NotOwner` |
| 5 | `LayoutNotFound` | `ProfileNotVerified` | `BatchLimitExceeded` |

To decode an error, pair the code with the module that owns the function you
called. `(module, code)` is the unique identity, which is exactly what
`StandardContractError::descriptor()` returns.

Some modules use offset ranges so their codes do not collide inside the
main contract:

| Range | Module |
|------:|--------|
| 100–102 | `rate_limiter::RateLimitError` |
| 200–205 | `resource_minter::MinterError` |

Some enum **names** repeat across modules (`OracleError` in `market_oracle`
and `randomness_oracle`, `RateLimitError`, `MinterError`, `BountyError`,
`BridgeError`). Always qualify them with the module path.

## 3. Error categories and default handling

Every code in the reference tables has one of these categories. The first six
match `ErrorKind` in [`src/error_standard.rs`](../src/error_standard.rs).
`Setup` and `State` are finer splits that make client handling clearer.

| Category | `ErrorKind` | Meaning | Retry? | Default client handling |
|----------|-------------|---------|--------|-------------------------|
| **Validation** | `Validation` | Arguments are malformed, out of range, zero, or inconsistent. | ❌ Never with the same input | Validate before sending. Show a field-level message. |
| **Authorization** | `Authorization` | Caller lacks the role, ownership, relationship, opt-in or eligibility. | ❌ | Prompt the right account to sign, or guide the user through the prerequisite. |
| **NotFound** | `NotFound` | Referenced entity/key does not exist (or has expired). | ❌ (✅ after creating it) | Refresh cached state. Create or register the entity first. |
| **Conflict** | `Conflict` | The action already happened, or the state collides. | ❌ | Treat as idempotent: re-read state and show the existing result. |
| **ResourceLimit** | `ResourceLimit` | A cap, quota, balance, or batch limit was hit. | ⏳ Often, after the window resets | Reduce the amount or batch size, top up, or back off and retry later. |
| **State** | `Conflict` | Wrong lifecycle phase or time window (paused, expired, time-locked, not started…). | ⏳ Sometimes | Read current state. Wait for the window or do the prerequisite step. |
| **Setup** | `NotFound` | Module not initialised. | ❌ until an admin initialises it | Operator issue: run the module's `init`. |
| **Internal** | `Internal` | Invariant violation or downstream failure. | ⚠️ Only if known transient | Log the tx hash and inputs, then report. Do not loop. |

Modules that already implement `StandardContractError` (marked in the
[module index](#6-module-index)) expose `kind` and `retryable` directly.
Prefer those values over the table above.

## 4. Most common errors: causes and fixes

These are the errors players and integrators run into most often.

### Nebula generation (`nebula_gen::NebulaError`)

| Code | Variant | Common cause | Fix |
|-----:|---------|--------------|-----|
| 1 | `NotInitialized` | `init` was never called on the deployed contract. | Admin calls `init(admin, default_size, min_size, max_size, layout_ttl)`. |
| 3 | `InvalidSeed` | Seed is 32 zero bytes (for example an unset buffer). | Use a random 32-byte seed, e.g. `crypto.getRandomValues(new Uint8Array(32))`. |
| 5 | `LayoutNotFound` | No layout for this ship, or it **expired** (default TTL is 24 h). | Call `generate_validated_nebula_layout` again. |
| 8 | `InvalidShipId` | `ship_id == 0`. | Use the ID returned by `mint_ship`. |
| 9 | `InvalidRegionId` | `region_id` is 0 or greater than `MAX_REGION_ID` (1,000,000). | Clamp the region to `1..=1_000_000`. |
| 10 | `AnomalyOutOfBounds` | Index is `>= layout.size`. | Read `get_layout(ship_id).size` first. |

### Resource minting (`resource_minter::MinterError`, codes 200+)

| Code | Variant | Common cause | Fix |
|-----:|---------|--------------|-----|
| 200 | `InvalidAmount` | Amount is `0`. | Send a positive amount. |
| 201 | `RateLimitExceeded` | Too many mints in the current window. | Back off and retry after the window. Batch harvests. |
| 202 | `NoLayoutForShip` | Harvesting before scanning. | Scan / generate a layout for the ship first. |
| 203 | `NoResourceAtAnomaly` | `anomaly_index` is outside the ship's layout (mapped from `NebulaError::AnomalyOutOfBounds`). | Read the layout and use an index `< layout.size`. |
| 204 | `ArithmeticOverflow` | Mint would overflow total supply. | Mint a smaller amount. Report if unexpected. |
| 205 | `InsufficientBalance` | Debit exceeds the account balance. | Check `get_resource_balance` before spending. |

### Ships (`ship_nft::ShipError`)

| Code | Variant | Common cause | Fix |
|-----:|---------|--------------|-----|
| 2 | `ShipNotFound` | Wrong ID, or the ship is on another deployment. | Re-read the player's ship list. |
| 3 | `NotOwner` | Transaction signed by someone other than the owner. | Sign with the owner's key. |
| 4 | `SameOwner` | Transfer to the current owner. | Pick a different recipient. |
| 5 | `BatchLimitExceeded` | More than 3 ships minted in one tx. | Split into batches of ≤ 3. |
| 7 | `ReentrancyDetected` | Re-entrant call through a hook or cross-contract callback. | Do not call back into ship functions from callbacks. |
| 8 | `InvalidMetadataUri` | URI scheme not supported by marketplaces. | Use `ipfs://`, `ar://`, or `https://`. |

### First-scan sponsorship (`gas_sponsor::SponsorError`)

| Code | Variant | Common cause | Fix |
|-----:|---------|--------------|-----|
| 1 | `AlreadySponsored` | Player already used the one-time sponsored scan. | Player pays their own fee. Hide the "free scan" UI. |
| 2 | `DailyCapReached` | Global daily sponsorship budget used up. | Retry after the 24 h window resets. |
| 3 | `InsufficientFunds` | Sponsorship pool is empty. | Operator tops up the fund. |
| 5 | `ProfileNotVerified` | Profile not initialised. | Create the player profile first. |
| 8 / 9 | `PerUserCapReached` / `PerUserDailyCapReached` | Per-player caps hit. | Wait for the daily reset, or pay directly. |
| 10 | `SessionKeyInvalid` | Mobile session key expired or revoked. | Create a new session with `create_mobile_session`. |

### Storage helpers (`storage_optim::StorageError`)

| Code | Variant | Common cause | Fix |
|-----:|---------|--------------|-----|
| 1 | `ReentrancyDetected` | A write re-entered while another held the lock. | Do not nest `store_*` calls inside each other. |
| 2 | `EntryNotFound` | Key was never stored, or its TTL lapsed. | Store it again. Keep hot keys alive with `extend_ttl`. |
| 3 | `BurstLimitExceeded` | More than `MAX_BURST_READS` (100) reads without a reset. | Use `get_optimized_entries` / `get_ship_nebula_batch`, which count once per batch, or call `reset_burst_counter`. |
| 4 | `InvalidTtl` | `default_ttl` is 0 or `default_ttl > max_ttl`. | Pass `0 < default_ttl ≤ max_ttl`. |
| 6 | `InvalidKey` | `batch_store_with_bump` got `keys` and `values` of different lengths. | Send equal-length vectors. |

### Global controls

| Module / Code | Variant | Common cause | Fix |
|---------------|---------|--------------|-----|
| `emergency_controls` #1 | `ContractPaused` | Operators paused the contract. | Show a maintenance banner and poll until unpaused. |
| `rate_limiter` #100 | `RateLimitExceeded` | Caller exceeded the per-operation call rate. | Exponential backoff (for example 2 s, 4 s, 8 s…). |
| `session_manager` #2 | `SessionExpired` | Session TTL elapsed. | Start a new session. |
| `player_profile` #1 | `ProfileNotFound` | New wallet with no profile yet. | Run onboarding: create the profile. |

## 5. Client error handling examples

### Rust (contract tests and cross-contract calls)

Generated clients have `try_*` variants that return
`Result<Result<T, _>, Result<ContractError, InvokeError>>`:

```rust
use stellar_nebula_nomad::nebula_gen::{NebulaGenClient, NebulaError};

match client.try_query_anomaly(&ship_id, &index) {
    Ok(Ok(anomaly)) => { /* use anomaly */ }
    // A declared contract error: match on the enum.
    Err(Ok(NebulaError::LayoutNotFound)) => {
        // Layout expired: regenerate, then retry once.
        client.generate_validated_nebula_layout(&caller, &ship_id, &region, &seed);
    }
    Err(Ok(NebulaError::InvalidIndex)) | Err(Ok(NebulaError::AnomalyOutOfBounds)) => {
        // Caller bug: do not retry.
    }
    Err(Ok(other)) => panic!("unexpected contract error: {other:?}"),
    // Host-level failure (panic, budget exceeded, auth failure).
    Err(Err(host_err)) => panic!("host error: {host_err:?}"),
    // Return value failed to convert (ABI mismatch between client and contract).
    Ok(Err(conv)) => panic!("conversion error: {conv:?}"),
}
```

For modules that implement `StandardContractError`, handle by category
instead of by variant:

```rust
use stellar_nebula_nomad::error_standard::{ErrorKind, StandardContractError};

fn should_retry<E: StandardContractError>(err: E) -> bool {
    let d = err.descriptor();
    d.retryable || d.kind == ErrorKind::ResourceLimit
}
```

### TypeScript (`@stellar/stellar-sdk`)

```ts
import { SorobanRpc } from "@stellar/stellar-sdk";

/** Pull the numeric contract error code out of a simulation/tx failure. */
export function contractErrorCode(err: unknown): number | null {
  const text =
    typeof err === "string" ? err : err instanceof Error ? err.message : JSON.stringify(err);
  const m = /Error\(Contract, #(\d+)\)/.exec(text);
  return m ? Number(m[1]) : null;
}

// Per-module maps: codes are only unique within a module (§2).
export const NebulaGenErrors: Record<number, string> = {
  1: "NotInitialized",
  2: "AlreadyInitialized",
  3: "InvalidSeed",
  4: "InvalidIndex",
  5: "LayoutNotFound",
  6: "InvalidSize",
  7: "InvalidTtl",
  8: "InvalidShipId",
  9: "InvalidRegionId",
  10: "AnomalyOutOfBounds",
};

const RETRYABLE = new Set(["LayoutNotFound"]); // regenerate, then retry

export async function simulateOrExplain(
  server: SorobanRpc.Server,
  tx: Parameters<SorobanRpc.Server["simulateTransaction"]>[0],
) {
  const sim = await server.simulateTransaction(tx);
  if (SorobanRpc.Api.isSimulationError(sim)) {
    const code = contractErrorCode(sim.error);
    const name = code !== null ? NebulaGenErrors[code] ?? `Unknown(#${code})` : "HostError";
    return { ok: false as const, code, name, retryable: RETRYABLE.has(name) };
  }
  return { ok: true as const, sim };
}
```

Guidelines for dApp integrations:

1. **Always simulate first.** Simulation shows contract errors before the
   user pays a fee.
2. **Map by module.** Keep one code → name map per contract/module you call.
3. **Validate locally.** Most `Validation` errors can be caught in the UI:
   non-zero amounts, ID ranges, 32-byte non-zero seeds.
4. **Idempotent conflicts.** For `Already*` errors, re-read state and show
   success rather than an error toast.
5. **Back off on limits.** For `RateLimitExceeded` / `*CapReached`, use
   exponential backoff capped at the documented window (hourly or daily).
6. **Surface Internal errors.** Log the tx hash, function, and arguments,
   and ask the user to report them.

See [`examples/js/error-handling.ts`](../examples/js/error-handling.ts) for
a runnable version.

---

## 6. Module index

✅ = compiled into the contract · ⚠️ = source exists but not declared in
`src/lib.rs` · **Std** = implements `StandardContractError`.

| Module | Enum | Variants | Codes | Built | Std |
|--------|------|---------:|-------|:-----:|:---:|
| [`access_control`](#access-control-accesscontrolerror) | `AccessControlError` | 17 | 1–17 | ✅ | ✅ |
| [`achievement_engine`](#achievement-engine-achievementerror) | `AchievementError` | 5 | 1–5 | ✅ | ✅ |
| [`achievements`](#achievements-achievementserror) | `AchievementsError` | 2 | 1–2 | ⚠️ not built |  |
| [`alliance_manager`](#alliance-manager-allianceerror) | `AllianceError` | 7 | 1–7 | ✅ | ✅ |
| [`analytics`](#analytics-analyticserror) | `AnalyticsError` | 1 | 1–1 | ✅ | ✅ |
| [`anomaly_classifier`](#anomaly-classifier-anomalyerror) | `AnomalyError` | 3 | 1–3 | ✅ | ✅ |
| [`audio_seed_generator`](#audio-seed-generator-audioerror) | `AudioError` | 4 | 1–4 | ✅ | ✅ |
| [`audit_logger`](#audit-logger-auditloggererror) | `AuditLoggerError` | 3 | 1–3 | ✅ | ✅ |
| [`badges`](#badges-badgeerror) | `BadgeError` | 4 | 1–4 | ⚠️ not built |  |
| [`batch_processor`](#batch-processor-batcherror) | `BatchError` | 5 | 1–5 | ✅ | ✅ |
| [`battle_pass`](#battle-pass-battlepasserror) | `BattlePassError` | 7 | 1–7 | ✅ | ✅ |
| [`blueprint_factory`](#blueprint-factory-blueprinterror) | `BlueprintError` | 5 | 1–5 | ✅ | ✅ |
| [`bot_detection`](#bot-detection-boterror) | `BotError` | 6 | 1–6 | ⚠️ not built |  |
| [`bounty_board`](#bounty-board-bountyerror) | `BountyError` | 8 | 1–8 | ✅ | ✅ |
| [`bridge::ethereum`](#bridge-ethereum-bridgeerror) | `BridgeError` | 11 | 1–11 | ⚠️ not built |  |
| [`bridge::stellar_ethereum`](#bridge-stellar-ethereum-bridgeerror) | `BridgeError` | 11 | 90–100 | ⚠️ not built |  |
| [`bridge::validator`](#bridge-validator-validatorerror) | `ValidatorError` | 9 | 1–9 | ⚠️ not built |  |
| [`bug_bounty_payout`](#bug-bounty-payout-bountyerror) | `BountyError` | 11 | 1–11 | ⚠️ not built |  |
| [`cache_ttl_manager`](#cache-ttl-manager-cachettlerror) | `CacheTtlError` | 5 | 1–5 | ✅ | ✅ |
| [`clan_wars`](#clan-wars-warerror) | `WarError` | 11 | 1–11 | ⚠️ not built |  |
| [`composability_examples`](#composability-examples-composabilityerror) | `ComposabilityError` | 9 | 1–9 | ✅ | ✅ |
| [`config_updater`](#config-updater-configerror) | `ConfigError` | 9 | 1–9 | ⚠️ not built |  |
| [`constellation_mapper`](#constellation-mapper-constellationerror) | `ConstellationError` | 4 | 1–4 | ✅ | ✅ |
| [`content_tools`](#content-tools-contenttoolserror) | `ContentToolsError` | 10 | 1–10 | ✅ | ✅ |
| [`contract_versioning`](#contract-versioning-versioningerror) | `VersioningError` | 5 | 1–5 | ✅ | ✅ |
| [`crafting`](#crafting-craftingerror) | `CraftingError` | 9 | 1–9 | ✅ | ✅ |
| [`daily_rewards`](#daily-rewards-dailyrewarderror) | `DailyRewardError` | 4 | 1–4 | ⚠️ not built |  |
| [`dao`](#dao-daoerror) | `DaoError` | 14 | 1–14 | ⚠️ not built |  |
| [`data_exporter`](#data-exporter-exporterror) | `ExportError` | 3 | 1–3 | ✅ | ✅ |
| [`difficulty_curve`](#difficulty-curve-curveerror) | `CurveError` | 5 | 1–5 | ✅ | ✅ |
| [`difficulty_scaler`](#difficulty-scaler-difficultyerror) | `DifficultyError` | 1 | 1–1 | ✅ | ✅ |
| [`emergency_controls`](#emergency-controls-emergencyerror) | `EmergencyError` | 6 | 1–6 | ✅ | ✅ |
| [`energy_manager`](#energy-manager-energyerror) | `EnergyError` | 5 | 1–5 | ✅ | ✅ |
| [`entanglement_comms`](#entanglement-comms-entanglementerror) | `EntanglementError` | 6 | 1–6 | ✅ | ✅ |
| [`environment_simulator`](#environment-simulator-environmenterror) | `EnvironmentError` | 3 | 1–3 | ✅ | ✅ |
| [`errors`](#errors-bondingerror) | `BondingError` | 6 | 400–405 | ⚠️ not built |  |
| [`errors`](#errors-mintererror) | `MinterError` | 5 | 200–204 | ⚠️ not built |  |
| [`errors`](#errors-nebulagenerror) | `NebulaGenError` | 5 | 1–5 | ⚠️ not built |  |
| [`errors`](#errors-ratelimiterror) | `RateLimitError` | 2 | 100–101 | ⚠️ not built |  |
| [`errors`](#errors-shipregistryerror) | `ShipRegistryError` | 5 | 300–304 | ⚠️ not built |  |
| [`escrow_trader`](#escrow-trader-escrowerror) | `EscrowError` | 8 | 1–8 | ✅ | ✅ |
| [`event_framework`](#event-framework-eventframeworkerror) | `EventFrameworkError` | 3 | 1–3 | ⚠️ not built |  |
| [`event_scheduler`](#event-scheduler-eventerror) | `EventError` | 24 | 1–24 | ✅ | ✅ |
| [`exploration_heatmap`](#exploration-heatmap-heatmaperror) | `HeatmapError` | 1 | 1–1 | ✅ | ✅ |
| [`fleet_manager`](#fleet-manager-fleeterror) | `FleetError` | 7 | 1–7 | ⚠️ not built |  |
| [`fractional_resources`](#fractional-resources-fractionalerror) | `FractionalError` | 10 | 1–10 | ✅ | ✅ |
| [`fraud_detection`](#fraud-detection-frauderror) | `FraudError` | 1 | 1–1 | ✅ | ✅ |
| [`gas_recovery`](#gas-recovery-refunderror) | `RefundError` | 5 | 1–5 | ✅ | ✅ |
| [`gas_sponsor`](#gas-sponsor-sponsorerror) | `SponsorError` | 12 | 1–12 | ✅ | ✅ |
| [`gifting_system`](#gifting-system-gifterror) | `GiftError` | 8 | 1–8 | ✅ | ✅ |
| [`governance`](#governance-goverror) | `GovError` | 7 | 1–7 | ✅ | ✅ |
| [`guild_economy`](#guild-economy-guildeconomyerror) | `GuildEconomyError` | 8 | 1–8 | ⚠️ not built |  |
| [`guild_quests`](#guild-quests-guildquesterror) | `GuildQuestError` | 5 | 1–5 | ✅ | ✅ |
| [`health_monitor`](#health-monitor-healtherror) | `HealthError` | 2 | 1–2 | ✅ | ✅ |
| [`indexer_callbacks`](#indexer-callbacks-indexererror) | `IndexerError` | 3 | 1–3 | ✅ | ✅ |
| [`input_validation`](#input-validation-validationerror) | `ValidationError` | 5 | 80–84 | ✅ | ✅ |
| [`leaderboards`](#leaderboards-leaderboarderror) | `LeaderboardError` | 8 | 1–8 | ✅ | ✅ |
| [`loot_system`](#loot-system-looterror) | `LootError` | 9 | 1–9 | ⚠️ not built |  |
| [`market_oracle`](#market-oracle-oracleerror) | `OracleError` | 8 | 1–8 | ✅ | ✅ |
| [`metadata_resolver`](#metadata-resolver-metadataerror) | `MetadataError` | 5 | 1–5 | ✅ | ✅ |
| [`metrics_exporter`](#metrics-exporter-metricserror) | `MetricsError` | 3 | 1–3 | ✅ | ✅ |
| [`migration_framework`](#migration-framework-migrationerror) | `MigrationError` | 8 | 1–8 | ✅ | ✅ |
| [`mini_games`](#mini-games-minigameerror) | `MiniGameError` | 9 | 1–9 | ⚠️ not built |  |
| [`mission_generator`](#mission-generator-missionerror) | `MissionError` | 5 | 1–5 | ✅ | ✅ |
| [`mobile_views`](#mobile-views-mobileviewerror) | `MobileViewError` | 1 | 1–1 | ✅ | ✅ |
| [`navigation_planner`](#navigation-planner-naverror) | `NavError` | 8 | 1–8 | ✅ | ✅ |
| [`nebula_archive`](#nebula-archive-archiveerror) | `ArchiveError` | 2 | 1–2 | ⚠️ not built |  |
| [`nebula_gen`](#nebula-gen-nebulaerror) | `NebulaError` | 11 | 1–11 | ✅ | ✅ |
| [`nft_marketplace`](#nft-marketplace-marketplaceerror) | `MarketplaceError` | 14 | 1–14 | ✅ | ✅ |
| [`nomad_bonding`](#nomad-bonding-bonderror) | `BondError` | 13 | 1–13 | ✅ | ✅ |
| [`offline_progress`](#offline-progress-offlineerror) | `OfflineError` | 2 | 1–2 | ⚠️ not built |  |
| [`onboarding_tutorial`](#onboarding-tutorial-onboardingerror) | `OnboardingError` | 11 | 1–11 | ✅ | ✅ |
| [`player_profile`](#player-profile-profileerror) | `ProfileError` | 5 | 1–5 | ✅ | ✅ |
| [`player_segmentation`](#player-segmentation-segmentationerror) | `SegmentationError` | 2 | 1–2 | ✅ | ✅ |
| [`portal_registry`](#portal-registry-portalerror) | `PortalError` | 5 | 1–5 | ✅ | ✅ |
| [`privacy_stats`](#privacy-stats-privacyerror) | `PrivacyError` | 5 | 1–5 | ✅ | ✅ |
| [`prize_distributor`](#prize-distributor-prizeerror) | `PrizeError` | 6 | 1–6 | ✅ | ✅ |
| [`proxy`](#proxy-proxyerror) | `ProxyError` | 6 | 1–6 | ⚠️ not built |  |
| [`pvp_combat`](#pvp-combat-pvperror) | `PvPError` | 13 | 1–13 | ✅ | ✅ |
| [`quest_system`](#quest-system-questerror) | `QuestError` | 16 | 1–16 | ✅ | ✅ |
| [`randomness_oracle`](#randomness-oracle-oracleerror) | `OracleError` | 2 | 1–2 | ✅ | ✅ |
| [`rate_limiter`](#rate-limiter-ratelimiterror) | `RateLimitError` | 3 | 100–102 | ✅ | ✅ |
| [`realtime_events`](#realtime-events-realtimeerror) | `RealtimeError` | 13 | 110–122 | ⚠️ not built |  |
| [`recipes`](#recipes-recipeerror) | `RecipeError` | 1 | 1–1 | ✅ | ✅ |
| [`recycling_crafter`](#recycling-crafter-recyclingerror) | `RecyclingError` | 5 | 1–5 | ✅ | ✅ |
| [`reentrancy_guard`](#reentrancy-guard-reentrancyerror) | `ReentrancyError` | 1 | 1–1 | ✅ | ✅ |
| [`referral_system`](#referral-system-referralerror) | `ReferralError` | 7 | 1–7 | ✅ | ✅ |
| [`referral_system`](#referral-system-referralv2error) | `ReferralV2Error` | 3 | 10–12 | ✅ | ✅ |
| [`reputation`](#reputation-reputationerror) | `ReputationError` | 8 | 1–8 | ✅ | ✅ |
| [`resource_minter`](#resource-minter-mintererror) | `MinterError` | 6 | 200–205 | ✅ | ✅ |
| [`resource_minter`](#resource-minter-harvesterror) | `HarvestError` | 7 | 1–7 | ✅ | ✅ |
| [`revenue_attribution`](#revenue-attribution-attributionerror) | `AttributionError` | 1 | 1–1 | ✅ | ✅ |
| [`rewards`](#rewards-rewarderror) | `RewardError` | 8 | 1–8 | ✅ | ✅ |
| [`seasons`](#seasons-seasonerror) | `SeasonError` | 7 | 1–7 | ✅ | ✅ |
| [`session_manager`](#session-manager-sessionerror) | `SessionError` | 4 | 1–4 | ✅ | ✅ |
| [`shared_lib`](#shared-lib-sharederror) | `SharedError` | 3 | 1–3 | ✅ | ✅ |
| [`ship_customization`](#ship-customization-skinerror) | `SkinError` | 13 | 1–13 | ✅ | ✅ |
| [`ship_nft`](#ship-nft-shiperror) | `ShipError` | 8 | 1–8 | ✅ | ✅ |
| [`ship_upgrade`](#ship-upgrade-shipupgradeerror) | `ShipUpgradeError` | 10 | 200–209 | ✅ | ✅ |
| [`smart_alerts`](#smart-alerts-alerterror) | `AlertError` | 2 | 1–2 | ✅ | ✅ |
| [`soul_binding`](#soul-binding-bindingerror) | `BindingError` | 3 | 1–3 | ⚠️ not built |  |
| [`staking`](#staking-stakingerror) | `StakingError` | 15 | 1–15 | ⚠️ not built |  |
| [`state_snapshot`](#state-snapshot-snapshoterror) | `SnapshotError` | 9 | 1–9 | ✅ | ✅ |
| [`storage_optim`](#storage-optim-storageerror) | `StorageError` | 6 | 1–6 | ✅ | ✅ |
| [`sustainability_metrics`](#sustainability-metrics-sustainabilityerror) | `SustainabilityError` | 3 | 1–3 | ✅ | ✅ |
| [`theme_customizer`](#theme-customizer-themeerror) | `ThemeError` | 3 | 1–3 | ✅ | ✅ |
| [`token_burning`](#token-burning-burningerror) | `BurningError` | 6 | 1–6 | ⚠️ not built |  |
| [`tournament`](#tournament-tournamenterror) | `TournamentError` | 15 | 1–15 | ⚠️ not built |  |
| [`trading`](#trading-ammerror) | `AmmError` | 9 | 100–108 | ✅ | ✅ |
| [`trading`](#trading-tradingerror) | `TradingError` | 6 | 1–6 | ✅ | ✅ |
| [`treasure_vault`](#treasure-vault-vaulterror) | `VaultError` | 6 | 1–6 | ✅ | ✅ |
| [`wallet_abstraction`](#wallet-abstraction-walleterror) | `WalletError` | 14 | 1–14 | ⚠️ not built |  |
| [`wormhole_traveler`](#wormhole-traveler-wormholeerror) | `WormholeError` | 10 | 1–10 | ✅ | ✅ |
| [`yield_farming`](#yield-farming-farmerror) | `FarmError` | 7 | 1–7 | ✅ | ✅ |
| [`yield_forecast`](#yield-forecast-forecasterror) | `ForecastError` | 7 | 1–7 | ✅ | ✅ |

---

## 7. Full reference by module

The *Meaning* column is taken from each variant's doc comment (or its name when it has none). *Category* and *How to handle* follow §3.

<a id="access-control-accesscontrolerror"></a>
### `access_control` — `AccessControlError`

Source: [`src/access_control.rs`](../src/access_control.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AdminRequired` | Admin required: Caller is not the current admin. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 2 | `RoleNotFound` | Role not found: The specified role does not exist or was never defined. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `UnauthorizedRole` | Unauthorized role: Caller does not hold the required role, or role is expired/revoked. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 4 | `RoleAlreadyGranted` | Role already granted: The address already holds the role (idempotency policy). | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 5 | `BatchLimitExceeded` | Batch limit exceeded: Batch grant attempted with more than 5 addresses. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `InvalidExpiry` | Invalid expiry: Expiry value is in the past or otherwise invalid. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `PermissionNotFound` | Permission not found: The (role, action) permission was never defined. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 8 | `InitializationFailed` | Initialization failed: init_roles was already called or another init error occurred. | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |
| 9 | `NotImplemented` | Not implemented: This function is a placeholder for future DAO integration. | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |
| 10 | `MultiSigNotConfigured` | Multi-sig not configured. | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |
| 11 | `ProposalNotFound` | Proposal not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 12 | `ProposalAlreadyExecuted` | Proposal already executed. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 13 | `ProposalExpired` | Proposal expired. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 14 | `InsufficientApprovals` | Insufficient approvals. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 15 | `AlreadyApproved` | Already approved. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 16 | `TimelockNotElapsed` | Timelock not elapsed. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 17 | `InvalidSignerConfig` | Invalid signer configuration. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="achievement-engine-achievementerror"></a>
### `achievement_engine` — `AchievementError`

Source: [`src/achievement_engine.rs`](../src/achievement_engine.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AlreadyUnlocked` | Already unlocked | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 2 | `TemplateNotFound` | Template not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `ProfileNotFound` | Profile not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `NotEligible` | Not eligible | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 5 | `BatchTooLarge` | Batch too large | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="achievements-achievementserror"></a>
### `achievements` — `AchievementsError`

Source: [`src/achievements.rs`](../src/achievements.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NotFound` | The requested achievement ID does not exist in the catalog. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `ProfileNotFound` | The player's profile could not be located. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |

<a id="alliance-manager-allianceerror"></a>
### `alliance_manager` — `AllianceError`

Source: [`src/alliance_manager.rs`](../src/alliance_manager.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AllianceFull` | Alliance full | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 2 | `AllianceNotFound` | Alliance not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `AlreadyInAlliance` | Already in alliance | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 4 | `NotMember` | Not member | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 5 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 6 | `InvalidName` | Invalid name | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `InsufficientVotes` | Insufficient votes | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="analytics-analyticserror"></a>
### `analytics` — `AnalyticsError`

Source: [`src/analytics.rs`](../src/analytics.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidTopN` | top_n must be in the range 1..=50. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="anomaly-classifier-anomalyerror"></a>
### `anomaly_classifier` — `AnomalyError`

Source: [`src/anomaly_classifier.rs`](../src/anomaly_classifier.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InsufficientFeatures` | Insufficient features | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 2 | `NotFound` | Not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |

<a id="audio-seed-generator-audioerror"></a>
### `audio_seed_generator` — `AudioError`

Source: [`src/audio_seed_generator.rs`](../src/audio_seed_generator.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidLayer` | Invalid layer | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `InvalidNebulaId` | Invalid nebula id | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `SeedNotFound` | Seed not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `InvalidPreset` | Invalid preset | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="audit-logger-auditloggererror"></a>
### `audit_logger` — `AuditLoggerError`

Source: [`src/audit_logger.rs`](../src/audit_logger.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `LogWriteFailed` | Log write failed | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |
| 2 | `QueryLimitExceeded` | Query limit exceeded | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 3 | `InvalidFilter` | Invalid filter | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="badges-badgeerror"></a>
### `badges` — `BadgeError`

Source: [`src/badges.rs`](../src/badges.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `BadgeNotFound` | Badge ID not found in storage. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `NotOwner` | Caller is not the current owner of the badge. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 3 | `NonTransferable` | This badge is soul-bound and cannot be transferred. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 4 | `SameOwner` | Cannot transfer to the current owner. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="batch-processor-batcherror"></a>
### `batch_processor` — `BatchError`

Source: [`src/batch_processor.rs`](../src/batch_processor.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `BatchLimitExceeded` | Batch size exceeds the maximum of 8. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 2 | `EmptyBatch` | No operations are queued for this player. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `OperationFailed` | One or more operations failed; batch was rolled back. | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |
| 4 | `GasLimitExceeded` | Gas limit enforcement: too many ops in-flight. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `ShipNotFound` | A referenced ship ID was not found in the provided list. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |

<a id="battle-pass-battlepasserror"></a>
### `battle_pass` — `BattlePassError`

Source: [`src/battle_pass.rs`](../src/battle_pass.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NotEnoughXP` | Not enough xp | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 2 | `AlreadyClaimed` | Already claimed | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 3 | `InvalidTier` | Invalid tier | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `NoActiveSeason` | No active season | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `PremiumRequired` | Premium required | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 6 | `TierOutOfRange` | Tier out of range | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `ChallengeXpAlreadyGranted` | XP for this challenge was already granted to this player. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |

<a id="blueprint-factory-blueprinterror"></a>
### `blueprint_factory` — `BlueprintError`

Source: [`src/blueprint_factory.rs`](../src/blueprint_factory.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `BlueprintNotFound` | Blueprint not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `InvalidComponents` | Invalid components | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `AlreadyApplied` | Already applied | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 4 | `NotOwner` | Not owner | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 5 | `BatchTooLarge` | Batch too large | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="bot-detection-boterror"></a>
### `bot_detection` — `BotError`

Source: [`src/bot_detection.rs`](../src/bot_detection.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `CaptchaRequired` | Player is flagged as suspicious and must solve a CAPTCHA. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 2 | `ChallengeNotFound` | CAPTCHA challenge not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `ChallengeExpired` | CAPTCHA challenge expired. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 4 | `IncorrectAnswer` | CAPTCHA answer incorrect. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 5 | `ActionTooFast` | Action too fast — potential automation detected. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 6 | `PlayerBlocked` | Player is temporarily blocked. | Authorization | The account is flagged or blocked. Contact an admin; do not retry automatically. |

<a id="bounty-board-bountyerror"></a>
### `bounty_board` — `BountyError`

Source: [`src/bounty_board.rs`](../src/bounty_board.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `BountyNotFound` | Bounty does not exist. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `BountyExpired` | Bounty has expired. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 3 | `NotPoster` | Caller is not the bounty poster. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 4 | `NotAuthorized` | Caller is not authorized (admin). | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 5 | `TooManyActiveBounties` | Maximum active bounties reached. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `InvalidReward` | Reward amount must be positive. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `InvalidProof` | Proof does not meet requirements. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 8 | `AlreadyClaimed` | Bounty already claimed. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |

<a id="bridge-ethereum-bridgeerror"></a>
### `bridge::ethereum` — `BridgeError`

Source: [`src/bridge/ethereum.rs`](../src/bridge/ethereum.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AmountTooLow` | Amount below minimum bridge threshold | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `BridgePaused` | Bridge is currently paused | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 3 | `NotAuthorized` | Caller is not authorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 4 | `TransferExists` | Transfer already exists | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 5 | `TransferNotFound` | Transfer not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 6 | `InvalidChain` | Invalid chain identifier | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `AssetNotSupported` | Asset not supported for bridging | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 8 | `InsufficientLocked` | Insufficient locked assets for unlock | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 9 | `NotConfirmed` | Transfer not yet confirmed by validators | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 10 | `FeeOverflow` | Fee calculation overflow | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 11 | `InvalidFee` | Invalid fee percentage | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="bridge-stellar-ethereum-bridgeerror"></a>
### `bridge::stellar_ethereum` — `BridgeError`

Source: [`src/bridge/stellar_ethereum.rs`](../src/bridge/stellar_ethereum.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 90 | `BridgePaused` | Bridge is paused. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 91 | `InvalidAmount` | Invalid amount (zero or negative). | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 92 | `AmountExceeded` | Amount exceeds bridge limit. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 93 | `InsufficientBalance` | Insufficient balance for bridging. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 94 | `RequestNotFound` | Request not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 95 | `InvalidRequestState` | Request is not in a valid state for this operation. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 96 | `InsufficientConfirmations` | Insufficient confirmations. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 97 | `Unauthorized` | Unauthorized action. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 98 | `TooManyPendingRequests` | Maximum pending requests reached. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 99 | `InvalidEthAddress` | Invalid Ethereum address format. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 100 | `AssetNotRegistered` | Asset not registered. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |

<a id="bridge-validator-validatorerror"></a>
### `bridge::validator` — `ValidatorError`

Source: [`src/bridge/validator.rs`](../src/bridge/validator.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NotValidator` | Caller is not a validator | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 2 | `NotAuthorized` | Caller is not authorized admin | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 3 | `AlreadyConfirmed` | Already confirmed this transfer | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 4 | `TransferNotFound` | Transfer not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `InsufficientConfirmations` | Insufficient confirmations | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `AlreadyInitialized` | Validator set already initialized | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |
| 7 | `InvalidValidatorSet` | Invalid validator set (empty or too small) | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 8 | `ValidatorInactive` | Validator inactive for too long | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 9 | `CannotRemoveLastValidator` | Cannot remove last validator | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |

<a id="bug-bounty-payout-bountyerror"></a>
### `bug_bounty_payout` — `BountyError`

Source: [`src/bug_bounty_payout.rs`](../src/bug_bounty_payout.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AlreadyInitialized` | Already initialized | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |
| 2 | `InvalidSeverity` | Invalid severity | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 4 | `ReportNotFound` | Report not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `ReportAlreadyPaid` | Report already paid | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 6 | `DuplicateApproval` | Duplicate approval | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 7 | `InvalidAmount` | Invalid amount | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 8 | `TimelockActive` | Timelock active | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 9 | `EmergencyPaused` | Emergency paused | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 10 | `ApprovalThresholdInvalid` | Approval threshold invalid | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 11 | `TooManyReports` | Too many reports | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="cache-ttl-manager-cachettlerror"></a>
### `cache_ttl_manager` — `CacheTtlError`

Source: [`src/cache_ttl_manager.rs`](../src/cache_ttl_manager.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `CacheExpired` | Cache entry has expired. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 2 | `EntryNotFound` | Cache entry not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `InvalidTtl` | Invalid TTL value. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `Unauthorized` | Unauthorized cache operation. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 5 | `ValidationFailed` | Cache validation failed. | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |

<a id="clan-wars-warerror"></a>
### `clan_wars` — `WarError`

Source: [`src/clan_wars.rs`](../src/clan_wars.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `WarNotFound` | War not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `WarNotActive` | War not active | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 3 | `NotAllianceMember` | Not alliance member | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 4 | `AlreadyAtWar` | Already at war | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 5 | `InvalidAlliance` | Invalid alliance | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 6 | `NotEnoughVotes` | Not enough votes | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 7 | `CooldownActive` | Cooldown active | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 8 | `TerritoryNotOwned` | Territory not owned | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 9 | `AttackTooFrequent` | Attack too frequent | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 10 | `WarAlreadyEnded` | War already ended | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 11 | `NotWarParticipant` | Not war participant | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |

<a id="composability-examples-composabilityerror"></a>
### `composability_examples` — `ComposabilityError`

Source: [`src/composability_examples.rs`](../src/composability_examples.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidTarget` | Invalid target contract address. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `CallFailed` | Method call failed on external contract. | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |
| 3 | `InvalidResponse` | Response validation failed. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `InputTooLarge` | Input data exceeds maximum allowed size. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `ContractNotFound` | Contract not found or not callable. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 6 | `Unauthorized` | Unauthorized cross-contract call. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 7 | `Timeout` | Timeout during cross-contract invocation. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 8 | `InvalidMethod` | Invalid method name or parameters. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 9 | `Reentrancy` | A cross-contract call re-entered a guarded section. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |

<a id="config-updater-configerror"></a>
### `config_updater` — `ConfigError`

Source: [`src/config_updater.rs`](../src/config_updater.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NotInitialized` | Not initialized | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |
| 2 | `AlreadyInitialized` | Already initialized | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |
| 3 | `InvalidParam` | Param name not in the allowed-parameter whitelist | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 5 | `PendingApproval` | Change is proposed but waiting for enough approvals | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 6 | `TimeLockActive` | Change has approvals but the time lock has not yet expired | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 7 | `BatchTooLarge` | Batch exceeds MAX_BATCH_PARAMS | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 8 | `NoPendingUpdate` | No pending update for this param | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 9 | `AlreadyApproved` | Signer has already approved this param update | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |

<a id="constellation-mapper-constellationerror"></a>
### `constellation_mapper` — `ConstellationError`

Source: [`src/constellation_mapper.rs`](../src/constellation_mapper.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NoMatchFound` | No known constellation matched the observed pattern. | NotFound | Nothing is available for this request yet. Check the preconditions or wait for new state; no need to retry immediately. |
| 2 | `TooFewStars` | Star vector is below the minimum count. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `ImmutableRecord` | Attempted to mutate an immutable historical record. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 4 | `BurstTooLarge` | Requested burst size exceeds MAX_MATCH_BURST. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="content-tools-contenttoolserror"></a>
### `content_tools` — `ContentToolsError`

Source: [`src/content_tools.rs`](../src/content_tools.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ContentAlreadyExists` | Content already exists. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 2 | `ContentNotFound` | Content not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `Unauthorized` | Unauthorized action. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 4 | `InvalidContent` | Invalid content data. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `ContentLimitReached` | Content limit reached. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `InvalidRating` | Invalid rating. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `AlreadyVoted` | Already voted. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 8 | `UnderReview` | Content is under review. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 9 | `ContentRejected` | Content has been rejected. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 10 | `AlreadyInitialized` | Admin has already been set; set_admin is a one-time initializer (Issue #237). | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |

<a id="contract-versioning-versioningerror"></a>
### `contract_versioning` — `VersioningError`

Source: [`src/contract_versioning.rs`](../src/contract_versioning.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `IncompatibleVersion` | Target version is not supported. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 2 | `AlreadyMigrated` | Migration already completed for this version pair. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 3 | `MigrationInProgress` | Migration is still in progress. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 4 | `BatchTooLarge` | Batch size exceeds limit. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `NotAuthorized` | Caller is not authorized to trigger migration. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |

<a id="crafting-craftingerror"></a>
### `crafting` — `CraftingError`

Source: [`src/crafting.rs`](../src/crafting.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `RecipeLocked` | Recipe locked | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 2 | `InsufficientLevel` | Insufficient level | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 3 | `InsufficientResources` | Insufficient resources | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 4 | `RecipeNotFound` | Recipe not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `SpecializationAlreadyChosen` | Player already chose a specialization; it cannot be changed (Issue #266). | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 6 | `WrongSpecialization` | The recipe requires a specialization the player has not chosen. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `NodeNotFound` | The requested skill node does not exist. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 8 | `NodeAlreadyUnlocked` | The skill node was already unlocked. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 9 | `InsufficientSkillPoints` | Not enough skill points to unlock this node. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="daily-rewards-dailyrewarderror"></a>
### `daily_rewards` — `DailyRewardError`

Source: [`src/daily_rewards.rs`](../src/daily_rewards.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AlreadyClaimedToday` | The player already claimed during the current UTC day. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 2 | `ProfileNotFound` | No profile exists for the claiming address. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `ArithmeticOverflow` | Reward math overflowed (unreachable with the constants above, but the crate policy is that no balance-modifying path may wrap). | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 4 | `InvalidCalendarDay` | `calendar_day` outside `1..=CALENDAR_DAYS`. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="dao-daoerror"></a>
### `dao` — `DaoError`

Source: [`src/dao.rs`](../src/dao.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AlreadyInitialized` | Already initialized | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |
| 2 | `NotInitialized` | Not initialized | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |
| 3 | `ProposalNotFound` | Proposal not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `InsufficientThreshold` | Insufficient threshold | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `VotingNotActive` | Voting not active | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 6 | `AlreadyVoted` | Already voted | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 7 | `TimelockNotExpired` | Timelock not expired | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 8 | `QuorumNotMet` | Quorum not met | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 9 | `InvalidStatus` | Invalid status | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 10 | `NotProposer` | Not proposer | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 11 | `NotAdmin` | Not admin | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 12 | `UnauthorizedCaller` | Unauthorized caller | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 13 | `InvalidAmount` | Invalid amount | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 14 | `Overflow` | Overflow | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="data-exporter-exporterror"></a>
### `data_exporter` — `ExportError`

Source: [`src/data_exporter.rs`](../src/data_exporter.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ExportLimitExceeded` | Export limit exceeded | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 2 | `NotOptedIn` | Not opted in | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 3 | `ProfileNotFound` | Profile not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |

<a id="difficulty-curve-curveerror"></a>
### `difficulty_curve` — `CurveError`

Source: [`src/difficulty_curve.rs`](../src/difficulty_curve.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidLevel` | Invalid level | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 3 | `CurveLocked` | Curve locked | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 4 | `InvalidParameter` | Invalid parameter | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `InvalidValue` | Invalid value | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="difficulty-scaler-difficultyerror"></a>
### `difficulty_scaler` — `DifficultyError`

Source: [`src/difficulty_scaler.rs`](../src/difficulty_scaler.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidLevel` | Player level is invalid (0 or > 100). | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="emergency-controls-emergencyerror"></a>
### `emergency_controls` — `EmergencyError`

Source: [`src/emergency_controls.rs`](../src/emergency_controls.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ContractPaused` | Contract is currently paused — operation blocked. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 2 | `NotAdmin` | Caller is not an authorized admin. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 3 | `NotPaused` | Contract is not paused; cannot unpause. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 4 | `UnpauseDelayNotMet` | Time-delayed unpause window has not elapsed. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 5 | `AlreadyInitialized` | Admin set has already been initialized. | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |
| 6 | `EmptyAdminSet` | Admin list must contain at least one address. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="energy-manager-energyerror"></a>
### `energy_manager` — `EnergyError`

Source: [`src/energy_manager.rs`](../src/energy_manager.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InsufficientEnergy` | Insufficient energy | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 2 | `ShipNotFound` | Ship not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `InvalidAmount` | Invalid amount | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `EnergyOverflow` | Energy overflow | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `NegativeBalance` | Negative balance | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="entanglement-comms-entanglementerror"></a>
### `entanglement_comms` — `EntanglementError`

Source: [`src/entanglement_comms.rs`](../src/entanglement_comms.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `PairNotActive` | The entanglement pair has expired or was never active. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 2 | `NotAuthorized` | Caller is not a participant of this pair. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 3 | `PairNotFound` | Pair does not exist. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `SameShip` | A ship cannot be entangled with itself. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `BurstTooLarge` | Burst size exceeded MAX_MESSAGE_BURST. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `EmptyBatch` | Message batch is empty. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="environment-simulator-environmenterror"></a>
### `environment_simulator` — `EnvironmentError`

Source: [`src/environment_simulator.rs`](../src/environment_simulator.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidCondition` | Invalid condition | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `InvalidNebula` | Invalid nebula | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `SimulationFailed` | Simulation failed | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |

<a id="errors-bondingerror"></a>
### `errors` — `BondingError`

Source: [`src/errors.rs`](../src/errors.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 400 | `BondAlreadyExists` | Bond already exists between these two addresses. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 401 | `BondNotFound` | Bond not found for the given pair. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 402 | `NotBondedParty` | Only bonded parties may interact with this bond. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 403 | `InvalidBondState` | Bond is not in the expected state for this operation. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 404 | `InvalidYieldPercent` | yield_percentage must be in [1, 100]. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 405 | `InsufficientBalance` | Delegated yield amount exceeds the delegator's balance. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="errors-mintererror"></a>
### `errors` — `MinterError`

Source: [`src/errors.rs`](../src/errors.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 200 | `InvalidAmount` | amount must be > 0. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 201 | `RateLimitExceeded` | Caller exceeded the minting rate limit. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 202 | `NoLayoutForShip` | No nebula layout found for the given ship_id. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 203 | `NoResourceAtAnomaly` | The specified anomaly_index contains no mintable resource. | NotFound | Nothing is available for this request yet. Check the preconditions or wait for new state; no need to retry immediately. |
| 204 | `SupplyOverflow` | Requested mint would overflow u64 total supply. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="errors-nebulagenerror"></a>
### `errors` — `NebulaGenError`

Source: [`src/errors.rs`](../src/errors.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidShipId` | ship_id must be > 0. Received ship_id=0. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `InvalidRegionId` | region_id must be in [1, MAX_REGION_ID]. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `InvalidSeed` | seed cannot be all-zero bytes. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `LayoutNotFound` | No nebula layout exists for the given ship_id. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `AnomalyOutOfBounds` | anomaly_index >= layout.size. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="errors-ratelimiterror"></a>
### `errors` — `RateLimitError`

Source: [`src/errors.rs`](../src/errors.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 100 | `RateLimitExceeded` | Caller exceeded the allowed call rate for this operation. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 101 | `Unauthorized` | Only the contract admin may update rate limit configuration. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |

<a id="errors-shipregistryerror"></a>
### `errors` — `ShipRegistryError`

Source: [`src/errors.rs`](../src/errors.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 300 | `ShipAlreadyRegistered` | ship_id is already registered; duplicate registration rejected. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 301 | `ShipNotFound` | Ship not found for this ship_id. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 302 | `NotShipOwner` | Only the ship owner may upgrade it. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 303 | `MaxLevelReached` | Ship level is already at the maximum allowed level. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 304 | `NameTooLong` | ship name string exceeds the maximum allowed length. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="escrow-trader-escrowerror"></a>
### `escrow_trader` — `EscrowError`

Source: [`src/escrow_trader.rs`](../src/escrow_trader.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `TradeExpired` | Trade expired | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 2 | `AlreadyConfirmed` | Already confirmed | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 3 | `NotParticipant` | Not participant | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 4 | `EscrowNotFound` | Escrow not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `MaxEscrowsReached` | Max escrows reached | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `InvalidAssets` | Invalid assets | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `NotFullyConfirmed` | Not fully confirmed | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 8 | `Reentrancy` | A guarded section was re-entered (Issue #238). | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |

<a id="event-framework-eventframeworkerror"></a>
### `event_framework` — `EventFrameworkError`

Source: [`src/event_framework.rs`](../src/event_framework.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidEventType` | Invalid event type | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `LimitTooLarge` | Limit too large | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 3 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |

<a id="event-scheduler-eventerror"></a>
### `event_scheduler` — `EventError`

Source: [`src/event_scheduler.rs`](../src/event_scheduler.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `EventAlreadyPassed` | Event start time is in the past. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 2 | `EventNotFound` | Event not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `EventAlreadyExecuted` | Event already executed. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 4 | `EventNotReady` | Event not yet ready to execute. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 5 | `Unauthorized` | Unauthorized — admin only. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 6 | `TooManyActiveEvents` | Too many active events (max 20). | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 7 | `BurstLimitExceeded` | Burst limit exceeded. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 8 | `InvalidEventType` | Invalid event type. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 9 | `ChallengeNotFound` | Challenge not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 10 | `ChallengeExpired` | Challenge has expired. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 11 | `AlreadyClaimed` | Player has already claimed reward for this challenge. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 12 | `ChallengeNotComplete` | Player has not yet completed the challenge. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 13 | `TooManyChallenges` | Too many active challenges. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 14 | `RecurringNotDue` | Recurring event fired too recently; interval not yet elapsed. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 15 | `ChallengeNotStarted` | Challenge window has not opened yet. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 16 | `InvalidEventWindow` | Event window is invalid or falls outside the current season. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 17 | `EventCooldownActive` | The event category is still cooling down from a previous event. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 18 | `ExclusiveEventConflict` | The window overlaps an exclusive event (or this exclusive event overlaps another). | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 19 | `InvalidEventState` | The event is not in the state required for this operation. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 20 | `NoActiveSeason` | No season is running. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 21 | `TooManySeasonalEvents` | Too many seasonal events scheduled or active. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 22 | `InvalidRewardPool` | Reward pool must be positive and must not overflow. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 23 | `NoEventReward` | Player has no reward for this event. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 24 | `ClaimWindowClosed` | The reward claim window has closed. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |

<a id="exploration-heatmap-heatmaperror"></a>
### `exploration_heatmap` — `HeatmapError`

Source: [`src/exploration_heatmap.rs`](../src/exploration_heatmap.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `OutOfBounds` | Coordinates fall outside the exploration grid. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="fleet-manager-fleeterror"></a>
### `fleet_manager` — `FleetError`

Source: [`src/fleet_manager.rs`](../src/fleet_manager.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `FleetLimitExceeded` | Fleet limit exceeded | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 2 | `EmptyFleet` | Empty fleet | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 4 | `ShipNotFound` | Ship not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `ShipOwnershipMismatch` | Ship ownership mismatch | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 6 | `FleetNotFound` | Fleet not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 7 | `AlreadyInitialized` | Already initialized | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |

<a id="fractional-resources-fractionalerror"></a>
### `fractional_resources` — `FractionalError`

Source: [`src/fractional_resources.rs`](../src/fractional_resources.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ResourceNotFound` | Resource not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `InsufficientShares` | Not enough shares for operation. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 3 | `InvalidShareCount` | Invalid share count specified. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `ShareTooSmall` | Share size below minimum. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `NotOwner` | Not the owner of the resource/shares. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 6 | `ShareNotFound` | Share does not exist. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 7 | `IncompatibleShares` | Cannot merge incompatible shares. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 8 | `Unauthorized` | Unauthorized caller. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 9 | `AlreadyFractionalized` | Resource already fractionalized. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 10 | `MaxFractionsExceeded` | Maximum fractions per transaction exceeded. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="fraud-detection-frauderror"></a>
### `fraud_detection` — `FraudError`

Source: [`src/fraud_detection.rs`](../src/fraud_detection.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `PlayerBlocked` | Player is currently blocked from performing the requested action. | Authorization | The account is flagged or blocked. Contact an admin; do not retry automatically. |

<a id="gas-recovery-refunderror"></a>
### `gas_recovery` — `RefundError`

Source: [`src/gas_recovery.rs`](../src/gas_recovery.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NotEligibleForRefund` | Transaction is not eligible for a refund. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 2 | `AlreadyRefunded` | Refund already processed for this transaction. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 3 | `NotAuthorized` | Caller is not authorized to process refunds. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 4 | `BatchTooLarge` | Batch size exceeds limit. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `InvalidPercentage` | Invalid refund percentage. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="gas-sponsor-sponsorerror"></a>
### `gas_sponsor` — `SponsorError`

Source: [`src/gas_sponsor.rs`](../src/gas_sponsor.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AlreadySponsored` | Player has already been sponsored (one-time limit). | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 2 | `DailyCapReached` | Daily sponsorship cap has been reached. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 3 | `InsufficientFunds` | Insufficient funds in the sponsorship pool. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 4 | `Unauthorized` | Unauthorized caller (not admin). | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 5 | `ProfileNotVerified` | Player profile not verified (must initialize profile first). | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 6 | `InvalidAmount` | Invalid amount specified. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `NotInitialized` | Sponsorship not initialized. | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |
| 8 | `PerUserCapReached` | Per-user lifetime sponsorship cap reached. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 9 | `PerUserDailyCapReached` | Per-user daily sponsorship cap reached. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 10 | `SessionKeyInvalid` | Session key expired or invalid. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 11 | `FraudDetected` | Fraud detection threshold exceeded. | Authorization | The account is flagged or blocked. Contact an admin; do not retry automatically. |
| 12 | `InvalidSignature` | Meta-transaction invalid signature. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="gifting-system-gifterror"></a>
### `gifting_system` — `GiftError`

Source: [`src/gifting_system.rs`](../src/gifting_system.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ZeroAmount` | Zero amount | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `InsufficientBalance` | Insufficient balance | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 3 | `GiftNotFound` | Gift not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `GiftExpired` | Gift expired | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 5 | `NotReceiver` | Not receiver | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 6 | `GiftAlreadyClaimed` | Gift already claimed | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 7 | `SelfGift` | Self gift | Validation | The action targets the caller itself. Pick a different counterparty. |
| 8 | `BurstLimitExceeded` | Burst limit exceeded | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="governance-goverror"></a>
### `governance` — `GovError`

Source: [`src/governance.rs`](../src/governance.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `VotingClosed` | Voting closed | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 2 | `AlreadyVoted` | Already voted | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 3 | `ProposalNotFound` | Proposal not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `QuorumNotMet` | Quorum not met | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 5 | `InsufficientEssence` | Insufficient essence | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `NotDao` | Not dao | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 7 | `NotAdmin` | Not admin | Authorization | Sign with the account that holds the required role or ownership; check role grants. |

<a id="guild-economy-guildeconomyerror"></a>
### `guild_economy` — `GuildEconomyError`

Source: [`src/guild_economy.rs`](../src/guild_economy.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ProposalNotFound` | Proposal not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `AlreadyVoted` | Already voted | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 3 | `ProposalExpired` | Proposal expired | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 4 | `ProposalNotExpired` | Proposal not expired | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 5 | `InsufficientTreasuryFunds` | Insufficient treasury funds | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `ProposalAlreadyExecuted` | Proposal already executed | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 7 | `NotAllianceMember` | Not alliance member | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 8 | `ThresholdNotMet` | Threshold not met | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |

<a id="guild-quests-guildquesterror"></a>
### `guild_quests` — `GuildQuestError`

Source: [`src/guild_quests.rs`](../src/guild_quests.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `QuestNotFound` | Quest not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `QuestExpired` | Quest expired | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 3 | `QuestAlreadyCompleted` | Quest already completed | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 4 | `NotAllianceMember` | Not alliance member | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 5 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |

<a id="health-monitor-healtherror"></a>
### `health_monitor` — `HealthError`

Source: [`src/health_monitor.rs`](../src/health_monitor.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `MetricBurstExceeded` | Metric burst exceeded | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 2 | `EmptyMetricBatch` | Empty metric batch | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="indexer-callbacks-indexererror"></a>
### `indexer_callbacks` — `IndexerError`

Source: [`src/indexer_callbacks.rs`](../src/indexer_callbacks.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidCallback` | Invalid callback | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 3 | `RateLimitExceeded` | Rate limit exceeded | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="input-validation-validationerror"></a>
### `input_validation` — `ValidationError`

Source: [`src/input_validation.rs`](../src/input_validation.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 80 | `StringTooLong` | String exceeds maximum allowed length. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 81 | `EmptyString` | String is empty when a value is required. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 82 | `InvalidUtf8` | String contains invalid UTF-8 encoding. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 83 | `InvalidCharacters` | String contains control characters or null bytes. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 84 | `InvalidCidFormat` | IPFS CID format is invalid. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="leaderboards-leaderboarderror"></a>
### `leaderboards` — `LeaderboardError`

Source: [`src/leaderboards.rs`](../src/leaderboards.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidCategory` | Category does not exist. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `InvalidTimePeriod` | Time period is not recognized. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `InvalidRegion` | Region is not valid. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `PlayerNotFound` | Player not found in leaderboard. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `Unauthorized` | Unauthorized admin action. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 6 | `LeaderboardFull` | Max leaderboard entries exceeded. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 7 | `ResetNotDue` | Reset is not yet due. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 8 | `AlreadyInitialized` | Admin has already been set; set_admin is a one-time initializer (Issue #237). | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |

<a id="loot-system-looterror"></a>
### `loot_system` — `LootError`

Source: [`src/loot_system.rs`](../src/loot_system.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AlreadyInitialized` | Admin already set — `set_loot_admin` is a one-time initializer. | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |
| 2 | `Unauthorized` | Caller is not the loot admin. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 3 | `BoxTypeNotFound` | Loot box type not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `InvalidOddsTable` | Odds table is empty, or its weights don't sum to exactly 10_000 bps. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `InsufficientLootTokens` | Player doesn't have enough `LootToken` to open this box. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `RequestNotFound` | Open request not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 7 | `AlreadyRevealed` | This request was already revealed. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 8 | `SeedMismatch` | The revealed seed doesn't hash to the value committed at open time. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 9 | `NotRequestOwner` | Caller doesn't own this open request. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |

<a id="market-oracle-oracleerror"></a>
### `market_oracle` — `OracleError`

Source: [`src/market_oracle.rs`](../src/market_oracle.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 2 | `StalePrice` | Stale price | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 3 | `InvalidPrice` | Invalid price | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `ResourceNotFound` | Resource not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `TooManyUpdates` | Too many updates | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `NoOracleSources` | No oracle sources | NotFound | Nothing is available for this request yet. Check the preconditions or wait for new state; no need to retry immediately. |
| 7 | `EventTriggerNotFound` | Event trigger not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 8 | `InvalidTriggerPrice` | Invalid trigger price | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="metadata-resolver-metadataerror"></a>
### `metadata_resolver` — `MetadataError`

Source: [`src/metadata_resolver.rs`](../src/metadata_resolver.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidCID` | CID bytes are empty or invalid. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `TokenNotFound` | No metadata stored for the given token ID. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `AlreadySet` | Metadata has already been set and is immutable after first set. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 4 | `BatchLimitExceeded` | Batch size exceeds the maximum of 10. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `GasBudgetExceeded` | Estimated gas for the batch exceeds the caller's gas budget. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="metrics-exporter-metricserror"></a>
### `metrics_exporter` — `MetricsError`

Source: [`src/metrics_exporter.rs`](../src/metrics_exporter.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `Unauthorized` | Unauthorized metrics access. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 2 | `NotInitialized` | Metrics not initialized. | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |
| 3 | `InvalidMetric` | Invalid metric value. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="migration-framework-migrationerror"></a>
### `migration_framework` — `MigrationError`

Source: [`src/migration_framework.rs`](../src/migration_framework.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `MigrationInProgress` | Migration already in progress. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 2 | `IncompatibleSchema` | Schema version incompatible. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `ValidationFailed` | Data validation failed during migration. | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |
| 4 | `BatchTooLarge` | Batch size exceeds maximum. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `Unauthorized` | Unauthorized caller. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 6 | `RollbackFailed` | Rollback failed. | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |
| 7 | `NoCheckpoint` | No rollback checkpoint available. | NotFound | Nothing is available for this request yet. Check the preconditions or wait for new state; no need to retry immediately. |
| 8 | `MigrationNotFound` | Migration not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |

<a id="mini-games-minigameerror"></a>
### `mini_games` — `MiniGameError`

Source: [`src/mini_games.rs`](../src/mini_games.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `GameNotFound` | Game not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `GameFull` | Game full | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 3 | `NotActive` | Not active | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 4 | `AlreadyPlayed` | Already played | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 5 | `CooldownActive` | Cooldown active | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 6 | `NotEnoughResources` | Not enough resources | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 7 | `DailyLimitReached` | Daily limit reached | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 8 | `InvalidMove` | Invalid move | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 9 | `LeaderboardFull` | Leaderboard full | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="mission-generator-missionerror"></a>
### `mission_generator` — `MissionError`

Source: [`src/mission_generator.rs`](../src/mission_generator.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `MissionAlreadyClaimed` | Mission already claimed | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 2 | `InvalidMission` | Invalid mission | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `DailyLimitReached` | Daily limit reached | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 4 | `NotCompleted` | Not completed | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 5 | `ProfileNotFound` | Profile not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |

<a id="mobile-views-mobileviewerror"></a>
### `mobile_views` — `MobileViewError`

Source: [`src/mobile_views.rs`](../src/mobile_views.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ShipNotFound` | No ship with the given ID exists on-chain. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |

<a id="navigation-planner-naverror"></a>
### `navigation_planner` — `NavError`

Source: [`src/navigation_planner.rs`](../src/navigation_planner.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NotInitialized` | Not initialized | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |
| 2 | `AlreadyInitialized` | Already initialized | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |
| 3 | `SameNebula` | start == dest | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `NoValidRoute` | No path exists within the hop limit | NotFound | Nothing is available for this request yet. Check the preconditions or wait for new state; no need to retry immediately. |
| 5 | `TooManyHops` | Provided route exceeds max_hops | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `RouteEmpty` | Route Vec is empty | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `InvalidNebula` | Nebula ID referenced but has no registered connections | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 8 | `BatchTooLarge` | Batch size exceeds MAX_CONNECTIONS_PER_BATCH | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="nebula-archive-archiveerror"></a>
### `nebula_archive` — `ArchiveError`

Source: [`src/nebula_archive.rs`](../src/nebula_archive.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ArchiveNotFound` | Archive not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `BurstLimitExceeded` | Burst limit exceeded | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="nebula-gen-nebulaerror"></a>
### `nebula_gen` — `NebulaError`

Source: [`src/nebula_gen.rs`](../src/nebula_gen.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NotInitialized` | Not initialized | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |
| 2 | `AlreadyInitialized` | Already initialized | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |
| 3 | `InvalidSeed` | Seed is degenerate (all-zero bytes). | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `InvalidIndex` | Requested anomaly index is out of bounds for this layout. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `LayoutNotFound` | No active (non-expired) layout found for the given ship. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 6 | `InvalidSize` | Requested nebula size is outside the configured [min_size, max_size] range. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `InvalidTtl` | Provided layout TTL is zero / invalid. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 8 | `InvalidShipId` | ship_id must be greater than zero. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 9 | `InvalidRegionId` | region_id must be between 1 and MAX_REGION_ID (inclusive). | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 10 | `AnomalyOutOfBounds` | Anomaly index is out of bounds for this layout. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 11 | `RateLimitExceeded` | Caller exceeded the layout-generation rate limit (DoS prevention). | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="nft-marketplace-marketplaceerror"></a>
### `nft_marketplace` — `MarketplaceError`

Source: [`src/nft_marketplace.rs`](../src/nft_marketplace.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AlreadyListed` | Already listed | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 2 | `NotListed` | Not listed | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 3 | `NotSeller` | Not seller | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 4 | `InvalidPrice` | Invalid price | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `SellerListingCapReached` | Seller listing cap reached | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `SelfPurchase` | Self purchase | Validation | The action targets the caller itself. Pick a different counterparty. |
| 7 | `SkinNotFound` | No skin exists with the given ID. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 8 | `NotSkinOwner` | The seller does not own the skin they are listing. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 9 | `SkinNotTradeable` | The skin is escrowed by another listing or otherwise locked. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 10 | `PriceBelowRarityFloor` | Price is below the floor for the cosmetic's rarity. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 11 | `RoyaltyTooHigh` | Requested royalty exceeds [`MAX_CREATOR_ROYALTY_BPS`]. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 12 | `RoyaltyAlreadyRegistered` | A royalty is already registered for this cosmetic. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 13 | `NothingToWithdraw` | The creator has no unwithdrawn royalties. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 14 | `ArithmeticOverflow` | Fee or royalty accounting overflowed. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="nomad-bonding-bonderror"></a>
### `nomad_bonding` — `BondError`

Source: [`src/nomad_bonding.rs`](../src/nomad_bonding.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `SelfBond` | A player attempted to bond with themselves. | Validation | The action targets the caller itself. Pick a different counterparty. |
| 2 | `BondNotFound` | No bond exists for the supplied id. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `NotDesignatedPartner` | Caller is not the bond's designated partner. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 4 | `BondNotPending` | Bond is not in `Pending` status. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 5 | `InvalidPercentage` | Delegation percentage is outside the 1..=100 range. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 6 | `BondNotActive` | Bond is not in `Active` status. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 7 | `NotBondMember` | Caller is not a member of the bond. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 8 | `NoDelegation` | No yield delegation is configured for the bond. | NotFound | Nothing is available for this request yet. Check the preconditions or wait for new state; no need to retry immediately. |
| 9 | `NotBeneficiary` | Caller is not the delegation's beneficiary. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 10 | `AlreadyDissolved` | Bond has already been dissolved. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 11 | `NotBondParty` | Caller is not a party to the bond. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 12 | `ArithmeticOverflow` | A checked arithmetic operation overflowed. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 13 | `Reentrancy` | A guarded section was re-entered (Issue #238). | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |

<a id="offline-progress-offlineerror"></a>
### `offline_progress` — `OfflineError`

Source: [`src/offline_progress.rs`](../src/offline_progress.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NoAccrualAvailable` | No accrual available | NotFound | Nothing is available for this request yet. Check the preconditions or wait for new state; no need to retry immediately. |
| 2 | `NotInitialized` | Not initialized | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |

<a id="onboarding-tutorial-onboardingerror"></a>
### `onboarding_tutorial` — `OnboardingError`

Source: [`src/onboarding_tutorial.rs`](../src/onboarding_tutorial.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AlreadyInitialized` | Already initialized | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |
| 2 | `ProfileAlreadyExists` | Profile already exists | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 3 | `ProfileNotFound` | Profile not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `TutorialAlreadyStarted` | Tutorial already started | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 5 | `TutorialNotStarted` | Tutorial not started | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 6 | `InvalidStep` | Invalid step | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `StepOutOfOrder` | Step out of order | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 8 | `StepAlreadyCompleted` | Step already completed | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 9 | `TutorialAlreadyCompleted` | Tutorial already completed | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 10 | `InvalidPath` | Invalid path | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 11 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |

<a id="player-profile-profileerror"></a>
### `player_profile` — `ProfileError`

Source: [`src/player_profile.rs`](../src/player_profile.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ProfileNotFound` | Profile not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `ProfileAlreadyExists` | Profile already exists | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 3 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 4 | `BatchTooLarge` | Batch too large | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `ArithmeticOverflow` | A balance-modifying operation would have wrapped. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="player-segmentation-segmentationerror"></a>
### `player_segmentation` — `SegmentationError`

Source: [`src/player_segmentation.rs`](../src/player_segmentation.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidSegment` | Invalid segment ID provided. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `PlayerNotFound` | Player not found in segment mapping. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |

<a id="portal-registry-portalerror"></a>
### `portal_registry` — `PortalError`

Source: [`src/portal_registry.rs`](../src/portal_registry.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `PortalUnstable` | Portal stability is below threshold — travel refused. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 2 | `NotAuthorized` | Caller is not the portal owner or admin. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 3 | `PortalNotFound` | Portal does not exist. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `BatchTooLarge` | Batch size exceeded MAX_PORTALS_PER_TX. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `SameNebula` | Source and target nebula IDs must differ. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="privacy-stats-privacyerror"></a>
### `privacy_stats` — `PrivacyError`

Source: [`src/privacy_stats.rs`](../src/privacy_stats.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NotOptedIn` | Player has not opted in to privacy features. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 2 | `InvalidProof` | Invalid proof provided for verification. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `CommitmentNotFound` | Commitment not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `BurstLimitExceeded` | Burst limit exceeded (max 10 commitments per tx). | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `CommitmentExists` | Commitment already exists for this stat type. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |

<a id="prize-distributor-prizeerror"></a>
### `prize_distributor` — `PrizeError`

Source: [`src/prize_distributor.rs`](../src/prize_distributor.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InsufficientPrizePool` | Prize pool has insufficient funds for the requested distribution. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 2 | `NotAuthorized` | Caller is not authorized to perform this action. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 3 | `TooManyPositions` | Requested top-N exceeds the maximum payout positions. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 4 | `NoSnapshot` | No leaderboard snapshot is available. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `InvalidAmount` | Amount must be positive. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 6 | `InvalidRank` | Snapshot rank out of range. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="proxy-proxyerror"></a>
### `proxy` — `ProxyError`

Source: [`src/proxy.rs`](../src/proxy.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NotAdmin` | Caller is not the authorized admin. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 2 | `AlreadyInitialized` | `initialize` has already been called. | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |
| 3 | `NoPendingUpgrade` | No pending upgrade exists; call `authorize_upgrade` first. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `MigrationInProgress` | A migration is already in progress. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 5 | `NoRollbackTarget` | No previous version to roll back to. | NotFound | Nothing is available for this request yet. Check the preconditions or wait for new state; no need to retry immediately. |
| 6 | `NotInitialized` | The contract has not been initialized yet. | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |

<a id="pvp-combat-pvperror"></a>
### `pvp_combat` — `PvPError`

Source: [`src/pvp_combat.rs`](../src/pvp_combat.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `PlayerNotFound` | Player not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `ChallengeAlreadyExists` | Challenge already exists. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 3 | `ChallengeNotFound` | Challenge not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `Unauthorized` | Not authorized. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 5 | `InvalidCombatParams` | Invalid combat parameters. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 6 | `AlreadyInCombat` | Player already in combat. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 7 | `CombatNotFound` | Combat not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 8 | `InvalidMove` | Invalid move. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 9 | `EloUpdateFailed` | ELO rating update failed. | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |
| 10 | `QueueFull` | Matchmaking queue full. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 11 | `NotInQueue` | Player not in queue. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 12 | `SpectatorLimitReached` | Spectator limit reached. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 13 | `AlreadyInitialized` | Admin has already been set; set_admin is a one-time initializer (Issue #237). | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |

<a id="quest-system-questerror"></a>
### `quest_system` — `QuestError`

Source: [`src/quest_system.rs`](../src/quest_system.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ChainNotFound` | No chain with the given ID. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `QuestNotFound` | No node with the given quest ID. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `QuestNotStarted` | The player has no state for this quest. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 4 | `InvalidStatus` | The quest is not in a state that allows this operation. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 5 | `QuestExpired` | The quest deadline has passed. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 6 | `AlreadyClaimed` | The reward for this quest was already taken. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 7 | `ChainFull` | Chain already holds [`MAX_CHAIN_LENGTH`] nodes. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 8 | `TooManyBranches` | Node declares more than [`MAX_BRANCHES_PER_NODE`] branches. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 9 | `InvalidBranch` | `choice_id` is not offered by this node. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 10 | `TooManyActiveQuests` | The player already has [`MAX_ACTIVE_QUESTS`] quests active. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 11 | `Unauthorized` | Only the chain's creator may extend it. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 12 | `ChainAlreadyStarted` | The player already started this chain. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 13 | `InvalidTarget` | `target_count` must be greater than zero. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 14 | `ProfileNotFound` | No profile exists for the player, so rewards cannot be credited. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 15 | `ArithmeticOverflow` | Reward accounting overflowed. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 16 | `DanglingBranch` | A branch points at a node that does not exist. | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |

<a id="randomness-oracle-oracleerror"></a>
### `randomness_oracle` — `OracleError`

Source: [`src/randomness_oracle.rs`](../src/randomness_oracle.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `SeedInvalid` | The provided seed failed validation. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `FallbackDepleted` | Fallback mechanism depleted. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="rate-limiter-ratelimiterror"></a>
### `rate_limiter` — `RateLimitError`

Source: [`src/rate_limiter.rs`](../src/rate_limiter.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 100 | `RateLimitExceeded` | Caller has exceeded the allowed call rate for this operation. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 101 | `Unauthorized` | Only the contract admin may update rate limit configuration. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 102 | `InvalidConfig` | `max_calls` and `window_seconds` must both be non-zero. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="realtime-events-realtimeerror"></a>
### `realtime_events` — `RealtimeError`

Source: [`src/realtime_events.rs`](../src/realtime_events.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 110 | `EventNotFound` | Event not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 111 | `EventNotActive` | Event is not active. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 112 | `EventFull` | Event is full. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 113 | `AlreadyParticipating` | Player already in event. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 114 | `NotParticipating` | Player not in event. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 115 | `Unauthorized` | Unauthorized action. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 116 | `TooManyEvents` | Too many simultaneous events. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 117 | `InvalidEventType` | Invalid event type. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 118 | `BossDefeated` | Boss already defeated. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 119 | `ChallengeCompleted` | Challenge already completed. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 120 | `InvalidContribution` | Invalid contribution amount. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 121 | `RewardAlreadyClaimed` | Reward already claimed. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 122 | `EventEnded` | Event has ended. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |

<a id="recipes-recipeerror"></a>
### `recipes` — `RecipeError`

Source: [`src/recipes.rs`](../src/recipes.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `RecipeNotFound` | Recipe not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |

<a id="recycling-crafter-recyclingerror"></a>
### `recycling_crafter` — `RecyclingError`

Source: [`src/recycling_crafter.rs`](../src/recycling_crafter.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidRecipe` | Recipe does not exist. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `InvalidInputs` | Inputs do not match recipe requirements. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `BatchTooLarge` | Batch size exceeds limit. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 4 | `InsufficientResources` | Insufficient resources to craft. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `AlreadyCrafted` | Crafted item already exists (idempotency guard). | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |

<a id="reentrancy-guard-reentrancyerror"></a>
### `reentrancy_guard` — `ReentrancyError`

Source: [`src/reentrancy_guard.rs`](../src/reentrancy_guard.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ReentrantCall` | A guarded section was entered while another was still in progress. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |

<a id="referral-system-referralerror"></a>
### `referral_system` — `ReferralError`

Source: [`src/referral_system.rs`](../src/referral_system.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AlreadyReferred` | Already referred | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 2 | `SelfReferral` | Self referral | Validation | The action targets the caller itself. Pick a different counterparty. |
| 3 | `ReferralNotFound` | Referral not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `AlreadyClaimed` | Already claimed | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 5 | `FirstScanNotDone` | First scan not done | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 6 | `DailyClaimCapReached` | Daily claim cap reached | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 7 | `InsufficientRewardPool` | Insufficient reward pool | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="referral-system-referralv2error"></a>
### `referral_system` — `ReferralV2Error`

Source: [`src/referral_system.rs`](../src/referral_system.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 10 | `PlayerBlocked` | Player is blocked due to fraud detection. | Authorization | The account is flagged or blocked. Contact an admin; do not retry automatically. |
| 11 | `VelocityTooHigh` | Referral velocity too high — possible fraud. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 12 | `TierNotFound` | Tier not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |

<a id="reputation-reputationerror"></a>
### `reputation` — `ReputationError`

Source: [`src/reputation.rs`](../src/reputation.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 2 | `ReputationNotFound` | Reputation not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `InvalidScore` | Invalid score | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `ReportNotFound` | Report not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `MaxReportsExceeded` | Max reports exceeded | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `InvalidBehavior` | Invalid behavior | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `AlreadyBanned` | Already banned | Authorization | The account is flagged or blocked. Contact an admin; do not retry automatically. |
| 8 | `NotInitialized` | Not initialized | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |

<a id="resource-minter-mintererror"></a>
### `resource_minter` — `MinterError`

Source: [`src/resource_minter.rs`](../src/resource_minter.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 200 | `InvalidAmount` | Amount must be > 0. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 201 | `RateLimitExceeded` | Caller exceeded the minting rate limit (DoS prevention). | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 202 | `NoLayoutForShip` | No nebula layout found for this ship (must scan first). | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 203 | `NoResourceAtAnomaly` | The specified anomaly index does not contain a resource. | NotFound | Nothing is available for this request yet. Check the preconditions or wait for new state; no need to retry immediately. |
| 204 | `ArithmeticOverflow` | A checked arithmetic operation overflowed (Issue #239). | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 205 | `InsufficientBalance` | The account holds less than the requested debit amount (Issue #281). | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="resource-minter-harvesterror"></a>
### `resource_minter` — `HarvestError`

Source: [`src/resource_minter.rs`](../src/resource_minter.rs) · Implements `StandardContractError`. Re-exported by `dex_integration`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ShipNotFound` | No ship with the given ID exists. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `EmptyHarvest` | The layout yielded no resources (all cells empty or non-resource). | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `InvalidPrice` | `min_price` was zero or negative. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `AssetNotHarvested` | The requested asset was not present in this harvest. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `PriceOverflow` | A checked arithmetic operation overflowed (Issue #239). | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |
| 6 | `DexFailure` | Generic DEX failure: unknown offer, already cancelled, or rate-limited. | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |
| 7 | `InsufficientBalance` | Seller does not hold enough of `resource` to cover the listing. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="revenue-attribution-attributionerror"></a>
### `revenue_attribution` — `AttributionError`

Source: [`src/revenue_attribution.rs`](../src/revenue_attribution.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ZeroAmount` | Revenue amount must be greater than zero. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="rewards-rewarderror"></a>
### `rewards` — `RewardError`

Source: [`src/rewards.rs`](../src/rewards.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidCode` | Invalid referral code | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `CodeExists` | Code already exists | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 3 | `ReferrerNotFound` | Referrer not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `NoRewardsToClaim` | No rewards to claim | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `AlreadyClaimed` | Already claimed this period | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 6 | `SuspiciousActivity` | Suspicious activity detected | Authorization | The account is flagged or blocked. Contact an admin; do not retry automatically. |
| 7 | `SelfReferral` | Self-referral not allowed | Validation | The action targets the caller itself. Pick a different counterparty. |
| 8 | `InvalidTier` | Invalid tier | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="seasons-seasonerror"></a>
### `seasons` — `SeasonError`

Source: [`src/seasons.rs`](../src/seasons.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NoActiveSeason` | No active season | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `SeasonAlreadyStarted` | Season already started | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 3 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 4 | `SeasonNotExpired` | Season not expired | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 5 | `NoRewardToClaim` | No reward to claim | NotFound | Nothing is available for this request yet. Check the preconditions or wait for new state; no need to retry immediately. |
| 6 | `ChapterNotReady` | Chapter advance attempted but season has not progressed far enough. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 7 | `AllChaptersDone` | All 3 chapters are already complete. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |

<a id="session-manager-sessionerror"></a>
### `session_manager` — `SessionError`

Source: [`src/session_manager.rs`](../src/session_manager.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `SessionNotFound` | Session not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `SessionExpired` | Session expired | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 3 | `TooManySessions` | Too many sessions | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 4 | `NotOwner` | Not owner | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |

<a id="shared-lib-sharederror"></a>
### `shared_lib` — `SharedError`

Source: [`src/shared_lib.rs`](../src/shared_lib.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidAddress` | Invalid address | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `MathOverflow` | Math overflow | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 3 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |

<a id="ship-customization-skinerror"></a>
### `ship_customization` — `SkinError`

Source: [`src/ship_customization.rs`](../src/ship_customization.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `SkinNotFound` | Skin not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `NotOwner` | Not owner | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 3 | `AlreadyApplied` | Already applied | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 4 | `InvalidRarity` | Invalid rarity | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `SkinLimitReached` | Skin limit reached | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `SkinPackEmpty` | Skin pack empty | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `InvalidFusion` | Invalid fusion | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 8 | `FusionLevelMax` | Fusion level max | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 9 | `NotEnoughSkins` | Not enough skins | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 10 | `AuctionNotFound` | Auction not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 11 | `AuctionNotActive` | Auction not active | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 12 | `BidTooLow` | Bid too low | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 13 | `NotHighestBidder` | Not highest bidder | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |

<a id="ship-nft-shiperror"></a>
### `ship_nft` — `ShipError`

Source: [`src/ship_nft.rs`](../src/ship_nft.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ShipAlreadyExists` | Ship ID unexpectedly already exists. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 2 | `ShipNotFound` | Ship with the given ID does not exist. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `NotOwner` | Caller is not the owner of the ship. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 4 | `SameOwner` | Cannot transfer to the current owner. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `BatchLimitExceeded` | Batch mint exceeds the maximum of 3 ships per transaction. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `InvalidShipType` | Invalid ship type provided. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 7 | `ReentrancyDetected` | Reentrancy guard is active. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 8 | `InvalidMetadataUri` | Metadata URI must use a marketplace-compatible URI scheme. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="ship-upgrade-shipupgradeerror"></a>
### `ship_upgrade` — `ShipUpgradeError`

Source: [`src/ship_upgrade.rs`](../src/ship_upgrade.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 200 | `NotInitialized` | Not initialized | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |
| 201 | `AlreadyInitialized` | Already initialized | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |
| 202 | `InsufficientResources` | Insufficient resources | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 203 | `UnknownComponent` | Unknown component | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 204 | `InvariantViolation` | Invariant violated: module cap or mass limit exceeded. | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |
| 205 | `BatchTooLarge` | Batch too large | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 206 | `InvalidShipId` | Ship ID must be greater than zero. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 207 | `EmptyBatch` | A batch must contain at least one component. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 208 | `InvalidBlueprint` | The blueprint map must contain at least one component. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 209 | `RateLimitExceeded` | Caller exceeded the ship-upgrade rate limit (DoS prevention). | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="smart-alerts-alerterror"></a>
### `smart_alerts` — `AlertError`

Source: [`src/smart_alerts.rs`](../src/smart_alerts.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ThresholdNotConfigured` | No threshold has been configured for this metric. | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |
| 2 | `AlertNotFound` | Referenced alert does not exist. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |

<a id="soul-binding-bindingerror"></a>
### `soul_binding` — `BindingError`

Source: [`src/soul_binding.rs`](../src/soul_binding.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AlreadyBound` | Already bound | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 2 | `NotOwner` | Not owner | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 3 | `BurstLimitExceeded` | Burst limit exceeded | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="staking-stakingerror"></a>
### `staking` — `StakingError`

Source: [`src/staking.rs`](../src/staking.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `AlreadyInitialized` | Already initialized | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |
| 2 | `NotInitialized` | Not initialized | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |
| 3 | `InvalidAmount` | Invalid amount | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `InsufficientBalance` | Insufficient balance | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `NoActiveStake` | No active stake | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 6 | `TimeLockActive` | Time lock active | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 7 | `StakeTooYoung` | Stake too young | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 8 | `NoActiveDelegation` | No active delegation | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 9 | `CircularDelegation` | Circular delegation | Validation | The action targets the caller itself. Pick a different counterparty. |
| 10 | `ActiveVotes` | Active votes | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 11 | `SelfDelegation` | Self delegation | Validation | The action targets the caller itself. Pick a different counterparty. |
| 12 | `InvalidTier` | Invalid tier | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 13 | `EarlyWithdrawPenalty` | Early withdraw penalty | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 14 | `PenaltyBelowMinimum` | Penalty below minimum | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 15 | `TierLocked` | Tier locked | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |

<a id="state-snapshot-snapshoterror"></a>
### `state_snapshot` — `SnapshotError`

Source: [`src/state_snapshot.rs`](../src/state_snapshot.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ShipNotFound` | Ship not found in storage. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `SnapshotNotFound` | Snapshot with the given ID does not exist. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `NotOwner` | Caller is not the owner of the ship. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 4 | `SnapshotInvalid` | Snapshot integrity check failed (hash mismatch). | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `SessionLimitExceeded` | Session snapshot limit exceeded. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `TooSoon` | Auto-snapshot interval has not elapsed. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 7 | `SnapshotImmutable` | Snapshot is immutable and cannot be modified. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 8 | `BackupTooSoon` | Backup interval not elapsed yet. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 9 | `BackupLimitReached` | Maximum backup retention limit reached. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="storage-optim-storageerror"></a>
### `storage_optim` — `StorageError`

Source: [`src/storage_optim.rs`](../src/storage_optim.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `ReentrancyDetected` | Re-entrancy detected — a mutating function is already in progress. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 2 | `EntryNotFound` | Entry not found for the given key. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `BurstLimitExceeded` | Burst read limit exceeded. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 4 | `InvalidTtl` | Invalid TTL value supplied. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `Unauthorized` | Caller is not authorized. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 6 | `InvalidKey` | Invalid key supplied. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="sustainability-metrics-sustainabilityerror"></a>
### `sustainability_metrics` — `SustainabilityError`

Source: [`src/sustainability_metrics.rs`](../src/sustainability_metrics.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `NoRewardEligible` | No reward eligible | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 2 | `InvalidGasValue` | Invalid gas value | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |

<a id="theme-customizer-themeerror"></a>
### `theme_customizer` — `ThemeError`

Source: [`src/theme_customizer.rs`](../src/theme_customizer.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidTheme` | Invalid theme | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `Unauthorized` | Unauthorized | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 3 | `ShipNotFound` | Ship not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |

<a id="token-burning-burningerror"></a>
### `token_burning` — `BurningError`

Source: [`src/token_burning.rs`](../src/token_burning.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidAmount` | Burn amount must be greater than zero. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `InsufficientBalance` | Holder does not have enough of the resource to burn. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 3 | `ArithmeticOverflow` | Burn accounting overflowed. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 4 | `FeeRateTooHigh` | Requested fee rate exceeds [`MAX_BURN_FEE_BPS`]. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `Unauthorized` | Caller is not the configured fee admin. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 6 | `AlreadyInitialized` | The fee admin has already been set and cannot be re-initialized. | Conflict | Already initialised. Skip the call; do not re-initialise a deployed contract. |

<a id="tournament-tournamenterror"></a>
### `tournament` — `TournamentError`

Source: [`src/tournament.rs`](../src/tournament.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidPlayerCount` | `max_players` must be a power of two in [4, 64]. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `InvalidPrizeDistribution` | Prize distribution basis points don't sum to <= 10_000, or the list is longer than the number of possible placements. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `TournamentNotFound` | Tournament not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `Unauthorized` | Not authorized (not the organizer/admin). | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 5 | `RegistrationClosed` | Registration window has closed, or the tournament isn't in the registration phase. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 6 | `AlreadyRegistered` | Player already registered for this tournament. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 7 | `TournamentFull` | Tournament already at max capacity. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 8 | `NotEnoughRegistrants` | Fewer than 2 players registered — can't start a bracket. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 9 | `NotInRegistration` | Tournament isn't in the registration phase (can't start it, or can't register once it's left that phase). | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 10 | `TournamentNotActive` | Tournament isn't currently active (bracket in progress). | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 11 | `InvalidMatch` | Round/match index out of range for this tournament's bracket. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 12 | `MatchNotReady` | The underlying combat for this match hasn't finished yet. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 13 | `MatchAlreadyResolved` | This match already has a recorded winner. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 14 | `PlayerNotInMatch` | Player isn't part of this specific match. | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 15 | `TournamentAlreadyDone` | Tournament already completed. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |

<a id="trading-ammerror"></a>
### `trading` — `AmmError`

Source: [`src/trading.rs`](../src/trading.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 100 | `PoolNotFound` | Pool not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 101 | `PoolAlreadyExists` | Pool already exists | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 102 | `InsufficientLiquidity` | Insufficient liquidity | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 103 | `InvalidAmount` | Invalid amount | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 104 | `SlippageExceeded` | Slippage exceeded | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 105 | `InvalidRoute` | Invalid route | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 106 | `InsufficientLpTokens` | Insufficient lp tokens | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 107 | `ZeroLiquidity` | Zero liquidity | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 108 | `Reentrancy` | A guarded section was re-entered (Issue #238). | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |

<a id="trading-tradingerror"></a>
### `trading` — `TradingError`

Source: [`src/trading.rs`](../src/trading.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidOrder` | Invalid order | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `OrderNotFound` | Order not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `NotOrderOwner` | Not order owner | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 4 | `OrderCapReached` | Order cap reached | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `InvalidPrice` | Invalid price | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 6 | `InvalidQuantity` | Invalid quantity | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |

<a id="treasure-vault-vaulterror"></a>
### `treasure_vault` — `VaultError`

Source: [`src/treasure_vault.rs`](../src/treasure_vault.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `VaultNotFound` | Vault not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 2 | `NotOwner` | Caller is not the vault owner. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 3 | `StillLocked` | Vault is still within its lock period. | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 4 | `AlreadyClaimed` | Vault has already been claimed. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 5 | `InvalidAmount` | Deposit amount must be positive. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 6 | `ArithmeticOverflow` | A checked arithmetic operation overflowed (Issue #239). | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="wallet-abstraction-walleterror"></a>
### `wallet_abstraction` — `WalletError`

Source: [`src/wallet_abstraction.rs`](../src/wallet_abstraction.rs) · *Not currently declared in `src/lib.rs`, so not in the deployed WASM. Documented for completeness and future wiring.*

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `WalletExists` | Wallet already exists for this player. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 2 | `WalletNotFound` | Wallet not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 3 | `SessionKeyInvalid` | Session key expired or exhausted. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `OperationNotAllowed` | Operation not allowed by this session key. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 5 | `GuardianExists` | Guardian already registered. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 6 | `GuardianNotFound` | Guardian not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 7 | `InsufficientApprovals` | Insufficient guardian approvals for recovery. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 8 | `RecoveryNotFound` | Recovery proposal not found. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 9 | `CannotBeOwnGuardian` | Cannot add self as guardian. | Validation | The action targets the caller itself. Pick a different counterparty. |
| 10 | `MultisigNotConfigured` | Multisig not configured. | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |
| 11 | `MultisigInsufficientApprovals` | Insufficient multisig approvals. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 12 | `NotASigner` | Not a registered signer. | Authorization | The caller does not hold the required relationship, opt-in or eligibility. Sign with the right account or complete the prerequisite (opt in, verify profile, solve the CAPTCHA, buy premium). |
| 13 | `AlreadyApproved` | Already approved. | Conflict | The action already happened. Treat it as idempotent: re-query state instead of retrying. |
| 14 | `SessionKeyLimitReached` | Session key limit reached. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |

<a id="wormhole-traveler-wormholeerror"></a>
### `wormhole_traveler` — `WormholeError`

Source: [`src/wormhole_traveler.rs`](../src/wormhole_traveler.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InvalidDestination` | Invalid destination | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 2 | `InsufficientEnergy` | Insufficient energy | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 3 | `ShipNotFound` | Ship not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `WormholeNotFound` | Wormhole not found | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 5 | `WormholeExpired` | Wormhole expired | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 6 | `MaxWormholesReached` | Max wormholes reached | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 7 | `UnauthorizedTravel` | Unauthorized travel | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 8 | `SameNebulaTravel` | Same nebula travel | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 9 | `WormholeClosed` | Wormhole closed | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 10 | `EnergyManagerError` | Energy manager error | Internal | Unexpected contract or downstream failure. Record the tx hash and inputs, check contract events, and report it. Retry only if the downstream dependency is known to be transient. |

<a id="yield-farming-farmerror"></a>
### `yield_farming` — `FarmError`

Source: [`src/yield_farming.rs`](../src/yield_farming.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `LockNotMet` | Lock not met | State | The operation is not allowed in the current state or time window. Read the entity's state, wait for the delay or approval, or complete the prerequisite step. |
| 2 | `InsufficientBalance` | Insufficient balance | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 3 | `InvalidPool` | Invalid pool | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 4 | `WhaleCapExceeded` | Whale cap exceeded | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `ArithmeticOverflow` | Arithmetic overflow | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 6 | `PoolNotActive` | Pool not active | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |
| 7 | `RewardNotReady` | Reward not ready | State | The entity is in the wrong state or time window. Read its current state, then wait or take the prerequisite step first. |

<a id="yield-forecast-forecasterror"></a>
### `yield_forecast` — `ForecastError`

Source: [`src/yield_forecast.rs`](../src/yield_forecast.rs) · Implements `StandardContractError`.

| Code | Variant | Meaning | Category | How to handle |
|-----:|---------|---------|----------|---------------|
| 1 | `InsufficientData` | Not enough historical data for forecast. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 2 | `InvalidDays` | Invalid number of days requested. | Validation | Fix the arguments: check ranges, non-zero values, lengths and IDs before sending. |
| 3 | `PlayerNotFound` | Player not found or no data. | NotFound | Check the ID/key. Create or register the entity first, or refresh cached client state. |
| 4 | `MaxDaysExceeded` | Forecast days exceed maximum. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
| 5 | `ModelNotInitialized` | Model not initialized. | Setup | Call the module's `init`/`initialize` entry point (admin) before using it. |
| 6 | `Unauthorized` | Unauthorized caller. | Authorization | Sign with the account that holds the required role or ownership; check role grants. |
| 7 | `BurstLimitExceeded` | Burst limit exceeded. | ResourceLimit | Reduce the amount or batch size, top up balances or pools, or wait for the limit window to reset. |
