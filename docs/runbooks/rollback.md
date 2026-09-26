# Rollback Runbook

How to reverse the economy rebalance, in order of increasing cost. Try the
cheapest lever that fixes the symptom.

Related: [deployment](deployment.md) · [troubleshooting](troubleshooting.md) ·
[incident response](incident-response.md)

---

## Rollback tiers at a glance

| Tier | Action | Cost | Reverses | Use when |
|---|---|---|---|---|
| 1 | Retune config on-chain | ~1 fee | Behaviour | Prices/volatility wrong, sink too hot or cold |
| 2 | Re-seed pricing baseline | ~1 fee | Volatility state | Feed data is corrupt |
| 3 | Blue/green alias flip | 0 | Which build is live | Code is the problem |
| 4 | Redeploy previous WASM | 1 deploy | Everything | Previous build was correct |

**Prefer tier 1.** A config rollback keeps the new code and takes effect
immediately; a redeploy costs a fee and leaves a new contract to audit.

---

## Tier 1 — Configuration rollback

All three subsystems are admin-configurable and reversible in place. No
transaction builds anything, so this can be done under pressure.

### 1.1 Ship upgrade curve → legacy flat pricing

The most likely rollback, and the cheapest. Set compounding to zero:

```bash
soroban contract invoke --source-account <admin> --network <network> --id <contract> \
  --fn set_upgrade_economy --arg '{ "growth_bps": 0, "max_cost": 100000 }'
```

`growth_bps = 0` makes `scaled_upgrade_cost` return the base cost for every
tier, which is exactly the legacy flat schedule. Verify:

```bash
soroban contract invoke --network <network> --id <contract> --fn get_upgrade_economy
```

This affects only *future* quotes. Hulls already upgraded keep their stats;
already-burned resources are not refunded. That is deliberate — the sink totals
stay truthful and the rebalance is not a windfall for players who upgraded
before the rollback.

### 1.2 Repair sink → throttled or paused

To slow the burn without removing the feature, reduce the daily budget by
lowering the rate-limit cap, or raise cost:

```bash
soroban contract invoke --source-account <admin> --network <network> --id <contract> \
  --fn set_repair_config \
  --arg '{ "cost_per_point": 4, "emergency_surcharge_bps": 5000, "max_repair_per_call": 50 }'
```

Doubling `cost_per_point` halves the burn rate at a given repair volume. There
is no "disable" switch: setting `cost_per_point = 0` is rejected by
`set_repair_config`, which requires a non-zero cost. That guard is intentional —
a free repair path would be an unbounded resource faucet. To effectively pause
repairs, set a high cost and rely on the rate limiter.

Verify with `get_repair_config` and confirm the rate limiter via
`get_rate_limit_config`.

### 1.3 Dynamic pricing → fixed prices

To freeze published prices, open the deviation cap and the volatility band to
their maximum and set smoothing to its minimum. The EMA then tracks the feed
almost exactly, which is equivalent to a fixed price as long as the feed is
trustworthy:

```bash
soroban contract invoke --source-account <admin> --network <network> --id <contract> \
  --fn set_pricing_config \
  --arg '{ "smoothing_bps": 1, "max_deviation_bps": 100000, "base_volatility_band_bps": 100000, "max_volatility_band_bps": 100000, "min_cooldown_secs": 60, "supply_demand_adj_bps": 0 }'
```

Widen the deviation cap **only** if the feed is known good. A bad feed plus a
wide cap means a bad feed sets the price directly. If the feed is the suspect,
go to tier 2 instead.

### 1.4 Dynamic pricing → pause observations

There is no pause switch. To stop the feed, revoke the admin's access to
`observe_price` operationally (do not submit updates), or migrate the admin by
`init_pricing_config` on a new deployment. The cooldown already limits the
update rate to one accepted print per resource per `min_cooldown_secs`, so a
paused feed converges to a stale price rather than a runaway one. Stale is
recoverable; a manipulated price is not.

---

## Tier 2 — Re-seed the pricing baseline

Use when the price history is corrupt: a bad oracle run, a decimal-units bug,
or a rejected/rejected sequence that left the average far from reality. The
engine has no per-resource reset, so re-seeding is a new observation.

A new observation is subject to the same deviation guard, so a wildly different
seed will be **rejected**. Sequence it:

```bash
# 1. Widen the cap so the correction is accepted
soroban contract invoke --source-account <admin> --network <network> --id <contract> \
  --fn set_pricing_config --arg '{ "max_deviation_bps": 100000, "...": "..." }'

# 2. Push one corrected observation per resource, spaced by the cooldown
soroban contract invoke --source-account <admin> --network <network> --id <contract> \
  --fn observe_price --arg <admin> --arg dust --arg <correct_price>

# 3. Re-narrow the cap to the intended value
soroban contract invoke --source-account <admin> --network <network> --id <contract> \
  --fn set_pricing_config --arg '{ "max_deviation_bps": 5000, "...": "..." }'
```

Volatility is an EWMA and decays over observations, so the band stays wide for a
while after a re-seed. Expect elevated spreads for roughly ten to twenty
observations and let it decay rather than repeatedly clamping it shut.

---

## Tier 3 — Blue/green alias flip

`deploy.sh` supports blue/green: the new build is deployed, verified, and only
promoted if verification passes. If a bad build reaches production, point the
alias back at the previous contract id.

```bash
cat deployment/artifacts/<network>/latest-id.txt   # what is live now
ls deployment/artifacts/<network>/                 # what was deployed before

# Restore the previous id
echo "<previous_contract_id>" > deployment/aliases/nebula-prod.txt
```

Then update the client configuration to read the alias rather than a hardcoded
id. **This does not migrate state** — the rolled-back contract is a different
contract with its own storage. Anything the new build wrote (repair burns,
upgrade spend, price history) is invisible to the old one. Tier 3 is therefore
only appropriate when the new build is fundamentally unsalvageable and the
state written since deployment is negligible.

---

## Tier 4 — Redeploy the previous WASM

Last resort. Use when the new build wrote state that the old build cannot read,
or when a tier-3 flip would lose too much.

```bash
# 1. Find the artifact for the last known-good deployment
grep '<last_known_good_contract_id>' .deploy-<network>.log

# 2. Redeploy that exact WASM
soroban contract deploy \
  --wasm deployment/artifacts/<network>/stellar_nebula_nomad-<id8>.wasm \
  --source-account <admin> --network <network>
```

Record the new id in `.deploy-<network>.log` and update the alias. Then
reconcile state by hand:

- **Dynamic pricing** — re-seed every resource (tier 2). No migration needed;
  the engine has no on-chain seed beyond the first observation.
- **Repair and upgrade sinks** — `get_total_repair_burn` and
  `get_total_upgrade_spend` start from zero on the new contract. Do not
  hand-edit them; instead record the pre-rollback values in the incident
  report and carry them forward in reporting only.
- **Hulls already upgraded** — the old build reads the same `Ship` struct, so
  installed modules persist. The cost curve reverts, future upgrades are cheap
  again.

---

## After any rollback

1. Verify with the §5 table in [deployment](deployment.md).
2. Update the player-facing numbers if any of them are user-visible — the
   upgrade schedule is the one players will notice immediately.
3. Record what was rolled back, to what values, and why, in the incident
   channel. A config rollback that nobody knows about will be reverted by the
   next person who reads the issue.
4. Leave the rolled-back config in place until the root cause is understood.
   Rolling back and immediately re-rolling-forward reproduces the incident.
