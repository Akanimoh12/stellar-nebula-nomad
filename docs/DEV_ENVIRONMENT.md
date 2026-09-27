# Local development environment

One command starts Stellar Quickstart, a hot-reload contract workspace, and the monitoring stack.

```bash
docker compose -f docker-compose.dev.yml up --build
```

| Service | Purpose |
|---------|---------|
| `stellar` | [Stellar Quickstart](https://github.com/stellar/quickstart) with Horizon + Soroban RPC on port 8000 |
| `contract` | `Dockerfile.dev` + `cargo watch` — rebuilds/tests on `src/` and `tests/` changes |
| `contract-exporter` | Prometheus scrape target for Horizon/contract metrics |
| `prometheus` / `grafana` / `alertmanager` | Observability (see `docs/MONITORING_SETUP.md`) |

Code is bind-mounted into `contract`, so host edits hot-reload inside the container.

Without Docker:

```bash
cargo install cargo-watch --locked
cargo watch -x check -x "test --lib"
```
