import { sleep } from 'k6';
import { buildOptions, makeSummary, simulate } from './config.js';

// Resource minting stress: ramp to 250 VUs over 10 min.
// Default profile: stress (override with PROFILE=baseline|stress|spike|soak|breakpoint|smoke).
export const options = buildOptions('stress');

export default function () {
  simulate(__ENV.TX_XDR_MINT, 'resource_mint');
  sleep(Math.random() * 0.5);
}

export const handleSummary = makeSummary('resource_mint');
