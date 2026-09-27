# Disaster Recovery

This is the operator runbook for Stellar Nebula Nomad contract-state backups and restores.

## Backup Strategy

Automated **Contract state** snapshots are taken daily at 02:00 UTC through Horizon:

- Latest ledger from `GET /ledgers`
- Contract record from `GET /contracts/{id}` (fallback `GET /accounts/{id}`)
- **Player profiles** via account effects
- **Leaderboard** / activity via operations
- **Contract WASM** when Stellar CLI is available

Archives are named `nebula_backup_YYYYMMDD_HHMMSS.tar.gz`, checksummed with SHA-256, and kept for 30 days locally. Optional S3 copies follow a 90-day lifecycle (`infrastructure/backup/`).

RTO target: **< 1 hour**. RPO target: **< 24 hours** (last successful daily backup).

## Recovery Procedures

1. Pick the newest healthy archive under `backups/` (or S3).
2. Verify checksums: `./scripts/restore.sh --backup <file> --verify-only`
3. Dry-run: `./scripts/restore.sh --backup <file> --dry-run`
4. Restore against a testnet contract before production, then switch aliases under `deployment/aliases/`.

### Scenario 1 — Corrupt contract instance

Redeploy WASM from the archive (or `cargo build --release --target wasm32-unknown-unknown`) and re-initialize from the Horizon snapshot metadata.

### Scenario 2 — Lost operator keys

Use the backup metadata (`metadata/contract_info.json`) plus Horizon history to reconstruct the contract id, then rotate to a new deployer identity.

### Scenario 3 — Horizon / RPC outage

Serve from the last local `.tar.gz`. The snapshot includes ledger JSON so you can resume once Horizon returns.

## Testing & Verification

- `./scripts/backup.sh --test-restore` takes a snapshot and runs restore in `--test-mode`.
- GitHub Actions `backup.yml` verifies archive integrity after each scheduled run.
- Monthly drill job performs a dry-run restore.

## Monitoring & Alerts

Prometheus rules in `monitoring/prometheus/alert-rules.yml` and `monitoring/backup-monitoring.yml`:

- **BackupFailed** — last backup reported failure
- **NoRecentBackup** — no success in 48 hours

Grafana: http://localhost:3000 (see `docs/MONITORING_SETUP.md`).

## Emergency Contacts

| Role | Contact |
|------|---------|
| On-call maintainer | GitHub `@Space-Nebula` maintainers |
| Stellar status | https://status.stellar.org |

### Escalation Path

1. Check backup logs (`backups/backup.log`) and Actions run.
2. Page the on-call maintainer if RPO is at risk (no backup in 24 hours).
3. If restore fails, open an incident and freeze further deploys.

## Restoration procedures (quick)

```bash
chmod +x scripts/backup.sh scripts/restore.sh
CONTRACT_ID=<id> STELLAR_NETWORK=testnet ./scripts/backup.sh
./scripts/restore.sh --backup backups/nebula_backup_*.tar.gz --verify-only
```

Full policy: `infrastructure/backup/README.md` and `docs/BACKUP_RECOVERY_README.md`.
