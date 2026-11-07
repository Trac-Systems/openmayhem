const rrp = require('resolve-reject-promise')
const safetyCatch = require('safety-catch')

const SystemView = require('./system.js')
const { EncryptionView } = require('./encryption.js')

const DEFAULT_OP_TIMEOUT = 5_000
const DEFAULT_MIN_FF = 16

module.exports = class FastForward {
  constructor (base, key, { timeout = DEFAULT_OP_TIMEOUT, verified = true, minimum = DEFAULT_MIN_FF, force = false } = {}) {
    this.base = base
    this.key = key
    this.timeout = timeout
    this.force = force
    this.core = base.store.get({ key, active: true, encryption: null })
    this.length = 0
    this.views = []
    this.encryption = null
    this.entropy = null
    this.indexers = []
    this.minimum = minimum
    this.verified = verified
    this.waiting = new Set()
    this.cores = [this.core]
    this.system = null
    this.destroyed = false
    this.upgrading = null
    this.failed = false
    this.failureRecorded = false
  }

  static MINIMUM = DEFAULT_MIN_FF

  async upgrade () {
    if (!this.upgrading) this.upgrading = this._upgrade()
    try {
      if (!await this.upgrading) return null
      return {
        length: this.length,
        force: this.force,
        key: this.key,
        indexers: this.indexers,
        views: this.views,
        entropy: this.entropy,
        manifestVersion: this.core.manifest.version,
        minimum: this.minimum
      }
    } catch (err) {
      this._recordReadFailure(err, null, null, 'upgrade')
      safetyCatch(err)
      this.failed = true
      return null
    } finally {
      await this.close()
    }
  }

  _recordReadFailure (err, core, index, stage) {
    if (this.failureRecorded || this.destroyed || this.base.closing || this.base.closed) return
    this.failureRecorded = true
    // One bounded metadata record, no blocks, transaction content or error text.
    // Diagnostics must never alter the success/error path of a verified read.
    try {
      const peers = []
      for (const peer of core?.peers || []) {
        if (peers.length === 4) break
        if (!peer.remotePublicKey) continue
        peers.push({ remoteKey: peer.remotePublicKey.toString('hex'),
          dataRx: peer.stats?.wireData?.rx ?? null,
          requestTx: peer.stats?.wireRequest?.tx ?? null, inflight: peer.inflight ?? null,
          remoteLength: peer.remoteLength ?? null, uploading: peer.remoteUploading ?? null,
          hasBlock: Number.isSafeInteger(index) ? peer.remoteBitfield?.get(index) ?? null : null })
      }
      this.base.fastForwardLastFailure = {
        at: Date.now(), stage,
        code: typeof err?.code === 'string' && /^[A-Z_]{1,48}$/.test(err.code) ? err.code : 'READ_FAILED',
        coreKey: core?.key?.toString('hex') ?? null,
        index: Number.isSafeInteger(index) ? index : null,
        peers
      }
    } catch {}
  }

  async _readBlock (core, index, stage) {
    try { return await core.get(index, { timeout: this.timeout }) } catch (err) {
      this._recordReadFailure(err, core, index, stage)
      throw err
    }
  }

  async _readSystem (key, stage) {
    let index = null
    try {
      return await this.system.get(key, { timeout: this.timeout, onwait: seq => { index = seq } })
    } catch (err) {
      this._recordReadFailure(err, this.system.core, index, stage)
      throw err
    }
  }

  _waitForAppend (core) {
    if (core.length > 0) return Promise.resolve()

    const promise = rrp()
    const timeout = setTimeout(this.close.bind(this), this.timeout)
    const wait = { promise, timeout }

    this.waiting.add(wait)

    core.once('append', () => {
      this.waiting.delete(wait)
      clearTimeout(timeout)
      promise.resolve()
    })

    return promise.promise
  }

  _minLength () {
    return this.base.core.length + this.minimum
  }

  async _upgrade () {
    await this.core.ready()

    if (!this.verified) await this._waitForAppend(this.core)
    if (this.destroyed) return false

    if (this.core.manifest.linked) {
      const appends = []

      for (const key of this.core.manifest.linked) {
        const core = this.base.store.get({ key, active: true })
        this.cores.push(core)

        await core.ready()
        appends.push(this._waitForAppend(core))
      }

      await Promise.all(appends)
    }

    this.length = this.core.length

    // note we use the persisted length here, as we might as well continue that work as thats ~ the same length
    if (!this.force && (this.length === 0 || this.length < this._minLength())) return false

    await this._readBlock(this.core, this.length - 1, 'system_tip')
    await this.base.ready()

    if (this.destroyed) return false

    if (this.base.encryptionKey && !(await this._ensureEncryption())) return false
    if (this.destroyed) return false

    this.system = new SystemView(this.core, { checkout: this.length })
    await this.system.ready()
    if (this.destroyed) return false

    this.entropy = this.system.entropy

    if (this.core.manifest.version >= 2) {
      const linked = this.core.manifest.linked

      if (this.system.encryptionLength === 0 || linked === null || !linked.length) {
        return false
      }

      // technically diverges from system.views to include encryption here, but makes sense
      this.views.push({
        key: linked[0],
        length: this.system.encryptionLength
      })
    }

    const promises = []

    // ensure local key is locally available always
    promises.push(this._readSystem(this.base.local.key, 'local_writer'))

    if (this.encryption && this.encryption.core) {
      promises.push(this._readBlock(this.encryption.core, this.system.encryptionLength - 1, 'encryption'))
    }

    for (const v of this.system.views) {
      this.views.push(v)
      const core = this.base.store.get({ key: v.key, active: true })
      this.cores.push(core)
      if (!v.length) continue // encryption view can be 0 length
      promises.push(this._readBlock(core, v.length - 1, 'view_tip'))
    }

    for (const idx of this.system.indexers) {
      this.indexers.push(idx)
      if (idx.length === 0) continue // need to make sure we have manifest...
      const core = this.base.store.get({ key: idx.key, active: true })
      this.cores.push(core)
      promises.push(this._readBlock(core, idx.length - 1, 'indexer_tip'))
      promises.push(this._readSystem(idx.key, 'indexer'))
    }

    for (const head of this.system.heads) {
      promises.push(this._readSystem(head.key, 'head'))
    }

    await Promise.all(promises)
    if (this.destroyed) return false

    return true
  }

  async _ensureEncryption () {
    // only need encryption if we migrated
    if (this.core.encryption !== null) return true

    const encCore = this.core.manifest.version <= 1 ? null : this.cores[1]

    // expects internal views to be loaded in order
    this.encryption = new EncryptionView(this.base, encCore)
    const encryption = this.encryption.getViewEncryption('_system')

    await this.core.setEncryption(encryption)

    return true
  }

  async close () {
    this.destroyed = true
    for (const { promise, timeout } of this.waiting) {
      clearTimeout(timeout)
      promise.resolve() // silently bail
    }
    if (this.system) await this.system.close()
    if (this.encryption) await this.encryption.close()
    for (const core of this.cores) await core.close()
    await this.core.close()
  }
}
