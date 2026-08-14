#!/usr/bin/env node
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';

const root = process.cwd();

function readJson(relativePath) {
  return JSON.parse(fs.readFileSync(path.join(root, relativePath), 'utf8'));
}

function stable(value) {
  if (Array.isArray(value)) return value.map(stable);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, stable(value[key])]),
    );
  }
  return value;
}

function same(left, right) {
  return JSON.stringify(stable(left)) === JSON.stringify(stable(right));
}

function stableBytes(value) {
  return Buffer.from(JSON.stringify(stable(value)));
}

function sha256(bytes) {
  return crypto.createHash('sha256').update(bytes).digest('hex');
}

const catalog = readJson('catalog/models.json');
const grid = readJson('catalog/comfy/outcome-classes-v1.json');
const definitions = new Map();
const errors = [];

for (const model of catalog.models || []) {
  const definition = model.workflow?.outcome_class_definition;
  if (!definition?.class_id) continue;
  const existing = definitions.get(definition.class_id);
  if (existing && !same(existing.definition, definition)) {
    errors.push(`${model.model_id}: conflicting embedded definition for ${definition.class_id}`);
  } else {
    definitions.set(definition.class_id, { model_id: model.model_id, definition });
  }
  const artifact = model.artifacts?.['workflow-class'];
  if (artifact?.source_sha256) {
    const bytes = stableBytes(definition);
    const actual = sha256(bytes);
    if (actual !== artifact.source_sha256) {
      errors.push(
        `${model.model_id}: workflow-class source_sha256 ${artifact.source_sha256} ` +
          `does not match embedded workflow definition ${actual}`,
      );
    }
  }
  if (!same(model.price_ref_au?.rate_map, definition.rate_map)) {
    errors.push(`${model.model_id}: price_ref_au.rate_map does not match workflow definition`);
  }
  if (!same(model.price_ref_au?.per_req_au, definition.per_req_au)) {
    errors.push(`${model.model_id}: price_ref_au.per_req_au does not match workflow definition`);
  }
  if (!same(model.price_ref_au?.min_session_au, definition.min_session_au)) {
    errors.push(`${model.model_id}: price_ref_au.min_session_au does not match workflow definition`);
  }
}

for (const row of grid.classes || []) {
  const entry = definitions.get(row.class_id);
  if (!entry) continue;
  for (const key of ['pricing_unit', 'rate_map', 'per_req_au', 'min_session_au']) {
    if (!same(row[key], entry.definition[key])) {
      errors.push(
        `${entry.model_id}: outcome grid ${row.class_id}.${key} ` +
          'does not match embedded workflow definition',
      );
    }
  }
}

if (errors.length) {
  for (const error of errors) console.error(error);
  process.exit(1);
}

console.log('Comfy pricing consistency ok');
