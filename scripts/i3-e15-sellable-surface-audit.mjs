#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const jsonMode = process.argv.includes('--json');

const launchProofs = new Map([
  ['qwen/qwen2.5-1.5b-instruct@small', 'I3-E11/I3-E14 real GGUF, TensorRT-LLM, and vLLM chat/tool paths'],
  ['meta/llama-3.1-8b-instruct@4bit', 'I3-E10 catalog/backend compatibility and launch source checks'],
  ['google/gemma-3-12b-it@4bit', 'I3-E10 catalog/backend compatibility and launch source checks'],
  ['deepseek/deepseek-r1-distill-qwen-14b@4bit', 'I3-E10 catalog/backend compatibility and launch source checks'],
  ['baai/bge-small-en-v1.5@gguf-q8_0', 'I3-E6 real embedding path'],
  ['huggingfacetb/smolvlm2-256m-video-instruct@gguf-q8_0', 'I3-E7/E12/E13 real vision chat path'],
  ['concedo/sdxs-512-tinysd-distilled@gguf-q8_0', 'I3-E8 real image-generation path'],
  ['openai/whisper-tiny-en@ggml', 'I3-E9 real STT path'],
  ['rhasspy/piper-en-us-lessac-low@onnx', 'I3-E9 real TTS path'],
]);

const classRoutes = {
  text: ['/v1/chat/completions', '/v1/completions'],
  embedding: ['/v1/embeddings'],
  'image-generation': ['/v1/images/generations'],
  stt: ['/v1/audio/transcriptions'],
  tts: ['/v1/audio/speech'],
};

const productionRoots = [
  'crates',
  'services',
  'intercom/contract',
  'intercom/features',
  'intercom/trac/trac-peer',
];

const productionExtensions = new Set(['.rs', '.js', '.mjs', '.cjs', '.ts', '.toml']);
const bannedPatterns = [
  ['deterministic', /deterministic_/],
  ['pending-marker', /PENDING_/],
  ['canned', /\bcanned\b/i],
  ['placeholder', /\bplaceholder\b/i],
  ['contract-simulate', /contract_simulate|contract\.simulate|MAYHEM_PAYGATE_CONTRACT_SIM/],
  ['unlocked-signer', /getSigner\s*\(/],
  ['empty-main', /fn\s+main\s*\(\s*\)\s*\{\s*\}/],
  ['mock', /\bmock\b|Mock[A-Z_]/],
];

const dashboardForbidden = [
  '/mayhem/dashboard/components',
  '1,240.00 TAP',
  '42 tok/s',
  'testtrac1n57',
];

const checks = [];

function repo(file) {
  return path.join(repoRoot, file);
}

function read(file) {
  return fs.readFileSync(repo(file), 'utf8');
}

function ok(id, message, extra = {}) {
  checks.push({ id, status: 'ok', message, ...extra });
}

function fail(id, message, extra = {}) {
  checks.push({ id, status: 'fail', message, ...extra });
}

function assertCheck(id, condition, message, extra = {}) {
  if (condition) ok(id, message, extra);
  else fail(id, message, extra);
}

function routeListFor(model) {
  const modelClass = model.model_class || 'text';
  const routes = classRoutes[modelClass] || [];
  if (modelClass === 'text' && model.caps?.vision === true) {
    return routes.map((route) => `${route} with vision input`);
  }
  return routes;
}

function markdownCells(line) {
  return line
    .trim()
    .replace(/^\|/, '')
    .replace(/\|$/, '')
    .split('|')
    .map((cell) => cell.trim());
}

function launchSurfaceSection(readme) {
  const startMarker = '<!-- MAYHEM-LAUNCH-SURFACE:START -->';
  const endMarker = '<!-- MAYHEM-LAUNCH-SURFACE:END -->';
  const start = readme.indexOf(startMarker);
  const end = readme.indexOf(endMarker);
  if (start === -1 || end === -1 || end <= start) {
    fail('readme.launch_surface.markers', 'README launch surface markers are missing or misordered');
    return null;
  }
  ok('readme.launch_surface.markers', 'README launch surface table is marker-bound');
  return readme.slice(start + startMarker.length, end);
}

function parseLaunchRows(section) {
  const rows = [];
  for (const line of section.split('\n')) {
    const cells = markdownCells(line);
    if (cells.length < 6) continue;
    const id = cells[0].replace(/^`|`$/g, '');
    if (!id.includes('/')) continue;
    rows.push({ id, cells, line });
  }
  return rows;
}

function listFiles(dir) {
  const root = repo(dir);
  if (!fs.existsSync(root)) return [];
  const out = [];
  const stack = [root];
  while (stack.length > 0) {
    const current = stack.pop();
    const stat = fs.lstatSync(current);
    const name = path.basename(current);
    if (stat.isDirectory()) {
      if (['target', 'node_modules', '.git', 'dist', 'build'].includes(name)) continue;
      for (const child of fs.readdirSync(current)) stack.push(path.join(current, child));
    } else if (stat.isFile()) {
      const rel = path.relative(repoRoot, current);
      if (rel.split(path.sep).includes('tests')) continue;
      if (rel.endsWith('package-lock.json')) continue;
      if (!productionExtensions.has(path.extname(current))) continue;
      out.push(rel);
    }
  }
  return out.sort();
}

function checkCatalogAndReadme() {
  const catalog = JSON.parse(read('catalog/models.json'));
  const readme = read('README.md');
  const launchModels = catalog.models.filter((model) => model.tier === 'launch');
  const devModels = catalog.models.filter((model) => model.tier === 'dev');
  const launchIds = launchModels.map((model) => model.model_id);
  const devIds = devModels.map((model) => model.model_id);

  assertCheck('catalog.count.launch', launchIds.length === launchProofs.size, `catalog has ${launchIds.length} launch models`, { launchIds });
  assertCheck('catalog.count.dev', devIds.length === 2, `catalog has ${devIds.length} dev-only models`, { devIds });

  for (const model of launchModels) {
    const modelClass = model.model_class || 'text';
    assertCheck(`catalog.${model.model_id}.proof`, launchProofs.has(model.model_id), `${model.model_id} has launch proof mapping`);
    assertCheck(`catalog.${model.model_id}.class`, !!classRoutes[modelClass], `${model.model_id} has routable class ${modelClass}`);
    assertCheck(`catalog.${model.model_id}.routes`, routeListFor(model).length > 0, `${model.model_id} has gateway routes`);
    assertCheck(`catalog.${model.model_id}.denom`, model.price_ref_au?.denom === 'au_usd', `${model.model_id} is priced in au_usd`);
    const artifacts = Object.entries(model.artifacts || {});
    assertCheck(`catalog.${model.model_id}.artifacts`, artifacts.length > 0, `${model.model_id} has signed artifacts`);
    for (const [name, artifact] of artifacts) {
      assertCheck(
        `catalog.${model.model_id}.${name}.root`,
        artifact.artifact_root_kind === 'blake3_merkle_v1',
        `${model.model_id}/${name} uses full Merkle artifact root`
      );
      assertCheck(
        `catalog.${model.model_id}.${name}.source`,
        artifact.source?.kind === 'huggingface' && !!artifact.source?.repo && !!artifact.source?.revision && !!artifact.path,
        `${model.model_id}/${name} has a pinned Hugging Face source`
      );
      for (const [sidecarName, sidecar] of Object.entries(artifact.sidecars || {})) {
        assertCheck(
          `catalog.${model.model_id}.${name}.${sidecarName}.root`,
          sidecar.artifact_root_kind === 'blake3_merkle_v1',
          `${model.model_id}/${name}/${sidecarName} uses full Merkle sidecar root`
        );
      }
    }
  }

  const section = launchSurfaceSection(readme);
  if (!section) return;
  const rows = parseLaunchRows(section);
  const rowIds = rows.map((row) => row.id);
  assertCheck(
    'readme.launch_surface.rows',
    JSON.stringify(rowIds) === JSON.stringify(launchIds),
    'README launch surface rows match catalog launch model order',
    { rowIds, launchIds }
  );
  for (const id of devIds) {
    assertCheck(`readme.launch_surface.dev.${id}`, !section.includes(id), `${id} is not advertised as launch sellable`);
  }
  assertCheck(
    'readme.launch_surface.tiers',
    !/\bTier [234]\b/.test(section),
    'launch table does not advertise higher tiers before D5'
  );
  for (const row of rows) {
    const model = launchModels.find((entry) => entry.model_id === row.id);
    if (!model) continue;
    const joined = row.line;
    for (const route of routeListFor(model)) {
      const routePath = route.split(' ')[0];
      assertCheck(`readme.${row.id}.route.${routePath}`, joined.includes(routePath), `${row.id} README row lists ${routePath}`);
    }
    for (const [artifactName, artifact] of Object.entries(model.artifacts || {})) {
      assertCheck(`readme.${row.id}.artifact.${artifactName}`, joined.includes(artifactName), `${row.id} README row lists ${artifactName}`);
      assertCheck(`readme.${row.id}.engine.${artifact.engine}`, joined.includes(artifact.engine), `${row.id} README row lists ${artifact.engine}`);
    }
    assertCheck(`readme.${row.id}.tier1`, joined.includes('Tier 1 launch'), `${row.id} README row is Tier 1 launch`);
  }
  assertCheck(
    'readme.catalog.discovery',
    readme.includes('mayhem models') && readme.includes('ledger anchor') && readme.includes('without requiring a repo update'),
    'README says catalog discovery is ledger-anchored and repo-update-free'
  );
  assertCheck(
    'readme.admin.control',
    readme.includes('Providers opt into canonical enclaves') && readme.includes('do not set prices') && readme.includes('submit arbitrary models'),
    'README preserves admin-only economy/catalog control'
  );
}

function checkGatewayAndCliSurface() {
  const gateway = read('crates/mayhem-gateway/src/openai.rs');
  const cli = read('crates/mayhem-cli/src/main.rs');
  const requiredGatewayRoutes = [
    '/v1/models',
    '/v1/chat/completions',
    '/v1/completions',
    '/v1/embeddings',
    '/v1/images/generations',
    '/v1/audio/speech',
    '/v1/audio/transcriptions',
    '/mayhem/dashboard',
    '/mayhem/dashboard/provider',
    '/mayhem/dashboard/session',
  ];
  for (const route of requiredGatewayRoutes) {
    assertCheck(`gateway.route.${route}`, gateway.includes(`"${route}"`), `gateway exposes ${route}`);
  }
  for (const forbidden of dashboardForbidden) {
    assertCheck(`gateway.dashboard.no_demo.${forbidden}`, !gateway.includes(forbidden), `dashboard source does not contain demo value ${forbidden}`);
  }
  const cliSignals = [
    'Commands::Models(args) => models(args).await',
    'read_catalog_release_anchor(&rpc)',
    'fetch_catalog_release_files',
    'CatalogListTier::Launch',
    'gateway_model_summaries',
  ];
  for (const signal of cliSignals) {
    assertCheck(`cli.models.${signal}`, cli.includes(signal), `mayhem models surface includes ${signal}`);
  }
}

function checkBannedPatterns() {
  const hits = [];
  for (const root of productionRoots) {
    for (const file of listFiles(root)) {
      const text = fs.readFileSync(repo(file), 'utf8');
      const lines = text.split('\n');
      lines.forEach((line, index) => {
        for (const [id, pattern] of bannedPatterns) {
          if (pattern.test(line)) {
            hits.push({ id, file, line: index + 1, text: line.trim() });
          }
        }
      });
    }
  }
  assertCheck('banned_patterns.production', hits.length === 0, 'production paths have zero banned §0 patterns', { hits });
}

checkCatalogAndReadme();
checkGatewayAndCliSurface();
checkBannedPatterns();

const failed = checks.filter((check) => check.status !== 'ok');
const report = {
  ok: failed.length === 0,
  checks,
  failures: failed,
};

if (jsonMode) {
  console.log(JSON.stringify(report, null, 2));
} else if (report.ok) {
  console.log(`I3-E15 sellable-surface audit passed (${checks.length} checks).`);
} else {
  console.error(`I3-E15 sellable-surface audit failed (${failed.length}/${checks.length} checks):`);
  for (const failure of failed) {
    console.error(`- ${failure.id}: ${failure.message}`);
    if (failure.hits) {
      for (const hit of failure.hits.slice(0, 20)) {
        console.error(`  ${hit.file}:${hit.line}: ${hit.text}`);
      }
    }
  }
}

process.exit(report.ok ? 0 : 1);
