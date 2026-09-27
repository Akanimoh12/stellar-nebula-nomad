# Monitoring setup

Prometheus scrapes contract metrics from `monitoring/exporters/contract_exporter.py`. Grafana loads `monitoring/grafana/dashboards/nebula-overview.json`. Alertmanager routes critical/warning alerts from `monitoring/prometheus/alert-rules.yml`.

On-chain counters live in `src/metrics_exporter.rs` (`record_tx_success`, `record_tx_failure`, `get_contract_health`). Failed-tx error frequencies are persisted under `MetricsKey::ErrorTypeFrequency`.

## Start the stack

```bash
# With the full local environment (Quickstart + hot reload + monitoring)
docker compose -f docker-compose.dev.yml up --build

# Monitoring only (public Horizon testnet by default)
docker compose -f monitoring/docker-compose.yml up -d
# or
./setup-monitoring.sh
```

| Service | URL |
|---------|-----|
| Grafana | http://localhost:3000 (admin / `GRAFANA_PASSWORD`) |
| Prometheus | http://localhost:9090 |
| Alertmanager | http://localhost:9093 |
| Contract exporter | http://localhost:9200/metrics |

Set `CONTRACT_ID` and optionally `HORIZON_URL` in the environment (or `monitoring/.env`) so the exporter can scrape the live contract.

## Alerts

Critical alerts include high contract error rate, Horizon latency, exporter/target down, **BackupFailed**, and **NoRecentBackup**. Slack webhooks are configured in `monitoring/alertmanager/alertmanager.yml`.
