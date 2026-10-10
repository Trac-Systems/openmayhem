import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { proxyEpochAcceptances, proxyEpochReceipt } from './proxy-epoch.mjs';

/** Select TAP work from the apply-bound canonical epoch artifact. A proxy's
 * accounting rail comes from its signed acceptance and exact canonical head;
 * neither this adapter nor its caller rewrites the signed receipt body. */
export async function deriveTapReceiptBundle(bundle, epoch, applyHash) {
  if (!bundle || bundle.epoch !== epoch || !Number.isSafeInteger(epoch) || epoch < 1
    || !/^[0-9a-f]{64}$/.test(applyHash) || !Array.isArray(bundle.receipts)) {
    throw new Error('finalized receipt bundle epoch or apply binding is invalid');
  }
  const accepted = proxyEpochAcceptances(bundle, bundle.receipts), receipts = [], selected = {};
  for (const entry of bundle.receipts) {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) throw new Error('receipt entry must be an object');
    const body = entry.receipt?.body;
    if (!body || typeof body !== 'object' || Array.isArray(body)) throw new Error('signed receipt body must be an object');
    let rail;
    if (entry.lane === 'proxy' || body.lane === 'proxy') {
      const verified = await proxyEpochReceipt(entry, accepted, epoch);
      rail = verified.body.rail;
    } else {
      if ((entry.lane !== undefined && entry.lane !== 'native') || (body.lane !== undefined && body.lane !== 'native')) {
        throw new Error('unsupported receipt lane');
      }
      rail = body.rail;
      if (typeof entry.rail !== 'string' || typeof rail !== 'string') throw new Error('receipt outer rail and signed body rail are required');
      if (entry.rail !== entry.rail.toLowerCase() || rail !== rail.toLowerCase() || entry.rail !== rail) {
        throw new Error('receipt outer rail does not match signed receipt rail');
      }
    }
    if (!['fiat', 'tnk', 'tap'].includes(rail)) throw new Error('signed receipt rail is unsupported');
    if (rail === 'tap') {
      receipts.push({ ...structuredClone(entry), receipt_epoch: epoch });
      if (entry.lane === 'proxy') selected[entry.accepted_terms] = bundle.proxy_acceptances[entry.accepted_terms];
    }
  }
  return { ...bundle, rail: 'tap', epoch_apply_hash: applyHash, receipts,
    ...(bundle.proxy_acceptances === undefined ? {} : { proxy_acceptances: structuredClone(selected) }) };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [source, target, epoch, applyHash] = process.argv.slice(2);
    const result = await deriveTapReceiptBundle(JSON.parse(await fs.readFile(source, 'utf8')), Number(epoch), applyHash);
    await fs.writeFile(target, JSON.stringify(result, null, 2) + '\n', { mode: 0o600 });
    process.stdout.write(String(result.receipts.length) + '\n');
  } catch (error) {
    process.stderr.write(String(error.message) + '\n'); process.exitCode = 1;
  }
}
