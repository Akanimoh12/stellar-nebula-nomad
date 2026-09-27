# Contract state backups

Daily Horizon snapshots of Stellar Nebula Nomad contract state.

## What gets captured

`scripts/backup.sh` calls the Stellar Horizon API (not only the Soroban CLI):

- latest ledger
- `/contracts/{id}` or `/accounts/{id}`
- recent effects, operations, and transactions (player/leaderboard proxies)
- optional WASM via `stellar contract fetch` when the CLI is installed

## Schedule

| Mechanism | When |
|-----------|------|
| GitHub Actions `.github/workflows/backup.yml` | `0 2 * * *` UTC |
| Local cron `./scripts/backup.sh --schedule` | 02:00 on the host |

Retention is 30 days locally / for Actions artifacts, 90 days for optional S3 objects. See `retention.yaml`.

## Run a backup

```bash
export CONTRACT_ID=C...          # optional but recommended
export STELLAR_NETWORK=testnet
./scripts/backup.sh
```

Verify the archive:

```bash
./scripts/restore.sh --backup backups/nebula_backup_YYYYMMDD_HHMMSS.tar.gz --verify-only
```

Dry-run restore (no chain writes):

```bash
./scripts/restore.sh --backup backups/nebula_backup_YYYYMMDD_HHMMSS.tar.gz --dry-run
```

## S3 (optional)

```bash
cd infrastructure/backup
terraform init
terraform apply -var="backup_bucket_name=nebula-nomad-backups"
```

Set `S3_BUCKET` / AWS credentials so `scripts/backup.sh` uploads the `.tar.gz`.
