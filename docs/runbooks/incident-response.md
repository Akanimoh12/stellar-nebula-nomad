# Incident Response Runbook

Severity definitions, first-response procedure, and specific playbooks for the
economy rebalance.

Related: [deployment](deployment.md) · [troubleshooting](troubleshooting.md) ·
[rollback](rollback.md)

---

## 1. Severity

| Sev | Definition | Response |
|---|---|---|
| **Sev-1** | Funds at risk, or sink totals corrupted. Total resource supply is wrong. | Page immediately. Halt feature. Roll back now. |
| **Sev-2** | Feature unusable or badly mispriced, no fund loss. Prices off by >2×, or a sink total decreased. | Respond within 1 hour. Retune config. |
| **Sev-3** | Degraded but bounded. One error path spiking, spreads wide, one resource unpriceable. | Same business day. Diagnose, schedule fix. |

Escalate one level immediately if any of these hold:

- A sink total (`get_total_repair_burn`, `get_total_upgrade_spend`,
  `get_total_craft_sink`) decreased.
- Resource supply is not conserved between two consecutive reads.
- A `list_at_market` call moved funds without creating a `DexOffer`.
- The repair path allowed a repair for zero cost.

---

## 2. First response (15 minutes)

### 2.1 Freeze the feature

Before diagnosing, stop the bleeding. Retune rather than diagnose under load.

```bash
# Freeze market pricing: widen nothing, just stop trusting the feed —
# push the deviation cap down so anomalous prints are rejected outright.
soroban contract invoke --source-account <admin> --network <network> --id <contract> \
  --fn set_pricing_config --arg '{ "max_deviation_bps": 1, "...": "..." }'

# Throttle the repair sink to near zero via cost
soroban contract invoke --source-account <admin> --network <network> --id <contract> \
  --fn set_repair_config \
  --arg '{ "cost_per_point": 100000, "emergency_surcharge_bps": 5000, "max_repair_per_call": 50 }'
```

A very low `max_deviation_bps` makes the engine reject nearly every print, so
published prices freeze at their last accepted value rather than moving toward a
bad one. Rejected prints are counted and evented but never enter history, so
this does not corrupt state.

### 2.2 Capture state

Record all of the following before changing anything else. Post-rollback
evidence is not recoverable.

```bash
# Config as it was
... --fn get_upgrade_economy
... --fn get_repair_config
... --fn get_pricing_config

# Per-resource pricing state and history
for r in dust ore crystal mineral exo_fuel bio_shard relic; do
  ... --fn get_price_state     --arg $r
  ... --fn get_pricing_history --arg $r
  ... --fn get_supply_demand   --arg $r
done

# Sink totals
... --fn get_total_repair_burn
... --fn get_total_upgrade_spend
... --fn get_total_craft_sink

# Deployment record
cat .deploy-<network>.log
```

### 2.3 Identify the layer

```
Funds moved incorrectly?          → Sev-1, go to §4.1
Sink total decreased?             → Sev-1, go to §4.2
Prices wrong, funds safe?         → Sev-2, go to §4.3
One resource unpriceable?         → Sev-3, go to §4.4
Errors spiking, no mispricing?    → Sev-3, go to §4.5
```

---

## 3. Communication

| When | Audience | Content |
|---|---|---|
| Sev-1 confirmed | Players + team | Feature halted, funds safe or not, next update time |
| Sev-1 unresolved 1h | Players | Progress update, no speculation on cause |
| Sev-2 confirmed | Team | Symptom, rollback applied, follow-up ticket |
| Post-incident | Team + changelog | Timeline, root cause, what changed to prevent recurrence |

State plainly whether funds are at risk. Do not state a cause before §4 has
produced evidence for it. If a rollback reverted player-visible pricing, say
so explicitly — a silently cheaper upgrade path invites a second incident.

---

## 4. Playbooks

### 4.1 Funds moved incorrectly (Sev-1)

1. Halt the feature using §2.1.
2. Determine whether the loss is a pricing error or a logic error:
   - **Pricing error** — the amount moved matches a published price that was
     wrong. No individual transfer is invalid; the price was. Roll back pricing
     (§4.3) and re-price. Funds already moved at the wrong price are not
     clawed back automatically — decide explicitly, and write the decision down.
   - **Logic error** — a transfer happened that should not have. Cross-check the
     `DexOffer` record against the resource movement. If an offer is missing,
     the harvest path bypassed the offer creation, and the listing cap may have
     been exceeded.
3. Check the listing cap: more than five concurrent offers from one address means
   `harvest_and_list` is not enforcing `MAX_ACTIVE_OFFERS`.
4. Roll back per [rollback.md](rollback.md), starting at tier 1.
5. Write up the exact transaction ids involved.

### 4.2 A sink total decreased (Sev-1)

This is almost always a storage-key collision and it means another module is
reading and writing the same storage slot.

1. Confirm the decrease by reading the total twice, spaced by a few ledgers. A
   stable read means state corruption, not a display bug.
2. Find the colliding key:

   ```bash
   # Bare unit variants encode to instance key ["<VariantName>"]
   grep -rn "^    Config,$\|^    Admin,$\|^    State,$" src/ | sort
   ```

   Two modules declaring a bare `Config` or `Admin` share one key; whichever
   `init` ran last owns it. `dynamic_pricing` uses `PricingConfig` /
   `PricingAdmin` for exactly this reason.
3. If the collision is in a module this release touched, it is a bug in this
   release — roll back to tier 1 config if the affected subsystem is tunable,
   otherwise tier 4.
4. If it is in a pre-existing unrelated module, do not fix it under incident
   pressure. Halt the affected subsystem, roll back, and file it separately.
5. Reconstruct the pre-corruption total from the events log if available. Do not
   write a corrected total into storage by hand — a hand-written total makes the
   sink unauditable.

### 4.3 Prices wrong, funds safe (Sev-2)

Diagnose with `get_price_state` before changing anything:

| Signal | Meaning | Action |
|---|---|---|
| `rejected_prints` high | Feed printing outside the cap | Guard working. Fix the feed, or widen the cap deliberately. |
| `rejected_prints` 0, `sma` stale | Cooldown, or wrong admin | Check `min_cooldown_secs` and the configured source. |
| `band_bps` at maximum | Volatility EWMA saturated | Feed is oscillating. Let it decay or re-seed (tier 2). |
| `observation_count` flat | Nobody is submitting | Operational, not a bug. |

Then roll back per [rollback.md](rollback.md) §1.3 and re-seed if the history
itself is wrong (§2).

### 4.4 One resource unpriceable (Sev-3)

Almost always `ResourceNotFound` — the resource has no observation, so the
engine has no average.

```bash
soroban contract invoke --source-account <admin> --network <network> --id <contract> \
  --fn observe_price --arg <admin> --arg <resource> --arg <price>
```

If the seed is rejected, the resource already has a history far from that value.
Re-seed with the tier-2 sequence in [rollback.md](rollback.md).

Also check the balancer side: a resource with no liquidity cannot be priced by
market even with a valid observation, and `list_at_market` will keep returning
`PriceUnavailable` for it until both exist.

### 4.5 Error spikes, no mispricing (Sev-3)

Rank the errors before acting:

- `RateLimitExceeded` — expected during a release spike. The repair budget is
  3 per 300s per address, and `apply_upgrade` is throttled too. If it is only
  the repair path, raise the cost rather than the cap; a higher cap lets one
  player drain the resource faster.
- `CooldownActive` — the feed is polling faster than `min_cooldown_secs`. Fix
  the feed's poll interval.
- `InvalidConfig` on `init_repair_config` or `init_pricing_config` — these are
  once-only. Use `set_repair_config` / `set_pricing_config`. A deployment
  script that re-runs init on an already-initialised contract will fail on every
  subsequent deploy; make init idempotent in the script, not in the contract.
- `AlreadyInitialized` from `init_upgrade_config` — same cause.

---

## 5. Post-incident

Within 48 hours:

1. Timeline: first signal, detection, mitigation, full resolution — with
   transaction ids and config values at each step.
2. Root cause, and specifically whether the guard that should have caught it
   fired. Every subsystem here has a guard: the deviation cap, the non-zero cost
   requirement, the listing cap, the rate limiter. An incident that reached
   production usually means a guard was bypassed, misconfigured, or never
   asserted in a test.
3. Add a regression test that fails before the fix. For this codebase that
   means a test in the matching target:
   `test_ship_upgrade`, `test_ship_repair`, `test_dynamic_pricing`, or
   `test_dex_integration`.
4. Update the relevant runbook. If this incident used a path not documented in
   [troubleshooting](troubleshooting.md), add it there — a runbook that did not
   help during a real incident is not a runbook.
5. Add a `CHANGELOG.md` entry describing the behavioural change from the
   player's perspective.
