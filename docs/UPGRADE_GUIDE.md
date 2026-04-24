# Upgrade Guide

This document describes how to safely upgrade the Stellar Nebula Nomad
smart contract using the upgradeable proxy pattern implemented in
[`src/proxy.rs`](../src/proxy.rs).

---

## Table of Contents

1. [Overview](#overview)
2. [How the Upgrade Pattern Works](#how-the-upgrade-pattern-works)
3. [Pre-Upgrade Checklist](#pre-upgrade-checklist)
4. [Step-by-Step Upgrade Procedure](#step-by-step-upgrade-procedure)
5. [State Migrations](#state-migrations)
6. [Rollback Procedure](#rollback-procedure)
7. [Version History](#version-history)
8. [Security Considerations](#security-considerations)
9. [Troubleshooting](#troubleshooting)

---

## Overview

Soroban contracts on Stellar are **immutable by default** — once deployed,
the code cannot change.  However, Soroban exposes a privileged primitive:

```
env.deployer().update_current_contract_wasm(new_wasm_hash)
```

This atomically swaps the Wasm blob associated with the contract address
**without changing the contract address or its persistent storage**.
All on-chain state (player profiles, ship NFTs, achievements, etc.) is
preserved across upgrades.

The `proxy.rs` module wraps this primitive with:

- **Admin-only authorization** — only the contract admin can trigger upgrades.
- **Two-step process** — `authorize_upgrade` then `execute_upgrade` prevents
  accidental upgrades.
- **Version tracking** — a monotonic counter is incremented on every upgrade.
- **Rollback** — the previous Wasm hash is stored, enabling emergency rollback.
- **Migration hooks** — `run_migration` supports incremental data migrations.
- **Audit log** — every upgrade is appended to a persistent history.

---

## How the Upgrade Pattern Works

```
Admin                 Contract (proxy.rs)           Stellar Network
  │                         │                              │
  │── authorize_upgrade ────▶│ stores PendingUpgrade        │
  │                         │                              │
  │── execute_upgrade ──────▶│ reads PendingUpgrade         │
  │                         │── update_current_wasm ──────▶│ swaps Wasm
  │                         │ increments version           │
  │                         │ emits prx_upgrd event        │
  │                         │                              │
  │── run_migration ────────▶│ migrates data batch by batch │
  │   (repeat until done)   │                              │
```

---

## Pre-Upgrade Checklist

Before upgrading, complete **all** of the following:

- [ ] **Test the new contract thoroughly** on testnet with a forked copy of
      mainnet state.
- [ ] **Audit the new Wasm** — have at least two reviewers inspect the changes.
- [ ] **Document storage schema changes** — any field additions, removals, or
      type changes must be covered by a migration in `run_migration`.
- [ ] **Announce the upgrade** — notify players via the project Discord / X
      account at least 24 hours in advance.
- [ ] **Verify the admin key** is secure and accessible.
- [ ] **Take a state snapshot** using `src/state_snapshot.rs` before upgrading.
- [ ] **Record the current Wasm hash** in case a rollback is needed:
      ```sh
      stellar contract info --id $CONTRACT_ID | grep wasm_hash
      ```

---

## Step-by-Step Upgrade Procedure

### 1. Build the new contract Wasm

```sh
cargo build --target wasm32-unknown-unknown --release
```

The output will be at:
```
target/wasm32-unknown-unknown/release/stellar_nebula_nomad.wasm
```

### 2. Upload the new Wasm to Stellar

```sh
stellar contract upload \
  --source-account $ADMIN_SECRET \
  --network testnet \
  --wasm target/wasm32-unknown-unknown/release/stellar_nebula_nomad.wasm
```

Note the **Wasm hash** printed by this command (64-character hex string).

### 3. Authorize the upgrade (two-step safety gate)

```sh
stellar contract invoke \
  --id $CONTRACT_ID \
  --source-account $ADMIN_SECRET \
  --network testnet \
  -- authorize_upgrade \
  --new_wasm_hash <WASM_HASH_FROM_STEP_2>
```

This stores a `PendingUpgrade` record but does **not** yet change the
running contract.

### 4. Execute the upgrade

```sh
stellar contract invoke \
  --id $CONTRACT_ID \
  --source-account $ADMIN_SECRET \
  --network testnet \
  -- execute_upgrade
```

The contract Wasm is now swapped.  The version counter is incremented and
a `prx_upgrd` event is emitted.

### 5. Verify the upgrade

```sh
# Check the new version number
stellar contract invoke \
  --id $CONTRACT_ID \
  --network testnet \
  -- get_version

# Check the upgrade history
stellar contract invoke \
  --id $CONTRACT_ID \
  --network testnet \
  -- get_upgrade_history
```

### 6. Run data migrations (if required)

If the new version introduces storage schema changes, run the migration:

```sh
# Repeat until the call returns `true`
stellar contract invoke \
  --id $CONTRACT_ID \
  --source-account $ADMIN_SECRET \
  --network testnet \
  -- run_migration \
  --batch_size 50
```

Continue calling `run_migration` until it returns `true`.

---

## State Migrations

When a new version changes the on-chain data layout, you must add migration
logic to the `run_migration` function in `src/proxy.rs`.

### Migration template

```rust
pub fn run_migration(env: &Env, batch_size: u32) -> Result<bool, ProxyError> {
    require_admin(env)?;

    let version = get_version(env);
    let step: u32 = env.storage().instance()
        .get(&ProxyKey::MigrationStep).unwrap_or(0);

    match version {
        2 => {
            // Example: migrate PlayerProfile to add a new `reputation` field
            let done = migrate_profiles_v1_to_v2(env, step, batch_size);
            if done {
                env.storage().instance().remove(&ProxyKey::MigrationStep);
            } else {
                env.storage().instance().set(&ProxyKey::MigrationStep, &(step + 1));
            }
            Ok(done)
        }
        _ => Ok(true), // no migration needed
    }
}
```

### Rules for safe migrations

1. **Never delete storage keys in the first batch** — other contract calls
   may still be using them until the migration finishes.
2. **Process in batches** — use the `batch_size` parameter and the
   `MigrationStep` counter to avoid hitting ledger resource limits.
3. **Make migrations idempotent** — if a migration is interrupted and
   restarted, it must produce the same result.
4. **Test with real data** — run the migration on a testnet fork of mainnet
   state before deploying to mainnet.

---

## Rollback Procedure

> **Warning:** Rollback reverts the contract code but does **not** reverse
> data migrations.  Only roll back if you are certain the migration has not
> run, or if you have a recovery plan for any migrated data.

### When to roll back

- A critical bug is discovered in the new version immediately after upgrade.
- The upgrade was executed against the wrong contract.
- The new Wasm hash was incorrect.

### How to roll back

```sh
stellar contract invoke \
  --id $CONTRACT_ID \
  --source-account $ADMIN_SECRET \
  --network testnet \
  -- rollback_upgrade
```

This reverts to the Wasm installed before the most recent `execute_upgrade`
call.  The version counter is decremented by 1.

### After rollback

1. Verify the version counter returned to the expected value.
2. Investigate the root cause before attempting another upgrade.
3. Remove the flawed Wasm from your CI artifacts to prevent accidental reuse.

---

## Version History

| Version | Description                                        | Date       |
|---------|----------------------------------------------------|------------|
| 1       | Initial deployment                                 | 2026-04-24 |

> Update this table with every upgrade.

---

## Security Considerations

### Admin key management

- The admin key should be a **hardware wallet** or a **multi-sig account**
  on mainnet.
- Never store the admin secret key in CI environment variables unencrypted.
- Consider rotating the admin key after each major upgrade by deploying a
  new admin address and calling a hypothetical `set_admin` function (must
  be implemented before needed).

### Upgrade authorization window

The two-step upgrade process (authorize → execute) provides a window for
monitoring systems to detect unexpected upgrade proposals.  We recommend:

- Setting up an alerting system that monitors for `prx_auth` events.
- Requiring a minimum 1-hour delay between `authorize_upgrade` and
  `execute_upgrade` on mainnet.
- Requiring a second admin signature (multi-sig) for mainnet upgrades.

### Wasm hash verification

Always verify the Wasm hash you are authorizing matches the binary you
tested.  Compute it locally:

```sh
stellar contract upload \
  --wasm target/wasm32-unknown-unknown/release/stellar_nebula_nomad.wasm \
  --network testnet \
  --dry-run 2>&1 | grep hash
```

Compare the output to the hash you pass to `authorize_upgrade`.

---

## Troubleshooting

| Symptom | Likely Cause | Solution |
|---------|--------------|----------|
| `NoPendingUpgrade` error on `execute_upgrade` | `authorize_upgrade` was not called | Call `authorize_upgrade` first |
| `NotAdmin` error | Wrong source account or admin not initialized | Check `get_admin` and sign with the correct key |
| `AlreadyInitialized` error on `initialize` | Proxy already set up | Use `get_admin` to verify the current admin |
| `NoRollbackTarget` error | Only one upgrade in history | Cannot roll back further; restore from a pre-upgrade snapshot |
| Migration never returns `true` | Bug in migration logic | Check `MigrationStep` counter and migration code |
| Contract behaves unexpectedly after upgrade | Data migration incomplete | Run `run_migration` until it returns `true` |
