import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import PeerWallet from 'trac-wallet';
import { safeEncodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { bigIntTo16ByteBuffer } from 'trac-msb/src/utils/amountSerialization.js';
import { OperationType } from 'trac-msb/src/utils/constants.js';

export const testTnkAddress = hex => PeerWallet.encodeBech32mSafe('testtrac', Buffer.from(hex, 'hex'));
export async function createTnkDiscoveryFixture({ hash, destination, position = 10 }) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'proxy-tnk-discovery-'));
  const store = new Corestore(directory), view = new Hyperbee(store.get({ name: 'fixture' }),
    { keyEncoding: 'utf-8', valueEncoding: 'binary', extension: false });
  await view.ready();
  while (view.core.length < position) await view.put(`padding/${view.core.length}`, Buffer.from('not a transaction'));
  const bytes = safeEncodeApplyOperation({ type: OperationType.TRANSFER, address: Buffer.from(destination),
    tro: { tx: Buffer.from(hash, 'hex'), txv: Buffer.alloc(32, 1), in: Buffer.alloc(32, 2),
      to: Buffer.from(destination), am: bigIntTo16ByteBuffer(9n), is: Buffer.alloc(64, 3) } });
  await view.put(hash, bytes);
  return { view, msb: { state: { base: { view }, getSignedLength: () => view.core.signedLength },
    getTxHashes() { throw Error('moving hash scan forbidden'); }, getTxDetails() { throw Error('moving payload read forbidden'); } },
    async close() { await view.close(); await store.close(); await fs.rm(directory, { recursive: true, force: true }); } };
}
