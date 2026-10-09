// Admission-only off-ledger worker contract. These records never create buyer
// credit/backing or modify native payment/payout semantics.
import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import { PROXY_ADMISSION_FEE_AU, proxyCanonicalSigningBytes, validateProxyAdmissionPermit } from '../contract/proxy-protocol.js';
import { normalizeTnkAddress } from './retail-crypto-verification.mjs';
export const PURPOSE = 'proxy_admission_fee';
export const MAX_WORK_BYTES = 16384;
export const INVOICE_DOMAIN = 'mayhem/proxy/admission-invoice/v1';
export const EVIDENCE_DOMAIN = 'mayhem/proxy/admission-evidence/v1';
export const need = (condition, code) => { if (!condition) throw new Error(`Admission worker: ${code}`); };
export const hex = v => typeof v === 'string' && /^[0-9a-f]{64}$/.test(v);
export const uint = (v, minimum = 0) => Number.isSafeInteger(v) && v >= minimum;
export const amount = v => typeof v === 'string' && /^(0|[1-9][0-9]{0,38})$/.test(v) && BigInt(v) <= (1n << 128n) - 1n;
const address = v => typeof v === 'string' && /^0x[0-9a-f]{40}$/.test(v);
const tx = v => typeof v === 'string' && /^0x[0-9a-f]{64}$/.test(v);
const opaque = v => typeof v === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(v);
export function shape(v, keys) {
  need(v && typeof v === 'object' && !Array.isArray(v) && Object.keys(v).sort().join('|') === [...keys].sort().join('|'), 'invalid fields');
}
export function validateNetwork(v) {
  shape(v, ['network_id', 'msb_bootstrap', 'subnet_bootstrap', 'contract_version']);
  need(typeof v.network_id === 'string' && /^[a-z0-9_-]{1,128}$/.test(v.network_id)
    && hex(v.msb_bootstrap) && hex(v.subnet_bootstrap) && uint(v.contract_version, 1) && v.contract_version <= 0xffffffff, 'invalid network');
}
export function validateInvoice(v) {
  shape(v, ['network', 'provider_pubkey', 'initial_operation_digest', 'entitlement_id', 'fee_policy_hash',
    'issuer_pubkey', 'invoice_commitment', 'rail', 'amount_base_units', 'accepted_value_au', 'created_at_ms', 'quote_expires_at_ms', 'collection']);
  validateNetwork(v.network);
  for (const key of ['provider_pubkey','initial_operation_digest','entitlement_id','fee_policy_hash','issuer_pubkey','invoice_commitment']) need(hex(v[key]), 'invalid invoice binding');
  need(['fiat','tap','tnk'].includes(v.rail) && amount(v.amount_base_units) && BigInt(v.amount_base_units) > 0n
    && v.accepted_value_au === PROXY_ADMISSION_FEE_AU && uint(v.created_at_ms, 1)
    && uint(v.quote_expires_at_ms, v.created_at_ms + 1), 'invalid fee quote');
  const c = v.collection;
  if (v.rail === 'tap') {
    shape(c, ['chain_id','token_contract','destination','allocation_id']);
    need(uint(c.chain_id, 1) && address(c.token_contract) && address(c.destination) && hex(c.allocation_id), 'invalid TAP collection');
  } else if (v.rail === 'tnk') {
    shape(c, ['network','destination','allocation_id']);
    need(['mainnet','testnet1'].includes(c.network) && hex(c.allocation_id), 'invalid TNK collection');
    need(normalizeTnkAddress(c.destination, c.network, 'destination') === c.destination, 'noncanonical TNK destination');
  } else {
    shape(c, ['stripe_account','livemode','currency']);
    need(/^acct_[A-Za-z0-9]{1,100}$/.test(c.stripe_account) && typeof c.livemode === 'boolean'
      && /^[a-z]{3}$/.test(c.currency), 'invalid FIAT collection');
  }
}
export function validateReference(v, invoice) {
  const c = invoice.collection;
  if (invoice.rail === 'tap') {
    shape(v, ['chain_id','token_contract','transaction_hash','log_index']);
    need(v.chain_id === c.chain_id && v.token_contract === c.token_contract && tx(v.transaction_hash) && uint(v.log_index), 'invalid TAP reference');
  } else if (invoice.rail === 'tnk') {
    shape(v, ['network','transaction_hash']);
    need(v.network === c.network && hex(v.transaction_hash), 'invalid TNK reference');
  } else {
    shape(v, ['stripe_account','livemode','payment_intent_id','verified_webhook_event_id']);
    need(v.stripe_account === c.stripe_account && v.livemode === c.livemode
      && /^pi_[A-Za-z0-9]{1,100}$/.test(v.payment_intent_id) && /^evt_[A-Za-z0-9]{1,100}$/.test(v.verified_webhook_event_id), 'invalid FIAT reference');
  }
}
export async function digest(domain, v) { return b4a.toString(await blake3(proxyCanonicalSigningBytes(domain, v)), 'hex'); }
export async function invoiceCommitment(invoiceId, invoice) {
  const { invoice_commitment, ...body } = invoice;
  return await digest(INVOICE_DOMAIN, { invoice_id: invoiceId, invoice: body });
}
export function base(work) {
  return { schema_version:1, purpose:PURPOSE, phase:work.phase, invoice_id:work.invoice_id,
    invoice_revision:work.invoice_revision, lease_token:work.lease_token };
}
export async function validateWork(v, phase) {
  need(Buffer.byteLength(JSON.stringify(v)) <= MAX_WORK_BYTES, 'work exceeds bound');
  shape(v, ['schema_version','purpose','phase','invoice_id','invoice_revision','lease_token','lease_expires_at_ms',
    'invoice','payment_reference','reference_assigned_at_ms','permit','evidence',...(v.previous_permit===undefined?[]:['previous_permit']),...(v.evidence?.format==='paged-v1'?['evidence_progress']:[])]);
  need(v.schema_version === 1 && v.purpose === PURPOSE && v.phase === phase && ['verify','issue'].includes(phase)
    && opaque(v.invoice_id) && uint(v.invoice_revision, 1) && hex(v.lease_token) && uint(v.lease_expires_at_ms, 1), 'invalid work identity or role');
  validateInvoice(v.invoice);
  if (phase === 'verify') {
    validateReference(v.payment_reference, v.invoice);
    need(uint(v.reference_assigned_at_ms, v.invoice.created_at_ms), 'invalid reference observation');
  } else need(v.payment_reference === null && v.reference_assigned_at_ms === null, 'issuer references belong to the evidence set');
  need(await invoiceCommitment(v.invoice_id, v.invoice) === v.invoice.invoice_commitment, 'invoice commitment differs');
  if (phase === 'verify') need(v.permit === null && v.evidence === null && v.previous_permit===undefined, 'verifier may not issue');
  else {
    validateProxyAdmissionPermit(v.permit); await validateEvidenceSet(v.evidence, v);
    if(v.previous_permit!==undefined) {
      validateProxyAdmissionPermit(v.previous_permit);
      need(v.permit.issuance_revision>1,'original issuance cannot claim predecessor');
    }
  }
  return v;
}
export async function evidenceCommitment(work, receipt) {
  return await digest(EVIDENCE_DOMAIN, { invoice_commitment:work.invoice.invoice_commitment,
    payment_reference:work.payment_reference, receipt });
}
// Stable receipt facts exclude verifier wall-clock observations and current
// confirmation count, so ACK recovery produces the same commitment.
export async function validateEvidence(v, work) {
  shape(v, ['canonical_epoch','evidence_commitment','receipt']);
  need(uint(v.canonical_epoch, 1) && hex(v.evidence_commitment), 'invalid canonical evidence');
  validateReceipt(v.receipt, work.invoice, work.payment_reference);
  need(await evidenceCommitment(work, v.receipt) === v.evidence_commitment, 'evidence commitment differs');
  return v;
}
export function validateReceipt(r, i, p) {
  validateReference(p, i);
  const shared = ['rail','physical_key','amount_base_units','finalized'];
  if (i.rail === 'tap') {
    shape(r, [...shared,'transaction_hash','log_index','chain_id','token_contract','from_address','to_address','block_number','block_hash','paid_at_ms']);
    need(r.chain_id === p.chain_id && r.token_contract === p.token_contract && r.transaction_hash === p.transaction_hash
      && r.log_index === p.log_index && address(r.from_address) && r.to_address === i.collection.destination
      && amount(r.block_number) && tx(r.block_hash) && uint(r.paid_at_ms, i.created_at_ms)
      && r.physical_key === `tap/${p.chain_id}/${p.token_contract}/${p.transaction_hash}/${p.log_index}`, 'invalid TAP receipt');
  } else if (i.rail === 'tnk') {
    shape(r, [...shared,'network','transaction_hash','from_address','to_address','confirmed_signed_length']);
    need(r.network === p.network && r.transaction_hash === p.transaction_hash && r.to_address === i.collection.destination
      && normalizeTnkAddress(r.from_address, p.network, 'sender') === r.from_address && uint(r.confirmed_signed_length, 1)
      && r.physical_key === `tnk/${p.network}/${p.transaction_hash}`, 'invalid TNK receipt');
  } else {
    shape(r, [...shared,'stripe_account','livemode','payment_intent_id','charge_id','currency','paid_at_ms']);
    need(r.stripe_account === p.stripe_account && r.livemode === p.livemode && r.payment_intent_id === p.payment_intent_id
      && /^ch_[A-Za-z0-9]{1,100}$/.test(r.charge_id) && r.currency === i.collection.currency && uint(r.paid_at_ms, i.created_at_ms)
      && r.physical_key === `fiat/${p.stripe_account}/${p.livemode ? 'live' : 'test'}/${p.payment_intent_id}`, 'invalid FIAT receipt');
  }
  need(r.rail === i.rail && r.finalized === true && amount(r.amount_base_units) && BigInt(r.amount_base_units) > 0n, 'unverified fee amount');
  return r;
}

export async function evidenceSetCommitment(work, receipts) {
  return await digest('mayhem/proxy/admission-evidence-set/v1', { invoice_commitment:work.invoice.invoice_commitment,
    receipts:receipts.map(({payment_reference,receipt})=>({payment_reference,receipt})) });
}
export async function validateEvidenceSet(v, work) {
  if(v?.format==='paged-v1') return validatePagedEvidence(v,work);
  shape(v, ['canonical_epoch','evidence_commitment','receipts']);
  need(uint(v.canonical_epoch,1) && hex(v.evidence_commitment) && Array.isArray(v.receipts)
    && v.receipts.length>0 && v.receipts.length<=32, 'invalid inline evidence set; larger invoices require paged transport');
  let total=0n, previous='';
  for(const item of v.receipts) {
    shape(item,['payment_reference','reference_assigned_at_ms','receipt']);
    need(uint(item.reference_assigned_at_ms,work.invoice.created_at_ms), 'invalid reference observation');
    const r=validateReceipt(item.receipt,work.invoice,item.payment_reference);
    need(r.physical_key>previous,'duplicate or unsorted physical evidence'); previous=r.physical_key;
    total+=BigInt(r.amount_base_units); need(total < (1n<<128n), 'evidence total exceeds u128');
  }
  need(await evidenceSetCommitment(work,v.receipts)===v.evidence_commitment,'evidence set commitment differs');
  return total;
}

export const EVIDENCE_PROGRESS_DOMAIN = 'mayhem/proxy/admission-evidence-progress/v1';
export const EVIDENCE_PAGE_SIZE = 4;
export async function evidenceSeed(work) {
  return digest('mayhem/proxy/admission-evidence-chain-seed/v1',{invoice_commitment:work.invoice.invoice_commitment});
}
export async function evidenceAppend(work,previous,sequence,member) {
  return digest('mayhem/proxy/admission-evidence-chain-member/v1',{invoice_commitment:work.invoice.invoice_commitment,previous,sequence,member});
}
export async function validatePagedEvidence(v,work) {
  shape(v,['format','canonical_epoch','evidence_commitment','receipt_count','total_amount','root']);
  need(v.format==='paged-v1'&&uint(v.canonical_epoch,1)&&hex(v.evidence_commitment)&&hex(v.root)
    &&uint(v.receipt_count,1)&&v.receipt_count<=0x7fffffff&&amount(v.total_amount),'invalid paged evidence');
  need(await digest('mayhem/proxy/admission-evidence-pages/v1',{invoice_commitment:work.invoice.invoice_commitment,
    root:v.root,receipt_count:v.receipt_count,total_amount:v.total_amount})===v.evidence_commitment,'paged evidence commitment differs');
  return BigInt(v.total_amount);
}
