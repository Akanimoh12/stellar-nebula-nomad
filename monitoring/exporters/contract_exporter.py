#!/usr/bin/env python3
"""Prometheus exporter for Stellar Nebula Nomad contract metrics.

Scrapes Horizon for ledger/contract snapshots and exposes Prometheus text
so Grafana dashboards and alert rules have a live target.
"""

from __future__ import annotations

import json
import os
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HORIZON_URL = os.environ.get("HORIZON_URL", "https://horizon-testnet.stellar.org").rstrip("/")
CONTRACT_ID = os.environ.get("CONTRACT_ID", "")
PORT = int(os.environ.get("EXPORTER_PORT", "9200"))
SCRAPE_TIMEOUT = float(os.environ.get("SCRAPE_TIMEOUT", "8"))


def horizon_get(path: str) -> tuple[int, dict | list | None]:
    url = f"{HORIZON_URL}{path}"
    req = urllib.request.Request(url, headers={"Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=SCRAPE_TIMEOUT) as resp:
            body = resp.read().decode("utf-8")
            return resp.status, json.loads(body) if body else None
    except urllib.error.HTTPError as exc:
        return exc.code, None
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, OSError):
        return 0, None


def collect() -> dict:
    metrics: dict[str, float] = {
        "nebula_exporter_up": 0,
        "nebula_horizon_up": 0,
        "nebula_horizon_latest_ledger": 0,
        "nebula_horizon_closed_at": 0,
        "nebula_contract_invocations_total": 0,
        "nebula_contract_errors_total": 0,
        "nebula_contract_scrape_success": 0,
        "horizon_request_duration_seconds": 0,
    }

    start = time.time()
    status, ledger_page = horizon_get("/ledgers?order=desc&limit=1")
    metrics["horizon_request_duration_seconds"] = time.time() - start

    if status == 200 and isinstance(ledger_page, dict):
        records = (
            ledger_page.get("_embedded", {}).get("records")
            if isinstance(ledger_page.get("_embedded"), dict)
            else None
        )
        if records:
            latest = records[0]
            metrics["nebula_horizon_up"] = 1
            metrics["nebula_horizon_latest_ledger"] = float(latest.get("sequence") or 0)
            closed = latest.get("closed_at") or latest.get("closedAt")
            if isinstance(closed, str):
                # Keep a numeric presence; Grafana uses the ledger gauge.
                metrics["nebula_horizon_closed_at"] = 1

    if CONTRACT_ID:
        c_status, _payload = horizon_get(f"/contracts/{CONTRACT_ID}")
        if c_status == 0:
            c_status, _payload = horizon_get(f"/accounts/{CONTRACT_ID}")
        if c_status == 200:
            metrics["nebula_contract_scrape_success"] = 1
            metrics["nebula_contract_invocations_total"] = 1
        elif c_status:
            metrics["nebula_contract_errors_total"] = 1

    metrics["nebula_exporter_up"] = 1
    return metrics


def render_metrics(values: dict) -> str:
    lines = [
        "# HELP nebula_exporter_up 1 if the contract exporter process is healthy.",
        "# TYPE nebula_exporter_up gauge",
        f"nebula_exporter_up {values['nebula_exporter_up']}",
        "# HELP nebula_horizon_up 1 if Horizon returned the latest ledger.",
        "# TYPE nebula_horizon_up gauge",
        f"nebula_horizon_up {values['nebula_horizon_up']}",
        "# HELP nebula_horizon_latest_ledger Latest Horizon ledger sequence.",
        "# TYPE nebula_horizon_latest_ledger gauge",
        f"nebula_horizon_latest_ledger {values['nebula_horizon_latest_ledger']}",
        "# HELP nebula_contract_invocations_total Contract scrape/invocation proxy counter.",
        "# TYPE nebula_contract_invocations_total counter",
        f"nebula_contract_invocations_total {values['nebula_contract_invocations_total']}",
        "# HELP nebula_contract_errors_total Contract scrape/error proxy counter.",
        "# TYPE nebula_contract_errors_total counter",
        f"nebula_contract_errors_total {values['nebula_contract_errors_total']}",
        "# HELP nebula_contract_scrape_success 1 if Horizon returned the configured contract.",
        "# TYPE nebula_contract_scrape_success gauge",
        f"nebula_contract_scrape_success {values['nebula_contract_scrape_success']}",
        "# HELP horizon_request_duration_seconds Horizon GET latency for the last scrape.",
        "# TYPE horizon_request_duration_seconds gauge",
        f"horizon_request_duration_seconds {values['horizon_request_duration_seconds']:.6f}",
        "# HELP nebula_contract_gas_instructions Dummy histogram bucket so dashboards resolve.",
        "# TYPE nebula_contract_gas_instructions histogram",
        "nebula_contract_gas_instructions_bucket{le=\"1000000\"} 0",
        "nebula_contract_gas_instructions_bucket{le=\"+Inf\"} 0",
        "nebula_contract_gas_instructions_sum 0",
        "nebula_contract_gas_instructions_count 0",
        "# HELP horizon_request_duration_seconds_bucket Histogram-compatible Horizon latency.",
        "# TYPE horizon_request_duration_seconds histogram",
        f"horizon_request_duration_seconds_bucket{{le=\"0.5\"}} {1 if values['horizon_request_duration_seconds'] <= 0.5 else 0}",
        f"horizon_request_duration_seconds_bucket{{le=\"1\"}} {1 if values['horizon_request_duration_seconds'] <= 1 else 0}",
        f"horizon_request_duration_seconds_bucket{{le=\"3\"}} {1 if values['horizon_request_duration_seconds'] <= 3 else 0}",
        "horizon_request_duration_seconds_bucket{le=\"+Inf\"} 1",
        f"horizon_request_duration_seconds_sum {values['horizon_request_duration_seconds']:.6f}",
        "horizon_request_duration_seconds_count 1",
        "",
    ]
    return "\n".join(lines)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt: str, *args) -> None:  # noqa: A003
        return

    def do_GET(self) -> None:  # noqa: N802
        if self.path.startswith("/health"):
            body = b'{"status":"ok"}\n'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        if self.path.startswith("/metrics"):
            body = render_metrics(collect()).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; version=0.0.4")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        self.send_response(404)
        self.end_headers()


def main() -> None:
    server = ThreadingHTTPServer(("0.0.0.0", PORT), Handler)
    print(f"nebula contract exporter listening on :{PORT} horizon={HORIZON_URL}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
