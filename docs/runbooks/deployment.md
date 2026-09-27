# Deployment Runbook

Operating procedure for shipping the economy rebalance (ship upgrades, repair
sink, dynamic pricing) to a Soroban network.

Related: [troubleshooting](troubleshooting.md) ·
[rollback](rollback.md) · [incident response](incident-response.md)

---

## 1. What is being deployed

Three economic subsystems, all behind admin-gated configuration so a bad
parameter set can be corrected on-chain without a code upgrade.

| Subsystem | Entry points | Default |
|---|---|---|
| Ship upgrade cost curve (#454) | `init_upgrade_config`, `set_upgrade_economy` | +60% per module, `100` base |
| Ship repair sink (#453) | `init_repair_config`, `set_repair_config` | 2 dust/point, +50% emergency, 50/call |
| Dynamic pricing (#452) | `init_pricing_config`, `set_pricing_config` | 20% EMA, ±50% deviation cap, ±30% band |

Every subsystem reads sane defaults when unconfigured, so a fresh deploy is
functional before any of these calls are made. Configuration only changes
behaviour; it never gates it.

> **Note on the upgrade curve.** `set_upgrade_economy` with `growth_bps = 0`
> restores the legacy flat schedule. That is the single most useful rollback
> lever in this release — see [rollback.md](rollback.md).

---

## 2. Pre-flight checks

```bash
# 1. Clean tree and known-good base
git status --short          # expect: only intended changes
git log --oneline -3

# 2. Targets for the build
rustup target add wasm32-unknown-unknown
rustup target list --installed

# 3. Build the release artifact
cargo build --target wasm32-unknown-unknown --release

# 4. The suites that cover this release
cargo test --lib ship_upgrade::
cargo test --lib ship_repair::
cargo test --lib dynamic_pricing::
cargo test --test test_ship_upgrade
cargo test --test test_ship_repair
cargo test --test test_dynamic_pricing
cargo test --test test_dex_integration
```

All seven must pass. The repository has a large pre-existing failure backlog in
unrelated modules; do **not** treat that as a green build. Use the seven
commands above as the release gate and record the result in the PR.

### Storage-key collision check

Before deploying, confirm the new modules' storage keys are unique. A
`#[contracttype]` unit variant encodes to the instance key `["<VariantName>"]`,
so two modules that both declare a bare `Config` or `Admin` variant silently
share one key, and whichever initialised last wins.

`src/dynamic_pricing.rs` uses `PricingConfig` / `PricingAdmin` /
`PricingState` / `PricingHistory` specifically to avoid this. If you add a
storage key to any of these modules, prefix the variant with the module name
and verify:

```bash
grep -rn "^    Config,$\|^    Admin,$" src/ | sort
```

If that command's output grows a new bare `Config`/`Admin` in a module that
already has one, rename it before deploying.

---

## 3. Deploy

```bash
# Testnet / Futurenet, with post-deploy verification
./scripts/deploy.sh testnet <identity> --with-verify --alias nebula-economy-r1

# Blue/green, only promoting the alias if verification passes
./scripts/deploy.sh testnet <identity> --with-verify --bluegreen --prod-alias nebula-prod
```

`scripts/deploy.sh` builds, optimises, hashes, deploys, records the artifact
under `deployment/artifacts/<network>/`, and appends to `.deploy-<network>.log`.
Keep that log — it is the record of what is live.

The native target is required to link tests but **not** to build the WASM. A
missing `link.exe` means the host toolchain (MSVC Build Tools on Windows) is
absent; the release build itself needs only the `wasm32-unknown-unknown` target.

---

## 4. Post-deploy configuration

Run in this order. Each step is independently reversible.

### 4.1 Ship upgrade economy

```bash
soroban contract invoke \
  --source-account <admin> --network <network> --id <contract> \
  --fn set_upgrade_economy \
  --arg '{ "growth_bps": 6000, "max_cost": 100000 }'
```

Read back and confirm the stored curve:

```bash
soroban contract invoke --network <network> --id <contract> \
  --fn get_upgrade_economy
```

Expected: `growth_bps = 6000`, `max_cost = 100000`. With `growth_bps = 0` the
stored value stays whatever was configured and pricing returns to flat.

### 4.2 Repair sink

```bash
soroban contract invoke \
  --source-account <admin> --network <network> --id <contract> \
  --fn init_repair_config \
  --arg '<admin_address>' \
  --arg '{ "cost_per_point": 2, "emergency_surcharge_bps": 5000, "max_repair_per_call": 50 }'

soroban contract invoke --network <network> --id <contract> --fn get_repair_config
```

`init_repair_config` is once-only; it returns `InvalidConfig` on a second call.
After that, retune with `set_repair_config`.

### 4.3 Dynamic pricing

```bash
soroban contract invoke \
  --source-account <admin> --network <network> --id <contract> \
  --fn init_pricing_config \
  --arg '<admin_address>' \
  --arg '{ "smoothing_bps": 2000, "max_deviation_bps": 5000, "base_volatility_band_bps": 3000, "max_volatility_band_bps": 5000, "min_cooldown_secs": 60, "supply_demand_adj_bps": 2500 }'

soroban contract invoke --network <network> --id <contract> --fn get_pricing_config
```

Seed one price per traded resource so the DEX has something to list against:

```bash
soroban contract invoke --source-account <admin> --network <network> --id <contract> \
  --fn observe_price --arg '<admin_address>' --arg dust --arg 100
```

`get_dynamic_price` returns `ResourceNotFound` until a resource has at least one
observation, and `list_at_market` returns `PriceUnavailable` in that state. Seed
every resource the DEX lists before directing players at market pricing.

---

## 5. Post-deploy verification

| Check | Command | Expected |
|---|---|---|
| Curve applied | `get_upgrade_economy` | `growth_bps = 6000` |
| Repair priced | `get_repair_config` | `2 / 5000 / 50` |
| Pricing live | `get_dynamic_price --arg dust` | `sma` and `published` > 0 |
| Sink accumulating | `get_total_repair_burn` | monotonic non-decreasing |
| Upgrade sink | `get_total_upgrade_spend` | monotonic non-decreasing |
| Crafting sink | `get_total_craft_sink` | monotonic non-decreasing |

The three sink totals must never decrease. A decrease means storage was
overwritten by a colliding key — treat it as a Sev-2 and go to
[incident response](incident-response.md).

---

## 6. Promotion

1. Deploy to testnet, run §4 and §5, observe for one full market cycle.
2. Confirm repair and upgrade sink rates are within the intended band; the
   tuning levers are in [troubleshooting](troubleshooting.md) §3.
3. Blue/green deploy to production with `--prod-alias nebula-prod`.
4. Announce the new upgrade schedule to players **before** promotion. The cost
   of the fifth module rises roughly 8× under the new curve, and that is the
   change most likely to generate support load.
