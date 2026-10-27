import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import MayhemContract from '../intercom/contract/contract.js';

// Use the installed contract's pure normalization rules. This performs no
// ledger reads, writes, signing or payment; canonical simulation still follows.
export function validateTapSettlementCheckpoint(report) {
  const contract = new MayhemContract({}, {});
  const normalized = contract.normalizeTargetedTapSettlementValue(
    report?.tap_settlement_checkpoint
  );
  if (normalized instanceof Error) throw normalized;
  return normalized;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.length !== 3) throw new Error('Expected one TAP settlement report path');
    validateTapSettlementCheckpoint(JSON.parse(fs.readFileSync(process.argv[2], 'utf8')));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
