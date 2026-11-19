import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import MayhemContract from '../intercom/contract/contract.js';

// Use the installed contract's pure normalization rules. This performs no
// ledger reads, writes, signing or payment; canonical simulation still follows.
export function validateTapSettlementCheckpoint(report, Contract = MayhemContract) {
  const contract = new Contract({}, {});
  const normalized = contract.normalizeTargetedTapSettlementValue(
    report?.tap_settlement_checkpoint
  );
  if (normalized instanceof Error) throw normalized;
  return normalized;
}

if (import.meta.main ?? (process.argv[1] && fs.existsSync(process.argv[1]) && fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url))) {
  try {
    const args = process.argv.slice(2);
    const historical = args.length === 2 && args[0] === '--confirmed-v30';
    if (!historical && args.length !== 1) throw new Error('Expected a TAP settlement report path');
    const Contract = historical ? (await import('../intercom/contract/v30.js')).default : MayhemContract;
    validateTapSettlementCheckpoint(JSON.parse(fs.readFileSync(args.at(-1), 'utf8')), Contract);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
