import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  applyCaptureState,
  enforceRetention,
  heapSnapshotSummary,
  optionsFromArgs,
  parseByteSize,
  parseThresholds,
  shouldCapture,
  thresholdLabel,
} from '../ops/pear-memory-threshold-capture.mjs';

test('Pear memory capture parses thresholds and labels them predictably', () => {
  assert.equal(parseByteSize('4GiB'), 4 * 1024 ** 3);
  assert.equal(parseByteSize('1.5GiB'), Math.round(1.5 * 1024 ** 3));
  assert.deepEqual(parseThresholds('8GiB,4GiB,4GiB'), [
    4 * 1024 ** 3,
    8 * 1024 ** 3,
  ]);
  assert.equal(thresholdLabel(16 * 1024 ** 3), '16GiB');
});

test('Pear memory capture fires once per threshold per PID with cooldown', () => {
  const thresholds = parseThresholds('4GiB,8GiB,16GiB');
  const pid = 1234;
  const now = 1_000_000;
  const first = shouldCapture({}, pid, 9 * 1024 ** 3, thresholds, now, 600_000);
  assert.equal(first.capture, true);
  assert.deepEqual(first.thresholds, [
    4 * 1024 ** 3,
    8 * 1024 ** 3,
  ]);

  const state = applyCaptureState({}, pid, first.thresholds, now);
  assert.equal(
    shouldCapture(state, pid, 9 * 1024 ** 3, thresholds, now + 600_001, 600_000).capture,
    false
  );
  assert.equal(
    shouldCapture(state, pid, 17 * 1024 ** 3, thresholds, now + 10_000, 600_000).reason,
    'cooldown'
  );
  const afterCooldown = shouldCapture(
    state,
    pid,
    17 * 1024 ** 3,
    thresholds,
    now + 600_001,
    600_000
  );
  assert.equal(afterCooldown.capture, true);
  assert.deepEqual(afterCooldown.thresholds, [16 * 1024 ** 3]);

  const restarted = shouldCapture(state, pid + 1, 5 * 1024 ** 3, thresholds, now + 1, 600_000);
  assert.equal(restarted.capture, true);
  assert.deepEqual(restarted.thresholds, [4 * 1024 ** 3]);
});

test('Pear memory capture options remain .30-friendly by default', () => {
  const options = optionsFromArgs({
    'artifact-root': '/tmp/mayhem-captures',
    thresholds: '4GiB',
    once: true,
    'no-heap': true,
  });
  assert.equal(options.service, 'mayhem-stack.service');
  assert.equal(options.artifactRoot, '/tmp/mayhem-captures');
  assert.equal(options.stateFile, '/tmp/mayhem-captures/state.json');
  assert.equal(options.heap, false);
  assert.equal(options.once, true);
  assert.equal(options.maxCaptures, 8);
  assert.equal(options.maxBytes, 8 * 1024 ** 3);
});

test('Pear memory capture retention prunes oldest captures', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-capture-retention-'));
  try {
    for (const [index, started] of [
      [1, '2026-07-29T10:00:00.000Z'],
      [2, '2026-07-29T10:01:00.000Z'],
      [3, '2026-07-29T10:02:00.000Z'],
    ]) {
      const dir = path.join(root, `pear-123-${index}-threshold`);
      fs.mkdirSync(dir);
      fs.writeFileSync(path.join(dir, 'manifest.json'), JSON.stringify({ started_at: started, files: {} }));
      fs.writeFileSync(path.join(dir, 'payload.bin'), Buffer.alloc(16));
    }
    const result = enforceRetention({
      artifactRoot: root,
      maxCaptures: 2,
      maxBytes: 1024,
    });
    assert.equal(result.removed.length, 1);
    assert.equal(path.basename(result.removed[0].dir), 'pear-123-1-threshold');
    assert.equal(fs.existsSync(path.join(root, 'pear-123-1-threshold')), false);
    assert.equal(fs.existsSync(path.join(root, 'pear-123-2-threshold')), true);
    assert.equal(fs.existsSync(path.join(root, 'pear-123-3-threshold')), true);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('Pear memory capture heap summary focuses retainers without raw string values', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'mayhem-heap-summary-'));
  const file = path.join(root, 'fixture.heapsnapshot');
  const strings = [
    '',
    'system / JSArrayBufferData',
    'Buffer',
    'buffer',
    'secret-token-value-that-must-not-be-reflected',
  ];
  const snapshot = {
    snapshot: {
      meta: {
        node_fields: ['type', 'name', 'id', 'self_size', 'edge_count'],
        node_types: [[
          'hidden',
          'array',
          'string',
          'object',
          'code',
          'closure',
          'regexp',
          'number',
          'native',
          'synthetic',
        ]],
        edge_fields: ['type', 'name_or_index', 'to_node'],
        edge_types: [[
          'context',
          'element',
          'property',
          'internal',
          'hidden',
          'shortcut',
          'weak',
        ]],
      },
    },
    strings,
    nodes: [
      9, 0, 1, 0, 2,
      3, 2, 2, 40, 1,
      8, 1, 3, 1024, 0,
      2, 4, 4, 256, 0,
    ],
    edges: [
      2, 3, 5,
      2, 4, 15,
      3, 3, 10,
    ],
  };
  fs.writeFileSync(file, JSON.stringify(snapshot));
  try {
    const summary = heapSnapshotSummary(file);
    assert.equal(summary.native_arraybuffer_bytes, 1024);
    assert.equal(
      summary.incoming_arraybuffer_retainers.some((entry) =>
        entry.name.includes('object:Buffer via internal:buffer')
      ),
      true
    );
    assert.equal(JSON.stringify(summary).includes('secret-token-value'), false);
    assert.equal(
      summary.by_kind.some((entry) => entry.name === 'string' && entry.bytes === 256),
      true
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
