import { sleep } from 'k6';
import { buildOptions, makeSummary, simulate } from './config.js';

// Trading spike: 10 -> 500 -> 10 VUs.
// Default profile: spike (override with PROFILE=baseline|stress|spike|soak|breakpoint|smoke).
export const options = buildOptions('spike');

export default function () {
  simulate(__ENV.TX_XDR_TRADE, 'trading');
  sleep(Math.random() * 0.5);
}

export const handleSummary = makeSummary('trading');
