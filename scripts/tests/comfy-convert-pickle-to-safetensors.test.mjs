import assert from 'node:assert/strict'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'

const python = process.env.PYTHON || 'python3'

function runPython (code, args = []) {
  return spawnSync(python, ['-', ...args], {
    input: code,
    encoding: 'utf8'
  })
}

function hasPythonDeps () {
  const result = runPython(`
import importlib.util
raise SystemExit(0 if importlib.util.find_spec("torch") and importlib.util.find_spec("safetensors") else 1)
`)
  return result.status === 0
}

if (!hasPythonDeps()) {
  console.log('skip: torch/safetensors are not installed for the local Python')
  process.exit(0)
}

const temp = mkdtempSync(join(tmpdir(), 'mayhem-comfy-convert-'))
try {
  const input = join(temp, 'legacy.pth')
  const output = join(temp, 'converted.safetensors')
  const create = runPython(`
import torch
torch.save({"state_dict": {"layer.weight": torch.arange(4).reshape(2, 2), "nested": {"bias": torch.ones(2)}}}, __import__("sys").argv[1])
`, [input])
  assert.equal(create.status, 0, create.stderr)

  const result = spawnSync(python, [
    'scripts/comfy-convert-pickle-to-safetensors.py',
    '--input',
    input,
    '--output',
    output
  ], { encoding: 'utf8' })
  assert.equal(result.status, 0, result.stderr)
  const report = JSON.parse(result.stdout)
  assert.equal(report.ok, true)
  assert.equal(report.file_format, 'safetensors')
  assert.equal(report.tensor_count, 2)
  assert.match(report.input_sha256, /^[0-9a-f]{64}$/)
  assert.match(report.output_sha256, /^[0-9a-f]{64}$/)

  const inspect = runPython(`
import safetensors.torch, sys
tensors = safetensors.torch.load_file(sys.argv[1])
assert sorted(tensors) == ["layer.weight", "nested.bias"], sorted(tensors)
`, [output])
  assert.equal(inspect.status, 0, inspect.stderr)
} finally {
  rmSync(temp, { recursive: true, force: true })
}
