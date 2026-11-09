#!/usr/bin/env node
// OPENMAYHEM-STALLFIX: install-time Hypercore patch (identical copy in trac-msb and trac-peer, run from postinstall).
//
// Why: Hypercore block requests have no age limit, and a responder that has paused a peer drops that peer's block
// requests silently. A node whose needed request sits on such a peer never advances until the connection closes.
// The patch (one exact-text patch set per supported Hypercore version, see hypercore-<version>.mjs here):
//   - cancels an in-flight request after 30 s on the wire, re-queues it for another peer and gives the silent peer a
//     cool-down (30 s doubling to 4 min); one unref'd sweep per replicator, bounded work per sweep;
//   - backports upstream 8cf8ad9 (#896) fix 2 (a stale block-queue entry no longer consumes a peer update);
//   - answers NOT_AVAILABLE (max 16 per minute per peer, silent beyond) to a peer we paused, instead of silence.
// No wire-format change: patched and unpatched nodes interoperate.
//
// What this script does: finds every Hypercore copy this package loads (its own resolution plus the copies that
// autobase, corestore, trac-msb and trac-peer resolve from here, hoisted or nested), and for each copy:
//   - supported version, file hash = original  -> applies the patch set (each original text must occur exactly once),
//                                                 checks the result hash, writes atomically (tmp + rename);
//   - supported version, file hash = patched   -> "already patched" (idempotent; safe when both packages run it);
//   - supported version, any other content      -> exit 1 with a clear message (nothing is written);
//   - unsupported version                       -> loud warning, no change.
// Usage: node scripts/hypercore-stallfix/apply.mjs [--check] [--quiet]
//   --check  only report; exit 1 if a supported copy is not patched.
import fs from 'node:fs'
import path from 'node:path'
import crypto from 'node:crypto'
import { fileURLToPath, pathToFileURL } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const PATCH_SETS = ['hypercore-11.18.3.mjs', 'hypercore-11.8.3.mjs']
const FOLLOW = ['autobase', 'corestore', 'trac-msb', 'trac-peer']
const MAX_DEPTH = 3

export async function loadPatchSets() {
  const sets = new Map()
  for (const name of PATCH_SETS) {
    const mod = await import(pathToFileURL(path.join(here, name)).href)
    sets.set(mod.default.version, mod.default)
  }
  return sets
}

function sha256(text) {
  return crypto.createHash('sha256').update(text).digest('hex')
}

function findPackageDir(name, fromDir) {
  let dir = fromDir
  for (;;) {
    const candidate = path.join(dir, 'node_modules', name)
    if (fs.existsSync(path.join(candidate, 'package.json'))) return fs.realpathSync(candidate)
    const parent = path.dirname(dir)
    if (parent === dir) return null
    dir = parent
  }
}

// Every Hypercore copy loaded from packageDir: bounded walk over FOLLOW packages, deduplicated by real path.
export function findHypercoreCopies(packageDir) {
  const copies = new Map()
  const seen = new Set()
  let level = [fs.realpathSync(packageDir)]
  for (let depth = 0; depth <= MAX_DEPTH && level.length > 0; depth++) {
    const next = []
    for (const dir of level) {
      if (seen.has(dir)) continue
      seen.add(dir)
      const hc = findPackageDir('hypercore', dir)
      if (hc !== null && !copies.has(hc)) copies.set(hc, path.relative(packageDir, dir) || '.')
      for (const name of FOLLOW) {
        const d = findPackageDir(name, dir)
        if (d !== null && !seen.has(d)) next.push(d)
      }
    }
    level = next
  }
  return [...copies.entries()].map(([dir, from]) => ({ dir, from }))
}

export function applyPatchSet(text, set) {
  let out = text
  for (const [original, replacement] of set.edits) {
    const parts = out.split(original)
    if (parts.length !== 2) {
      throw new Error('expected original text exactly once, found ' + (parts.length - 1) + ': ' + JSON.stringify(original.slice(0, 80)))
    }
    out = parts[0] + replacement + parts[1]
  }
  return out
}

// Reverse of applyPatchSet (tests and forensics): restores the original text from a patched file.
export function unapplyPatchSet(text, set) {
  let out = text
  for (let i = set.edits.length - 1; i >= 0; i--) {
    const [original, replacement] = set.edits[i]
    const parts = out.split(replacement)
    if (parts.length !== 2) throw new Error('expected patched text exactly once, found ' + (parts.length - 1))
    out = parts[0] + original + parts[1]
  }
  return out
}

export function sha256Hex(text) {
  return sha256(text)
}

export function inspectCopy(copy, sets) {
  const pkg = JSON.parse(fs.readFileSync(path.join(copy.dir, 'package.json'), 'utf8'))
  const set = sets.get(pkg.version) || null
  const result = { dir: copy.dir, from: copy.from, version: pkg.version, supported: set !== null, state: null, sha256: null }
  if (set === null) {
    result.state = 'unsupported-version'
    return result
  }
  const file = path.join(copy.dir, set.file)
  const text = fs.readFileSync(file, 'utf8')
  // keep the inspected text: patching must not re-read the file, a parallel run may have replaced it meanwhile
  Object.defineProperty(result, 'text', { value: text, enumerable: false })
  result.sha256 = sha256(text)
  if (result.sha256 === set.patchedSha256) result.state = 'patched'
  else if (result.sha256 === set.originalSha256) result.state = 'original'
  else if (text.includes('OPENMAYHEM-STALLFIX')) result.state = 'other-stallfix-revision'
  else result.state = 'mismatch'
  return result
}

export async function run({ packageDir, check = false, quiet = false, log = console.log, warn = console.error } = {}) {
  const sets = await loadPatchSets()
  const copies = findHypercoreCopies(packageDir)
  const name = JSON.parse(fs.readFileSync(path.join(packageDir, 'package.json'), 'utf8')).name
  const tag = '[hypercore-stallfix ' + name + ']'
  let failed = false

  if (copies.length === 0) warn(tag + ' WARNING: no hypercore copy found from ' + packageDir + ' (nothing patched)')

  const results = []
  for (const copy of copies) {
    const r = inspectCopy(copy, sets)
    const where = r.dir + ' (via ' + r.from + ')'
    if (r.state === 'unsupported-version') {
      warn(tag + ' WARNING: hypercore ' + r.version + ' at ' + where + ' is not a supported version (' + [...sets.keys()].join(', ') + '); NOT patched')
    } else if (r.state === 'patched') {
      if (!quiet) log(tag + ' hypercore ' + r.version + ' at ' + where + ': already patched (sha256 ' + r.sha256 + ')')
    } else if (r.state === 'original' && check) {
      warn(tag + ' hypercore ' + r.version + ' at ' + where + ': NOT patched')
      failed = true
    } else if (r.state === 'original') {
      const set = sets.get(r.version)
      const file = path.join(r.dir, set.file)
      let patched
      try {
        patched = applyPatchSet(r.text, set)
      } catch (err) {
        warn(tag + ' ERROR: hypercore ' + r.version + ' at ' + where + ': ' + err.message + '; nothing written')
        failed = true
        results.push(r)
        continue
      }
      if (sha256(patched) !== set.patchedSha256) {
        warn(tag + ' ERROR: hypercore ' + r.version + ' at ' + where + ': patched result has an unexpected hash; nothing written')
        failed = true
        results.push(r)
        continue
      }
      // atomic: a parallel run (trac-msb and trac-peer postinstall) writes the same bytes, rename keeps one
      const tmp = file + '.stallfix-' + process.pid + '.tmp'
      fs.writeFileSync(tmp, patched)
      fs.renameSync(tmp, file)
      r.state = 'patched'
      r.sha256 = set.patchedSha256
      log(tag + ' hypercore ' + r.version + ' at ' + where + ': PATCHED (sha256 ' + r.sha256 + ')')
    } else {
      const set = sets.get(r.version)
      warn(tag + ' ERROR: hypercore ' + r.version + ' at ' + where + ' has unexpected ' + set.file + ' content (sha256 ' + r.sha256 +
        (r.state === 'other-stallfix-revision' ? ', patched by a different stallfix revision' : '') +
        '; expected original ' + set.originalSha256 + ' or patched ' + set.patchedSha256 + '). Refusing to patch.')
      failed = true
    }
    results.push(r)
  }
  return { ok: !failed, results }
}

if (process.argv[1] && fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2)
  const packageDir = path.resolve(here, '..', '..')
  run({ packageDir, check: args.includes('--check'), quiet: args.includes('--quiet') }).then(({ ok }) => {
    if (!ok) {
      console.error('[hypercore-stallfix] FAILED: see the messages above. The Hypercore replication stall fix is not in place.')
      process.exit(1)
    }
  }, (err) => {
    console.error('[hypercore-stallfix] FAILED:', err && err.stack ? err.stack : err)
    process.exit(1)
  })
}
