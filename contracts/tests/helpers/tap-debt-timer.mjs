// Launch one iteration of the installed shell loop in a fresh process. Only
// the transports cross inherited pipes. The coordinator, financial checks,
// spool locks, signing requests, EVM transactions and Core reducer are real.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import { registerHooks } from 'node:module';
import path from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { ethers } from 'ethers';
import { POOL_SETTLEMENT_ABI } from '../../scripts/tap-settlement-roller.mjs';

const moduleFile = fileURLToPath(import.meta.url);
const repo = fileURLToPath(new URL('../../../', import.meta.url));
const stringify = x => JSON.stringify(x, (_, v) => typeof v === 'bigint' ? String(v) : v);
let request;
if (process.env.TAP_TEST_PIPE === '1') {
  let id = 0;
  // Anonymous inherited pipes also work on hosts without loopback access.
  // Keep no reader alive between calls so a normal idle CLI exits on its own.
  request = (op, params) => {
    const retry = fn => {
      for (;;) {
        try { return fn(); } catch (e) {
          if (e.code !== 'EAGAIN' && e.code !== 'EINTR') throw e;
          Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 1);
        }
      }
    };
    const packet = Buffer.from(stringify({ id: ++id, op, params }) + '\n');
    for (let pos = 0; pos < packet.length;) pos += retry(() => fs.writeSync(4, packet, pos, packet.length - pos));
    const chunks = [];
    for (;;) {
      const buf = Buffer.allocUnsafe(65536);
      const n = retry(() => fs.readSync(3, buf, 0, buf.length));
      if (!n) throw Error('TAP timer fixture transport closed');
      chunks.push(buf.subarray(0, n));
      if (buf[n - 1] === 10) break;
    }
    const reply = JSON.parse(Buffer.concat(chunks));
    assert.equal(reply.id, id);
    if (reply.error) throw Error(reply.error);
    return reply.value;
  };
  globalThis.fetch = async (url, options = {}) => {
    const result = await request('core', [String(url), options.method, options.body]);
    return Response.json(result.body, { status: result.status });
  };
  registerHooks({
    resolve(spec, ctx, next) {
      if (spec === './tap-settlement-roller.mjs' && ctx.parentURL?.endsWith('/tap-debt-worker.mjs')) {
        return { shortCircuit: true, url: 'fixture:timer-transports' };
      }
      return next(spec, ctx);
    },
    load(url, ctx, next) {
      if (url !== 'fixture:timer-transports') return next(url, ctx);
      const actual = new URL('../../scripts/tap-settlement-roller.mjs', import.meta.url);
      return { format: 'module', shortCircuit: true, source:
        'export * from ' + JSON.stringify(actual.href) + '\nexport { connectTapSettlementPool, tapPreparationAdminSignerFromEnv } from ' +
        JSON.stringify(import.meta.url) };
    },
  });

}

export async function connectTapSettlementPool({ poolAddress }) {
  const provider = new ethers.BrowserProvider({ request: async ({ method, params }) => request('eth', [method, params]) },
    undefined, { cacheTimeout: -1 });
  const owner = await provider.getSigner();
  const governance = {
    getAddress: () => request('governance address', []),
    signTypedData: (...args) => request('governance signature', args),
  };
  return { provider, pool: new ethers.Contract(poolAddress, POOL_SETTLEMENT_ABI, provider),
    signer: { wallet: owner }, governance: { wallet: governance } };
}

export async function tapPreparationAdminSignerFromEnv() {
  return { publicKey: await request('admin key', []), sign: msg => request('admin signature', [msg]) };
}

export async function runDebtTimer({ spool, provider, governanceSigner, adminSigner, fetchImpl, peerRpcUrl }) {
  const envFile = path.join(spool, 'timer-test-env');
  if (!fs.existsSync(envFile)) fs.writeFileSync(envFile, `
flock() {
  python3 -c 'import sys,fcntl
mode = {"-n": fcntl.LOCK_EX|fcntl.LOCK_NB, "-x": fcntl.LOCK_EX, "-u": fcntl.LOCK_UN}[sys.argv[1]]
fcntl.flock(int(sys.argv[2]),mode)' "$@"
}
timeout() { shift; "$@"; }
node() {
  if [[ "$1" == */mainnet-proof.mjs ]]; then echo '{}'; return; fi
  command "$TAP_TEST_NODE" "$@"
}
sleep() { exit 0; }
`);
  const options = [];
  for (let i = 0; i < process.execArgv.length; i++) {
    if (process.execArgv[i] === '--import') options.push('--import=' + JSON.stringify(path.resolve(process.execArgv[++i])));
  }
  options.push('--import=' + JSON.stringify(moduleFile));
  const child = spawn('bash', [path.join(repo, 'scripts/ops/run-tap-settlement-roller.sh')], {
    env: { PATH: process.env.PATH, TAP_TEST_NODE: process.execPath, BASH_ENV: envFile, NODE_OPTIONS: options.join(' '),
      TAP_TEST_PIPE: '1', MAYHEM_REPO: repo, MAYHEM_PEER_RPC: peerRpcUrl,
      MAYHEM_TAP_SETTLEMENT_SPOOL: spool, MAYHEM_CADENCE_STATE_DIR: spool, MAYHEM_NETWORK: 'mainnet',
      MAYHEM_TAP_ETH_CHAIN_ID: '1', MAYHEM_TAP_POOL_ADDRESS: '0xcFEA9A256F1F96269D848cABF1eCb00fD2DD6a28',
      MAYHEM_TAP_OPERATOR_ADDRESS: '0x' + '8'.padStart(40, '0'),
      // Shape-only shell inputs. Neither is a valid secp256k1 scalar.
      MAYHEM_TAP_ROLLER_PRIVATE_KEY: '0x' + '0'.repeat(64),
      MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY: '0x' + 'f'.repeat(64) },
    stdio: ['ignore', 'pipe', 'pipe', 'pipe', 'pipe'], timeout: 30000,
  });
  let stdout = '', stderr = '';
  child.stdout.on('data', x => { stdout += x; });
  child.stderr.on('data', x => { stderr += x; });
  const replies = child.stdio[3];
  replies.on('error', () => {});
  const lines = createInterface({ input: child.stdio[4] });
  lines.on('line', async line => {
    const { id, op, params } = JSON.parse(line);
    let value, error;
    try {
      switch (op) {
        case 'eth': value = await provider.send(...params); break;
        case 'admin key': value = adminSigner.publicKey; break;
        case 'admin signature': value = await adminSigner.sign(...params); break;
        case 'governance address': value = await governanceSigner.getAddress(); break;
        case 'governance signature': value = await governanceSigner.signTypedData(...params); break;
        case 'core': {
          const [url, method, body] = params;
          const r = await fetchImpl(new URL(url), { method, body });
          value = { status: r.status, body: await r.json() };
          break;
        }
        default: throw Error('Unexpected timer transport');
      }
    } catch (e) { error = e.message; }
    replies.write(stringify({ id, value, error }) + '\n');
  });
  const status = await new Promise((resolve, reject) => { child.on('error', reject); child.on('close', resolve); });
  assert.equal(status, 0, stderr);
  if (stderr) throw Error(stderr.trim());
  return stdout.trim() || 'idle';
}
