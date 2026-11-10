// Exact-text patch set for hypercore 11.18.3 lib/replicator.js (OPENMAYHEM-STALLFIX v2).
// Each edit is [original, replacement]; the original must occur exactly once in the unpatched file.
// Keep this file free of backticks and dollar-brace sequences inside the edit texts (String.raw templates).

// Upstream c24f37a6 (#911): a close event that already fired cannot detach a later attachment.
const closedStreamGuard = [
  String.raw`  attachTo(protomux) {
    if (this.core.closed) return
`,
  String.raw`  attachTo(protomux) {
    if (this.core.closed) return
    if (protomux.stream.destroying) return
`
]

export default {
  version: '11.18.3',
  file: 'lib/replicator.js',
  originalSha256: '2e7a2631a0d8e805861339bbc75e6f6de75565813cf201256105408c53c8ac93',
  patchedSha256: '1ba2f169d20840d41333cc55e8fbb7da9bc5b4698d808739ad97acce0cbe0f1a',
  // Only this exact released v1 file is eligible for an in-place upgrade.
  previousPatchedSha256: '79944caa33cc875d563f7eba8b820ffbd9323984d30e2fd6c58030804ded2f6c',
  upgradeEdits: [
    ['// OPENMAYHEM-STALLFIX v1 (hypercore 11.18.3).', '// OPENMAYHEM-STALLFIX v2 (hypercore 11.18.3).'],
    ["const STALLFIX = 'openmayhem-stallfix-v1'", "const STALLFIX = 'openmayhem-stallfix-v2'"],
    closedStreamGuard
  ],
  edits: [
    // 1. Constants + marker.
    [
      String.raw`const MAX_BACKOFFS = MAX_INVALID_REQUESTS / 2
`,
      String.raw`const MAX_BACKOFFS = MAX_INVALID_REQUESTS / 2

// OPENMAYHEM-STALLFIX v2 (hypercore 11.18.3). Install-time patch shipped by trac-msb and trac-peer
// (scripts/hypercore-stallfix/). Fixes the waiting-side replication stall: an in-flight request that a peer
// never answers is cancelled after STALLFIX_REQUEST_MAX_AGE and asked elsewhere; that peer gets a cool-down;
// a peer we paused gets a capped NOT_AVAILABLE instead of silence; an INVALID_OPERATION verification error pauses
// a peer for an escalating time instead of until disconnect. Wire format unchanged.
const STALLFIX = 'openmayhem-stallfix-v2'
const STALLFIX_REQUEST_MAX_AGE = 30000 // ms an in-flight request may wait for its answer
const STALLFIX_SWEEP_INTERVAL = 5000 // one unref'd timer per replicator, only while requests are in flight
const STALLFIX_MAX_SCAN = 4096 // request slots looked at per sweep
const STALLFIX_MAX_EXPIRE = 64 // requests expired per sweep
const STALLFIX_COOLDOWN_MIN = 30000 // first cool-down of a peer that let a request expire (doubles, capped)
const STALLFIX_COOLDOWN_MAX = 240000
const STALLFIX_NOT_AVAILABLE_WINDOW = 60000
const STALLFIX_NOT_AVAILABLE_MAX = 16 // NOT_AVAILABLE replies per window to a peer we paused; silent beyond
const STALLFIX_OP_PAUSE_MIN = 60000 // first timed pause after an INVALID_OPERATION verification error (doubles)
const STALLFIX_OP_PAUSE_EPISODES = 5 // timed pauses without valid data in between; the next one is permanent
`
    ],
    // 2. Per-peer state.
    [
      String.raw`    this.lastExtensionRecv = ''

    replicator._ifAvailable++
    replicator._active++
  }
`,
      String.raw`    this.lastExtensionRecv = ''

    this._sfTimeouts = 0 // OPENMAYHEM-STALLFIX: cool-down level of this peer
    this._sfCoolUntil = 0
    this._sfNaWindow = 0
    this._sfNaCount = 0
    this._sfOpUntil = 0 // timed pause after INVALID_OPERATION (0 = none)
    this._sfOpLevel = 0

    replicator._ifAvailable++
    replicator._active++
  }
`
    ],
    // 3. Responder: reply NOT_AVAILABLE (capped) to a peer we paused instead of dropping the request silently.
    //    Also covers proof === null for an inactive peer (upstream 03f7156: that case threw a TypeError here).
    [
      String.raw`    if (!this.isActive() && proof.block !== null) {
      return
    }
`,
      String.raw`    if (!this.isActive() && (proof === null || proof.block !== null)) {
      // OPENMAYHEM-STALLFIX: upstream returns silently here and the remote waits forever. Never INVALID_REQUEST.
      this._sfNotAvailable(req.msg.id)
      return
    }
`
    ],
    // 3b. INVALID_OPERATION during verification: timed pause instead of paused until disconnect. On 11.18.3 this
    //     error is also raised for VALID replies whose upgrade starts at a length we no longer have (local commit
    //     or another upgrade moved our length while the request was in flight; upstream 11.40.0 #895 refits such
    //     proofs). Other verification errors (possible forks, bad data) keep the conflict check and the permanent pause.
    [
      String.raw`      if (err.code !== 'INVALID_OPERATION') {
        // might be a fork, verify
        this._checkIfConflict()
      }

      this.paused = true
      this.replicator._onnodata(this, req, 0)
`,
      String.raw`      if (err.code !== 'INVALID_OPERATION') {
        // might be a fork, verify
        this._checkIfConflict()
        this.paused = true
      } else {
        // OPENMAYHEM-STALLFIX: never a fork (no conflict check); pause for a bounded, escalating time
        this._sfInvalidOperationPause(Date.now())
      }

      this.replicator._onnodata(this, req, 0)
`
    ],
    // 3c. A timed pause makes the peer inactive like a pause, until it expires.
    [
      String.raw`  isActive() {
    if (this.paused || this.removed || this.core.header.frozen) return false
    return true
  }
`,
      String.raw`  isActive() {
    if (this.paused || this.removed || this.core.header.frozen) return false
    if (this._sfOpUntil !== 0 && this._sfOpUntil > Date.now()) return false // OPENMAYHEM-STALLFIX timed pause
    return true
  }
`
    ],
    // 4. Peer helpers: capped NOT_AVAILABLE and request expiry.
    [
      String.raw`  _cancelRequest(req) {
    if (req.priority === PRIORITY.CANCELLED) return
`,
      String.raw`  // OPENMAYHEM-STALLFIX: at most STALLFIX_NOT_AVAILABLE_MAX replies per window, silent beyond the cap.
  _sfNotAvailable(id) {
    if (this.removed === true) return
    const now = Date.now()
    if (now - this._sfNaWindow >= STALLFIX_NOT_AVAILABLE_WINDOW) {
      this._sfNaWindow = now
      this._sfNaCount = 0
    }
    if (this._sfNaCount >= STALLFIX_NOT_AVAILABLE_MAX) {
      this.replicator._sfStats.notAvailableSuppressed++
      return
    }
    this._sfNaCount++
    this.replicator._sfStats.notAvailableSent++
    this.wireNoData.send({ request: id, reason: NOT_AVAILABLE })
  }

  // OPENMAYHEM-STALLFIX: timed pause after INVALID_OPERATION. Replies already in flight do not extend it.
  // Episodes double (60 s .. 16 min); valid block data from the peer resets the level (_ondata); after
  // STALLFIX_OP_PAUSE_EPISODES episodes without valid data the peer is paused until disconnect, as upstream.
  _sfInvalidOperationPause(now) {
    if (this.paused === true || this._sfOpUntil > now) return
    if (this._sfOpLevel >= STALLFIX_OP_PAUSE_EPISODES) {
      this._sfOpUntil = 0
      this.paused = true
      this.replicator._sfStats.invalidOperationPermanent++
      return
    }
    this._sfOpLevel++
    this._sfOpUntil = now + this.replicator._sfOpPauseMin * 2 ** (this._sfOpLevel - 1)
    this.replicator._sfStats.invalidOperationPauses++
    this.replicator._sfArm()
  }

  // OPENMAYHEM-STALLFIX: give up on one request this peer did not answer in time. Bookkeeping as for a
  // NOT_AVAILABLE reply (_clearRequest re-queues a wanted block) plus a wire cancel. The id is only reused
  // after the next roundtrip (as in _cancelRequest), so a late reply finds no request and is dropped
  // in ondata/onnodata: it is not invalid data and does not pause the peer.
  _sfExpireRequest(req, now) {
    if (req.priority === PRIORITY.CANCELLED) return false
    req.priority = PRIORITY.CANCELLED

    this.inflight--
    this.replicator._requestDone(req.id, false)

    if (this.roundtripQueue === null) this.roundtripQueue = new RoundtripQueue()
    this.roundtripQueue.add(req.id)
    if (this.removed === false) {
      this.wireCancel.send({ request: req.id })
      incrementTx(this.stats.wireCancel, this.replicator.stats.wireCancel)
    }

    if (this._sfCoolUntil <= now) {
      if (this._sfTimeouts < 8) this._sfTimeouts++
      const cooldown = this.replicator._sfCooldownMin * 2 ** (this._sfTimeouts - 1)
      this._sfCoolUntil = now + Math.min(STALLFIX_COOLDOWN_MAX, cooldown)
    }

    if (this.replicator._manifestPeer === this && this.core.header.manifest === null) {
      this.replicator._manifestPeer = null
    }

    this.replicator._clearRequest(this, req)
    return true
  }

  _cancelRequest(req) {
    if (req.priority === PRIORITY.CANCELLED) return
`
    ],
    // 5. Arm the sweep when a request goes in flight.
    [
      String.raw`    this.inflight++
    this.replicator._inflight.add(req)
`,
      String.raw`    this.inflight++
    this.replicator._inflight.add(req)
    if (this.replicator._sfTimer === null) this.replicator._sfArm() // OPENMAYHEM-STALLFIX
`
    ],
    // 6. Marker on the Replicator class (runtime check: core.replicator.constructor.STALLFIX).
    [
      String.raw`  static Peer = Peer // hack to be able to access Peer from outside this module
`,
      String.raw`  static Peer = Peer // hack to be able to access Peer from outside this module
  static STALLFIX = STALLFIX // OPENMAYHEM-STALLFIX marker
`
    ],
    // 7. Replicator state.
    [
      String.raw`    this._notDownloadingLinger = notDownloadingLinger
    this._downloadingTimer = null
`,
      String.raw`    this._notDownloadingLinger = notDownloadingLinger
    this._downloadingTimer = null

    // OPENMAYHEM-STALLFIX state (the _sf* numbers are only changed by tests)
    this._sfTimer = null
    this._sfCursor = 0
    this._sfAllCooling = false
    this._sfMaxAge = STALLFIX_REQUEST_MAX_AGE
    this._sfInterval = STALLFIX_SWEEP_INTERVAL
    this._sfCooldownMin = STALLFIX_COOLDOWN_MIN
    this._sfOpPauseMin = STALLFIX_OP_PAUSE_MIN
    this._sfStats = {
      expired: 0,
      notAvailableSent: 0,
      notAvailableSuppressed: 0,
      invalidOperationPauses: 0,
      invalidOperationPermanent: 0
    }
`
    ],
    // 8. Valid block data from a peer ends its cool-down.
    [
      String.raw`  _ondata(peer, req, data) {
    req.elapsed = Date.now() - req.timestamp
    if (data.block !== null) {
`,
      String.raw`  _ondata(peer, req, data) {
    req.elapsed = Date.now() - req.timestamp
    if (data.block !== null) {
      peer._sfTimeouts = 0 // OPENMAYHEM-STALLFIX
      peer._sfCoolUntil = 0
      peer._sfOpLevel = 0
`
    ],
    // 9. The sweep (bounded work, one timer per replicator).
    [
      String.raw`  _maybeUpdate() {
    return this._upgrade !== null && this._upgrade.inflight.length === 0
  }
`,
      String.raw`  _maybeUpdate() {
    return this._upgrade !== null && this._upgrade.inflight.length === 0
  }

  // OPENMAYHEM-STALLFIX: one unref'd timer per replicator, armed by _send, re-armed only while requests are
  // in flight or a peer is cooling down. Looks at <= STALLFIX_MAX_SCAN slots, expires <= STALLFIX_MAX_EXPIRE.
  _sfArm() {
    if (this.destroyed || this._sfTimer !== null) return
    this._sfTimer = setTimeout(sfSweepLater, this._sfInterval, this)
    if (this._sfTimer.unref) this._sfTimer.unref()
  }

  _sfSweep() {
    this._sfTimer = null
    if (this.destroyed) return

    const now = Date.now()
    const reqs = this._inflight._requests
    const scan = Math.min(reqs.length, STALLFIX_MAX_SCAN)
    let live = 0
    let expired = 0

    for (let i = 0; i < scan && expired < STALLFIX_MAX_EXPIRE; i++) {
      if (this._sfCursor >= reqs.length) this._sfCursor = 0
      const req = reqs[this._sfCursor++]
      if (!req) continue
      live++
      if (now - req.timestamp < this._sfMaxAge) continue
      if (req.peer._sfExpireRequest(req, now) === true) expired++
    }

    let cooling = 0
    let usable = 0
    let opPaused = 0
    let cooled = false
    for (const peer of this.peers) {
      if (peer._sfCoolUntil !== 0 && peer._sfCoolUntil <= now) {
        peer._sfCoolUntil = 0
        cooled = true
      }
      if (peer._sfOpUntil !== 0) {
        if (peer._sfOpUntil <= now) {
          peer._sfOpUntil = 0
          cooled = true
        } else {
          opPaused++
        }
      }
      if (peer._sfCoolUntil !== 0) cooling++
      else if (peer.isActive()) usable++
    }
    // if no other peer is usable, a cooling peer may still be asked (no deadlock with a single peer)
    this._sfAllCooling = cooling > 0 && usable === 0

    if (expired > 0) this._sfStats.expired += expired
    if (expired > 0 || cooled) this.updateAll()

    if (live > 0 || cooling > 0 || opPaused > 0 || scan < reqs.length) this._sfArm()
  }

  _sfCoolingDown(peer) {
    return peer._sfCoolUntil !== 0 && this._sfAllCooling === false && peer._sfCoolUntil > Date.now()
  }
`
    ],
    // 10. Hotswap does not pick a cooling peer.
    [
      String.raw`    if (!peer.isActive() || peer.inflight >= maxHotswaps) return
`,
      String.raw`    if (!peer.isActive() || peer.inflight >= maxHotswaps || this._sfCoolingDown(peer)) return
`
    ],
    // 11. _updatePeer: skip a cooling peer + backport upstream 8cf8ad9 fix 2 (stale queue entry must not
    //     consume the peer's update).
    [
      String.raw`  _updatePeer(peer) {
    if (!peer.isActive() || peer.inflight >= peer.getMaxInflight() || !peer.remoteUploading) {
      return false
    }
`,
      String.raw`  _updatePeer(peer) {
    if (
      !peer.isActive() ||
      peer.inflight >= peer.getMaxInflight() ||
      !peer.remoteUploading ||
      this._sfCoolingDown(peer)
    ) {
      return false
    }
`
    ],
    [
      String.raw`    for (const b of blks) {
      if (b.queued === false || peer._requestBlock(b) === true) {
        b.queued = false
        blks.dequeue()
        return true
      }
    }
`,
      String.raw`    for (const b of blks) {
      if (b.queued === false) {
        // upstream 8cf8ad9 (#896) fix 2, backported by OPENMAYHEM-STALLFIX
        blks.dequeue()
        continue
      }
      if (peer._requestBlock(b) === true) {
        b.queued = false
        blks.dequeue()
        return true
      }
    }
`
    ],
    // 12. _updatePeerNonPrimary: skip a cooling peer.
    [
      String.raw`  _updatePeerNonPrimary(peer) {
    if (!peer.isActive() || peer.inflight >= peer.getMaxInflight() || !peer.remoteUploading) {
      return false
    }
`,
      String.raw`  _updatePeerNonPrimary(peer) {
    if (
      !peer.isActive() ||
      peer.inflight >= peer.getMaxInflight() ||
      !peer.remoteUploading ||
      this._sfCoolingDown(peer)
    ) {
      return false
    }
`
    ],
    // 13. destroy() clears the sweep timer.
    [
      String.raw`    if (this._downloadingTimer) {
      clearTimeout(this._downloadingTimer)
      this._downloadingTimer = null
    }

    while (this.peers.length) {
`,
      String.raw`    if (this._downloadingTimer) {
      clearTimeout(this._downloadingTimer)
      this._downloadingTimer = null
    }

    if (this._sfTimer !== null) {
      clearTimeout(this._sfTimer) // OPENMAYHEM-STALLFIX
      this._sfTimer = null
    }

    while (this.peers.length) {
`
    ],
    // 14. Timer trampoline.
    [
      String.raw`function setDownloadingLater(repl, downloading, session) {
`,
      String.raw`function sfSweepLater(repl) {
  repl._sfSweep() // OPENMAYHEM-STALLFIX
}

function setDownloadingLater(repl, downloading, session) {
`
    ],
    // Upstream #911, in addition to the existing stall fixes.
    closedStreamGuard
  ]
}
