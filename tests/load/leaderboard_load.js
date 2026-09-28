import { sleep } from 'k6';
import { buildOptions, makeSummary, simulate } from './config.js';

// Leaderboard query soak: 30 VUs for 2 hours (watch for leaks/latency drift).
// Default profile: soak (override with PROFILE=baseline|stress|spike|soak|breakpoint|smoke).
export const options = buildOptions('soak');

export default function () {
  simulate(__ENV.TX_XDR_LEADERBOARD, 'leaderboard');
  sleep(Math.random() * 0.5);
}

export const handleSummary = makeSummary('leaderboard');
