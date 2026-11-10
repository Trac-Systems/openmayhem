// Tests for scripts/hypercore-stallfix/apply.mjs (Node only: uses fs and child processes).
// The pristine Hypercore sources are rebuilt from the installed (patched) copies by reversing the patch set,
// and checked against the recorded original hashes.
import test from 'brittle'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { loadPatchSets, unapplyPatchSet, applyPatchSet, sha256Hex, findHypercoreCopies, run } from '../../../scripts/hypercore-stallfix/apply.mjs'

const here = path.dirname(fileURLToPath(import.meta.url))
const pkgRoot = path.resolve(here, '..', '..', '..')
const scriptsDir = path.join(pkgRoot, 'scripts', 'hypercore-stallfix')
const sets = await loadPatchSets()

// Pristine replicator.js per supported version that is installed here (patched or not).
const pristine = new Map()
for (const copy of findHypercoreCopies(pkgRoot)) {
  const version = JSON.parse(fs.readFileSync(path.join(copy.dir, 'package.json'), 'utf8')).version
  const set = sets.get(version)
  if (!set || pristine.has(version)) continue
  const text = fs.readFileSync(path.join(copy.dir, set.file), 'utf8')
  const sha = sha256Hex(text)
  if (sha === set.originalSha256) pristine.set(version, text)
  else if (sha === set.patchedSha256) pristine.set(version, unapplyPatchSet(text, set))
}
const versions = [...pristine.keys()]

function tmpdir() {
  return fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()), 'hc-stallfix-'))
}

function writePkg(dir, json) {
  fs.mkdirSync(dir, { recursive: true })
  fs.writeFileSync(path.join(dir, 'package.json'), JSON.stringify(json))
}

function hypercoreAt(dir, version, text) {
  writePkg(dir, { name: 'hypercore', version })
  fs.mkdirSync(path.join(dir, 'lib'), { recursive: true })
  fs.writeFileSync(path.join(dir, 'lib', 'replicator.js'), text)
}

function packageWithScript(dir, name) {
  writePkg(dir, { name, version: '0.0.0' })
  fs.cpSync(scriptsDir, path.join(dir, 'scripts', 'hypercore-stallfix'), { recursive: true })
}

function runScript(dir, args = []) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [path.join(dir, 'scripts', 'hypercore-stallfix', 'apply.mjs'), ...args], { stdio: ['ignore', 'pipe', 'pipe'] })
    let out = ''
    child.stdout.on('data', (d) => { out += d })
    child.stderr.on('data', (d) => { out += d })
    child.on('close', (code) => resolve({ code, out }))
  })
}

const replicatorSha = (dir) => sha256Hex(fs.readFileSync(path.join(dir, 'lib', 'replicator.js'), 'utf8'))

function releasedV1(version) {
  const set = sets.get(version)
  return unapplyPatchSet(applyPatchSet(pristine.get(version), set), { edits: set.upgradeEdits })
}

test('apply: pristine sources rebuilt from the installed copies match the recorded hashes', (t) => {
  t.ok(versions.length >= 1, 'at least one supported hypercore installed: ' + versions.join(', '))
  for (const v of versions) {
    t.is(sha256Hex(pristine.get(v)), sets.get(v).originalSha256, v + ' original hash')
    t.is(sha256Hex(applyPatchSet(pristine.get(v), sets.get(v))), sets.get(v).patchedSha256, v + ' patched hash')
  }
})

test('apply: hoisted + nested layout, both packages, idempotent, concurrent', async (t) => {
  const root = tmpdir()
  t.teardown(() => fs.rmSync(root, { recursive: true, force: true }))
  writePkg(root, { name: 'app', version: '1.0.0' })
  const msb = path.join(root, 'node_modules', 'trac-msb')
  const peer = path.join(root, 'node_modules', 'trac-peer')
  packageWithScript(msb, 'trac-msb')
  packageWithScript(peer, 'trac-peer')
  const v18 = versions.includes('11.18.3') ? '11.18.3' : versions[0]
  const hoisted = path.join(root, 'node_modules', 'hypercore')
  hypercoreAt(hoisted, v18, pristine.get(v18))
  const nested = path.join(peer, 'node_modules', 'hypercore')
  const v8 = versions.includes('11.8.3') ? '11.8.3' : null
  if (v8) hypercoreAt(nested, v8, pristine.get(v8))
  // autobase nested under trac-msb resolves the hoisted copy
  writePkg(path.join(msb, 'node_modules', 'autobase'), { name: 'autobase', version: '7.20.1' })

  const copiesFromPeer = findHypercoreCopies(peer).map((c) => c.dir).sort()
  t.alike(copiesFromPeer, (v8 ? [nested, hoisted] : [hoisted]).map((d) => fs.realpathSync(d)).sort(), 'trac-peer finds its own copy and trac-msb\'s copy')

  // both postinstalls at the same time (npm may run them in parallel), several rounds on fresh copies
  for (let round = 0; round < 8; round++) {
    hypercoreAt(hoisted, v18, round % 2 ? releasedV1(v18) : pristine.get(v18))
    if (v8) hypercoreAt(nested, v8, round % 2 ? releasedV1(v8) : pristine.get(v8))
    const [a, b] = await Promise.all([runScript(msb), runScript(peer)])
    t.is(a.code, 0, 'trac-msb script ok, round ' + round)
    t.is(b.code, 0, 'trac-peer script ok, round ' + round)
  }
  t.is(replicatorSha(hoisted), sets.get(v18).patchedSha256, 'hoisted copy patched')
  if (v8) t.is(replicatorSha(nested), sets.get(v8).patchedSha256, 'nested copy patched')
  t.absent(fs.readdirSync(path.join(hoisted, 'lib')).some((f) => f.includes('.stallfix-')), 'no temp files left')

  const again = await runScript(peer)
  t.is(again.code, 0, 'second run ok')
  t.ok(/already patched/.test(again.out), 'second run reports already patched')
  const check = await runScript(msb, ['--check'])
  t.is(check.code, 0, '--check passes on patched copies')
})

test('apply: mismatching content on a supported version exits non-zero and writes nothing', async (t) => {
  const root = tmpdir()
  t.teardown(() => fs.rmSync(root, { recursive: true, force: true }))
  const v = versions[0]
  const msb = path.join(root, 'node_modules', 'trac-msb')
  packageWithScript(msb, 'trac-msb')
  const hc = path.join(root, 'node_modules', 'hypercore')
  const changed = pristine.get(v).replace('const MAX_RANGES = 64', 'const MAX_RANGES = 65')
  hypercoreAt(hc, v, changed)
  const before = replicatorSha(hc)
  const r = await runScript(msb)
  t.is(r.code, 1, 'exit 1')
  t.ok(/Refusing to patch/.test(r.out), 'clear message')
  t.is(replicatorSha(hc), before, 'file unchanged')

  // a copy patched by another stallfix revision is refused too
  hypercoreAt(hc, v, applyPatchSet(pristine.get(v), sets.get(v)).replace("'openmayhem-stallfix-v2'", "'openmayhem-stallfix-v0'"))
  const r2 = await runScript(msb)
  t.is(r2.code, 1, 'exit 1 for another revision')
  t.ok(/different stallfix revision/.test(r2.out), 'names the other revision')
})

test('apply: unsupported version warns loudly and changes nothing; --check fails on unpatched copies', async (t) => {
  const root = tmpdir()
  t.teardown(() => fs.rmSync(root, { recursive: true, force: true }))
  const msb = path.join(root, 'node_modules', 'trac-msb')
  packageWithScript(msb, 'trac-msb')
  const hc = path.join(root, 'node_modules', 'hypercore')
  hypercoreAt(hc, '11.35.1', 'module.exports = class Replicator {}\n')
  const before = replicatorSha(hc)
  const r = await runScript(msb)
  t.is(r.code, 0, 'exit 0')
  t.ok(/WARNING: hypercore 11\.35\.1 .* NOT patched/.test(r.out), 'loud warning')
  t.is(replicatorSha(hc), before, 'file unchanged')

  hypercoreAt(hc, versions[0], pristine.get(versions[0]))
  const c = await runScript(msb, ['--check'])
  t.is(c.code, 1, '--check fails while unpatched')
  t.is(replicatorSha(hc), sets.get(versions[0]).originalSha256, '--check writes nothing')
})

test('apply: run() API reports every copy', async (t) => {
  const res = await run({ packageDir: pkgRoot, check: true, log: () => {}, warn: () => {} })
  t.ok(res.ok, 'this package\'s own copies are patched (postinstall ran)')
  t.ok(res.results.length >= 1)
})

test('apply: released v1 upgrades to v2; check is read-only and altered v1 is refused', async (t) => {
  const root = tmpdir()
  t.teardown(() => fs.rmSync(root, { recursive: true, force: true }))
  const msb = path.join(root, 'node_modules', 'trac-msb')
  const hc = path.join(root, 'node_modules', 'hypercore')
  packageWithScript(msb, 'trac-msb')
  for (const v of versions) {
    const set = sets.get(v)
    const previous = releasedV1(v)
    t.is(sha256Hex(previous), set.previousPatchedSha256, v + ' exact released v1 bytes')
    hypercoreAt(hc, v, previous)
    const check = await runScript(msb, ['--check'])
    t.is(check.code, 1, v + ' v1 is not current')
    t.is(replicatorSha(hc), set.previousPatchedSha256, v + ' check leaves v1 unchanged')
    const upgrade = await runScript(msb)
    t.is(upgrade.code, 0, v + ' upgrade succeeds')
    t.ok(upgrade.out.includes('UPGRADED v1 -> v2'), v + ' visible upgrade')
    t.is(replicatorSha(hc), set.patchedSha256, v + ' upgrade matches pristine install')
    t.is((await runScript(msb)).code, 0, v + ' repeat succeeds')
    t.is((await runScript(msb, ['--check'])).code, 0, v + ' current check passes')
    const changed = previous + '\n// unexpected modification\n'
    hypercoreAt(hc, v, changed)
    t.is((await runScript(msb)).code, 1, v + ' modified v1 refused')
    t.is(replicatorSha(hc), sha256Hex(changed), v + ' refused file unchanged')
  }
})

test('apply: exact-text entries and result hashes fail closed', async (t) => {
  for (const v of versions) {
    const set = sets.get(v)
    const guard = set.edits.at(-1)
    t.exception(() => applyPatchSet('', { edits: [guard] }), /exactly once, found 0/, v + ' missing match')
    t.exception(() => applyPatchSet(guard[0] + guard[0], { edits: [guard] }), /exactly once, found 2/, v + ' duplicate match')
  }
  const root = tmpdir()
  t.teardown(() => fs.rmSync(root, { recursive: true, force: true }))
  const msb = path.join(root, 'node_modules', 'trac-msb')
  const hc = path.join(root, 'node_modules', 'hypercore')
  packageWithScript(msb, 'trac-msb')
  const v = versions[0]
  const patchFile = path.join(msb, 'scripts', 'hypercore-stallfix', 'hypercore-' + v + '.mjs')
  fs.writeFileSync(patchFile, fs.readFileSync(patchFile, 'utf8').replace(sets.get(v).patchedSha256, '0'.repeat(64)))
  for (const input of [pristine.get(v), releasedV1(v)]) {
    hypercoreAt(hc, v, input)
    const result = await runScript(msb)
    t.is(result.code, 1, 'unexpected output hash refused')
    t.ok(result.out.includes('patched result has an unexpected hash'))
    t.is(replicatorSha(hc), sha256Hex(input), 'no write on result mismatch')
  }
})
