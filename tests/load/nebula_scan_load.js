import { sleep } from 'k6';
import { buildOptions, makeSummary, simulate } from './config.js';

// Nebula scanning baseline: 50 VUs for 5 min (100+ concurrent scans under spike/stress).
// Default profile: baseline (override with PROFILE=baseline|stress|spike|soak|breakpoint|smoke).
export const options = buildOptions('baseline');

export default function () {
  simulate(__ENV.TX_XDR_SCAN, 'nebula_scan');
  sleep(Math.random() * 0.5);
}

export const handleSummary = makeSummary('nebula_scan');
