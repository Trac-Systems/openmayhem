import assert from 'node:assert/strict';
import {randomBytes} from 'node:crypto';
import {digest} from '../../scripts/proxy-admission-wire.mjs';
import {RECOVERY,RefundJournal,signed} from '../../scripts/proxy-admission-refund-common.mjs';

// All keys/custody come from isolated fixtures. This never reads configuration.
export async function recoveryFor(f,work,now,{previous=null,expires=now+59000}={}) {
  return signed(RECOVERY,{schema_version:1,purpose:'proxy_admission_refund_recovery',refund_id:work.refund_id,
    authorization_digest:work.authorization_digest,policy_hash:work.authorization.body.policy_hash,authorizer_pubkey:f.review.hex,
    revision:previous?previous.body.revision+1:1,previous_recovery_digest:previous?await digest(RECOVERY,previous.body):null,
    preparation_digest:work.preparation?await digest('refund-preparation',work.preparation):null,first_dispatch_at_ms:work.first_dispatch_at_ms,
    review_code:'refund_authorization_expired',review_evidence_hash:randomBytes(32).toString('hex'),approved_at_ms:now,expires_at_ms:expires},f.review.privateKey);
}

export async function checkRecoveredExecution(f) {
  const base=Date.now(),authDomain='mayhem/proxy/admission-refund-authorization/v1';let now=base-60000;
  f.options.now=()=>now;
  const b={...f.work.authorization.body,approved_at_ms:base-60001,expires_at_ms:base-1001};
  f.work.authorization=signed(authDomain,b,f.review.privateKey);f.work.authorization_digest=await digest(authDomain,b);
  const preparation=await f.adapter().prepare(f.work),journal=new RefundJournal(f.root,f.work.authorization_digest),retained=journal.get('request');
  now=base;await assert.rejects(f.adapter().prepare(f.work),e=>e.reason==='refund_authorization_expired');
  const first=await recoveryFor(f,f.work,base);
  for(const delta of [{authorization_digest:'00'.repeat(32)},{policy_hash:'00'.repeat(32)},{refund_id:'other'},
    {amount_base_units:'999'},{expires_at_ms:base},{revision:2},{approved_at_ms:base+1}]) {
    const recovery=signed(RECOVERY,{...first.body,...delta},f.review.privateKey);
    await assert.rejects(f.adapter().prepare({...f.work,recovery}));
  }
  await assert.rejects(f.adapter().prepare({...f.work,recovery:{...first,signature:'00'.repeat(64)}}));
  f.work.recovery=first;f.grant.first_dispatch_at_ms=base;
  assert.deepEqual(await f.adapter().prepare(f.work),preparation);
  const delivery=await f.adapter().execute(f.work,f.grant,new AbortController().signal);
  const resumed=f.resumed(preparation),second=await recoveryFor(f,resumed,base,{previous:first,expires:base+1000});
  resumed.recovery=second;now=base+2000;
  assert.deepEqual(await f.adapter().execute(resumed,{...f.grant,action:'reconcile'},new AbortController().signal),delivery);
  assert.deepEqual(journal.get('request'),retained,'renewal must retain original bytes, nonce and Stripe retry age');
  const bad=signed(RECOVERY,{...second.body,preparation_digest:'00'.repeat(32)},f.review.privateKey);
  await assert.rejects(f.adapter().prepare({...resumed,recovery:bad}));
  await assert.rejects(f.adapter().execute(resumed,{...f.grant,action:'reconcile',first_dispatch_at_ms:base+1},new AbortController().signal));
}
