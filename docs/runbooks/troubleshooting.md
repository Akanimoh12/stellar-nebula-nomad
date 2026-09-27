# Troubleshooting Runbook

Symptom-first diagnostics for the economy rebalance. Each entry gives the
observable symptom, the query that confirms it, and the fix.

Related: [deployment](deployment.md) · [rollback](rollback.md) ·
[incident response](incident-response.md)

---

## 1. Reading the current configuration

Start here. Almost every problem below is a configuration mismatch, and all
three subsystems are readable without admin rights.

```bash
soroban contract invoke --network <network> --id <contract> --fn get_upgrade_economy
soroban contract invoke --network <network> --id <contract> --fn get_repair_config
soroban contract invoke --network <network> --id <contract> --fn get_pricing_config
soroban contract invoke --network <network> --id <contract> --fn get_dynamic_price --arg dust
```

---

## 2. Ship upgrades (#454)

### 2.1 `InvalidEconomy` (error 210)

`set_upgrade_economy` refused the parameters. Constraints:

| Field | Rule |
|---|---|
| `growth_bps` | ≤ 5000 (50% compounding). At 10000 the curve stops smoothing and prices become unusable. |
| `max_cost` | ≥ 1. Caps a single module's price so the curve cannot overflow. |

### 2.2 Upgrade prices did not change

Most likely `growth_bps = 0`. At zero, `scaled_upgrade_cost` returns the base
cost for every tier — that is the legacy flat schedule, and it is intentional.
Confirm with `get_upgrade_economy`.

### 2.3 `NotFound` on `set_upgrade_economy`

`init_upgrade_config` has not run. Without blueprints registered there is no
base cost to scale, so the economy cannot be set. Run `init_upgrade_config`
first; it is once-only and rejects a second call with `AlreadyInitialized`.

### 2.4 `RateLimitExceeded` during a batch upgrade

`apply_upgrade` and `batch_upgrade` are throttled per address. A batch is one
call against the quota regardless of batch size, so a large batch is the cheap
path. Reduce the number of transactions rather than the work per transaction.

### 2.5 Costs are higher than the issue predicted

Expected tier prices for `resource_cost = 100`, `growth_bps = 6000`:

| Modules installed | Cost of the next | Cumulative |
|---|---|---|
| 0 | 100 | 100 |
| 1 | 160 | 260 |
| 2 | 256 | 516 |
| 3 | 409 | 925 |
| 4 | 654 | 1579 |

The legacy flat schedule totalled 500 for the same build-out. If the observed
numbers differ, check that `resource_cost` on the blueprint is what you think it
is — the curve scales the blueprint's base, it does not set an absolute price.

---

## 3. Ship repair (#453)

### 3.1 `ShipNotFound` (1) / `NotOwner` (2)

The ship does not exist, or the caller does not own it. Both are checked before
any pricing or balance work, so a rejection here never moves funds.

`quote_repair` takes the **owner's** address, not a payer — quoting is not
chargeable, but it still refuses to price a ship you do not own.

### 3.2 `InsufficientResources` (3)

The player's harvested balance cannot cover the price. Check the quote first:

```bash
soroban contract invoke --network <network> --id <contract> \
  --fn quote_repair --arg <owner> --arg <ship_id> --arg dust --arg false
```

`resource_cost` is the amount that will be burned. Note that `quote_repair`
takes an `emergency` flag too; the field-patch surcharge is only applied when
it is `true`, so a quote taken with `false` will understate an emergency
repair by 50%.

### 3.3 `ShipAlreadyFull` (4)

Durability is already at `max_durability`. Repairing a full hull is a no-op by
design — there is nothing to sink. Read durability from the ship record.

### 3.4 `RateLimitExceeded` (7)

The default budget is **3 repairs per 300 seconds, per address**. Two
consequences that surprise people:

- The cap is per address across *all* ships. A player with nine damaged ships
  still gets three repairs per window.
- A 100-point hull takes two full repairs at the default 50-point cap. The third
  call in a window is therefore frequently a rate-limit error *or* an
  `AlreadyFull`, depending on hull state — check which before assuming the
  limiter is broken.

### 3.5 `InvalidConfig` (5) on `init_repair_config`

Either it has already run (once-only), or the config is out of range:
`cost_per_point` and `max_repair_per_call` must both be non-zero, and
`emergency_surcharge_bps` must be ≤ 100000.

### 3.6 `get_total_repair_burn` is not moving

It only counts **successful** repairs. A rejected repair burns nothing and
records nothing — check the failure reason first.

---

## 4. Dynamic pricing (#452)

### 4.1 `ResourceNotFound` (5)

No observation has ever been recorded for that resource. The engine has no
average to publish, so it refuses rather than inventing a price. Seed one:

```bash
soroban contract invoke --source-account <admin> --network <network> --id <contract> \
  --fn observe_price --arg <admin> --arg dust --arg 100
```

### 4.2 `PriceUnavailable` from `list_at_market`

Same root cause as 4.1 — no price for that resource. The DEX cannot list
against a price that does not exist. Seed every resource the exchange carries
before pointing players at market pricing.

### 4.3 `CooldownActive` (6)

`min_cooldown_secs` (default 60) has not elapsed since the last accepted print
**for that resource**. The window is per resource, not global, so three
resources can each be updated in the same minute. A feed pushing every 30
seconds will be throttled by design.

### 4.4 The published price is not moving

Check `rejected_prints` on the state:

```bash
soroban contract invoke --network <network> --id <contract> \
  --fn get_price_state --arg dust
```

- `rejected_prints` climbing → the feed is printing values more than
  `max_deviation_bps` (±50% by default) away from the average. Those prints are
  discarded, which is the guard working. If the *legitimate* feed is being
  rejected, the price has genuinely moved further than the cap allows: raise
  `max_deviation_bps` deliberately, or re-seed the average.
- `rejected_prints` flat and `observation_count` flat → the cooldown is
  rejecting, or the admin is not the configured source.

### 4.5 `Unauthorized` (2) on `observe_price`

The caller is not the admin recorded by `init_pricing_config`. The admin is
stored once and is not changed by `set_pricing_config`. Query
`get_pricing_config` for the guards, and confirm the calling address.

### 4.6 The price looks stuck between two values

Read `band_bps` on `get_dynamic_price`. The published price is clamped to the
average ± `band_bps`, and `band_bps` widens with realized volatility. A wide
band with a small `sma` move means the clamp, not the average, is setting the
price. The supply/demand nudge is applied *before* the clamp, so a large nudge
can be silently trimmed.

### 4.7 Supply/demand seems to do nothing

`observe_supply_demand` uses the balancer's parity convention: a ratio of
`1000` is balanced. The resulting nudge is clamped to
`supply_demand_adj_bps` (±2500 = ±25% by default) and then clamped again by the
volatility band. A nudge can be correct and still have no visible effect if the
band is tighter.

### 4.8 `sma` will not converge on a small move

By design. The EMA takes `smoothing_bps` of the gap, and integer division
floors — so a 1-unit gap against a large average rounds to zero. The
implementation moves the average by one unit anyway so progress is guaranteed,
but convergence on a very small gap takes many observations. Raise
`smoothing_bps` if the feed needs to track small moves faster.

---

## 5. Cross-cutting

### 5.1 A sink total decreased

**Stop and escalate.** `get_total_repair_burn`, `get_total_upgrade_spend` and
`get_total_craft_sink` are cumulative and must never decrease. A decrease means
storage was overwritten — almost always a `#[contracttype]` unit-variant
storage-key collision (see [deployment](deployment.md) §2). Go to
[incident response](incident-response.md).

### 5.2 Native link error during tests

```
error: linker `link.exe` not found
```

The host toolchain is missing. On Windows install Visual Studio Build Tools
with the MSVC v143 toolset and the Windows SDK. The release WASM build does not
need it; only the native test binaries do.

### 5.3 `wasm32-unknown-unknown` target missing

```bash
rustup target add wasm32-unknown-unknown
rustup target list --installed
```

`deploy.sh` builds this target, so a deploy fails without it.

### 5.4 Repository-wide test run is red

Expected on this repository — there is a large pre-existing failure backlog in
unrelated modules. Use the seven release-gate commands in
[deployment](deployment.md) §2 and compare against the baseline failure list
rather than requiring a green `cargo test`.
