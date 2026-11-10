// OPENMAYHEM-STALLFIX reproducers for the install-time Hypercore patch (scripts/hypercore-stallfix/).
// Runs against the hypercore copy this package resolves (trac-msb: 11.18.3, trac-peer: 11.8.3).
// Same file in trac-msb and trac-peer. Uses only hypercore + brittle so it also runs under Bare.
import test from 'brittle'
import Hypercore from 'hypercore'
import b4a from 'b4a'

const BLOCKS = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j']

async function create(t, key) {
  const core = new Hypercore(await t.tmp(), key)
  await core.ready()
  t.teardown(() => core.close().catch(() => {}), { order: 1 })
  return core
}

function replicate(t, a, b) {
  const s1 = a.replicate(true, { keepAlive: false })
  const s2 = b.replicate(false, { keepAlive: false })
  s1.on('error', () => {})
  s2.on('error', () => {})
  s1.pipe(s2).pipe(s1)
  t.teardown(() => { s1.destroy(); s2.destroy() }, { order: 0 })
  return { s1, s2 }
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
// On an unpatched copy the counters do not exist: fall back to zeros so the asserts fail instead of crashing.
const sfStats = (core) => core.replicator._sfStats || { expired: 0, notAvailableSent: 0, notAvailableSuppressed: 0 }

async function until(fn, ms = 10000, step = 20) {
  const end = Date.now() + ms
  while (Date.now() < end) {
    if (fn()) return true
    await sleep(step)
  }
  return false
}

// The Peer object on `core` that talks over replication stream `stream` (returned by core.replicate()).
function peerOf(core, stream) {
  const noise = stream.noiseStream || stream
  return core.replicator.peers.find((p) => p.stream === noise) || null
}

// Make `peer` (on the responder) behave like an unpatched responder that paused us: block requests are dropped
// silently; everything else (upgrades, hashes) is still answered.
function dropBlockRequests(peer) {
  const orig = peer._fulfillRequest.bind(peer)
  peer._fulfillRequest = async function (req) {
    if (req.msg.block) {
      peer.remoteRequests.delete(req.msg.id)
      return
    }
    return orig(req)
  }
}

function countInvalid(replicator) {
  const box = { invalid: 0 }
  for (const name of ['_oninvaliddata', '_oninvalid']) {
    if (typeof replicator[name] !== 'function') continue
    const orig = replicator[name].bind(replicator)
    replicator[name] = (...args) => { box.invalid++; return orig(...args) }
  }
  return box
}

function fast(core, maxAge = 3000) {
  const r = core.replicator
  r._sfMaxAge = maxAge
  r._sfInterval = 500
  r._sfCooldownMin = 30000
}

async function writerWithClones(t, n) {
  const w = await create(t)
  await w.append(BLOCKS)
  const clones = []
  for (let i = 0; i < n; i++) {
    const c = await create(t, w.key)
    replicate(t, w, c)
    for (let j = 0; j < BLOCKS.length; j++) await c.get(j)
    clones.push(c)
  }
  return { w, clones }
}

test('stallfix: the loaded hypercore carries the stallfix marker', async (t) => {
  const core = await create(t)
  t.is(core.replicator.constructor.STALLFIX, 'openmayhem-stallfix-v2')
})

async function silentRespondersCase(t, maxAge) {
  const { w, clones } = await writerWithClones(t, 3)
  const r = await create(t, w.key)
  if (maxAge !== null) fast(r, maxAge)
  const invalid = countInvalid(r.replicator)

  const silent = []
  for (const s of clones) {
    const { s1, s2 } = replicate(t, s, r)
    t.ok(await until(() => peerOf(s, s1) !== null && peerOf(r, s2) !== null), 'connected')
    dropBlockRequests(peerOf(s, s1))
    silent.push(peerOf(r, s2))
  }
  await r.update({ wait: true })
  t.is(r.length, BLOCKS.length, 'upgrade answered by the silent peers')

  const started = Date.now()
  let value = null
  const get = r.get(7).then((v) => { value = v })

  // Put a copy of the request on every silent peer (hotswap allows at most 3), so the good peer cannot hotswap it.
  t.ok(await until(() => {
    for (const p of r.replicator.peers) r.replicator.updatePeer(p)
    const b = r.replicator._blocks.get(7)
    return b !== null && b.inflight.length === 3
  }), 'request in flight on all 3 silent peers')

  const { s2: good } = replicate(t, w, r)
  t.ok(await until(() => peerOf(r, good) !== null), 'good peer connected')

  return { r, get, started, invalid, silent, value: () => value }
}

test('stallfix: request held by silent peers is re-routed after the age limit (fast T = 3 s)', async (t) => {
  const c = await silentRespondersCase(t, 3000)
  await Promise.race([c.get, sleep(15000)])
  const took = Date.now() - c.started
  t.alike(c.value(), b4a.from('h'), 'block 7 delivered by the good peer')
  t.ok(took >= 2500 && took < 9000, 'completed after the age limit (' + took + ' ms)')
  t.ok(sfStats(c.r).expired >= 3, 'expired the 3 stuck copies')
  for (const p of c.silent) {
    t.is(p.paused, false, 'silent peer not paused')
    t.ok((p._sfCoolUntil || 0) > Date.now(), 'silent peer in cool-down')
    t.is(p.inflight, 0, 'silent peer inflight counter back to 0')
  }
  t.is(c.invalid.invalid, 0, 'no invalid data')
  t.is(c.r.replicator._blocks.get(7), null, 'block request cleaned up')
})

test('stallfix: default age limit (T = 30 s) re-routes within ~T', { timeout: 90000 }, async (t) => {
  const c = await silentRespondersCase(t, null)
  t.is(c.r.replicator._sfMaxAge, 30000)
  await Promise.race([c.get, sleep(50000)])
  const took = Date.now() - c.started
  t.alike(c.value(), b4a.from('h'), 'block 7 delivered by the good peer')
  t.ok(took >= 29000 && took < 40000, 'completed within ~T (' + took + ' ms)')
})

test('stallfix: late reply after the cancel is ignored safely (single peer, no deadlock)', async (t) => {
  const { clones } = await writerWithClones(t, 1)
  const s = clones[0]
  const r = await create(t, s.key)
  fast(r, 2000)
  const invalid = countInvalid(r.replicator)
  const { s1, s2 } = replicate(t, s, r)
  t.ok(await until(() => peerOf(s, s1) !== null && peerOf(r, s2) !== null))
  await r.update({ wait: true })

  // Responder: hold the FIRST block request; when its cancel arrives, send the real reply first (it "crossed" the
  // cancel on the wire), then process the cancel. Later requests are answered normally.
  const sp = peerOf(s, s1)
  const orig = sp._fulfillRequest.bind(sp)
  const origCancel = sp.oncancel.bind(sp)
  let held = null
  let lateSent = 0
  sp._fulfillRequest = async function (req) {
    if (req.msg.block && held === null) {
      held = req
      return
    }
    return orig(req)
  }
  sp.oncancel = function (msg) {
    if (held !== null && msg.request === held.msg.id && lateSent === 0) {
      lateSent++
      held.fulfill().then((proof) => {
        sp.wireData.send({ request: held.msg.id, fork: held.msg.fork, block: proof.block, hash: proof.hash, seek: proof.seek, upgrade: proof.upgrade, manifest: proof.manifest })
        origCancel(msg)
      })
      return
    }
    return origCancel(msg)
  }

  const rp = peerOf(r, s2)
  const v = await Promise.race([r.get(3), sleep(15000).then(() => null)])
  t.alike(v, b4a.from('d'), 'got the block from the same (only) peer after expiry')
  t.ok(await until(() => lateSent === 1), 'late reply was sent')
  await sleep(300)
  t.is(rp.paused, false, 'peer not paused by the late reply')
  t.is(invalid.invalid, 0, 'late reply not counted as invalid data')
  t.ok(sfStats(r).expired >= 1, 'first request expired')
  t.is(rp.inflight, 0, 'inflight counter back to 0')
  t.alike(await Promise.race([r.get(4), sleep(15000).then(() => null)]), b4a.from('e'), 'replication still works afterwards')
})

test('stallfix: NOT_AVAILABLE to a peer we paused is capped (no request storm)', async (t) => {
  const { clones } = await writerWithClones(t, 1)
  const p = clones[0]
  const r = await create(t, p.key)
  const { s1, s2 } = replicate(t, p, r)
  t.ok(await until(() => peerOf(p, s1) !== null && peerOf(r, s2) !== null))
  await r.update({ wait: true })

  const pp = peerOf(p, s1)
  pp.paused = true // p paused r (as after invalid data from r)
  const rx0 = pp.stats.wireRequest.rx

  const gets = []
  for (let i = 0; i < 4; i++) gets.push(r.get(i, { timeout: 4000 }).catch((err) => err.code))
  const res = await Promise.all(gets)
  const sent = sfStats(p).notAvailableSent
  const suppressed = sfStats(p).notAvailableSuppressed
  const requests = pp.stats.wireRequest.rx - rx0
  t.comment('requests from r: ' + requests + ', NOT_AVAILABLE sent: ' + sent + ', suppressed: ' + suppressed)
  t.ok(sent >= 1, 'answered with NOT_AVAILABLE')
  t.ok(sent <= 16, 'at most 16 NOT_AVAILABLE per minute')
  t.ok(requests <= sent + suppressed + 4, 'requests bounded by the replies (no storm)')
  t.ok(res.every((x) => x === 'REQUEST_TIMEOUT'), 'gets time out (p will not serve r)')
  const rp = peerOf(r, s2)
  t.is(rp.paused, false, 'requester did not pause p (never INVALID_REQUEST)')
  if (rp.stats.backoffs !== undefined) t.is(rp.stats.backoffs, 0, 'no INVALID_REQUEST back-offs')
})

test('stallfix: a timed out get does not swallow the next block request (upstream #896)', async (t) => {
  for (let i = 0; i < 6; i++) {
    const a = await create(t)
    await a.append(BLOCKS)
    const b = await create(t, a.key)
    replicate(t, a, b)
    await b.update({ wait: true })
    await b.get(12, { timeout: 300 }).catch(() => {}) // leaves a stale entry in the block queue
    const block = await b.get(7, { timeout: 3000 }).catch((err) => err.code)
    t.alike(block, b4a.from('h'), 'round ' + i)
  }
})

test('stallfix: sweep timer stops when idle and on close', async (t) => {
  const { w } = await writerWithClones(t, 0)
  const r = await create(t, w.key)
  fast(r, 3000)
  replicate(t, w, r)
  await r.update({ wait: true })
  await r.get(1)
  t.ok(await until(() => r.replicator._sfTimer === null, 3000), 'no timer once nothing is in flight')
  r.get(2).catch(() => {})
  await r.close()
  t.is(r.core.replicator._sfTimer, null, 'no timer after close')
})

// Delays the replies of `core` to upgrade requests (all peers of that core), restored on teardown.
function delayUpgradeReplies(t, core, ms) {
  const proto = core.replicator.constructor.Peer.prototype
  const orig = proto._fulfillRequest
  const box = { delayed: 0 }
  proto._fulfillRequest = async function (req) {
    if (this.core === core.core && req.msg.upgrade) {
      box.delayed++
      await sleep(ms)
    }
    return orig.call(this, req)
  }
  t.teardown(() => { proto._fulfillRequest = orig }, { order: -1 })
  return box
}

test('stallfix: a valid upgrade reply that went stale (local length moved) does not pause the responder for good', async (t) => {
  // The race seen live on MSB followers: Autobase commits a signed view length locally while an upgrade request is
  // in flight; the valid reply was built for the old start and fails with INVALID_OPERATION "Expected node N, got
  // node M". Here the local length moves through an append of the same blocks with the writer's key pair.
  const w = await create(t)
  await w.append(BLOCKS)
  const p = await create(t, w.key)
  replicate(t, w, p)
  for (let i = 0; i < BLOCKS.length; i++) await p.get(i)
  const r = new Hypercore(await t.tmp(), { keyPair: w.keyPair })
  await r.ready()
  t.teardown(() => r.close().catch(() => {}), { order: 1 })
  await r.append(BLOCKS.slice(0, 5))
  const timed = r.replicator._sfOpPauseMin !== undefined
  if (timed) { r.replicator._sfOpPauseMin = 300; r.replicator._sfInterval = 100 }
  const errors = []
  r.on('verification-error', (err) => errors.push(err.code + ' ' + err.message))
  delayUpgradeReplies(t, p, 400)
  const { s2 } = replicate(t, p, r)
  r.update({ wait: true }).catch(() => {})
  t.ok(await until(() => r.replicator._upgrade !== null && r.replicator._upgrade.inflight.length > 0, 5000, 5), 'upgrade request in flight')
  t.is(r.replicator._upgrade.inflight[0].upgrade.start, 5)
  await r.append(BLOCKS.slice(5, 7)) // local length 5 -> 7, identical data
  t.ok(await until(() => errors.length > 0, 3000), 'stale reply fails verification')
  t.ok(/INVALID_OPERATION.*Expected node \d+, got node \d+/.test(errors[0]), errors[0])
  const rp = peerOf(r, s2)
  t.is(rp.paused, false, 'not paused until disconnect')
  if (timed) {
    t.ok(rp._sfOpUntil > Date.now() && rp.isActive() === false, 'timed pause')
    t.ok(await until(() => rp.isActive() === true, 3000), 'active again after the timed pause')
  } else {
    t.is(rp.isActive(), true, 'this Hypercore version never pauses on INVALID_OPERATION')
  }
  await w.append(['k', 'l'])
  t.ok(await until(() => p.length === 12, 5000), 'responder follows the writer')
  t.ok(await until(() => r.length === 12, 10000), 'next upgrade comes from the same (only) peer')
  t.is(rp.paused, false)
})

test('stallfix: a peer that keeps sending INVALID_OPERATION replies is bounded and ends paused', async (t) => {
  const w = await create(t)
  const many = []
  for (let i = 0; i < 64; i++) many.push('block-' + i)
  await w.append(many)
  const bad = await create(t, w.key)
  replicate(t, w, bad)
  await bad.download({ start: 0, end: 64 }).done()
  const r = await create(t, w.key)
  if (r.replicator._sfOpPauseMin === undefined) {
    t.pass('this Hypercore version never pauses on INVALID_OPERATION (unchanged upstream behaviour)')
    return
  }
  r.replicator._sfOpPauseMin = 100
  r.replicator._sfInterval = 50
  const errors = { op: 0, other: 0 }
  r.on('verification-error', (err) => { if (err.code === 'INVALID_OPERATION') errors.op++; else errors.other++ })
  const { s1, s2 } = replicate(t, bad, r)
  t.ok(await until(() => peerOf(bad, s1) !== null && peerOf(r, s2) !== null))
  await r.update({ wait: true })
  // the bad peer corrupts every block proof (wrong sibling index) -> INVALID_OPERATION on our side
  const bp = peerOf(bad, s1)
  const orig = bp._fulfillRequest.bind(bp)
  bp._fulfillRequest = async (req) => {
    if (!req.msg.block) return orig(req)
    const fulfill = req.fulfill.bind(req)
    req.fulfill = async () => {
      const proof = await fulfill()
      if (proof && proof.block && proof.block.nodes.length > 0) proof.block.nodes[0] = { ...proof.block.nodes[0], index: proof.block.nodes[0].index + 2 }
      return proof
    }
    return orig(req)
  }
  const gets = []
  for (let i = 0; i < 64; i++) gets.push(r.get(i).catch(() => null))
  const rp = peerOf(r, s2)
  t.ok(await until(() => rp.paused === true, 15000), 'paused until disconnect after the last episode')
  const st = r.replicator._sfStats
  t.is(st.invalidOperationPauses, 5, '5 timed episodes')
  t.is(st.invalidOperationPermanent, 1, 'then permanent')
  const maxInflight = rp.getMaxInflight()
  t.comment('invalid replies verified: ' + errors.op + ' (max inflight ' + maxInflight + ')')
  t.ok(errors.op >= 6 && errors.op <= 6 * maxInflight, 'bounded: at most one window of in-flight replies per episode')
  t.is(errors.other, 0)
  const before = errors.op
  await sleep(500)
  t.is(errors.op, before, 'nothing more once paused')
  // a good peer still serves everything
  replicate(t, w, r)
  const values = await Promise.race([Promise.all(gets), sleep(15000).then(() => null)])
  t.ok(values !== null && values.every((v, i) => v !== null && b4a.toString(v) === 'block-' + i), 'all blocks from the good peer')
})
