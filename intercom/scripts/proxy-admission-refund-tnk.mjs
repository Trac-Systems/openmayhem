// Exact signed TNK returns, separate from retail credits and epoch payouts.
import { createPublicKey } from 'node:crypto';
import { bigIntToDecimalString } from 'trac-msb/src/utils/amountSerialization.js';
import { prepareSettlementTransferPayload, validatePreparedSettlementTransferPayload } from '../src/msb-settlement-transfer-helper.js';
import { validateAdmissionMsbSnapshot } from '../features/mayhem/proxy-admission-msb.js';
import { verifyTnkObservedTransfer } from './proxy-admission-tnk.mjs';
import { digest, need, shape, uint, validateNetwork } from './proxy-admission-wire.mjs';
import { RefundJournal, validateRefundWork, signed, same, PREPARE, DELIVERY } from './proxy-admission-refund-common.mjs';
import { RetryWork, ReviewWork } from './retail-crypto-verification.mjs';

export class TnkAdmissionRefund {
  constructor({ msb, network, networkName, canonicalFrontier, policy, key, journalRoot, finality, timeoutSeconds, now=Date.now }) {
    validateNetwork(network);
    need(['mainnet','testnet1'].includes(networkName)&&msb?.wallet?.address&&typeof canonicalFrontier==='function'
      &&uint(finality,1)&&uint(timeoutSeconds,1)&&timeoutSeconds<=10&&key.asymmetricKeyType==='ed25519','explicit TNK return custody required');
    need(String(msb.config?.networkId)===network.network_id&&Buffer.from(msb.config?.bootstrap??[]).toString('hex')===network.msb_bootstrap
      &&msb.config.addressPrefix===(networkName==='mainnet'?'trac':'testtrac'),'TNK custody network differs');
    this.pubkey=createPublicKey(key).export({format:'der',type:'spki'}).subarray(-32).toString('hex');
    this.o={msb,network,networkName,canonicalFrontier,policy,key,journalRoot,finality,timeoutSeconds,now};
    this.pending=null;
  }
  // Aborting observation never starts another unresolved MSB operation. A slow
  // underlying read/send keeps this one permit until it actually terminates.
  async bounded(operation,signal) {
    signal.throwIfAborted();
    if(this.pending) throw new RetryWork('refund_transport_busy',1);
    const work=Promise.resolve().then(()=>{signal.throwIfAborted();return operation();});
    this.pending=work;
    void work.then(()=>{if(this.pending===work)this.pending=null;},()=>{if(this.pending===work)this.pending=null;});
    let abort;
    try {
      return await Promise.race([work,new Promise((_,reject)=>{
        abort=()=>reject(signal.reason);signal.addEventListener('abort',abort,{once:true});if(signal.aborted)abort();
      })]);
    } finally {signal.removeEventListener('abort',abort);}
  }
  async checked(work) {
    const o=this.o;
    await validateRefundWork(work,o.policy,this.pubkey,'tnk',o.now());
    need(Object.keys(o.network).every(k=>o.network[k]===work.invoice.network[k])
      &&work.authorization.body.destination.network===o.networkName&&work.payment_evidence.receipt.to_address===o.msb.wallet.address,
    'TNK return does not belong to configured custody');
    return new RefundJournal(o.journalRoot,work.authorization_digest);
  }
  args(work) {
    return {msb:this.o.msb,config:this.o.msb.config,network:this.o.networkName,to:work.authorization.body.destination.address,
      amount:bigIntToDecimalString(BigInt(work.authorization.body.amount_base_units))};
  }
  async prepare(work,signal=AbortSignal.timeout(15000)) {
    const journal=await this.checked(work),args=this.args(work);let stored=journal.get('request');
    if(!stored) {
      if(work.action!=='prepare') throw new ReviewWork('refund_original_transaction_missing');
      signal.throwIfAborted();
      if(this.o.now()>=work.lease_expires_at_ms) throw new RetryWork('refund_lease_expired',1);
      // Existing helper signs the actual MSB transfer. Zero wait here: the
      // durable queue owns retries; this adapter never sleeps inside a lease.
      let p;
      try {p=await this.bounded(()=>prepareSettlementTransferPayload({...args,timeoutSeconds:0,stderr:{write:()=>{}}}),signal);}
      catch(error) {
        const code=new Map([
          ['insufficient balance for settlement transfer and fee','refund_treasury_short'],
          ['sender account did not sync before timeout','refund_reader_unavailable'],
          ['no validator connection before timeout','refund_transport_unavailable'],
        ]).get(error.message);
        if(code)throw new RetryWork(code,30);throw error;
      }
      signal.throwIfAborted();
      stored=journal.retain('request',{schema_version:1,purpose:'proxy_admission_tnk_refund_request',authorization_digest:work.authorization_digest,
        from:args.msb.wallet.address,network:this.o.networkName,transaction_hash:p.tx_hash,payload:p.payload});
    }
    shape(stored,['schema_version','purpose','authorization_digest','from','network','transaction_hash','payload']);
    need(stored.schema_version===1&&stored.purpose==='proxy_admission_tnk_refund_request'&&stored.authorization_digest===work.authorization_digest
      &&stored.from===args.msb.wallet.address&&stored.network===this.o.networkName,'retained TNK refund differs');
    await validatePreparedSettlementTransferPayload({...args,from:stored.from,payload:stored.payload,txHash:stored.transaction_hash});
    signal.throwIfAborted();
    const proof=signed(PREPARE,{schema_version:1,purpose:'proxy_admission_refund_preparation',refund_id:work.refund_id,
      authorization_digest:work.authorization_digest,executor_pubkey:this.pubkey,reference:{rail:'tnk',transaction_hash:stored.transaction_hash}},this.o.key);
    if(work.preparation!==null)need(same(proof,work.preparation),'retained TNK preparation differs');
    return proof;
  }
  async observed(work,stored,signal) {
    const o=this.o;
    const snapshot=await this.bounded(()=>o.canonicalFrontier(signal),signal);
    validateAdmissionMsbSnapshot(snapshot,o.network,o.now());
    const receipt=await this.bounded(()=>verifyTnkObservedTransfer(o.msb,{transaction_hash:stored.transaction_hash,destination:work.authorization.body.destination.address},
      {frontier:snapshot.signed_length,canonicalProof:snapshot,finality:o.finality,timeoutSeconds:o.timeoutSeconds,signal,addressPrefix:o.msb.config.addressPrefix}),signal);
    need(receipt.fromAddress===stored.from&&String(receipt.tokenAmountBaseUnits)===work.authorization.body.amount_base_units,'confirmed TNK refund differs');
    return {snapshot,receipt};
  }
  async execute(work,grant,signal) {
    const o=this.o,preparation=await this.prepare(work,signal),journal=await this.checked(work),stored=journal.get('request'),b=work.authorization.body;
    shape(grant,['schema_version','purpose','refund_id','action','first_dispatch_at_ms']);
    need(grant.schema_version===1&&grant.purpose==='proxy_admission_refund'&&grant.refund_id===work.refund_id
      &&['dispatch','reconcile'].includes(grant.action)&&uint(grant.first_dispatch_at_ms,b.approved_at_ms)&&grant.first_dispatch_at_ms<b.expires_at_ms,
    'invalid TNK refund dispatch grant');
    const retained=journal.get('refund');
    let found;
    try {found=await this.observed(work,stored,signal);}
    catch(error) {
      // Only exact-key absence in a fresh canonical prefix permits a broadcast
      // of the SAME retained payload. Pending finality/forks/unavailable readers
      // must never be interpreted as absence or create a substitute transfer.
      if(!(error instanceof RetryWork)||error.code!=='transfer_pending'||retained)throw error;
      signal.throwIfAborted();
      if(o.now()>=work.lease_expires_at_ms)throw new RetryWork('refund_lease_expired',1);
      const accepted=await this.bounded(()=>o.msb.broadcastPartialTransaction(stored.payload),signal);
      if(accepted!==true)throw new RetryWork('refund_broadcast_unconfirmed',30);
      found=await this.observed(work,stored,signal);
    }
    signal.throwIfAborted();
    if(retained) {
      shape(retained,['snapshot','delivery']);
      const old=retained.snapshot,current=found.snapshot;
      validateAdmissionMsbSnapshot(old,o.network,old.observed_at_ms);
      need(old.view_key===current.view_key&&old.fork===current.fork&&old.signed_length<=current.signed_length,'TNK refund canonical prefix changed');
      const base=o.msb.state.base.view,core=base.core;
      const hash=await this.bounded(()=>core.treeHash(old.signed_length),signal);
      need(o.msb.state.base.view===base&&base.core===core&&core.fork===current.fork&&Buffer.from(core.key).toString('hex')===current.view_key
        &&o.msb.state.getSignedLength()>=current.signed_length&&Buffer.from(hash).toString('hex')===old.tree_hash,'TNK refund retained prefix differs');
      const expected=await this.delivery(work,preparation,stored,old.signed_length);
      need(same(retained.delivery,expected),'retained TNK delivery differs');
      return expected; // Byte-identical completion after ACK loss and chain growth.
    }
    const value=await this.delivery(work,preparation,stored,found.snapshot.signed_length);
    journal.retain('refund',{snapshot:found.snapshot,delivery:value});return value;
  }
  async delivery(work,preparation,stored,length) {
    return signed(DELIVERY,{schema_version:1,purpose:'proxy_admission_refund_delivery',refund_id:work.refund_id,
      authorization_digest:work.authorization_digest,preparation_digest:await digest('refund-preparation',preparation),executor_pubkey:this.pubkey,
      receipt:{rail:'tnk',network:this.o.networkName,transaction_hash:stored.transaction_hash,to_address:work.authorization.body.destination.address,
        from_address:stored.from,amount_base_units:work.authorization.body.amount_base_units,confirmed_signed_length:length,finalized:true}},this.o.key);
  }
}
