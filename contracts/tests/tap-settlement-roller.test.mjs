import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import Ganache from 'ganache';
import { ethers } from 'ethers';
import b4a from '../../intercom/node_modules/b4a/index.js';
import PeerWallet from '../../intercom/node_modules/trac-wallet/index.js';
import { receiptMessage as contractReceiptMessage } from '../../intercom/contract/contract.js';

import { deployPool } from '../scripts/deploy-local.mjs';
import { distribution } from '../scripts/merkle.mjs';
import { signRootProposal } from '../scripts/pool-governance.mjs';
import {
  buildCanonicalTapPreparationPlan,
  buildTapSettlement,
  encodeBurnCalldata,
  encodeWithdrawOperatorCalldata,
  POOL_SETTLEMENT_ABI,
  guardianPreSignReport,
  auToTapWei,
  providerShareWei,
  receiptMessage,
  resolveTargetedTapPayoutsFromLedger,
  resolveTapSettlementRate,
  resolveTapSettlementEpochPolicy,
  resolveTapSettlementPayoutMinimum,
  rollTapSettlement,
  verifyReceiptEnvelope,
} from '../scripts/tap-settlement-roller.mjs';
import { makeReceiptIdentity, signedTapReceipt } from './helpers/signed-receipt.mjs';

const TAP_USD_AU = '1000000000000000000';
const usdAu = (value) => (BigInt(value) * 1_000_000_000_000_000_000n).toString();
const providerNetAu = (grossAu) => {
  const gross = BigInt(grossAu);
  return gross - gross * 1_500n / 10_000n - gross * 1_000n / 10_000n;
};
const U = (n) => ethers.parseUnits(String(n), 18);
const SCRIPT_PATH = fileURLToPath(new URL('../scripts/tap-settlement-roller.mjs', import.meta.url));
const OPERATOR_KEY = `0x${'11'.repeat(32)}`;
const BUYER_KEY = `0x${'22'.repeat(32)}`;
const PROVIDER_KEY = `0x${'33'.repeat(32)}`;
const GANACHE_BALANCE = ethers.toBeHex(ethers.parseEther('100'));
const BURN_SINK = '0x000000000000000000000000000000000000dEaD';

function runNode(args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, args, {
      ...options,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    const timeout = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`child process timed out: ${args.join(' ')}`));
    }, 20_000);
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', (chunk) => { stdout += chunk; });
    child.stderr.on('data', (chunk) => { stderr += chunk; });
    child.on('error', (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    child.on('close', (status, signal) => {
      clearTimeout(timeout);
      resolve({ status, signal, stdout, stderr });
    });
  });
}

function receipt(options) {
  const session = String(options?.session ?? '');
  return signedTapReceipt({
    ...options,
    extraBody: {
      schema_version: 10,
      billing_epoch: options?.epoch ?? 1,
      reservation_id: crypto.createHash('sha256')
        .update(`reservation:${session}`)
        .digest('hex'),
      payout_revision: '11'.repeat(32),
      ...options?.extraBody,
    },
  });
}

test('TAP roller uses the canonical contract schema-10 receipt bytes', () => {
  const body = receipt({
    session: 'tap-contract-golden',
    au: '123',
    epoch: 17,
    extraBody: {
      reservation_expires_after_epoch: 23,
      reservation_receipt_grace_epochs: 6,
      payout_revision: '42'.repeat(32),
    },
  }).receipt.body;

  assert.equal(receiptMessage(body), contractReceiptMessage(body));
});

test('TAP roller reconstructs Rust receipt wire order from sorted retained values', () => {
  const user = makeReceiptIdentity();
  const enclave = makeReceiptIdentity();
  const signed = receipt({
    session: 'tap-retained-wire-order',
    au: '123',
    epoch: 17,
    user,
    enclave,
  });
  const body = signed.receipt.body;
  body.locked_rate_map = [{
    granularity: 1,
    per_unit_au: '123',
    unit: 'input_token',
  }];
  body.workflow = {
    graph_hash: '31'.repeat(32),
    outcome_class: 'image.heavy.le1_2mp',
    quoted_usage: { step: 4, image: 1 },
    runtime_id: 'comfyui-v0.30.1',
    endpoint_family: 'mayhem_comfy_workflows',
  };
  body.workflow_output = {
    metrics: { step: 4, image: 1 },
    output_modalities: ['image'],
  };

  const rustWireBody = {
    schema_version: body.schema_version,
    session_id: body.session_id,
    billing_id: body.billing_id,
    billing_attempt: body.billing_attempt,
    billing_prior_usage: body.billing_prior_usage,
    billing_prior_au_owed_cum: body.billing_prior_au_owed_cum,
    billing_epoch: body.billing_epoch,
    reservation_id: body.reservation_id,
    reservation_expires_after_epoch: body.reservation_expires_after_epoch,
    reservation_receipt_grace_epochs: body.reservation_receipt_grace_epochs,
    payout_revision: body.payout_revision,
    seq: body.seq,
    final: body.final,
    rail: body.rail,
    user: body.user,
    provider: body.provider,
    enclave_id: body.enclave_id,
    model_id: body.model_id,
    price_ver: body.price_ver,
    locked_rate_map: [{ unit: 'input_token', per_unit_au: '123', granularity: 1 }],
    locked_per_req_au: body.locked_per_req_au,
    locked_min_session_au: body.locked_min_session_au,
    served_ctx: body.served_ctx,
    ctx_bracket: body.ctx_bracket,
    ctx_bracket_table_ver: body.ctx_bracket_table_ver,
    rules_ver: body.rules_ver,
    workflow: {
      endpoint_family: 'mayhem_comfy_workflows',
      graph_hash: '31'.repeat(32),
      runtime_id: 'comfyui-v0.30.1',
      outcome_class: 'image.heavy.le1_2mp',
      quoted_usage: { image: 1, step: 4 },
    },
    workflow_output: {
      output_modalities: ['image'],
      metrics: { image: 1, step: 4 },
    },
    usage: body.usage,
    au_owed_cum: body.au_owed_cum,
    prompt_hash: body.prompt_hash,
    ts: body.ts,
  };
  const message = Buffer.from(JSON.stringify({
    domain: 'mayhem-session-receipt',
    signing_version: 2,
    body: rustWireBody,
  }));
  signed.receipt.enclave_sig = crypto.sign(null, message, enclave.privateKey).toString('hex');
  signed.receipt.user_sig = crypto.sign(null, message, user.privateKey).toString('hex');

  assert.equal(receiptMessage(body), message.toString());
  assert.doesNotThrow(() => verifyReceiptEnvelope(signed.receipt));
});

test('TAP roller accepts current schema 11 while rejecting unknown receipt schemas', () => {
  const current = receipt({
    session: 'tap-current-schema',
    au: '123',
    epoch: 17,
    extraBody: { schema_version: 11 },
  });
  assert.doesNotThrow(() => verifyReceiptEnvelope(current.receipt));

  for (const schemaVersion of [9, 12]) {
    const unsupported = structuredClone(current.receipt);
    unsupported.body.schema_version = schemaVersion;
    assert.throws(
      () => verifyReceiptEnvelope(unsupported),
      /receipt schema_version must be one of 10, 11/
    );
  }
});

test('TAP roller locks payout minimum from confirmed historical parameter evidence', async () => {
  const calls = [];
  const fetchImpl = async (url) => {
    calls.push(String(url));
    return {
      ok: true,
      async json() {
        return {
          key: 'params/payout_min_au',
          confirmed: true,
          signed_length: 321,
          value: {
            key: 'payout_min_au',
            current: { value: '100', ver: 1, effective_at: 10 },
            pending: { value: '250', ver: 2, effective_at: 20 },
          },
        };
      },
    };
  };

  const before = await resolveTapSettlementPayoutMinimum({
    peerRpcUrl: 'http://127.0.0.1:49223/v1',
    at: 19,
    fetchImpl,
  });
  const after = await resolveTapSettlementPayoutMinimum({
    peerRpcUrl: 'http://127.0.0.1:49223/v1',
    at: 20,
    fetchImpl,
  });

  assert.equal(before.value, '100');
  assert.deepEqual(after.evidence, {
    type: 'tap_payout_minimum_lock',
    key: 'params/payout_min_au',
    at: 20,
    signed_length: 321,
    value: '250',
    version: 2,
    effective_at: 20,
  });
  assert.equal(calls.length, 2);
  assert.ok(calls.every((url) => url.includes('confirmed=true')));
});

function targetedBindingsFor(bundle, providerAccounts, revisions = {}) {
  bundle.params ??= {};
  bundle.params.payout_min_au ??= '0';
  const bindings = {};
  const billingTotals = new Map();
  const entries = [...(bundle.receipts ?? [])].sort((left, right) => {
    const a = left.receipt?.body ?? left.body ?? left;
    const b = right.receipt?.body ?? right.body ?? right;
    return String(a.billing_id).localeCompare(String(b.billing_id)) ||
      Number(a.billing_attempt) - Number(b.billing_attempt) ||
      Number(a.seq) - Number(b.seq);
  });
  for (const entry of entries) {
    const body = entry.receipt?.body ?? entry.body ?? entry;
    const epoch = entry.receipt_epoch ?? body.receipt_epoch ?? body.epoch ?? bundle.epoch;
    const key = `${epoch}/${body.user.toLowerCase()}/${body.session_id}`;
    const currentAu = BigInt(body.au_owed_cum);
    const previousAu = billingTotals.get(body.billing_id) ??
      BigInt(body.billing_prior_au_owed_cum);
    const au = currentAu - previousAu;
    billingTotals.set(body.billing_id, currentAu);
    const existing = bindings[key];
    bindings[key] = {
      epoch,
      session_id: body.session_id,
      user: body.user,
      provider: body.provider,
      payout_revision: revisions[body.provider] ?? '11'.repeat(32),
      account: providerAccounts[body.provider],
      chain_id: 61_000,
      context_revision: '22'.repeat(32),
      payment_config_version: 1,
      au: ((existing ? BigInt(existing.au) : 0n) + au).toString(),
    };
  }
  return bindings;
}

function canonicalLiabilitiesFor(bundle, providerAccounts, revisions = {}, overrides = {}) {
  const grossByIdentity = new Map();
  const billingTotals = new Map();
  for (const entry of [...(bundle.receipts ?? [])].sort((left, right) => {
    const a = left.receipt?.body ?? left.body ?? left;
    const b = right.receipt?.body ?? right.body ?? right;
    return String(a.billing_id).localeCompare(String(b.billing_id)) ||
      Number(a.billing_attempt) - Number(b.billing_attempt) ||
      Number(a.seq) - Number(b.seq);
  })) {
    const body = entry.receipt?.body ?? entry.body ?? entry;
    const currentAu = BigInt(body.au_owed_cum);
    const previousAu = billingTotals.get(body.billing_id) ??
      BigInt(body.billing_prior_au_owed_cum);
    const grossAu = currentAu - previousAu;
    billingTotals.set(body.billing_id, currentAu);
    const revision = revisions[body.provider] ?? body.payout_revision ?? '11'.repeat(32);
    const target = providerAccounts[body.provider].toLowerCase();
    const key = `${body.provider}/${revision}/${target}`;
    const current = grossByIdentity.get(key) ?? {
      provider: body.provider,
      rail: 'tap',
      payout_revision: revision,
      target,
      chain_id: 61_000,
      gross_au: 0n,
      updated_epoch: bundle.epoch ?? 1,
      updated_at: `epoch/targeted/${bundle.epoch ?? 1}/${'aa'.repeat(32)}`,
    };
    current.gross_au += grossAu;
    grossByIdentity.set(key, current);
  }
  return [...grossByIdentity].map(([key, entry]) => {
    const feeAu = entry.gross_au * 1_500n / 10_000n;
    const burnAu = entry.gross_au * 1_000n / 10_000n;
    const totalAu = entry.gross_au - feeAu - burnAu;
    return {
      provider: entry.provider,
      rail: 'tap',
      payout_revision: entry.payout_revision,
      target: entry.target,
      chain_id: entry.chain_id,
      total_au: totalAu.toString(),
      held_au: '0',
      paid_cum_au: '0',
      aggregate_paid_cum_au: '0',
      updated_epoch: entry.updated_epoch,
      updated_at: entry.updated_at,
      ...(overrides[key] ?? {}),
    };
  });
}

function checkpointArgs(bundle, poolAddress, tokenAddress) {
  bundle.epoch_apply_hash ??= 'ab'.repeat(32);
  return {
    epochApplyHash: bundle.epoch_apply_hash,
    tapRateLock: {
      type: 'tap_settlement_rate_lock',
      epoch: bundle.epoch,
      bundle_sha256: 'cd'.repeat(32),
      denom: 'tap_usd_au',
      tap_usd_au: TAP_USD_AU,
      source: 'test-fixed-rate',
      rate_ts: 3_600,
      rate_record_key: `rate/tap/3600/${'ef'.repeat(32)}`,
      posted_by: 'aa'.repeat(32),
      posted_by_role: 'admin',
      chain_id: 61_000,
      token_address: tokenAddress,
      pool_address: poolAddress,
      payment_config_ver: 1,
    },
  };
}

function memoryPreparationSubmitter() {
  const records = new Map();
  const submitter = async ({ plan }) => {
    submitter.calls += 1;
    let allExisting = true;
    const confirmed = [];
    for (const preparation of plan.preparations) {
      const existing = records.get(preparation.economic_op_id);
      if (existing === undefined) {
        allExisting = false;
        const record = {
          type: 'targeted_payout_preparation',
          ...structuredClone(preparation),
          consumed: false,
        };
        records.set(preparation.economic_op_id, record);
        submitter.created += 1;
        confirmed.push(record);
      } else {
        assert.deepEqual(
          {
            economic_op_id: existing.economic_op_id,
            kind: existing.kind,
            output_index: existing.output_index,
            payload: existing.payload,
            liability: existing.liability,
            external_effect_ids: existing.external_effect_ids,
          },
          preparation
        );
        confirmed.push(existing);
      }
    }
    return {
      ...plan,
      all_existing: allExisting,
      records: confirmed,
    };
  };
  submitter.records = records;
  submitter.calls = 0;
  submitter.created = 0;
  return submitter;
}

function crashAfterConfirmedPoolEffect(pool, effectName) {
  let armed = true;
  const wrap = (target) => new Proxy(target, {
    get(contract, property) {
      if (property === 'connect') {
        return (runner) => wrap(contract.connect(runner));
      }
      const value = Reflect.get(contract, property, contract);
      if (property !== effectName || typeof value !== 'function') {
        return typeof value === 'function' ? value.bind(contract) : value;
      }
      const effect = async (...args) => {
        const transaction = await value(...args);
        return new Proxy(transaction, {
          get(sent, transactionProperty) {
            const transactionValue = Reflect.get(sent, transactionProperty, sent);
            if (transactionProperty !== 'wait' || typeof transactionValue !== 'function') {
              return typeof transactionValue === 'function'
                ? transactionValue.bind(sent)
                : transactionValue;
            }
            return async (...waitArgs) => {
              const receipt = await transactionValue.apply(sent, waitArgs);
              if (armed) {
                armed = false;
                throw new Error(`simulated crash after confirmed ${effectName}`);
              }
              return receipt;
            };
          },
        });
      };
      effect.staticCall = (...args) => value.staticCall(...args);
      effect.estimateGas = (...args) => value.estimateGas(...args);
      return effect;
    },
  });
  return wrap(pool);
}

async function mineTapConfirmations(provider, count = 12) {
  for (let index = 0; index < count; index += 1) {
    await provider.send('evm_mine', []);
  }
}

async function rollWithTapFinality(provider, args) {
  const first = await rollTapSettlement(args);
  if (first.awaiting_finality !== true) return first;
  await mineTapConfirmations(provider);
  await new Promise((resolve) => setTimeout(resolve, 300));
  const confirmed = await rollTapSettlement({ ...args, prior: first });
  return {
    ...confirmed,
    posted: first.posted,
    proposal_tx: confirmed.proposal_tx ?? first.proposal_tx,
    execution_tx: confirmed.execution_tx ?? first.execution_tx,
    operator_fee: {
      ...confirmed.operator_fee,
      ...(first.operator_fee?.auto_sent ? first.operator_fee : {}),
    },
    burn: {
      ...confirmed.burn,
      ...(first.burn?.auto_sent ? first.burn : {}),
    },
  };
}

async function tapRollFixture(session) {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: { totalAccounts: 4 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const buyer = await provider.getSigner(1);
  const providerSigner = await provider.getSigner(2);
  const operatorTreasury = await provider.getSigner(3);
  const { token, pool, poolAddr, governanceWallet } = await deployPool(operator);
  await (await token.mint(await buyer.getAddress(), U(5))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(5))).wait();
  await (await pool.connect(buyer).deposit(U(5))).wait();
  const providerId = makeReceiptIdentity();
  const providerAccounts = {
    [providerId.publicKeyHex]: await providerSigner.getAddress(),
  };
  const bundle = {
    epoch: 1,
    receipts: [receipt({ session, provider: providerId, au: usdAu(1) })],
  };
  return {
    provider,
    pool,
    args: {
      bundle,
      targetedSessionBindings: targetedBindingsFor(bundle, providerAccounts),
      canonicalLiabilities: canonicalLiabilitiesFor(bundle, providerAccounts),
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      settleThroughEpoch: 7,
      pool,
      ownerSigner: operator,
      governanceSigner: governanceWallet,
      operatorAddress: await operatorTreasury.getAddress(),
      ...checkpointArgs(bundle, poolAddr, await token.getAddress()),
      post: true,
    },
  };
}

test('targeted TAP roots preserve each session payout revision across rotation', async () => {
  const providerId = makeReceiptIdentity();
  const oldAccount = ethers.Wallet.createRandom().address.toLowerCase();
  const newAccount = ethers.Wallet.createRandom().address.toLowerCase();
  const oldRevision = '11'.repeat(32);
  const newRevision = '22'.repeat(32);
  const oldReceipt = {
    ...receipt({
      session: 'targeted-old',
      provider: providerId,
      au: usdAu(1),
      extraBody: { payout_revision: oldRevision },
    }),
    receipt_epoch: 1,
  };
  const newReceipt = {
    ...receipt({
      session: 'targeted-new',
      provider: providerId,
      au: usdAu(1),
      extraBody: { payout_revision: newRevision },
    }),
    receipt_epoch: 2,
  };
  const state = new Map();
  for (const [entry, epoch, revision, account] of [
    [oldReceipt, 1, oldRevision, oldAccount],
    [newReceipt, 2, newRevision, newAccount],
  ]) {
    const body = entry.receipt.body;
    state.set(`payout/allocation/${epoch}/${body.session_id}`, {
      type: 'provider_payout_session_allocation',
      epoch,
      page: 0,
      session_id: body.session_id,
      user: body.user,
      rail: 'tap',
      provider: body.provider,
      payout_revision: revision,
      au: usdAu(1),
      feature_key: `epoch/targeted/${epoch}/${'aa'.repeat(32)}`,
    });
    state.set(`payout/binding/tap/${body.provider}/${revision}`, {
      verified: true,
      provider: body.provider,
      rail: 'tap',
      revision,
      target: account,
      chain_id: 61_000,
      activation_epoch: epoch,
      context_revision: '33'.repeat(32),
      payment_config_version: 7,
    });
    state.set(`payout/liability/tap/${body.provider}/${revision}`, {
      provider: body.provider,
      rail: 'tap',
      revision,
      target: account,
      currency: null,
      chain_id: 61_000,
      total_au: providerNetAu(usdAu(1)).toString(),
      held_au: '0',
      paid_cum_au: '0',
      updated_epoch: epoch,
      updated_at: `epoch/targeted/${epoch}/${'aa'.repeat(32)}`,
    });
    state.set(`earn/tap/${body.provider}`, {
      provider: body.provider,
      rail: 'tap',
      paid_cum_au: '0',
    });
  }
  const fetchImpl = async (url) => ({
    ok: true,
    json: async () => ({ value: state.get(new URL(url).searchParams.get('key')) ?? null }),
  });
  const bundle = {
    params: { payout_min_au: '0' },
    receipts: [oldReceipt, newReceipt],
  };
  const targeted = await resolveTargetedTapPayoutsFromLedger({
    bundle,
    peerRpcUrl: 'http://127.0.0.1:49223/v1',
    fetchImpl,
  });
  const settlement = buildTapSettlement({
    bundle,
    targetedSessionBindings: targeted.sessionBindings,
    canonicalLiabilities: targeted.liabilities,
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 8,
    challengeEpochs: 6,
  });
  const claim = providerShareWei(auToTapWei(usdAu(1), TAP_USD_AU));
  assert.deepEqual(
    settlement.providers,
    [oldAccount, newAccount]
      .sort()
      .map((account) => ({ account, cumulative_wei: claim.toString() }))
  );
  assert.deepEqual(
    settlement.payout_bindings.map((binding) => binding.payout_revision),
    [oldRevision, newRevision]
  );

  state.get(`payout/allocation/1/${oldReceipt.receipt.body.session_id}`).au = usdAu(2);
  const substituted = await resolveTargetedTapPayoutsFromLedger({
    bundle,
    peerRpcUrl: 'http://127.0.0.1:49223/v1',
    fetchImpl,
  });
  assert.throws(
    () => buildTapSettlement({
      bundle,
      targetedSessionBindings: substituted.sessionBindings,
      canonicalLiabilities: substituted.liabilities,
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      settleThroughEpoch: 8,
      challengeEpochs: 6,
    }),
    /does not equal targeted session allocation/
  );
});

test('TAP settlement rate lock survives oracle updates and rejects a different bundle', async (t) => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-tap-rate-lock-'));
  t.after(() => fs.rmSync(tmp, { recursive: true, force: true }));
  const lockPath = path.join(tmp, 'epoch-1.tap-rate.json');
  const admin = 'aa'.repeat(32);
  const rateKey = `rate/tap/3600/${'bb'.repeat(32)}`;
  const bundle = { epoch: 1, receipts: [] };
  const poolAddress = '0x1111111111111111111111111111111111111111';
  const tokenAddress = '0x2222222222222222222222222222222222222222';
  let canonicalPoolAddress = poolAddress;
  let fetches = 0;
  const fetchImpl = async (url) => {
    fetches += 1;
    const key = new URL(url).searchParams.get('key');
    return {
      ok: true,
      async json() {
        if (key === 'admin') return { value: admin };
        if (key === 'payments/current') {
          return {
            value: {
              denom: 'au_usd',
              ver: 4,
              tap: {
                chain_id: 1,
                token_address: tokenAddress,
                pool_address: canonicalPoolAddress,
              },
              set_by: admin,
              set_by_role: 'admin',
            },
          };
        }
        if (key === 'tap/rate/latest') {
          return {
            value: {
              denom: 'tap_usd_au',
              tap_usd_au: TAP_USD_AU,
              source: 'uniswap-v2-twap-median',
              ts: 3_600,
              updated_at: rateKey,
              posted_by: admin,
              posted_by_role: 'admin',
            },
          };
        }
        return { value: null };
      },
    };
  };

  const first = await resolveTapSettlementRate({
    bundle,
    tapRateLockPath: lockPath,
    peerRpcUrl: 'http://127.0.0.1:1/v1',
    fetchImpl,
  });
  assert.equal(first.tap_usd_au, TAP_USD_AU);
  assert.equal(first.rate_record_key, rateKey);
  assert.equal(first.chain_id, 1);
  assert.equal(first.token_address, tokenAddress);
  assert.equal(first.pool_address, poolAddress);
  assert.equal(first.payment_config_ver, 4);
  assert.equal(fetches, 3);
  assert.equal(fs.statSync(lockPath).mode & 0o777, 0o600);

  const replay = await resolveTapSettlementRate({
    bundle,
    tapRateLockPath: lockPath,
    peerRpcUrl: 'http://127.0.0.1:1/v1',
    fetchImpl,
  });
  assert.deepEqual(replay, first);
  assert.equal(fetches, 5);
  canonicalPoolAddress = '0x3333333333333333333333333333333333333333';
  await assert.rejects(
    resolveTapSettlementRate({
      bundle,
      tapRateLockPath: lockPath,
      peerRpcUrl: 'http://127.0.0.1:1/v1',
      fetchImpl,
    }),
    /does not match the canonical payment pool/
  );
  canonicalPoolAddress = poolAddress;
  await assert.rejects(
    resolveTapSettlementRate({
      bundle: { epoch: 1, receipts: [{ changed: true }] },
      tapRateLockPath: lockPath,
      peerRpcUrl: 'http://127.0.0.1:1/v1',
      fetchImpl,
    }),
    /does not match bundle content/
  );
});

test('TAP settlement reads challenge and maturity epochs from active ledger state', async () => {
  const state = new Map([
    ['epoch/apply/state', { updated_epoch: 19 }],
    ['params/challenge_epochs', {
      current: { value: 8, effective_at: 0 },
      pending: { value: 12, effective_at: Math.floor(Date.now() / 1_000) + 3_600 },
    }],
  ]);
  const fetchImpl = async (url) => ({
    ok: true,
    json: async () => ({ value: state.get(new URL(url).searchParams.get('key')) ?? null }),
  });

  assert.deepEqual(
    await resolveTapSettlementEpochPolicy({
      peerRpcUrl: 'http://127.0.0.1:49223/v1',
      fetchImpl,
    }),
    { settleThroughEpoch: 19, challengeEpochs: 8 }
  );
});

test('TAP payout minimum defers from canonical liability and crossing pays full accrued once', () => {
  const provider = makeReceiptIdentity();
  const account = '0x1111111111111111111111111111111111111111';
  const firstBundle = {
    epoch: 1,
    params: { payout_min_au: '100' },
    receipts: [receipt({ session: 'threshold-1', provider, au: '100', epoch: 1 })],
  };
  const below = buildTapSettlement({
    bundle: firstBundle,
    targetedSessionBindings: targetedBindingsFor(firstBundle, {
      [provider.publicKeyHex]: account,
    }),
    canonicalLiabilities: canonicalLiabilitiesFor(firstBundle, {
      [provider.publicKeyHex]: account,
    }),
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
  });
  assert.equal(below.root, undefined);
  assert.equal(below.blocked, undefined);
  assert.equal(below.reason, 'provider earnings are below payout minimum');
  assert.equal(below.threshold_held_au, '75');
  assert.equal(below.cumulative_spent_wei, '0');
  assert.deepEqual(below.providers, []);
  assert.deepEqual(below.canonical_deferred_liabilities, [{
    provider: provider.publicKeyHex,
    payout_revision: '11'.repeat(32),
    to: account,
    payable_au: '75',
    reason: 'below_payout_minimum',
  }]);
  assert.equal(Object.hasOwn(below, 'pending_provider_au'), false);

  const secondBundle = {
    epoch: 2,
    params: { payout_min_au: '100' },
    receipts: [receipt({ session: 'threshold-2', provider, au: '100', epoch: 2 })],
  };
  const crossingArgs = {
    bundle: secondBundle,
    targetedSessionBindings: targetedBindingsFor(secondBundle, {
      [provider.publicKeyHex]: account,
    }),
    canonicalLiabilities: canonicalLiabilitiesFor(
      secondBundle,
      { [provider.publicKeyHex]: account },
      {},
      {
        [`${provider.publicKeyHex}/${'11'.repeat(32)}/${account}`]: {
          total_au: '150',
        },
      }
    ),
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 8,
    prior: below,
  };
  const crossing = buildTapSettlement(crossingArgs);
  assert.equal(crossing.cumulative_spent_wei, '200');
  assert.equal(crossing.providers[0].cumulative_wei, '150');
  assert.equal(crossing.checkpoint_outputs[0].net_au_paid, '150');
  assert.equal(crossing.checkpoint_outputs[0].tap_wei, '150');

  const exactRetry = buildTapSettlement(crossingArgs);
  assert.equal(exactRetry.root, crossing.root);
  assert.equal(exactRetry.cumulative_spent_wei, crossing.cumulative_spent_wei);
  assert.deepEqual(exactRetry.entries, crossing.entries);
  assert.deepEqual(exactRetry.checkpoint_outputs, crossing.checkpoint_outputs);

  const laterBundle = {
    epoch: 3,
    params: { payout_min_au: '100' },
    receipts: [],
  };
  const later = buildTapSettlement({
    bundle: laterBundle,
    targetedSessionBindings: {},
    canonicalLiabilities: [],
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 9,
    prior: crossing,
  });
  assert.equal(later.root, crossing.root);
  assert.equal(later.cumulative_spent_wei, '200');
  assert.equal(later.providers[0].cumulative_wei, '150');
  assert.equal(later.spent_au, '0');
});

test('TAP exact payout threshold produces an exact provider cap with no fee or burn drift', () => {
  const provider = makeReceiptIdentity();
  const account = '0x1111111111111111111111111111111111111111';
  const bundle = {
    epoch: 1,
    params: { payout_min_au: '100' },
    receipts: [receipt({ session: 'exact-threshold', provider, au: '132', epoch: 1 })],
  };
  const settlement = buildTapSettlement({
    bundle,
    targetedSessionBindings: targetedBindingsFor(bundle, {
      [provider.publicKeyHex]: account,
    }),
    canonicalLiabilities: canonicalLiabilitiesFor(bundle, {
      [provider.publicKeyHex]: account,
    }),
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
  });
  assert.equal(settlement.spent_au, '100');
  assert.equal(settlement.providers[0].cumulative_wei, '100');
  assert.equal(settlement.cumulative_spent_wei, '134');
  assert.equal(settlement.provider_cap_wei, '100');
  assert.equal(134n * 1_500n / 10_000n, 20n);
  assert.equal(134n - 100n - 20n, 14n);
  assert.equal(settlement.provider_dust_wei, '0');
});

test('TAP canonical paid_cum_au prevents duplicate liability payout after restart', () => {
  const provider = makeReceiptIdentity();
  const account = '0x1111111111111111111111111111111111111111';
  const bundle = {
    epoch: 1,
    params: { payout_min_au: '100' },
    receipts: [receipt({ session: 'paid-cumulative', provider, au: '132', epoch: 1 })],
  };
  const baseLiability = canonicalLiabilitiesFor(
    bundle,
    { [provider.publicKeyHex]: account },
    {},
    {
      [`${provider.publicKeyHex}/${'11'.repeat(32)}/${account}`]: {
        total_au: '175',
        paid_cum_au: '75',
        aggregate_paid_cum_au: '75',
      },
    }
  );
  const first = buildTapSettlement({
    bundle,
    targetedSessionBindings: targetedBindingsFor(bundle, {
      [provider.publicKeyHex]: account,
    }),
    canonicalLiabilities: baseLiability,
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
  });
  assert.equal(first.checkpoint_outputs[0].paid_cum_au_before, '75');
  assert.equal(first.checkpoint_outputs[0].net_au_paid, '100');

  const paidLiability = structuredClone(baseLiability);
  paidLiability[0].paid_cum_au = '175';
  const replay = buildTapSettlement({
    bundle,
    targetedSessionBindings: targetedBindingsFor(bundle, {
      [provider.publicKeyHex]: account,
    }),
    canonicalLiabilities: paidLiability,
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 8,
    prior: first,
  });
  assert.equal(replay.root, first.root);
  assert.equal(replay.spent_au, '0');
  assert.deepEqual(replay.checkpoint_outputs, []);

  const tampered = {
    ...first,
    entries: structuredClone(first.entries),
    providers: structuredClone(first.providers),
    refunds: structuredClone(first.refunds),
  };
  tampered.providers[0].cumulative_wei = '101';
  assert.throws(
    () => buildTapSettlement({
      bundle,
      targetedSessionBindings: targetedBindingsFor(bundle, {
        [provider.publicKeyHex]: account,
      }),
      canonicalLiabilities: paidLiability,
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      settleThroughEpoch: 8,
      prior: tampered,
    }),
    /provider\/refund distributions do not match the confirmed root/
  );
});

test('TAP providers mature independently against the canonical minimum', () => {
  const providerA = makeReceiptIdentity();
  const providerB = makeReceiptIdentity();
  const accountA = '0x1111111111111111111111111111111111111111';
  const accountB = '0x2222222222222222222222222222222222222222';
  const bundle = {
    epoch: 1,
    params: { payout_min_au: '100' },
    receipts: [
      receipt({ session: 'provider-below', provider: providerA, au: '100', epoch: 1 }),
      receipt({ session: 'provider-mature', provider: providerB, au: '132', epoch: 1 }),
    ],
  };
  const accounts = {
    [providerA.publicKeyHex]: accountA,
    [providerB.publicKeyHex]: accountB,
  };
  const settlement = buildTapSettlement({
    bundle,
    targetedSessionBindings: targetedBindingsFor(bundle, accounts),
    canonicalLiabilities: canonicalLiabilitiesFor(bundle, accounts),
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
  });
  assert.deepEqual(settlement.providers, [{ account: accountB, cumulative_wei: '100' }]);
  assert.equal(settlement.canonical_deferred_liabilities[0].provider, providerA.publicKeyHex);
  assert.equal(settlement.checkpoint_outputs[0].provider, providerB.publicKeyHex);
});

test('TAP payout revisions and targets remain isolated', () => {
  const providerA = makeReceiptIdentity();
  const accountA = '0x1111111111111111111111111111111111111111';
  const accountB = '0x2222222222222222222222222222222222222222';
  const oldRevision = '11'.repeat(32);
  const newRevision = '22'.repeat(32);
  const bundle = {
    epoch: 1,
    params: { payout_min_au: '100' },
    receipts: [
      receipt({
        session: 'revision-old',
        provider: providerA,
        au: '100',
        epoch: 1,
        extraBody: { payout_revision: oldRevision },
      }),
      receipt({
        session: 'revision-new',
        provider: providerA,
        au: '132',
        epoch: 1,
        extraBody: { payout_revision: newRevision },
      }),
    ],
  };
  const bindings = targetedBindingsFor(bundle, { [providerA.publicKeyHex]: accountA });
  const bindingValues = Object.values(bindings).sort((left, right) => (
    left.session_id.localeCompare(right.session_id)
  ));
  bindingValues.find((entry) => entry.session_id === 'revision-old').payout_revision = oldRevision;
  bindingValues.find((entry) => entry.session_id === 'revision-new').payout_revision = newRevision;
  bindingValues.find((entry) => entry.session_id === 'revision-new').account = accountB;
  const settlement = buildTapSettlement({
    bundle,
    targetedSessionBindings: bindings,
    canonicalLiabilities: [
      {
        provider: providerA.publicKeyHex,
        rail: 'tap',
        payout_revision: oldRevision,
        target: accountA,
        chain_id: 61_000,
        total_au: '75',
        held_au: '0',
        paid_cum_au: '0',
        aggregate_paid_cum_au: '0',
        updated_epoch: 1,
        updated_at: 'epoch/targeted/1/old',
      },
      {
        provider: providerA.publicKeyHex,
        rail: 'tap',
        payout_revision: newRevision,
        target: accountB,
        chain_id: 61_000,
        total_au: '100',
        held_au: '0',
        paid_cum_au: '0',
        aggregate_paid_cum_au: '0',
        updated_epoch: 1,
        updated_at: 'epoch/targeted/1/new',
      },
    ],
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
  });
  assert.deepEqual(settlement.providers, [{ account: accountB, cumulative_wei: '100' }]);
  assert.equal(settlement.canonical_deferred_liabilities[0].to, accountA);
  assert.equal(settlement.checkpoint_outputs[0].payout_revision, newRevision);
  assert.equal(settlement.checkpoint_outputs[0].to, accountB);
});

test('TAP challenge and holdback defer without blocking epoch advancement', () => {
  const provider = makeReceiptIdentity();
  const account = '0x1111111111111111111111111111111111111111';
  const bundle = {
    epoch: 1,
    params: { payout_min_au: '0' },
    receipts: [receipt({ session: 'challenge-holdback', provider, au: '100', epoch: 1 })],
  };
  const bindings = targetedBindingsFor(bundle, { [provider.publicKeyHex]: account });
  const deferred = buildTapSettlement({
    bundle,
    targetedSessionBindings: bindings,
    canonicalLiabilities: canonicalLiabilitiesFor(
      bundle,
      { [provider.publicKeyHex]: account },
      {},
      {
        [`${provider.publicKeyHex}/${'11'.repeat(32)}/${account}`]: {
          held_au: '75',
        },
      }
    ),
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 1,
    challengeEpochs: 2,
    holdbackEpochs: 3,
  });
  assert.equal(deferred.root, undefined);
  assert.equal(deferred.blocked, undefined);
  assert.equal(deferred.reason, 'provider earnings await challenge or holdback maturity');
  assert.equal(deferred.held_receipt_count, 1);
  assert.equal(deferred.held_au, '75');

  const matured = buildTapSettlement({
    bundle,
    targetedSessionBindings: bindings,
    canonicalLiabilities: canonicalLiabilitiesFor(bundle, {
      [provider.publicKeyHex]: account,
    }),
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 4,
    challengeEpochs: 2,
    holdbackEpochs: 3,
    prior: deferred,
  });
  assert.equal(matured.blocked, undefined);
  assert.equal(matured.cumulative_spent_wei, '100');
  assert.deepEqual(matured.providers, [{ account, cumulative_wei: '75' }]);
});

test('TAP settlement refuses payout minimum outside frozen admin-ledger params', () => {
  const provider = makeReceiptIdentity();
  const account = '0x1111111111111111111111111111111111111111';
  const missing = {
    epoch: 1,
    receipts: [receipt({ session: 'missing-payout-min', provider, au: '100', epoch: 1 })],
  };
  const bindings = targetedBindingsFor(missing, { [provider.publicKeyHex]: account });
  delete missing.params;
  assert.throws(
    () => buildTapSettlement({
      bundle: missing,
      targetedSessionBindings: bindings,
      canonicalLiabilities: canonicalLiabilitiesFor(missing, {
        [provider.publicKeyHex]: account,
      }),
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      settleThroughEpoch: 7,
    }),
    /requires frozen admin-ledger params\.payout_min_au/
  );
  assert.throws(
    () => buildTapSettlement({
      bundle: { ...missing, payout_min_au: '0' },
      targetedSessionBindings: bindings,
      canonicalLiabilities: canonicalLiabilitiesFor(missing, {
        [provider.publicKeyHex]: account,
      }),
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      settleThroughEpoch: 7,
    }),
    /top-level payout_min_au is not accepted/
  );
});

test('TAP preparation crash before on-chain effect resumes without duplicate preparation or root', async () => {
  const { provider, pool, args } = await tapRollFixture('crash-before-effect');
  const submitter = memoryPreparationSubmitter();
  const plans = [];
  let crash = true;
  const crashingSubmitter = async ({ plan }) => {
    plans.push(structuredClone(plan));
    const confirmed = await submitter({ plan });
    if (crash) {
      crash = false;
      throw new Error('simulated crash after canonical preparation confirmation');
    }
    return confirmed;
  };

  await assert.rejects(
    rollTapSettlement({
      ...args,
      canonicalPreparationSubmitter: crashingSubmitter,
    }),
    /simulated crash after canonical preparation confirmation/
  );
  assert.equal(submitter.created, 2);
  assert.equal(submitter.records.size, 2);
  assert.equal(await pool.epoch(), 0n);
  assert.equal((await pool.queryFilter(pool.filters.RootProposed(1))).length, 0);

  const resumed = await rollWithTapFinality(provider, {
    ...args,
    canonicalPreparationSubmitter: crashingSubmitter,
  });
  assert.equal(resumed.root_confirmed, true);
  assert.equal(submitter.created, 2);
  assert.equal(submitter.records.size, 2);
  assert.equal((await pool.queryFilter(pool.filters.RootProposed(1))).length, 1);
  assert.equal((await pool.queryFilter(pool.filters.RootPosted(1))).length, 1);
  assert.equal(resumed.external_effect_ids.length, 2);
  assert.deepEqual(plans.slice(1).map((plan) => plan.external_effect_ids), [
    plans[0].external_effect_ids,
    plans[0].external_effect_ids,
  ]);
});

test('TAP crash after confirmed proposeRoot resumes with one proposal and one execution', async () => {
  const { provider, pool, args } = await tapRollFixture('crash-after-proposal');
  const submitter = memoryPreparationSubmitter();
  const crashingPool = crashAfterConfirmedPoolEffect(pool, 'proposeRoot');
  const rollArgs = {
    ...args,
    pool: crashingPool,
    canonicalPreparationSubmitter: submitter,
  };

  await assert.rejects(
    rollTapSettlement(rollArgs),
    /simulated crash after confirmed proposeRoot/
  );
  assert.equal(submitter.created, 2);
  assert.equal(submitter.records.size, 2);
  assert.equal(await pool.epoch(), 0n);
  assert.equal((await pool.queryFilter(pool.filters.RootProposed(1))).length, 1);
  assert.equal((await pool.queryFilter(pool.filters.RootPosted(1))).length, 0);

  const resumed = await rollWithTapFinality(provider, {
    ...args,
    canonicalPreparationSubmitter: submitter,
  });
  assert.equal(resumed.root_confirmed, true);
  assert.equal(submitter.created, 2);
  assert.equal(submitter.records.size, 2);
  assert.equal((await pool.queryFilter(pool.filters.RootProposed(1))).length, 1);
  assert.equal((await pool.queryFilter(pool.filters.RootPosted(1))).length, 1);
  assert.equal(await pool.epoch(), 1n);
});

test('TAP settlement roller posts root and provider proof verifies independently', async () => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: { totalAccounts: 5 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const buyer = await provider.getSigner(1);
  const providerA = await provider.getSigner(2);
  const providerB = await provider.getSigner(3);
  const operatorTreasury = await provider.getSigner(4);
  const { token, pool, poolAddr, governanceWallet } = await deployPool(operator);

  await (await token.mint(await buyer.getAddress(), U(10))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(10))).wait();
  await (await pool.connect(buyer).deposit(U(10))).wait();

  const providerAId = makeReceiptIdentity();
  const providerBId = makeReceiptIdentity();
  const providerAccounts = {
    [providerAId.publicKeyHex]: await providerA.getAddress(),
    [providerBId.publicKeyHex]: await providerB.getAddress(),
  };
  const bundle = {
    epoch: 1,
    receipts: [
      receipt({ session: 's1', provider: providerAId, au: usdAu(1) }),
      receipt({ session: 's2', provider: providerBId, au: usdAu(3) }),
    ],
  };
  const preparationSubmitter = memoryPreparationSubmitter();
  const rolled = await rollWithTapFinality(provider, {
    bundle,
    targetedSessionBindings: targetedBindingsFor(bundle, providerAccounts),
    canonicalLiabilities: canonicalLiabilitiesFor(bundle, providerAccounts),
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
    pool,
    ownerSigner: operator,
    governanceSigner: governanceWallet,
    operatorAddress: await operatorTreasury.getAddress(),
    canonicalPreparationSubmitter: preparationSubmitter,
    ...checkpointArgs(bundle, poolAddr, await token.getAddress()),
    post: true,
  });

  const spentA = auToTapWei(usdAu(1), TAP_USD_AU);
  const spentB = auToTapWei(usdAu(3), TAP_USD_AU);
  const claimA = providerShareWei(spentA);
  const claimB = providerShareWei(spentB);
  const expectedDist = distribution([
    { account: providerAccounts[providerAId.publicKeyHex].toLowerCase(), amount: claimA },
    { account: providerAccounts[providerBId.publicKeyHex].toLowerCase(), amount: claimB },
  ]);

  assert.equal(rolled.posted, true);
  assert.equal(rolled.epoch, 1);
  assert.equal(rolled.cumulative_spent_wei, (spentA + spentB).toString());
  assert.equal(rolled.provider_claimed_wei, (claimA + claimB).toString());
  assert.equal(rolled.root, expectedDist.root);
  assert.equal(await pool.epoch(), 1n);
  assert.equal(await pool.cumulativeSpent(), spentA + spentB);
  assert.equal(await pool.merkleRoot(), expectedDist.root);
  assert.equal(rolled.operator_fee.auto_sent, true);
  assert.equal(rolled.operator_fee.predicted_claimable_wei, ((spentA + spentB) * 1500n / 10_000n).toString());
  assert.equal(rolled.operator_fee.actual_claimable_wei, rolled.operator_fee.predicted_claimable_wei);
  assert.match(rolled.operator_fee.tx, /^0x[0-9a-f]{64}$/i);
  assert.equal(await token.balanceOf(await operatorTreasury.getAddress()), (spentA + spentB) * 1500n / 10_000n);
  assert.equal(await pool.operatorClaimable(), 0n);
  assert.equal(rolled.burn.auto_sent, true);
  assert.equal(rolled.burn.completed, true);
  assert.equal(rolled.burn.predicted_claimable_wei, ((spentA + spentB) * 1000n / 10_000n).toString());
  assert.equal(rolled.burn.actual_claimable_wei, rolled.burn.predicted_claimable_wei);
  assert.equal(rolled.burn.calldata, encodeBurnCalldata());
  assert.match(rolled.burn.tx, /^0x[0-9a-f]{64}$/i);
  assert.equal(await token.balanceOf(BURN_SINK), (spentA + spentB) * 1000n / 10_000n);
  assert.equal(await pool.burnClaimable(), 0n);
  assert.equal(rolled.tap_settlement_checkpoint.epoch_apply_hash, bundle.epoch_apply_hash);
  assert.equal(rolled.tap_settlement_checkpoint.root, rolled.root);
  assert.equal(rolled.tap_settlement_checkpoint.provider_count, 2);
  assert.equal(rolled.tap_settlement_checkpoint.root_confirmed, true);
  assert.match(rolled.tap_settlement_checkpoint.proposal_tx, /^0x[0-9a-f]{64}$/);
  assert.match(rolled.tap_settlement_checkpoint.execution_tx, /^0x[0-9a-f]{64}$/);
  assert.match(
    rolled.tap_settlement_checkpoint.proposal_block_hash,
    /^0x[0-9a-f]{64}$/
  );
  assert.deepEqual(
    Object.keys(rolled.tap_settlement_checkpoint.outputs[0]).sort(),
    [
      'aggregate_paid_cum_au_before',
      'cumulative_claim_wei',
      'paid_au',
      'paid_cum_au_before',
      'payout_revision',
      'prior_cumulative_claim_wei',
      'provider',
      'tap_wei',
      'to',
    ]
  );

  await (await pool.connect(providerA).claim(
    rolled.epoch,
    providerAccounts[providerAId.publicKeyHex],
    claimA,
    rolled.proofs[providerAccounts[providerAId.publicKeyHex].toLowerCase()].proof
  )).wait();
  assert.equal(await token.balanceOf(providerAccounts[providerAId.publicKeyHex]), claimA);
});

test('TAP settlement roller includes buyer refund leaves in the claim root', async () => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: { totalAccounts: 4 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const buyer = await provider.getSigner(1);
  const providerA = await provider.getSigner(2);
  const operatorTreasury = await provider.getSigner(3);
  const { token, pool, poolAddr, governanceWallet } = await deployPool(operator);

  await (await token.mint(await buyer.getAddress(), U(10))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(10))).wait();
  await (await pool.connect(buyer).deposit(U(10))).wait();

  const userId = makeReceiptIdentity();
  const providerId = makeReceiptIdentity();
  const providerAccount = await providerA.getAddress();
  const buyerAccount = await buyer.getAddress();
  const bundle = {
    epoch: 1,
    receipts: [receipt({ session: 'refund-s1', user: userId, provider: providerId, au: usdAu(4) })],
    buyer_refunds: [{ user: userId.publicKeyHex, refund_au: usdAu(6) }],
  };
  const rolled = await rollWithTapFinality(provider, {
    bundle,
    targetedSessionBindings: targetedBindingsFor(bundle, {
      [providerId.publicKeyHex]: providerAccount,
    }),
    canonicalLiabilities: canonicalLiabilitiesFor(bundle, {
      [providerId.publicKeyHex]: providerAccount,
    }),
    buyerAccounts: { [userId.publicKeyHex]: buyerAccount },
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
    pool,
    ownerSigner: operator,
    governanceSigner: governanceWallet,
    operatorAddress: await operatorTreasury.getAddress(),
    canonicalPreparationSubmitter: memoryPreparationSubmitter(),
    ...checkpointArgs(bundle, poolAddr, await token.getAddress()),
    post: true,
  });

  const spentWei = auToTapWei(usdAu(4), TAP_USD_AU);
  const providerClaim = providerShareWei(spentWei);
  const buyerRefund = auToTapWei(usdAu(6), TAP_USD_AU);
  const expectedDist = distribution([
    { account: providerAccount.toLowerCase(), amount: providerClaim },
    { account: buyerAccount.toLowerCase(), amount: buyerRefund },
  ]);

  assert.equal(rolled.posted, true);
  assert.equal(rolled.cumulative_spent_wei, spentWei.toString());
  assert.equal(rolled.provider_claimed_wei, providerClaim.toString());
  assert.equal(rolled.buyer_refund_wei, buyerRefund.toString());
  assert.equal(rolled.total_claimed_wei, (providerClaim + buyerRefund).toString());
  assert.equal(rolled.providers.length, 1);
  assert.equal(rolled.refunds.length, 1);
  assert.equal(rolled.root, expectedDist.root);

  await (await pool.connect(providerA).claim(
    rolled.epoch,
    providerAccount,
    providerClaim,
    rolled.proofs[providerAccount.toLowerCase()].proof
  )).wait();
  await (await pool.connect(buyer).claim(
    rolled.epoch,
    buyerAccount,
    buyerRefund,
    rolled.proofs[buyerAccount.toLowerCase()].proof
  )).wait();
  assert.equal(await token.balanceOf(providerAccount), providerClaim);
  assert.equal(await token.balanceOf(buyerAccount), buyerRefund);
});

test('TAP settlement roller requires targeted bindings and skips repeated roots', async () => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: { totalAccounts: 4 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const buyer = await provider.getSigner(1);
  const providerA = await provider.getSigner(2);
  const operatorTreasury = await provider.getSigner(3);
  const { token, pool, poolAddr, governanceWallet } = await deployPool(operator);
  await (await token.mint(await buyer.getAddress(), U(5))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(5))).wait();
  await (await pool.connect(buyer).deposit(U(5))).wait();

  const providerId = makeReceiptIdentity();
  const bundle = {
    epoch: 1,
    receipts: [receipt({ session: 's1', provider: providerId, au: usdAu(1) })],
  };
  assert.throws(
    () => buildTapSettlement({ bundle, tapUsdAu: TAP_USD_AU, ledgerFeeBps: 1500, settleThroughEpoch: 7 }),
    /targeted TAP session bindings are required/
  );
  assert.throws(
    () => buildTapSettlement({
      bundle,
      targetedSessionBindings: targetedBindingsFor(bundle, {
        [providerId.publicKeyHex]: '0x1111111111111111111111111111111111111111',
      }),
      canonicalLiabilities: canonicalLiabilitiesFor(bundle, {
        [providerId.publicKeyHex]: '0x1111111111111111111111111111111111111111',
      }),
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      settleThroughEpoch: 1,
      challengeEpochs: 0,
    }),
    /challenge_epochs must be non-zero/
  );
  assert.throws(
    () => buildTapSettlement({
      bundle,
      targetedSessionBindings: targetedBindingsFor(bundle, {
        [providerId.publicKeyHex]: '0x1111111111111111111111111111111111111111',
      }),
      canonicalLiabilities: canonicalLiabilitiesFor(bundle, {
        [providerId.publicKeyHex]: '0x1111111111111111111111111111111111111111',
      }),
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1200,
      settleThroughEpoch: 7,
    }),
    /must equal on-chain OPERATOR_BPS/
  );

  const providerAccounts = { [providerId.publicKeyHex]: await providerA.getAddress() };
  const preparationSubmitter = memoryPreparationSubmitter();
  const first = await rollWithTapFinality(provider, {
    bundle,
    targetedSessionBindings: targetedBindingsFor(bundle, providerAccounts),
    canonicalLiabilities: canonicalLiabilitiesFor(bundle, providerAccounts),
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
    pool,
    ownerSigner: operator,
    governanceSigner: governanceWallet,
    operatorAddress: await operatorTreasury.getAddress(),
    canonicalPreparationSubmitter: preparationSubmitter,
    ...checkpointArgs(bundle, poolAddr, await token.getAddress()),
    post: true,
  });
  assert.equal(first.posted, true);
  const ownerNonceBeforeReplay = await provider.getTransactionCount(
    await operator.getAddress()
  );

  const replay = await rollTapSettlement({
    bundle,
    targetedSessionBindings: targetedBindingsFor(bundle, providerAccounts),
    canonicalLiabilities: canonicalLiabilitiesFor(bundle, providerAccounts),
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
    pool,
    ownerSigner: operator,
    governanceSigner: governanceWallet,
    operatorAddress: await operatorTreasury.getAddress(),
    canonicalPreparationSubmitter: preparationSubmitter,
    ...checkpointArgs(bundle, poolAddr, await token.getAddress()),
    prior: first,
    post: true,
  });
  assert.equal(replay.posted, false);
  assert.equal(replay.root_confirmed, true);
  assert.equal(replay.root_already_posted, true);
  assert.equal(replay.blocked, undefined);
  assert.equal(replay.operator_fee.completed, true);
  assert.equal(replay.burn.completed, true);
  assert.equal(replay.tap_settlement_checkpoint.root, first.tap_settlement_checkpoint.root);
  assert.deepEqual(
    replay.tap_settlement_checkpoint.preparation_ids,
    first.tap_settlement_checkpoint.preparation_ids
  );
  assert.equal(
    replay.tap_settlement_checkpoint.proposal_tx,
    first.tap_settlement_checkpoint.proposal_tx
  );
  assert.equal(
    replay.tap_settlement_checkpoint.execution_tx,
    first.tap_settlement_checkpoint.execution_tx
  );
  assert.equal(
    await provider.getTransactionCount(await operator.getAddress()),
    ownerNonceBeforeReplay
  );
});

test('TAP settlement nets one logical bill across provider redispatch attempts', () => {
  const user = makeReceiptIdentity();
  const enclave = makeReceiptIdentity();
  const providerA = makeReceiptIdentity();
  const providerB = makeReceiptIdentity();
  const accountA = '0x1111111111111111111111111111111111111111';
  const accountB = '0x2222222222222222222222222222222222222222';
  const billingId = '7'.repeat(64);
  const lockedRateMap = [
    { unit: 'input_token', per_unit_au: '100', granularity: 1 },
    { unit: 'output_token', per_unit_au: '50', granularity: 1 },
  ];
  const first = receipt({
    session: 'tap-logical-a',
    user,
    enclave,
    provider: providerA,
    au: '100',
    extraBody: {
      billing_id: billingId,
      billing_attempt: 0,
      billing_prior_usage: {},
      billing_prior_au_owed_cum: '0',
      final: false,
      locked_rate_map: lockedRateMap,
      usage: { input_token: 1 },
      au_owed_cum: '100',
    },
  });
  const second = receipt({
    session: 'tap-logical-b',
    user,
    enclave,
    provider: providerB,
    au: '200',
    extraBody: {
      billing_id: billingId,
      billing_attempt: 1,
      billing_prior_usage: { input_token: 1 },
      billing_prior_au_owed_cum: '100',
      final: true,
      locked_rate_map: lockedRateMap,
      usage: { input_token: 1, output_token: 2 },
      au_owed_cum: '200',
    },
  });
  const providerAccounts = {
    [providerA.publicKeyHex]: accountA,
    [providerB.publicKeyHex]: accountB,
  };
  const bundle = { epoch: 1, receipts: [second, first] };

  const settlement = buildTapSettlement({
    bundle,
    targetedSessionBindings: targetedBindingsFor(bundle, providerAccounts),
    canonicalLiabilities: canonicalLiabilitiesFor(bundle, providerAccounts),
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
  });
  assert.equal(settlement.receipt_count, 2);
  assert.equal(settlement.spent_au, '150');
  assert.equal(settlement.cumulative_spent_wei, '200');
  assert.deepEqual(
    Object.fromEntries(settlement.providers.map((entry) => [entry.account, entry.cumulative_wei])),
    {
      [accountA.toLowerCase()]: '75',
      [accountB.toLowerCase()]: '75',
    }
  );

  const wrongAmount = structuredClone(second);
  wrongAmount.receipt.body.billing_prior_au_owed_cum = '99';
  wrongAmount.receipt.enclave_sig = crypto.sign(
    null,
    Buffer.from(receiptMessage(wrongAmount.receipt.body)),
    enclave.privateKey
  ).toString('hex');
  wrongAmount.receipt.user_sig = crypto.sign(
    null,
    Buffer.from(receiptMessage(wrongAmount.receipt.body)),
    user.privateKey
  ).toString('hex');
  assert.throws(
    () => {
      const invalidBundle = { epoch: 1, receipts: [first, wrongAmount] };
      return buildTapSettlement({
        bundle: invalidBundle,
        targetedSessionBindings: targetedBindingsFor(invalidBundle, providerAccounts),
        canonicalLiabilities: canonicalLiabilitiesFor(invalidBundle, providerAccounts),
        tapUsdAu: TAP_USD_AU,
        ledgerFeeBps: 1500,
        settleThroughEpoch: 7,
      });
    },
    /redispatch baseline does not match prior logical settlement/
  );
  assert.throws(
    () => {
      const invalidBundle = { epoch: 1, receipts: [second] };
      return buildTapSettlement({
        bundle: invalidBundle,
        targetedSessionBindings: targetedBindingsFor(invalidBundle, providerAccounts),
        canonicalLiabilities: canonicalLiabilitiesFor(invalidBundle, providerAccounts),
        tapUsdAu: TAP_USD_AU,
        ledgerFeeBps: 1500,
        settleThroughEpoch: 7,
      });
    },
    /logical billing baseline has no prior signed receipt/
  );
});

test('TAP settlement roller resumes fee and burn after an exact root-only partial run', async () => {
  const ganache = Ganache.provider({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: { totalAccounts: 4 },
  });
  const provider = new ethers.BrowserProvider(ganache);
  const operator = await provider.getSigner(0);
  const buyer = await provider.getSigner(1);
  const providerSigner = await provider.getSigner(2);
  const operatorTreasury = await provider.getSigner(3);
  const { token, pool, poolAddr, governanceWallet } = await deployPool(operator);
  await (await token.mint(await buyer.getAddress(), U(5))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(5))).wait();
  await (await pool.connect(buyer).deposit(U(5))).wait();

  const providerId = makeReceiptIdentity();
  const providerAccounts = { [providerId.publicKeyHex]: await providerSigner.getAddress() };
  const bundle = {
    epoch: 1,
    receipts: [receipt({ session: 'partial-root', provider: providerId, au: usdAu(1) })],
  };
  const settlement = buildTapSettlement({
    bundle,
    targetedSessionBindings: targetedBindingsFor(bundle, providerAccounts),
    canonicalLiabilities: canonicalLiabilitiesFor(bundle, providerAccounts),
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
  });
  const checkpoint = checkpointArgs(bundle, poolAddr, await token.getAddress());
  const preparationSubmitter = memoryPreparationSubmitter();
  await preparationSubmitter({
    plan: await buildCanonicalTapPreparationPlan({
      settlement,
      bundle,
      epoch: 1,
      epochApplyHash: checkpoint.epochApplyHash,
      tapRateLock: checkpoint.tapRateLock,
    }),
  });
  const governanceSignature = await signRootProposal({
    signer: governanceWallet,
    pool,
    merkleRoot: settlement.root,
    newEpoch: 1,
    newCumulativeSpent: BigInt(settlement.cumulative_spent_wei),
  });
  await (await pool.proposeRoot(
    settlement.root,
    1,
    BigInt(settlement.cumulative_spent_wei),
    governanceSignature
  )).wait();
  await (await pool.executeRoot()).wait();
  await mineTapConfirmations(provider);
  assert((await pool.operatorClaimable()) > 0n);
  assert((await pool.burnClaimable()) > 0n);

  const resumed = await rollTapSettlement({
    bundle,
    targetedSessionBindings: targetedBindingsFor(bundle, providerAccounts),
    canonicalLiabilities: canonicalLiabilitiesFor(bundle, providerAccounts),
    tapUsdAu: TAP_USD_AU,
    ledgerFeeBps: 1500,
    settleThroughEpoch: 7,
    pool,
    ownerSigner: operator,
    governanceSigner: governanceWallet,
    operatorAddress: await operatorTreasury.getAddress(),
    canonicalPreparationSubmitter: preparationSubmitter,
    ...checkpoint,
    post: true,
  });

  assert.equal(resumed.posted, false);
  assert.equal(resumed.root_confirmed, true);
  assert.equal(resumed.root_already_posted, true);
  assert.equal(resumed.propose_root_dry_run.skipped, true);
  assert.equal(resumed.operator_fee.auto_sent, true);
  assert.equal(resumed.operator_fee.completed, true);
  assert.equal(resumed.burn.auto_sent, true);
  assert.equal(resumed.burn.completed, true);
  assert.equal(await pool.operatorClaimable(), 0n);
  assert.equal(await pool.burnClaimable(), 0n);
});

test('TAP settlement roller refuses unsigned multi-provider split controls', () => {
  const providerId = makeReceiptIdentity();
  const a = '0x1111111111111111111111111111111111111111';
  const b = '0x2222222222222222222222222222222222222222';
  const bundle = {
    epoch: 1,
    receipts: [receipt({
      session: 's1',
      provider: providerId,
      au: usdAu(4),
      extraBody: {
        provider_refs: ['pa', 'pb'],
        contribution_weights_bps: [2_500, 7_500],
      },
    })],
  };
  assert.throws(
    () => buildTapSettlement({
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      bundle,
      targetedSessionBindings: targetedBindingsFor(bundle, {
        [providerId.publicKeyHex]: a,
      }),
      canonicalLiabilities: canonicalLiabilitiesFor(bundle, {
        [providerId.publicKeyHex]: a,
      }),
      settleThroughEpoch: 7,
    }),
    /multi-provider TAP receipts require a signed contribution schema/
  );
});

test('TAP settlement roller rejects unsigned and tampered receipts before root construction', () => {
  const providerId = makeReceiptIdentity();
  const account = '0x1111111111111111111111111111111111111111';
  const signed = receipt({ session: 's1', provider: providerId, au: usdAu(2) });
  const unsignedBundle = {
    epoch: 1,
    receipts: [{
      receipt: {
        body: signed.receipt.body,
      },
    }],
  };
  assert.throws(
    () => buildTapSettlement({
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      bundle: unsignedBundle,
      targetedSessionBindings: targetedBindingsFor(unsignedBundle, {
        [providerId.publicKeyHex]: account,
      }),
      canonicalLiabilities: canonicalLiabilitiesFor(unsignedBundle, {
        [providerId.publicKeyHex]: account,
      }),
      settleThroughEpoch: 7,
    }),
    /Invalid enclave receipt signature/
  );

  const tampered = structuredClone(signed);
  tampered.receipt.body.au_owed_cum = usdAu(3);
  const tamperedBundle = { epoch: 1, receipts: [tampered] };
  assert.throws(
    () => buildTapSettlement({
      tapUsdAu: TAP_USD_AU,
      ledgerFeeBps: 1500,
      bundle: tamperedBundle,
      targetedSessionBindings: targetedBindingsFor(tamperedBundle, {
        [providerId.publicKeyHex]: account,
      }),
      canonicalLiabilities: canonicalLiabilitiesFor(tamperedBundle, {
        [providerId.publicKeyHex]: account,
      }),
      settleThroughEpoch: 7,
    }),
    /Invalid enclave receipt signature/
  );
});

test('guardian pre-sign screen halts invariant violations', async () => {
  const a = '0x1111111111111111111111111111111111111111';
  const root = `0x${'11'.repeat(32)}`;

  const overAllocated = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1000',
      entries: [{ account: a, cumulative_wei: '751' }],
    },
    epoch: 1,
    currentEpoch: 0,
    prevSpentWei: '0',
    totalDepositedWei: '1000',
    maxEpochDeltaWei: '0',
  });
  assert.equal(overAllocated.ok, false);
  assert.match(overAllocated.reasons.join('; '), /provider owed > 75% spent cap/);
  assert.match(overAllocated.reasons.join('; '), /owed \+ operator cap \+ burn cap > deposited/);

  const b = '0x2222222222222222222222222222222222222222';
  const refundWithinEscrow = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1000',
      providers: [{ account: a, cumulative_wei: '750' }],
      refunds: [{ account: b, cumulative_wei: '100' }],
      entries: [
        { account: a, cumulative_wei: '750' },
        { account: b, cumulative_wei: '100' },
      ],
    },
    epoch: 1,
    currentEpoch: 0,
    prevSpentWei: '0',
    totalDepositedWei: '1100',
    maxEpochDeltaWei: '0',
  });
  assert.equal(refundWithinEscrow.ok, true);
  assert.equal(refundWithinEscrow.provider_owed_wei, '750');
  assert.equal(refundWithinEscrow.total_owed_wei, '850');

  const spentPastDeposits = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1001',
      entries: [{ account: a, cumulative_wei: '750' }],
    },
    epoch: 1,
    currentEpoch: 0,
    prevSpentWei: '0',
    totalDepositedWei: '1000',
    maxEpochDeltaWei: '0',
  });
  assert.equal(spentPastDeposits.ok, false);
  assert.match(spentPastDeposits.reasons.join('; '), /spent > deposited/);

  const capExceeded = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '2000',
      entries: [{ account: a, cumulative_wei: '1500' }],
    },
    epoch: 1,
    currentEpoch: 0,
    prevSpentWei: '0',
    totalDepositedWei: '2000',
    maxEpochDeltaWei: '1999',
  });
  assert.equal(capExceeded.ok, false);
  assert.match(capExceeded.reasons.join('; '), /epoch delta > cap/);

  const decreasedProvider = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1100',
      entries: [{ account: a, cumulative_wei: '700' }],
    },
    previous: {
      epoch: 1,
      cumulative_spent_wei: '1000',
      entries: [{ account: a, cumulative_wei: '800' }],
    },
    epoch: 2,
    currentEpoch: 1,
    prevSpentWei: '1000',
    totalDepositedWei: '1100',
    maxEpochDeltaWei: '0',
  });
  assert.equal(decreasedProvider.ok, false);
  assert.match(decreasedProvider.reasons.join('; '), /cumulative for .* decreased/);

  const droppedWithoutClaimCheck = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1100',
      entries: [],
    },
    previous: {
      epoch: 1,
      cumulative_spent_wei: '1000',
      entries: [{ account: a, cumulative_wei: '700' }],
    },
    epoch: 2,
    currentEpoch: 1,
    prevSpentWei: '1000',
    totalDepositedWei: '1100',
    maxEpochDeltaWei: '0',
  });
  assert.equal(droppedWithoutClaimCheck.ok, false);
  assert.match(droppedWithoutClaimCheck.reasons.join('; '), /on-chain claimed check required/);

  const droppedUnclaimed = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1100',
      entries: [],
    },
    previous: {
      epoch: 1,
      cumulative_spent_wei: '1000',
      entries: [{ account: a, cumulative_wei: '700' }],
    },
    epoch: 2,
    currentEpoch: 1,
    prevSpentWei: '1000',
    totalDepositedWei: '1100',
    maxEpochDeltaWei: '0',
    pool: { claimed: async () => 100n },
  });
  assert.equal(droppedUnclaimed.ok, false);
  assert.match(droppedUnclaimed.reasons.join('; '), /dropped below unclaimed prior/);

  const droppedAfterFullyClaimed = await guardianPreSignReport({
    settlement: {
      root,
      cumulative_spent_wei: '1100',
      entries: [],
    },
    previous: {
      epoch: 1,
      cumulative_spent_wei: '1000',
      entries: [{ account: a, cumulative_wei: '700' }],
    },
    epoch: 2,
    currentEpoch: 1,
    prevSpentWei: '1000',
    totalDepositedWei: '1100',
    maxEpochDeltaWei: '0',
    pool: { claimed: async () => 700n },
  });
  assert.equal(droppedAfterFullyClaimed.ok, true);
});

test('TAP settlement CLI dry-runs and broadcasts with env key against a locked JSON-RPC node', async (t) => {
  const server = Ganache.server({
    logging: { quiet: true },
    chain: { chainId: 61_000 },
    wallet: {
      lock: true,
      accounts: [
        { secretKey: OPERATOR_KEY, balance: GANACHE_BALANCE },
        { secretKey: BUYER_KEY, balance: GANACHE_BALANCE },
        { secretKey: PROVIDER_KEY, balance: GANACHE_BALANCE },
      ],
    },
  });
  await new Promise((resolve, reject) => {
    server.listen(0, '127.0.0.1', (error) => (error ? reject(error) : resolve()));
  });
  let provider = null;
  t.after(() => {
    if (provider?.destroy) provider.destroy();
    try {
      const closing = server.close();
      if (closing?.catch) closing.catch(() => {});
    } catch (_error) {
      // Best-effort cleanup for Ganache's mixed callback/promise close API in node:test.
    }
  });
  const rpc = `http://127.0.0.1:${server.address().port}`;
  provider = new ethers.JsonRpcProvider(rpc);
  const operator = new ethers.NonceManager(new ethers.Wallet(OPERATOR_KEY, provider));
  const buyer = new ethers.Wallet(BUYER_KEY, provider);
  const providerSigner = new ethers.Wallet(PROVIDER_KEY, provider);
  const operatorTreasury = ethers.Wallet.createRandom().connect(provider);
  const { token, pool, poolAddr, governanceWallet } = await deployPool(operator);

  await (await token.mint(await buyer.getAddress(), U(10))).wait();
  await (await token.connect(buyer).approve(poolAddr, U(10))).wait();
  await (await pool.connect(buyer).deposit(U(10))).wait();

  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-tap-roller-'));
  t.after(() => fs.rmSync(tmp, { recursive: true, force: true }));
  const bundlePath = path.join(tmp, 'bundle.json');
  const providerId = makeReceiptIdentity();
  const bundle = {
    epoch: 1,
    epoch_apply_hash: 'ab'.repeat(32),
    params: { payout_min_au: '0' },
    receipts: [receipt({ session: 'cli-s1', provider: providerId, au: usdAu(1) })],
  };
  fs.writeFileSync(bundlePath, JSON.stringify(bundle, null, 2));
  const body = bundle.receipts[0].receipt.body;
  const payoutRevision = '44'.repeat(32);
  const adminWallet = new PeerWallet();
  await adminWallet.ready;
  await adminWallet.generateKeyPair();
  const adminKeypairPath = path.join(tmp, 'admin-keypair.json');
  adminWallet.exportToFile(adminKeypairPath, b4a.alloc(0));
  fs.chmodSync(adminKeypairPath, 0o600);
  const admin = b4a.toString(adminWallet.publicKey, 'hex');
  const ledgerState = new Map([
    ['admin', admin],
    ['payments/current', {
      denom: 'au_usd',
      ver: 1,
      tap: {
        chain_id: 61_000,
        token_address: await token.getAddress(),
        pool_address: poolAddr,
      },
      set_by: admin,
      set_by_role: 'admin',
    }],
    ['epoch/apply/state', { updated_epoch: 7 }],
    ['params/challenge_epochs', { current: { value: 6, effective_at: 0 }, pending: null }],
    ['tap/rate/latest', {
      denom: 'tap_usd_au',
      tap_usd_au: String(TAP_USD_AU),
      source: 'uniswap-v2-twap-median',
      ts: 3_600,
      updated_at: `rate/tap/3600/${'bb'.repeat(32)}`,
      posted_by: admin,
      posted_by_role: 'admin',
    }],
    [`payout/allocation/1/${body.session_id}`, {
      type: 'provider_payout_session_allocation',
      epoch: 1,
      page: 0,
      session_id: body.session_id,
      user: body.user,
      rail: 'tap',
      provider: body.provider,
      payout_revision: payoutRevision,
      au: usdAu(1),
      feature_key: `epoch/targeted/1/${'55'.repeat(32)}`,
    }],
    [`payout/binding/tap/${body.provider}/${payoutRevision}`, {
      verified: true,
      provider: body.provider,
      rail: 'tap',
      revision: payoutRevision,
      target: providerSigner.address.toLowerCase(),
      chain_id: 61_000,
      activation_epoch: 1,
      context_revision: '66'.repeat(32),
      payment_config_version: 1,
    }],
    [`payout/liability/tap/${body.provider}/${payoutRevision}`, {
      provider: body.provider,
      rail: 'tap',
      revision: payoutRevision,
      target: providerSigner.address.toLowerCase(),
      currency: null,
      chain_id: 61_000,
      total_au: providerNetAu(usdAu(1)).toString(),
      held_au: '0',
      paid_cum_au: '0',
      updated_epoch: 1,
      updated_at: `epoch/targeted/1/${'55'.repeat(32)}`,
    }],
    [`earn/tap/${body.provider}`, {
      provider: body.provider,
      rail: 'tap',
      paid_cum_au: '0',
    }],
  ]);
  const ledgerServer = http.createServer(async (request, response) => {
    const url = new URL(request.url, 'http://127.0.0.1');
    if (request.method === 'POST' && url.pathname.endsWith('/contract/feature')) {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      const feature = JSON.parse(Buffer.concat(chunks).toString('utf8'));
      const value = feature.value;
      ledgerState.set(
        `payout/preparation/tap/${value.economic_op_id}`,
        {
          type: 'targeted_payout_preparation',
          ...value,
          consumed: false,
          feature_key: feature.key,
        }
      );
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ ok: true }));
      return;
    }
    const key = url.searchParams.get('key');
    const value = ledgerState.get(key) ?? null;
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ value }));
  });
  await new Promise((resolve) => ledgerServer.listen(0, '127.0.0.1', resolve));
  t.after(() => ledgerServer.close());
  const ledgerRpc = `http://127.0.0.1:${ledgerServer.address().port}/v1`;

  const baseArgs = [
    SCRIPT_PATH,
    '--bundle', bundlePath,
    '--peer-rpc', ledgerRpc,
    '--tap-rate-lock', path.join(tmp, 'epoch-1.tap-rate.json'),
    '--ledger-fee-bps', '1500',
    '--eth-rpc', rpc,
    '--pool', poolAddr,
    '--operator-address', await operatorTreasury.getAddress(),
    '--json',
  ];
  const baseEnv = { ...process.env };
  delete baseEnv.MAYHEM_TAP_ROLLER_PRIVATE_KEY;
  delete baseEnv.MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY;
  const signingEnv = {
    ...baseEnv,
    MAYHEM_TAP_ROLLER_PRIVATE_KEY: OPERATOR_KEY,
    MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY: governanceWallet.privateKey,
    MAYHEM_TRAC_ADMIN_KEYPAIR_PATH: adminKeypairPath,
  };
  const localPolicyOverride = await runNode([...baseArgs, '--challenge-epochs', '1'], {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: signingEnv,
  });
  assert.notEqual(localPolicyOverride.status, 0);
  assert.match(localPolicyOverride.stderr, /active admin ledger state/);

  const rawRateOverride = await runNode([...baseArgs, '--tap-usd-au', TAP_USD_AU], {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: signingEnv,
  });
  assert.notEqual(rawRateOverride.status, 0);
  assert.match(rawRateOverride.stderr, /not supported.*rate-lock/i);

  const missingKey = await runNode(baseArgs, {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: baseEnv,
  });
  assert.notEqual(missingKey.status, 0);
  assert.match(missingKey.stderr, /MAYHEM_TAP_ROLLER_PRIVATE_KEY/);

  const dryRun = await runNode(baseArgs, {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: signingEnv,
  });
  assert.equal(dryRun.status, 0, dryRun.stderr);
  const report = JSON.parse(dryRun.stdout);
  assert.equal(report.posted, false);
  assert.equal(report.tap_rate_lock.tap_usd_au, TAP_USD_AU);
  assert.equal(report.tap_rate_lock.epoch, 1);
  assert.equal(report.tap_rate_lock.chain_id, 61_000);
  assert.equal(report.tap_rate_lock.pool_address, poolAddr.toLowerCase());
  assert.equal(report.signer_env, 'MAYHEM_TAP_ROLLER_PRIVATE_KEY');
  assert.equal(report.signing_address, (await operator.getAddress()).toLowerCase());
  assert.equal(report.governance_signer_env, 'MAYHEM_TAP_GOVERNANCE_PRIVATE_KEY');
  assert.equal(report.governance_signing_address, governanceWallet.address.toLowerCase());
  assert.equal(report.propose_root_dry_run.ok, true);
  assert.equal(report.propose_root_dry_run.static_call_ok, true);
  assert.match(report.propose_root_dry_run.gas_estimate, /^[0-9]+$/);
  const proposal = new ethers.Interface(POOL_SETTLEMENT_ABI)
    .decodeFunctionData('proposeRoot', report.propose_root_calldata);
  assert.equal(proposal.newRoot.toLowerCase(), report.root.toLowerCase());
  assert.equal(proposal.newEpoch, BigInt(report.epoch));
  assert.equal(proposal.newCumulativeSpent, BigInt(report.cumulative_spent_wei));
  assert.match(proposal.governanceSignature, /^0x[0-9a-f]+$/i);
  assert.equal(report.operator_fee.destination, (await operatorTreasury.getAddress()).toLowerCase());
  assert.equal(report.operator_fee.predicted_claimable_wei, (auToTapWei(usdAu(1), TAP_USD_AU) * 1500n / 10_000n).toString());
  assert.equal(report.operator_fee.calldata, encodeWithdrawOperatorCalldata({
    to: await operatorTreasury.getAddress(),
    amountWei: report.operator_fee.predicted_claimable_wei,
  }));
  assert.equal(report.operator_fee.auto_sent, false);
  assert.equal(report.burn.predicted_claimable_wei, (auToTapWei(usdAu(1), TAP_USD_AU) * 1000n / 10_000n).toString());
  assert.equal(report.burn.calldata, encodeBurnCalldata());
  assert.equal(report.burn.auto_sent, false);
  assert.match(report.copy_paste_confirm_command, /--confirm/);
  assert.match(report.copy_paste_confirm_command, /--operator-address/);
  assert.doesNotMatch(
    report.copy_paste_confirm_command,
    /--(?:settle-through-epoch|challenge-epochs|holdback-epochs)/
  );
  assert.doesNotMatch(JSON.stringify(report), new RegExp(OPERATOR_KEY.slice(2), 'i'));
  assert.doesNotMatch(report.copy_paste_replay_command, /--eth-rpc/);
  assert.doesNotMatch(report.copy_paste_replay_command, new RegExp(rpc.replaceAll('.', '\\.')));
  assert.equal(await pool.epoch(), 0n);

  const confirmed = await runNode([...baseArgs, '--confirm'], {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: signingEnv,
  });
  assert.equal(confirmed.status, 0, confirmed.stderr);
  const pendingFinality = JSON.parse(confirmed.stdout);
  assert.equal(pendingFinality.posted, true, JSON.stringify(pendingFinality));
  assert.equal(pendingFinality.awaiting_finality, true);
  await mineTapConfirmations(provider);
  const replayed = await runNode([...baseArgs, '--confirm'], {
    cwd: path.join(path.dirname(SCRIPT_PATH), '..'),
    env: signingEnv,
  });
  assert.equal(replayed.status, 0, replayed.stderr);
  const posted = JSON.parse(replayed.stdout);
  assert.equal(posted.root_confirmed, true, JSON.stringify(posted));
  assert.equal(posted.root_already_posted, true);
  assert.match(pendingFinality.proposal_tx, /^0x[0-9a-f]{64}$/i);
  assert.match(pendingFinality.execution_tx, /^0x[0-9a-f]{64}$/i);
  assert.match(posted.tap_settlement_checkpoint.proposal_tx, /^0x[0-9a-f]{64}$/i);
  assert.match(posted.tap_settlement_checkpoint.execution_tx, /^0x[0-9a-f]{64}$/i);
  assert.equal(posted.signing_address, (await operator.getAddress()).toLowerCase());
  assert.equal(pendingFinality.operator_fee.auto_sent, true);
  assert.match(pendingFinality.operator_fee.tx, /^0x[0-9a-f]{64}$/i);
  assert.equal(posted.operator_fee.completed, true);
  assert.equal(
    await token.balanceOf(await operatorTreasury.getAddress()),
    BigInt(pendingFinality.operator_fee.predicted_claimable_wei)
  );
  assert.equal(await pool.operatorClaimable(), 0n);
  assert.equal(pendingFinality.burn.auto_sent, true);
  assert.equal(posted.burn.completed, true);
  assert.match(pendingFinality.burn.tx, /^0x[0-9a-f]{64}$/i);
  assert.equal(
    await token.balanceOf(BURN_SINK),
    BigInt(pendingFinality.burn.predicted_claimable_wei)
  );
  assert.equal(await pool.burnClaimable(), 0n);
  assert.equal(await pool.epoch(), 1n);
});
