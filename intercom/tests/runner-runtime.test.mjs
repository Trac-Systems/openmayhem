import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const fixture = fileURLToPath(new URL('./fixtures/runner-runtime.mjs', import.meta.url));
const cwd = fileURLToPath(new URL('../', import.meta.url));
const args = ['checkpoint-writer', '--rpc-port', '49223', '--headless'];
const env = {
  ...process.env,
  MAYHEM_WRITER_CHECKPOINTS: '1',
  MAYHEM_WRITER_CHECKPOINT_DIR: '/test/checkpoints',
  MAYHEM_WRITER_CHECKPOINT_RESERVE_AU: '5000000000000000000',
};

for (const bare of [false, true]) {
  test(`runtime reads inherited checkpoint settings and arguments under ${bare ? 'Pear/Bare' : 'Node'}`, () => {
    const command = bare ? ['--input-type=module', '-e', `
      import PearRuntime from 'pear-runtime';
      const worker = PearRuntime.run(${JSON.stringify(fixture)}, ${JSON.stringify(args)});
      worker.stdout.pipe(process.stdout);
      worker.stderr.pipe(process.stderr);
      worker.on('exit', code => { process.exitCode = code ?? 1; });
    `] : [fixture, ...args];
    const result = spawnSync(process.execPath, command, { cwd, env, encoding: 'utf8', timeout: 30000 });
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stderr);
    const report = JSON.parse(result.stdout.trim());
    assert.equal(report.bare, bare);
    if (bare) assert.equal(report.globalProcess, false, 'Exercise production without a global Node process');
    assert.equal(report.checkpoints, '1');
    assert.equal(report.directory, env.MAYHEM_WRITER_CHECKPOINT_DIR);
    assert.equal(report.reserve, env.MAYHEM_WRITER_CHECKPOINT_RESERVE_AU);
    assert.deepEqual(report.argv, args);
    assert.equal(report.storeLabel, args[0]);
    assert.deepEqual(report.flags, { 'rpc-port': '49223', headless: true });
  });
}
