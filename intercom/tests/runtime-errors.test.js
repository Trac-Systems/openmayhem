import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';

import { installFatalRuntimeErrorPolicy } from '../src/runtime-errors.js';

test('fatal runtime policy logs and exits once for an unhandled rejection', () => {
  const runtime = new EventEmitter();
  const exits = [];
  const logs = [];
  runtime.exit = (code) => exits.push(code);
  installFatalRuntimeErrorPolicy(runtime, { log: (...args) => logs.push(args.join(' ')) });

  runtime.emit('unhandledRejection', new Error('boom'));
  runtime.emit('uncaughtException', new Error('later'));

  assert.equal(runtime.exitCode, 1);
  assert.deepEqual(exits, [1]);
  assert.equal(logs.length, 1);
  assert.match(logs[0], /fatal unhandled rejection/);
  assert.match(logs[0], /boom/);
});
