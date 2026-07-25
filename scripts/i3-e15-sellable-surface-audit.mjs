#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import {
  launchRowsMatchingModel,
  parseLaunchRows,
} from './lib/launch-roster.mjs';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const jsonMode = process.argv.includes('--json');

const endpointRoutes = {
  openai_chat_completions: '/v1/chat/completions',
  openai_completions: '/v1/completions',
  openai_responses: '/v1/responses',
  hf_multimodal_chat: '/v1/chat/completions',
  openai_embeddings: '/v1/embeddings',
  hf_feature_extraction: '/v1/embeddings',
  openai_image_generations: '/v1/images/generations',
  hf_text_to_image: '/v1/images/generations',
  openai_audio_transcriptions: '/v1/audio/transcriptions',
  hf_automatic_speech_recognition: '/v1/audio/transcriptions',
  openai_audio_speech: '/v1/audio/speech',
  hf_text_to_speech: '/v1/audio/speech',
  openai_videos: '/v1/videos',
  hf_text_to_video: '/v1/videos',
  mayhem_audio_generations: '/v1/audio/generations',
  mayhem_music_generations: '/v1/music/generations',
  hf_text_to_audio: '/v1/audio/generations',
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
  ['pending-marker', /\bPENDING_(?:IMPLEMENTATION|MODEL|PROOF|REPLACE_ME)\b/],
  ['canned', /\bcanned\b/i],
  ['placeholder', /\bplaceholder_(?:implementation|output|response|result)\b|\b(?:TODO|FIXME)\b[^\n]{0,40}\bplaceholder\b/i],
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
  return [...new Set((model.adapter?.endpoint_families || []).map(({ family }) => endpointRoutes[family]).filter(Boolean))];
}

function launchSurfaceSection(readme) {
  const startMarker = '## Available Models';
  const start = readme.indexOf(startMarker);
  const nextHeading = start === -1 ? -1 : readme.indexOf('\n## ', start + startMarker.length);
  if (start === -1 || nextHeading === -1) {
    fail('readme.launch_surface.section', 'README Available Models section is missing or unterminated');
    return null;
  }
  ok('readme.launch_surface.section', 'README Available Models section is heading-bound');
  return readme.slice(start + startMarker.length, nextHeading);
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

function rustProductionText(text) {
  const topLevelTests = /^#\[cfg\(test\)\]\nmod tests \{/m.exec(text);
  if (topLevelTests) {
    const production = text.slice(0, topLevelTests.index);
    text = production + '\n'.repeat(text.slice(topLevelTests.index).split('\n').length - 1);
  }
  const lines = text.split('\n');
  let pendingTestItem = null;
  let skippedItemIndent = null;

  return lines.map((line) => {
    if (skippedItemIndent !== null) {
      const closing = new RegExp(`^${skippedItemIndent.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\}(?:;)?\\s*$`);
      if (closing.test(line)) skippedItemIndent = null;
      return '';
    }

    if (/^\s*#\[cfg\(test\)\]\s*$/.test(line)) {
      pendingTestItem = line.match(/^\s*/)[0];
      return '';
    }

    if (pendingTestItem !== null) {
      const trimmed = line.trim();
      if (trimmed === '' || trimmed.startsWith('#[')) return '';
      if (line.includes('{')) skippedItemIndent = pendingTestItem;
      if (line.includes(';') || skippedItemIndent !== null) pendingTestItem = null;
      return '';
    }

    return line;
  }).join('\n');
}

function checkCatalogAndReadme() {
  const catalog = JSON.parse(read('catalog/models.json'));
  const readme = read('README.md');
  const launchModels = catalog.models.filter((model) => model.tier === 'launch');
  const launchIds = launchModels.map((model) => model.model_id);
  const uniqueIds = new Set(catalog.models.map((model) => model.model_id));

  assertCheck('catalog.count.launch', launchIds.length > 0, `catalog has ${launchIds.length} launch models`, { launchIds });
  assertCheck('catalog.ids.unique', uniqueIds.size === catalog.models.length, 'catalog model ids are unique');

  for (const model of launchModels) {
    const endpointFamilies = (model.adapter?.endpoint_families || []).map(({ family }) => family);
    const calibratedArtifacts = new Set(Object.keys(model.modality_assessment?.calibrated_fingerprints || {}));
    assertCheck(
      `catalog.${model.model_id}.proof`,
      !!model.canary?.set_id && !!model.canary?.verification_method,
      `${model.model_id} has signed canary evidence`
    );
    assertCheck(
      `catalog.${model.model_id}.endpoints`,
      endpointFamilies.length > 0 && endpointFamilies.every((family) => !!endpointRoutes[family]),
      `${model.model_id} declares only routed endpoint families`,
      { endpointFamilies }
    );
    assertCheck(`catalog.${model.model_id}.routes`, routeListFor(model).length > 0, `${model.model_id} has gateway routes`);
    assertCheck(`catalog.${model.model_id}.denom`, model.price_ref_au?.denom === 'au_usd', `${model.model_id} is priced in au_usd`);
    const artifacts = Object.entries(model.artifacts || {});
    assertCheck(`catalog.${model.model_id}.artifacts`, artifacts.length > 0, `${model.model_id} has signed artifacts`);
    for (const [name, artifact] of artifacts) {
      assertCheck(
        `catalog.${model.model_id}.${name}.calibration`,
        calibratedArtifacts.has(name),
        `${model.model_id}/${name} has calibrated modality evidence`
      );
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
  const rows = parseLaunchRows(section).filter(({ status }) => status === 'live');
  for (const model of launchModels) {
    const matchingRows = launchRowsMatchingModel(rows, model);
    assertCheck(
      `readme.launch_surface.model.${model.model_id}`,
      matchingRows.length === 1,
      `README launch roster includes exactly one row for ${model.model_id}`,
      { matchingRows: matchingRows.map(({ id }) => id) }
    );
  }
  assertCheck(
    'readme.launch_surface.tiers',
    !/\bTier [234]\b/.test(section),
    'launch table does not advertise higher tiers before D5'
  );
  assertCheck('readme.launch_surface.rows', rows.length > 0, 'README launch roster contains model rows');
  for (const model of launchModels) {
    for (const route of routeListFor(model)) {
      assertCheck(`readme.${model.model_id}.route.${route}`, section.includes(route), `${model.model_id} README surface lists ${route}`);
    }
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
    '/v1/responses',
    '/v1/embeddings',
    '/v1/images/generations',
    '/v1/videos',
    '/v1/audio/speech',
    '/v1/audio/transcriptions',
    '/v1/audio/generations',
    '/v1/music/generations',
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
      const rawText = fs.readFileSync(repo(file), 'utf8');
      const text = file.endsWith('.rs') ? rustProductionText(rawText) : rawText;
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
