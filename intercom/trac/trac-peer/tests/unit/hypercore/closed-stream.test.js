// Upstream c24f37a6 (#911), ported to the storage API in Hypercore 11.18.3 and 11.8.3.
// Identical in trac-msb and trac-peer; no network discovery or remote peers are used.
import test from 'brittle'
import Hypercore from 'hypercore'
import b4a from 'b4a'

const closed = (stream) => new Promise((resolve, reject) => {
  stream.once('error', reject)
  stream.once('close', resolve)
})

test('stream that closes while the core opens is not attached', async (t) => {
  const core = new Hypercore(await t.tmp())
  t.teardown(() => core.close())
  const stream = core.replicate(true, { keepAlive: false })
  const noise = stream.noiseStream
  const closing = closed(noise)
  noise.destroy()
  await closing
  await core.ready()
  t.absent(core.core.replicator.attached(noise.userData), 'closed stream is not attached')
  t.is(core.core.replicator._attached.size, 0, 'no retained mux')
})

test('200 connections closing during open leave no destroyed streams attached', async (t) => {
  // The public preload option holds opening until all close events have fired. Only the
  // replicator retains old muxes after each round; do not keep a test-owned stream array.
  let release
  const preload = new Promise((resolve) => { release = resolve })
  const core = new Hypercore(await t.tmp(), { preload })
  t.teardown(() => core.close())
  try {
    for (let round = 0; round < 200; round++) {
      const stream = core.replicate(true, { keepAlive: false })
      const noise = stream.noiseStream
      const closing = closed(noise)
      noise.destroy()
      await closing
    }
  } finally {
    release({})
  }
  await core.ready()
  const attached = core.core.replicator._attached
  const destroyed = [...attached].filter((mux) => mux.stream.destroyed).length
  t.comment('rounds=200 attached=' + attached.size + ' destroyed=' + destroyed)
  t.is(attached.size, 0, 'no retained closed connections')
  t.is(destroyed, 0, 'no destroyed streams reachable through _attached')

  // The same core must still serve data over a healthy connection after the churn.
  await core.append('after closed-stream churn')
  const clone = new Hypercore(await t.tmp(), core.key)
  t.teardown(() => clone.close())
  await clone.ready()
  const a = core.replicate(true, { keepAlive: false })
  const b = clone.replicate(false, { keepAlive: false })
  a.pipe(b).pipe(a)
  t.teardown(() => { a.destroy(); b.destroy() }, { order: 1 })
  t.alike(await clone.get(0, { timeout: 5000 }), b4a.from('after closed-stream churn'), 'normal replication still works')
  const aClosed = closed(a.noiseStream)
  const bClosed = closed(b.noiseStream)
  a.destroy()
  b.destroy()
  await Promise.all([aClosed, bClosed])
  t.is(core.core.replicator._attached.size, 0, 'normal close detaches writer')
  t.is(clone.core.replicator._attached.size, 0, 'normal close detaches clone')
})
