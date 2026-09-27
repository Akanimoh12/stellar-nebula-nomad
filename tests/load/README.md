# Load Testing Suite

This directory contains load testing scripts for the Stellar Nebula Nomad contract to aid in capacity planning and bottleneck identification.

## Prerequisites

- Install [k6](https://k6.io/docs/get-started/installation/)

## Running the Stress Test

The `k6_stress_test.js` script simulates concurrent read traffic (such as fetching events) against a specified RPC endpoint.

To execute the test, export the contract ID and run:

```bash
export CONTRACT_ID="C..."
# Optional: export RPC_URL="https://your-custom-rpc.local"
k6 run tests/load/k6_stress_test.js
```

### Metrics and Thresholds
- By default, the script ramps up to 20 concurrent Virtual Users (VUs).
- The p95 response time threshold is set to `< 500ms`. If the RPC nodes take longer, the test will be marked as failed.
This directory contains load tests for the Stellar Nebula Nomad project using k6.

## Running tests
To run the load tests locally, ensure you have k6 installed and run:

```sh
k6 run k6-script.js
```

## Scenario suite (Issue #481)

| Script | Default profile | Load |
| --- | --- | --- |
| `nebula_scan_load.js` | baseline | 50 VUs, 5 min |
| `resource_mint_load.js` | stress | ramp to 250 VUs, 10 min |
| `trading_load.js` | spike | 10 → 500 → 10 VUs |
| `leaderboard_load.js` | soak | 30 VUs, 2 h |
| any script with `-e PROFILE=breakpoint` | breakpoint | 0 → 1000 req/s, aborts on SLA breach |

Shared settings live in `config.js`. Pass signed transaction envelopes
(`TX_XDR_SCAN`, `TX_XDR_MINT`, `TX_XDR_TRADE`, `TX_XDR_LEADERBOARD`) so
calls go through `simulateTransaction`. Without them, the scripts fall back
to `getLatestLedger`.

```sh
k6 run -e PROFILE=stress -e TX_XDR_MINT="$XDR" tests/load/resource_mint_load.js
```

- **SLAs:** p95 < 2s, success rate > 99.9% (`success_rate`, `http_req_failed`).
- **Gas metrics:** `gas_min_resource_fee` and `gas_cpu_instructions` trends, per endpoint.
- **Reports:** each run writes `load-report-<name>.html` (graphs) and `load-summary-<name>.json`.
- **CI:** `.github/workflows/load-tests.yml` runs a smoke profile on PRs that
  touch critical paths and posts the summary as a PR comment.

### Known bottlenecks / optimization opportunities
- `scan_nebula` is rate limited to 5 scans/min per player, so concurrent-scan
  tests must spread load across many accounts.
- Leaderboard reads sort the full player set on each query. Cache snapshots
  or tier the storage to keep soak latency flat.
