import http from 'k6/http';
import { check } from 'k6';
import { Rate, Trend } from 'k6/metrics';
import { htmlReport } from 'https://raw.githubusercontent.com/benc-uk/k6-reporter/2.4.0/dist/bundle.js';
import { textSummary } from 'https://jslib.k6.io/k6-summary/0.0.2/index.js';

// Shared config for the Stellar Nebula Nomad k6 suite (Issue #481).
//
// Each scenario simulates a pre-built, signed transaction envelope against
// Soroban RPC. Build the envelopes with the stellar CLI and pass them in:
//   RPC_URL           Soroban RPC endpoint (default: testnet)
//   TX_XDR_SCAN       scan_nebula tx envelope (base64 XDR)
//   TX_XDR_MINT       resource mint tx envelope
//   TX_XDR_TRADE      trade tx envelope
//   TX_XDR_LEADERBOARD leaderboard query tx envelope
export const RPC_URL = __ENV.RPC_URL || 'https://soroban-testnet.stellar.org';

// SLAs: p95 < 2s, success rate > 99.9%.
export const SLA_THRESHOLDS = {
  http_req_duration: ['p(95)<2000'],
  success_rate: ['rate>0.999'],
  http_req_failed: ['rate<0.001'],
};

export const successRate = new Rate('success_rate');
export const errorRate = new Rate('error_rate');
// Custom gas metric: Soroban reports the min resource fee (stroops) and CPU
// instructions for every simulated invocation.
export const gasFee = new Trend('gas_min_resource_fee');
export const gasCpu = new Trend('gas_cpu_instructions');

// Load profiles shared by all scripts; pick one with PROFILE=<name>.
export const PROFILES = {
  baseline: { executor: 'constant-vus', vus: 50, duration: '5m' },
  stress: {
    executor: 'ramping-vus',
    stages: [
      { duration: '2m', target: 250 },
      { duration: '6m', target: 250 },
      { duration: '2m', target: 0 },
    ],
  },
  spike: {
    executor: 'ramping-vus',
    stages: [
      { duration: '1m', target: 10 },
      { duration: '10s', target: 500 },
      { duration: '1m', target: 500 },
      { duration: '10s', target: 10 },
      { duration: '1m', target: 10 },
    ],
  },
  soak: { executor: 'constant-vus', vus: 30, duration: '2h' },
  breakpoint: {
    executor: 'ramping-arrival-rate',
    startRate: 0,
    timeUnit: '1s',
    preAllocatedVUs: 100,
    maxVUs: 1000,
    stages: [{ duration: '20m', target: 1000 }],
  },
  // Tiny profile for CI smoke runs.
  smoke: { executor: 'constant-vus', vus: 1, duration: '10s' },
};

export function buildOptions(defaultProfile) {
  const name = __ENV.PROFILE || defaultProfile;
  const scenario = Object.assign({}, PROFILES[name]);
  // Abort the breakpoint test as soon as SLAs break.
  const thresholds =
    name === 'breakpoint'
      ? {
          http_req_duration: [{ threshold: 'p(95)<2000', abortOnFail: true }],
          success_rate: [{ threshold: 'rate>0.999', abortOnFail: true }],
        }
      : SLA_THRESHOLDS;
  return { scenarios: { [name]: scenario }, thresholds };
}

// Call Soroban RPC. With an envelope, simulate it (gives gas figures);
// without one, fall back to getLatestLedger so the script still exercises RPC.
export function simulate(xdr, tag) {
  const body = xdr
    ? { jsonrpc: '2.0', id: 1, method: 'simulateTransaction', params: { transaction: xdr } }
    : { jsonrpc: '2.0', id: 1, method: 'getLatestLedger' };
  const res = http.post(RPC_URL, JSON.stringify(body), {
    headers: { 'Content-Type': 'application/json' },
    tags: { endpoint: tag },
  });

  let json = null;
  try {
    json = res.json();
  } catch (_) {
    json = null;
  }
  const ok = check(res, {
    'status is 200': (r) => r.status === 200,
    'no rpc error': () => json !== null && !json.error && !(json.result && json.result.error),
  });
  successRate.add(ok);
  errorRate.add(!ok);

  if (ok && json.result && json.result.minResourceFee) {
    gasFee.add(Number(json.result.minResourceFee), { endpoint: tag });
    const cost = json.result.cost;
    if (cost && cost.cpuInsns) gasCpu.add(Number(cost.cpuInsns), { endpoint: tag });
  }
  return ok;
}

// Writes an HTML report (with graphs) plus a stdout summary for PR comments.
export function makeSummary(name) {
  return (data) => ({
    [`load-report-${name}.html`]: htmlReport(data, { title: `Nebula Nomad load: ${name}` }),
    [`load-summary-${name}.json`]: JSON.stringify(data, null, 2),
    stdout: textSummary(data, { indent: ' ', enableColors: false }),
  });
}
