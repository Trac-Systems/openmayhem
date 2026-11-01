const MIN_FAST_FORWARD = 16;

// trac-peer's pinned base dependency has no forceFastForward()/repair(). Its normal
// verified FF queue is append-driven, with a five-minute failure cooldown but no
// retry timer. Watch the underlying signed SYSTEM head: the batch/view signed
// lengths can stay at the applied prefix even when newer signed data is known.
export function createSubnetStallWatch({ base, recover, stallMs = 60000, checkIntervalMs = 15000,
  now = Date.now, log = console.log }) {
  if (typeof base._queueFastForward !== 'function') throw new Error('Subnet reader fast-forward hook unavailable');
  let mark = null, timer = null, stopped = false;
  const emit = (event, extra) => log(`[trac-peer] ${JSON.stringify({ chain: 'SUBNET', event, ...extra })}`);
  const check = () => {
    if (stopped || !base.opened || base.closing || base.closed || base.writable || base.isIndexer ||
        base.paused || base._interrupting || base.fastForwardEnabled === false) {
      mark = null;
      return false;
    }
    const core = base.core, applied = base.indexedLength, total = core?.length;
    const signed = core?.core?.state?.length;
    if (![applied, total, signed].every(Number.isSafeInteger) || applied < 0 || total < applied ||
        signed - total < MIN_FAST_FORWARD) {
      mark = null;
      return false;
    }
    const at = now(), id = core.id;
    if (!mark || mark.id !== id || mark.applied !== applied || mark.total !== total || at < mark.since) {
      mark = { id, applied, total, since: at };
      return false;
    }
    const elapsedMs = at - mark.since;
    if (elapsedMs < stallMs) return false;
    mark.since = at;
    emit('subnet_apply_stalled', { appliedLength: applied, systemLength: total, systemSignedLength: signed,
      viewSignedLength: base.view?.core?.signedLength ?? null, elapsedMs,
      systemKey: core.key?.toString('hex') ?? null, applying: !!base._applyState?.applying, draining: !!base._draining,
      fastForwarding: !!base.fastForwarding, fastForwardTo: base.fastForwardTo?.length ?? null,
      fastForwardFailedAt: base.fastForwardFailedAt });
    // A valid signed head does not guarantee that a connected replica has
    // the blocks FF/replay needs. Repair transport even while FF is in flight.
    try { recover?.({ code: 'SUBNET_APPLY_STALLED', elapsedMs }); }
    catch (error) { emit('subnet_recovery_failed', { error: error.message }); }
    // Keep the base's threshold, failure cooldown, verification and serialized
    // apply path. Never replace/cancel an in-flight or already prepared FF.
    if (!base.fastForwarding && !base.fastForwardTo) {
      try { base._queueFastForward(); }
      catch (error) { emit('subnet_recovery_failed', { error: error.message }); }
    }
    return true;
  };
  return {
    check,
    start() {
      if (timer !== null) return;
      stopped = false;
      check();
      timer = setInterval(check, checkIntervalMs);
      timer.unref?.();
    },
    stop() {
      stopped = true;
      if (timer !== null) clearInterval(timer);
      timer = null;
      mark = null;
    }
  };
}
