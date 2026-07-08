#!/usr/bin/env node
// Etherscan V2 source verification for MayhemInferencePool.
//
// Uses the same pinned local solc/npm OpenZeppelin tree as compile.mjs:
// solc 0.8.30, evmVersion paris, optimizer runs 200. Dry-run builds the exact
// request without submitting it.
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { ethers } from 'ethers';
import solc from 'solc';

import { DEFAULT_MAINNET_DEPLOYMENT_FILE } from './deploy-mainnet.mjs';

const REPO = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const SRC_DIR = join(REPO, 'contracts', 'src');
const NODE_MODULES = join(REPO, 'contracts', 'node_modules');
export const ETHERSCAN_V2_API = 'https://api.etherscan.io/v2/api';
export const MAYHEM_POOL_CONTRACT_NAME = 'MayhemInferencePool.sol:MayhemInferencePool';

function parseArgs(argv = process.argv.slice(2)) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (!arg.startsWith('--')) continue;
    const key = arg.slice(2);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith('--')) {
      out[key] = true;
    } else {
      out[key] = next;
      i += 1;
    }
  }
  return out;
}

function boolArg(value, fallback = false) {
  if (value === undefined) return fallback;
  if (value === true) return true;
  const text = String(value).trim().toLowerCase();
  return ['1', 'true', 'yes', 'on'].includes(text);
}

function firstEnv(env, names) {
  for (const name of names) {
    const value = env?.[name];
    if (value !== undefined && String(value).trim() !== '') return String(value).trim();
  }
  return null;
}

function normalizeAddress(value, label) {
  try {
    return ethers.getAddress(String(value ?? '').trim());
  } catch (_error) {
    throw new Error(`${label} must be an Ethereum address`);
  }
}

function parseNonNegativeBigInt(value, label) {
  try {
    const parsed = BigInt(String(value ?? '').trim());
    if (parsed < 0n) throw new Error();
    return parsed;
  } catch (_error) {
    throw new Error(`${label} must be a non-negative integer`);
  }
}

function readJson(file, label) {
  if (!existsSync(file)) throw new Error(`Missing ${label}: ${file}`);
  const parsed = JSON.parse(readFileSync(file, 'utf8'));
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error(`${label} must be a JSON object`);
  }
  return parsed;
}

function deploymentFileFrom(args, env) {
  return String(
    args['deployment-file']
      ?? env.MAYHEM_TAP_DEPLOYMENT_FILE
      ?? DEFAULT_MAINNET_DEPLOYMENT_FILE
  );
}

function explorerBaseForChain(chainId) {
  if (chainId === 1) return 'https://etherscan.io';
  if (chainId === 11155111) return 'https://sepolia.etherscan.io';
  if (chainId === 17000) return 'https://holesky.etherscan.io';
  return 'https://etherscan.io';
}

export function solcCompilerVersion() {
  const full = solc.version();
  const match = full.match(/^(\d+\.\d+\.\d+\+commit\.[0-9a-fA-F]+)/);
  if (!match) throw new Error(`Cannot parse solc version: ${full}`);
  return `v${match[1]}`;
}

export function buildStandardJsonInput() {
  const localSources = {};
  for (const file of readdirSync(SRC_DIR)) {
    if (file.endsWith('.sol')) {
      localSources[file] = { content: readFileSync(join(SRC_DIR, file), 'utf8') };
    }
  }
  const resolvedImports = {};
  function findImports(importPath) {
    try {
      const contents = readFileSync(join(NODE_MODULES, importPath), 'utf8');
      resolvedImports[importPath] = { content: contents };
      return { contents };
    } catch (_error) {
      return { error: `File not found: ${importPath}` };
    }
  }
  const settings = {
    evmVersion: 'paris',
    optimizer: { enabled: true, runs: 200 },
    outputSelection: { '*': { '*': ['abi', 'evm.bytecode.object'] } },
  };
  const probe = JSON.parse(solc.compile(JSON.stringify({
    language: 'Solidity',
    sources: localSources,
    settings,
  }), { import: findImports }));
  const fatal = (probe.errors || []).filter((error) => error.severity === 'error');
  if (fatal.length) {
    throw new Error(`Local solc errors:\n${fatal.map((error) => error.formattedMessage).join('\n')}`);
  }
  if (!probe.contracts?.['MayhemInferencePool.sol']?.MayhemInferencePool) {
    throw new Error('MayhemInferencePool not found in compile output');
  }
  return {
    standardJson: {
      language: 'Solidity',
      sources: { ...localSources, ...resolvedImports },
      settings,
    },
    localSourceCount: Object.keys(localSources).length,
    importSourceCount: Object.keys(resolvedImports).length,
  };
}

export function buildVerificationRequest({
  args = parseArgs(),
  env = process.env,
} = {}) {
  const deploymentFile = deploymentFileFrom(args, env);
  const deployment = readJson(deploymentFile, 'deployment file');
  const pool = normalizeAddress(args.pool || env.MAYHEM_TAP_POOL_ADDRESS || deployment.pool, 'pool address');
  const token = normalizeAddress(deployment.token || env.MAYHEM_TAP_TOKEN_ADDRESS || env.MAYHEM_TAP_TOKEN_ADDR, 'TAP token');
  const owner = normalizeAddress(deployment.owner, 'pool owner');
  const maxEpochDelta = parseNonNegativeBigInt(deployment.maxEpochDelta ?? deployment.max_epoch_delta_wei, 'maxEpochDelta');
  const chainId = Number(deployment.chainId ?? env.MAYHEM_TAP_ETH_CHAIN_ID ?? env.MAYHEM_TAP_CHAIN_ID ?? 1);
  if (!Number.isSafeInteger(chainId) || chainId <= 0) throw new Error('chainId must be a positive safe integer');

  const compilerVersion = solcCompilerVersion();
  const { standardJson, localSourceCount, importSourceCount } = buildStandardJsonInput();
  const constructorArgs = new ethers.AbiCoder()
    .encode(['address', 'address', 'uint256'], [token, owner, maxEpochDelta])
    .slice(2);
  const apiKey = firstEnv(env, ['MAYHEM_ETHERSCAN_API_KEY']);
  const sourceCode = JSON.stringify(standardJson);
  const form = new URLSearchParams({
    chainid: String(chainId),
    apikey: apiKey || '',
    module: 'contract',
    action: 'verifysourcecode',
    codeformat: 'solidity-standard-json-input',
    contractaddress: pool,
    sourceCode,
    contractname: MAYHEM_POOL_CONTRACT_NAME,
    compilerversion: compilerVersion,
    constructorArguements: constructorArgs,
  });

  return {
    deploymentFile,
    deployment,
    apiKey,
    form,
    sourceCode,
    report: {
      submitted: false,
      api_url: ETHERSCAN_V2_API,
      explorer_url: `${explorerBaseForChain(chainId)}/address/${pool}#code`,
      chain_id: chainId,
      pool,
      token,
      owner,
      max_epoch_delta_wei: maxEpochDelta.toString(),
      contract_name: MAYHEM_POOL_CONTRACT_NAME,
      compiler_version: compilerVersion,
      evm_version: 'paris',
      optimizer_runs: 200,
      constructor_args: constructorArgs,
      deployment_file: deploymentFile,
      source_file_count: Object.keys(standardJson.sources).length,
      local_source_file_count: localSourceCount,
      import_source_file_count: importSourceCount,
      source_code_bytes: Buffer.byteLength(sourceCode),
      request: {
        chainid: String(chainId),
        module: 'contract',
        action: 'verifysourcecode',
        codeformat: 'solidity-standard-json-input',
        contractaddress: pool,
        contractname: MAYHEM_POOL_CONTRACT_NAME,
        compilerversion: compilerVersion,
        constructorArguements: constructorArgs,
      },
    },
  };
}

function printVerificationPlan(report, { dryRun = false } = {}) {
  console.log('\n=== Etherscan verify (V2) ===');
  console.log('  chainId        :', report.chain_id);
  console.log('  pool           :', report.pool);
  console.log('  contract       :', report.contract_name);
  console.log('  compiler       :', report.compiler_version, 'evmVersion paris optimizer runs 200');
  console.log('  ctor args      : token', report.token, 'owner', report.owner, 'maxEpochDelta', report.max_epoch_delta_wei);
  console.log('  source files   :', report.source_file_count, `(${report.local_source_file_count} local + ${report.import_source_file_count} imports)`);
  console.log('  explorer       :', report.explorer_url);
  if (dryRun) console.log('\nDRY RUN - verification request built but not submitted.');
}

async function postForm(body) {
  const res = await fetch(ETHERSCAN_V2_API, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body,
  });
  const text = await res.text();
  try {
    return JSON.parse(text);
  } catch (_error) {
    throw new Error(`Non-JSON response from Etherscan: ${text.slice(0, 300)}`);
  }
}

async function getStatus(chainId, apiKey, guid) {
  const qs = new URLSearchParams({
    chainid: String(chainId),
    apikey: apiKey,
    module: 'contract',
    action: 'checkverifystatus',
    guid,
  });
  const res = await fetch(`${ETHERSCAN_V2_API}?${qs}`);
  return res.json();
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export async function verifyEtherscan({
  args = parseArgs(),
  env = process.env,
} = {}) {
  const json = boolArg(args.json, false);
  const dryRun = boolArg(args['dry-run'], false);
  const built = buildVerificationRequest({ args, env });
  const report = { ...built.report };
  if (!json) printVerificationPlan(report, { dryRun });
  if (dryRun) {
    if (json) console.log(JSON.stringify(report, null, 2));
    return report;
  }
  if (!built.apiKey) throw new Error('Missing MAYHEM_ETHERSCAN_API_KEY');

  if (!json) console.log('\nsubmitting...');
  const submit = await postForm(built.form);
  if (submit.status !== '1') {
    if (typeof submit.result === 'string' && /already verified/i.test(submit.result)) {
      const done = { ...report, submitted: true, verified: true, already_verified: true, result: submit.result };
      if (json) console.log(JSON.stringify(done, null, 2));
      else console.log(`\nverified: ${submit.result}\n${done.explorer_url}`);
      return done;
    }
    throw new Error(`Etherscan submit rejected: ${JSON.stringify(submit)}`);
  }

  const guid = submit.result;
  if (!json) console.log('  guid:', guid, '- polling status...');
  for (let i = 0; i < 30; i++) {
    await sleep(5000);
    const status = await getStatus(report.chain_id, built.apiKey, guid);
    const result = typeof status.result === 'string' ? status.result : JSON.stringify(status.result);
    if (status.status === '1' || /already verified/i.test(result)) {
      const done = { ...report, submitted: true, verified: true, guid, result };
      if (json) console.log(JSON.stringify(done, null, 2));
      else console.log(`\nverified: ${result}\n${done.explorer_url}`);
      return done;
    }
    if (/^Fail/i.test(result)) throw new Error(`Verification failed: ${result}`);
    if (!json) console.log(`  [${i + 1}/30] ${result}`);
  }
  throw new Error('Timed out waiting for Etherscan verification; re-run later to re-check status.');
}

if (import.meta.url === `file://${process.argv[1]}`) {
  verifyEtherscan().catch((error) => {
    console.error('verify-etherscan:', error?.shortMessage || error?.message || String(error));
    process.exit(1);
  });
}
