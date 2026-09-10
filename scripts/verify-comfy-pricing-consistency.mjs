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

// `megapixel_step` rounds each frame's area up to a whole megapixel before
// multiplying by frames, so every sub-megapixel canvas bills identically. Video
// outcomes must be metered in exact `pixel_frame` units instead, and Core fails
// admission closed if it ever sees a video outcome priced in `megapixel_step`.
const VIDEO_FORBIDDEN_UNITS = new Set(['megapixel_step']);
const PIXEL_FRAME_GRANULARITY = 1000000;
const DECIMAL_INTEGER = /^[0-9]+$/;

// A fixed per-request fee is money, so it must be an exact decimal-integer string
// of atto-USD. A JSON number here would silently lose precision.
function checkFixedFee(label, value, field) {
  if (typeof value !== 'string' || !DECIMAL_INTEGER.test(value)) {
    errors.push(`${label}: ${field} must be a decimal integer string of atto-USD, got ${JSON.stringify(value)}`);
    return false;
  }
  return true;
}

function checkUnits(label, media, pricingUnit, rateMap) {
  if (media === 'video' && VIDEO_FORBIDDEN_UNITS.has(pricingUnit)) {
    errors.push(`${label}: video pricing_unit ${pricingUnit} is image-only; use pixel_frame`);
  }
  for (const entry of rateMap || []) {
    if (media === 'video' && VIDEO_FORBIDDEN_UNITS.has(entry.unit)) {
      errors.push(`${label}: video rate_map unit ${entry.unit} is image-only; use pixel_frame`);
    }
    if (media !== 'video' && entry.unit === 'pixel_frame') {
      errors.push(`${label}: pixel_frame is video-only but media is ${media}`);
    }
    if (entry.unit === 'pixel_frame' && entry.granularity !== PIXEL_FRAME_GRANULARITY) {
      errors.push(
        `${label}: pixel_frame granularity must be ${PIXEL_FRAME_GRANULARITY}, ` +
          `got ${entry.granularity}`,
      );
    }
  }
}

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
  checkUnits(model.model_id, definition.media, definition.pricing_unit, definition.rate_map);
  checkFixedFee(model.model_id, definition.per_req_au, 'workflow definition per_req_au');
  checkFixedFee(model.model_id, definition.min_session_au, 'workflow definition min_session_au');
  checkFixedFee(model.model_id, model.price_ref_au?.per_req_au, 'price_ref_au.per_req_au');
  checkFixedFee(model.model_id, model.price_ref_au?.min_session_au, 'price_ref_au.min_session_au');
  if (model.workflow?.pricing_unit !== definition.pricing_unit) {
    errors.push(
      `${model.model_id}: workflow.pricing_unit ${model.workflow?.pricing_unit} ` +
        `does not match workflow definition pricing_unit ${definition.pricing_unit}`,
    );
  }
}

const modelsByClassId = new Map();
for (const model of catalog.models || []) {
  const classId = model.workflow?.outcome_class_definition?.class_id;
  if (classId) modelsByClassId.set(classId, model);
}

for (const row of grid.classes || []) {
  checkUnits(`outcome grid ${row.class_id}`, row.media, row.pricing_unit, row.rate_map);
  checkFixedFee(`outcome grid ${row.class_id}`, row.per_req_au, 'per_req_au');
  checkFixedFee(`outcome grid ${row.class_id}`, row.min_session_au, 'min_session_au');

  // The fixed per-request fee is a second, independent money field: compare the
  // grid straight to the shipped catalog price rather than trusting the chain
  // through the embedded definition.
  const model = modelsByClassId.get(row.class_id);
  if (model) {
    for (const field of ['per_req_au', 'min_session_au']) {
      if (!same(row[field], model.price_ref_au?.[field])) {
        errors.push(
          `${model.model_id}: outcome grid ${row.class_id}.${field} ${JSON.stringify(row[field])} ` +
            `does not match catalog price_ref_au.${field} ${JSON.stringify(model.price_ref_au?.[field])}`,
        );
      }
    }
    if (!same(row.rate_map, model.price_ref_au?.rate_map)) {
      errors.push(
        `${model.model_id}: outcome grid ${row.class_id}.rate_map does not match catalog price_ref_au.rate_map`,
      );
    }
  }

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
