// Original-asset TAP returns with exact signed bytes and durable nonce claims.
import {createPublicKey} from 'node:crypto';
import {tapTransferData,tapBalanceData,inspectTapRefundTransaction} from './proxy-admission-refund-tap-transaction.mjs';
import {RefundJournal,validateRefundWork,signed,same,PREPARE,DELIVERY} from './proxy-admission-refund-common.mjs';
import {digest,need,shape,uint,amount} from './proxy-admission-wire.mjs';
import {RetryWork,ReviewWork,verifyTapTransferReceipt,parseHexInt,ERC20_TRANSFER_TOPIC,addressTopic} from './retail-crypto-verification.mjs';
const eth=v=>typeof v==='string'&&/^0x[0-9a-f]{40}$/.test(v)&&v!==`0x${'0'.repeat(40)}`;
const hash=v=>typeof v==='string'&&/^0x[0-9a-f]{64}$/.test(v);
const quantity=n=>`0x${BigInt(n).toString(16)}`;
export class TapAdmissionRefund {
 constructor({chainId,token,receiver,signer,rpc,policy,key,journalRoot,maxGas,maxFeePerGas,priorityFee,maxFee,now=Date.now}) {
  need(uint(chainId,1)&&eth(token)&&eth(receiver)&&signer?.address?.toLowerCase()===receiver&&typeof signer.signTransaction==='function'
   &&typeof rpc==='function'&&key.asymmetricKeyType==='ed25519'&&[maxGas,maxFeePerGas,priorityFee,maxFee].every(v=>amount(v)&&BigInt(v)>0n)
   &&BigInt(priorityFee)<=BigInt(maxFeePerGas),'explicit bounded TAP return custody required');
  this.pubkey=createPublicKey(key).export({format:'der',type:'spki'}).subarray(-32).toString('hex');
  this.o={chainId,token,receiver,signer,rpc,policy,key,journalRoot,maxGas,maxFeePerGas,priorityFee,maxFee,now};this.active=false;
 }
 async checked(work) {
  const o=this.o;await validateRefundWork(work,o.policy,this.pubkey,'tap',o.now());
  const d=work.authorization.body.destination;
  need(d.chain_id===o.chainId&&d.token_contract===o.token&&work.payment_evidence.receipt.to_address===o.receiver,'TAP return does not belong to configured custody');
  return new RefundJournal(o.journalRoot,work.authorization_digest);
 }
 expected(work) {const o=this.o;return {chainId:o.chainId,token:o.token,from:o.receiver,to:work.authorization.body.destination.address,
  amount:work.authorization.body.amount_base_units,maxGas:o.maxGas,maxFeePerGas:o.maxFeePerGas,maxFee:o.maxFee};}
 async claim(nonce) {
  return new RefundJournal(this.o.journalRoot,await digest('mayhem/proxy/admission-refund-tap-nonce/v1',{chain_id:this.o.chainId,from:this.o.receiver,nonce}));
 }
 async rpc(method,params,signal){signal.throwIfAborted();const r=await this.o.rpc(method,params,signal);signal.throwIfAborted();return r;}
 async nonce(tag,signal){const n=parseHexInt(await this.rpc('eth_getTransactionCount',[this.o.receiver,tag],signal),'TAP nonce');need(n<=BigInt(Number.MAX_SAFE_INTEGER),'TAP nonce exceeds safe representation');return Number(n);}
 async balance(work,fee,signal) {
  const raw=await this.rpc('eth_call',[{to:this.o.token,data:tapBalanceData(this.o.receiver)},'latest'],signal);
  need(hash(raw),'invalid TAP balance response');
  if(BigInt(raw)<BigInt(work.authorization.body.amount_base_units))throw new RetryWork('refund_treasury_short',30);
  if(parseHexInt(await this.rpc('eth_getBalance',[this.o.receiver,'pending'],signal),'ETH balance')<fee)throw new RetryWork('refund_gas_funding_short',30);
 }
 async prepare(work,signal=AbortSignal.timeout(15000)) {
  const o=this.o,journal=await this.checked(work);let stored=journal.get('request');
  if(!stored){
   if(work.action!=='prepare')throw new ReviewWork('refund_original_transaction_missing');
   const nonce=await this.nonce('latest',signal),pending=await this.nonce('pending',signal);
   if(pending!==nonce)throw new RetryWork('refund_wallet_busy',15);
   const nonceJournal=await this.claim(nonce),owner=nonceJournal.get('request');
   if(owner&&owner.authorization_digest!==work.authorization_digest)throw new RetryWork('refund_nonce_reserved',30);
   if(owner)throw new ReviewWork('refund_original_transaction_missing');
   const b=await this.rpc('eth_getBlockByNumber',['latest',false],signal);
   need(b&&hash(b.hash),'TAP fee block unavailable');
   const base=parseHexInt(b.baseFeePerGas,'base fee'),priority=BigInt(o.priorityFee),ceiling=BigInt(o.maxFeePerGas);
   if(base+priority>ceiling)throw new RetryWork('refund_gas_price_exceeds_policy',30);
   const fee=base*2n+priority>ceiling?ceiling:base*2n+priority;
   const data=tapTransferData(work.authorization.body.destination.address,work.authorization.body.amount_base_units);
   const estimated=parseHexInt(await this.rpc('eth_estimateGas',[{from:o.receiver,to:o.token,data,value:'0x0'}],signal),'gas estimate');
   need(estimated>0n,'invalid zero gas estimate');const gas=(estimated*120n+99n)/100n;
   if(gas>BigInt(o.maxGas)||gas*fee>BigInt(o.maxFee))throw new ReviewWork('refund_fee_budget_exceeded');
   await this.balance(work,gas*fee,signal);
   const raw=await o.signer.signTransaction({type:2,chainId:o.chainId,nonce,to:o.token,value:0n,data,gasLimit:gas,maxFeePerGas:fee,maxPriorityFeePerGas:priority,accessList:[]});
   signal.throwIfAborted();
   if(o.now()>=work.lease_expires_at_ms)throw new RetryWork('refund_lease_expired',1);
   const tx=inspectTapRefundTransaction(raw,this.expected(work));
   stored=journal.retain('request',{schema_version:1,purpose:'proxy_admission_tap_refund_request',authorization_digest:work.authorization_digest,transaction_hash:tx.hash,raw_transaction:raw});
  }
  shape(stored,['schema_version','purpose','authorization_digest','transaction_hash','raw_transaction']);
  need(stored.schema_version===1&&stored.purpose==='proxy_admission_tap_refund_request'&&stored.authorization_digest===work.authorization_digest,'retained TAP refund differs');
  const tx=inspectTapRefundTransaction(stored.raw_transaction,this.expected(work));need(tx.hash===stored.transaction_hash,'retained TAP transaction hash differs');
  const claim=await this.claim(tx.nonce),value={schema_version:1,purpose:'proxy_admission_tap_refund_nonce',authorization_digest:work.authorization_digest,
   chain_id:o.chainId,from:o.receiver,nonce:tx.nonce,transaction_hash:tx.hash};
  const owner=claim.get('request');
  if(owner&&!same(owner,value))throw new ReviewWork('refund_nonce_claim_conflict');
  claim.retain('request',value);
  const proof=signed(PREPARE,{schema_version:1,purpose:'proxy_admission_refund_preparation',refund_id:work.refund_id,
   authorization_digest:work.authorization_digest,executor_pubkey:this.pubkey,reference:{rail:'tap',transaction_hash:tx.hash}},o.key);
  if(work.preparation!==null)need(same(work.preparation,proof),'retained TAP preparation differs');return proof;
 }
 async confirmed(work,receipt,tx,signal) {
  const o=this.o;
  need(receipt&&receipt.transactionHash===tx.hash&&receipt.from?.toLowerCase()===o.receiver&&receipt.to?.toLowerCase()===o.token
   &&hash(receipt.blockHash)&&Array.isArray(receipt.logs)&&receipt.logs.length<=512,'TAP refund receipt bindings differ');
  const blockNumber=parseHexInt(receipt.blockNumber,'receipt block'),frontier=await this.rpc('eth_getBlockByNumber',['finalized',false],signal);
  need(frontier&&hash(frontier.hash),'TAP finality unavailable');const finalized=parseHexInt(frontier.number,'finalized block');
  if(blockNumber>finalized)throw new RetryWork('refund_awaiting_finality',20);
  const canonical=await this.rpc('eth_getBlockByNumber',[quantity(blockNumber),false],signal);
  need(canonical?.number===receipt.blockNumber&&canonical.hash===receipt.blockHash,'TAP refund canonical block differs');
  if(parseHexInt(receipt.status,'receipt status')===0n)throw new ReviewWork('refund_transfer_reverted');
  const matches=receipt.logs.filter(l=>l.address?.toLowerCase()===o.token&&l.topics?.[0]===ERC20_TRANSFER_TOPIC);
  need(matches.length===1,'TAP refund transfer logs ambiguous');const log=matches[0];
  need(log.removed!==true&&log.transactionHash===tx.hash&&log.blockHash===receipt.blockHash&&log.blockNumber===receipt.blockNumber
   &&log.topics.length===3&&log.topics[1]===addressTopic(o.receiver)&&log.topics[2]===addressTopic(work.authorization.body.destination.address)&&hash(log.data),
  'TAP refund log differs');
  const verified=verifyTapTransferReceipt(receipt,{transactionHash:tx.hash,token:o.token,destination:work.authorization.body.destination.address,
   amountBaseUnits:work.authorization.body.amount_base_units,latestBlock:finalized,finalizedBlock:finalized});
  // A second exact receipt read fences provider/reorg changes during verification.
  const fresh=await this.rpc('eth_getTransactionReceipt',[tx.hash],signal);
  need(same(receipt,fresh),'TAP refund receipt changed during confirmation');
  const finalBlock=await this.rpc('eth_getBlockByNumber',[quantity(blockNumber),false],signal);
  need(finalBlock?.number===receipt.blockNumber&&finalBlock.hash===receipt.blockHash,'TAP refund canonical block changed during confirmation');
  return {rail:'tap',chain_id:o.chainId,token_contract:o.token,transaction_hash:tx.hash,log_index:verified.logIndex,from_address:o.receiver,
   to_address:verified.toAddress,amount_base_units:String(verified.tokenAmountBaseUnits),block_number:String(blockNumber),block_hash:verified.blockHash,finalized:true};
 }
 async execute(work,grant,signal) {
  need(!this.active,'TAP refund execution already active');this.active=true;
  try {
   const o=this.o,p=await this.prepare(work,signal),journal=await this.checked(work),stored=journal.get('request'),b=work.authorization.body;
   shape(grant,['schema_version','purpose','refund_id','action','first_dispatch_at_ms']);
   need(grant.schema_version===1&&grant.purpose==='proxy_admission_refund'&&grant.refund_id===work.refund_id&&['dispatch','reconcile'].includes(grant.action)
    &&uint(grant.first_dispatch_at_ms,b.approved_at_ms)&&grant.first_dispatch_at_ms<b.expires_at_ms
    &&(work.first_dispatch_at_ms===null||work.first_dispatch_at_ms===grant.first_dispatch_at_ms),'invalid TAP refund dispatch grant');
   const tx=inspectTapRefundTransaction(stored.raw_transaction,this.expected(work)),retained=journal.get('refund');
   let receipt=await this.rpc('eth_getTransactionReceipt',[tx.hash],signal);
   if(!receipt){
    if(retained)throw new ReviewWork('refund_canonical_delivery_missing');
    const pendingTx=await this.rpc('eth_getTransactionByHash',[tx.hash],signal);
    if(pendingTx){
     need(pendingTx.hash===tx.hash&&pendingTx.from?.toLowerCase()===o.receiver&&pendingTx.to?.toLowerCase()===o.token
      &&pendingTx.input===tx.data&&parseHexInt(pendingTx.nonce,'transaction nonce')===BigInt(tx.nonce)&&parseHexInt(pendingTx.value,'transaction value')===0n,
     'TAP pending transaction differs');throw new RetryWork('refund_transfer_pending',15);
    }
    const latest=await this.nonce('latest',signal),pending=await this.nonce('pending',signal);
    if(latest>tx.nonce)throw new ReviewWork('refund_nonce_consumed_elsewhere');
    if(latest!==tx.nonce||pending!==tx.nonce)throw new RetryWork('refund_wallet_busy',15);
    const block=await this.rpc('eth_getBlockByNumber',['latest',false],signal);
    if(parseHexInt(block?.baseFeePerGas,'base fee')>tx.maxFee)throw new RetryWork('refund_gas_price_exceeds_retained_fee',30);
    await this.balance(work,tx.gas*tx.maxFee,signal);
    signal.throwIfAborted();if(o.now()>=work.lease_expires_at_ms)throw new RetryWork('refund_lease_expired',1);
    const sent=await this.rpc('eth_sendRawTransaction',[stored.raw_transaction],signal);need(sent===tx.hash,'TAP broadcast hash differs');
    receipt=await this.rpc('eth_getTransactionReceipt',[tx.hash],signal);if(!receipt)throw new RetryWork('refund_transfer_pending',15);
   }
   const r=await this.confirmed(work,receipt,tx,signal);
   const delivery=signed(DELIVERY,{schema_version:1,purpose:'proxy_admission_refund_delivery',refund_id:work.refund_id,
    authorization_digest:work.authorization_digest,preparation_digest:await digest('refund-preparation',p),executor_pubkey:this.pubkey,receipt:r},o.key);
   if(retained)need(same(retained,{delivery}),'TAP refund confirmed delivery changed');else journal.retain('refund',{delivery});
   return delivery;
  } finally {this.active=false;}
 }
}
