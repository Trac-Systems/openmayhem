// Offline before/after reproduction. Run with Node from either package checkout.
// Tests pristine, released v1 and v2 bytes in disposable copies; never edits the installed dependency.
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import { execFileSync } from 'node:child_process'
import { loadPatchSets, findHypercoreCopies, applyPatchSet, unapplyPatchSet, sha256Hex } from '../../../scripts/hypercore-stallfix/apply.mjs'

const here = path.dirname(fileURLToPath(import.meta.url))
const require = createRequire(import.meta.url)

function packageDir(name, from) {
  for (let dir = from; ; dir = path.dirname(dir)) {
    const candidate = path.join(dir, 'node_modules', name)
    if (fs.existsSync(path.join(candidate, 'package.json'))) return fs.realpathSync(candidate)
    if (dir === path.dirname(dir)) throw new Error('Cannot resolve ' + name + ' from ' + from)
  }
}

const closed = (stream) => new Promise((resolve, reject) => {
  stream.once('error', reject)
  stream.once('close', resolve)
})

async function measure(Hypercore, storage, rounds) {
  let release
  const preload = new Promise((resolve) => { release = resolve })
  const core = new Hypercore(path.join(storage, 'writer'), { preload })
  let clone
  let a
  let b
  try {
    for (let i = 0; i < rounds; i++) {
      const noise = core.replicate(true, { keepAlive: false }).noiseStream
      const closing = closed(noise)
      noise.destroy()
      await closing
      assert.equal(noise.destroyed, true)
    }
    release({})
    await core.ready()
    const attached = core.core.replicator._attached
    const result = {
      rounds,
      attached: attached.size,
      destroyed: [...attached].filter((mux) => mux.stream.destroyed).length
    }
    await core.append('normal replication after churn')
    clone = new Hypercore(path.join(storage, 'clone'), core.key)
    await clone.ready()
    a = core.replicate(true, { keepAlive: false })
    b = clone.replicate(false, { keepAlive: false })
    a.pipe(b).pipe(a)
    assert.equal((await clone.get(0, { timeout: 5000 })).toString(), 'normal replication after churn')
    const aClosed = closed(a.noiseStream)
    const bClosed = closed(b.noiseStream)
    a.destroy()
    b.destroy()
    await Promise.all([aClosed, bClosed])
    assert.equal(attached.size, result.attached)
    assert.equal(clone.core.replicator._attached.size, 0)
    result.normalReplication = true
    return result
  } finally {
    release({})
    a?.destroy()
    b?.destroy()
    await clone?.close()
    await core.close()
  }
}

if (process.argv[2] === '--measure') {
  const [, , , moduleDir, mode, storage] = process.argv
  const Hypercore = require(moduleDir)
  const version = require(path.join(moduleDir, 'package.json')).version
  const samples = []
  for (const rounds of [0, 50, 100, 200]) {
    const sample = await measure(Hypercore, path.join(storage, String(rounds)), rounds)
    assert.equal(sample.attached, mode === 'v2' ? 0 : rounds)
    assert.equal(sample.destroyed, mode === 'v2' ? 0 : rounds)
    samples.push(sample)
  }
  console.log(JSON.stringify({ version, mode, samples }))
} else {
  const root = path.resolve(here, '../../..')
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'hc-closed-stream-repro-'))
  try {
    const sets = await loadPatchSets()
    const seen = new Set()
    for (const copy of findHypercoreCopies(root)) {
      const pkg = JSON.parse(fs.readFileSync(path.join(copy.dir, 'package.json'), 'utf8'))
      const set = sets.get(pkg.version)
      if (!set || seen.has(pkg.version)) continue
      seen.add(pkg.version)
      let original = fs.readFileSync(path.join(copy.dir, set.file), 'utf8')
      if (sha256Hex(original) === set.previousPatchedSha256) original = applyPatchSet(original, { edits: set.upgradeEdits })
      if (sha256Hex(original) === set.patchedSha256) original = unapplyPatchSet(original, set)
      assert.equal(sha256Hex(original), set.originalSha256, 'verified pristine input')
      const v2 = applyPatchSet(original, set)
      const v1 = unapplyPatchSet(v2, { edits: set.upgradeEdits })
      assert.equal(sha256Hex(v1), set.previousPatchedSha256, 'exact released v1')
      assert.equal(sha256Hex(v2), set.patchedSha256, 'exact v2')
      for (const [mode, source] of [['pristine', original], ['v1', v1], ['v2', v2]]) {
        const trial = path.join(scratch, pkg.version, mode)
        const moduleDir = path.join(trial, 'hypercore')
        fs.cpSync(copy.dir, moduleDir, { recursive: true })
        for (const name of Object.keys(pkg.dependencies)) {
          const target = path.join(trial, 'node_modules', name)
          fs.mkdirSync(path.dirname(target), { recursive: true })
          fs.symlinkSync(packageDir(name, copy.dir), target, 'dir')
        }
        fs.writeFileSync(path.join(moduleDir, set.file), source)
        process.stdout.write(execFileSync(process.execPath, [fileURLToPath(import.meta.url), '--measure', moduleDir, mode, path.join(trial, 'storage')], { encoding: 'utf8', timeout: 60000 }))
      }
    }
    assert.ok(seen.size > 0, 'at least one supported Hypercore tested')
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true })
  }
}
