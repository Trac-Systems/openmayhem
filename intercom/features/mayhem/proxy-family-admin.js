// Operator-configured, introduction-only adapter. No provider relay, fee, raw
// policy API or authority inferred from SITE roles. Uses the existing canonical
// writer, admission gate, pending journal and exact signed feature result.
import crypto from 'crypto';
import b4a from 'b4a';
import { proxyPolicyFeatureKey } from '../../contract/proxy-policy.js';
import { createProxyCanonicalReader } from './proxy-canonical-view.js';

const stable = value => JSON.stringify(value, (_, item) => item && typeof item === 'object' && !Array.isArray(item)
  ? Object.fromEntries(Object.keys(item).sort().map(key => [key, item[key]])) : item);
const hex = value => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
const fail = (code, status = 409) => { throw Object.assign(new Error(code), { code: `proxy_family_admin_${code}`, status }); };
const exact = (value, keys) => value && typeof value === 'object' && !Array.isArray(value)
  && Object.keys(value).sort().join('|') === [...keys].sort().join('|');
const family = value => exact(value, ['family_id', 'label']) && /^[a-z][a-z0-9_-]{0,63}$/.test(value.family_id)
  && typeof value.label === 'string' && /^[\x20-\x7e]{1,128}$/.test(value.label) && value.label.trim().length > 0;
const networkOf = ({ epoch, ...network }) => network;
const digest = (domain, value) => crypto.createHash('sha256').update(`${domain}\0`).update(stable(value)).digest('hex');
export const familyIntentDigest = intent => digest('mayhem/proxy/family-intent/v1', intent);
export const familyIntentNonce = (intent, operationId) => digest('mayhem/proxy/family-intent-nonce/v1', { intent, operation_id: operationId });
export function validateFamilyIntent(intent) {
  if (!exact(intent, ['schema_version', 'network', 'admin', 'expected_policy_revision', 'family']) || intent.schema_version !== 1
    || !exact(intent.network, ['network_id', 'msb_bootstrap', 'subnet_bootstrap', 'contract_version'])
    || !/^[a-z0-9_-]{1,128}$/.test(intent.network.network_id) || !hex(intent.network.msb_bootstrap) || !hex(intent.network.subnet_bootstrap)
    || !Number.isSafeInteger(intent.network.contract_version) || intent.network.contract_version < 1 || intent.network.contract_version > 0xffff_ffff
    || !hex(intent.admin) || !Number.isSafeInteger(intent.expected_policy_revision) || intent.expected_policy_revision < 0
    || intent.expected_policy_revision >= Number.MAX_SAFE_INTEGER || !family(intent.family)) fail('invalid', 400);
  return intent;
}
export function createProxyFamilyAdmin(feature, contractVersion) {
  const peer = feature.peer, reader = createProxyCanonicalReader(peer, contractVersion);
  const ready = () => { if (!feature.proxyPublicationController || feature.stopped) fail('unavailable', 503); };
  const envelopeFor = intent => JSON.parse(stable({ op: 'proxy_policy', context: intent.network, revision: intent.expected_policy_revision + 1,
    action: { kind: 'set_family', family_id: intent.family.family_id, label: intent.family.label, enabled: true } }));
  const confirmed = async (intent, envelope, key, nonce) => {
    const snapshot = await reader.pin();
    try {
      if (snapshot.admin !== intent.admin || stable(networkOf(snapshot.context)) !== stable(intent.network)) fail('identity_changed');
      const signature = peer.wallet.sign(`${JSON.stringify(envelope)}${nonce}`);
      const hash = b4a.isBuffer(signature) ? b4a.toString(signature, 'hex') : String(signature);
      if (!/^[0-9a-f]{128}$/.test(hash)) fail('unavailable', 503);
      const result = await snapshot.read(`fr/${hash}`);
      if (result === null) return null;
      if (result.type !== 'feature_result' || typeof result.ok !== 'boolean' || result.feature_key !== key
        || result.hash !== hash || result.address !== intent.admin || result.ok &&
          (result.result?.revision !== envelope.revision || result.result?.operation_key !== key || result.result?.action !== 'set_family')) fail('result_invalid', 503);
      snapshot.assertCanonical();
      return { schema_version: 1, intent_digest: familyIntentDigest(intent), status: result.ok ? 'applied' : 'rejected',
        operation_key: key, result_key: `fr/${hash}`, network: intent.network, admin: intent.admin,
        family: intent.family, policy_revision: envelope.revision, proof: snapshot.proof };
    } finally { await snapshot.view.close(); }
  };
  return {
    async preview(body) {
      ready();
      if (!exact(body, ['schema_version', 'family']) || body.schema_version !== 1 || !family(body.family)) fail('invalid', 400);
      const snapshot = await reader.pin();
      try {
        if (await snapshot.read(`proxy/v1/family/${body.family.family_id}`) !== null) fail('already_exists');
        const head = await snapshot.read('proxy/v1/policy-head');
        if (!head || !Number.isSafeInteger(head.revision) || head.revision < 0 || head.revision >= Number.MAX_SAFE_INTEGER) fail('unavailable', 503);
        const intent = validateFamilyIntent({ schema_version: 1, network: networkOf(snapshot.context), admin: snapshot.admin,
          expected_policy_revision: head.revision, family: { ...body.family } });
        snapshot.assertCanonical();
        return { schema_version: 1, intent, intent_digest: familyIntentDigest(intent), proof: snapshot.proof };
      } finally { await snapshot.view.close(); }
    },
    async submit(body) {
      ready();
      if (!exact(body, ['schema_version', 'intent', 'operation_id', 'nonce']) || body.schema_version !== 1
        || typeof body.operation_id !== 'string' || !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(body.operation_id)) fail('invalid', 400);
      const intent = validateFamilyIntent(body.intent);
      if (body.nonce !== familyIntentNonce(intent, body.operation_id)) fail('nonce_mismatch', 400);
      const envelope = envelopeFor(intent), key = await proxyPolicyFeatureKey(envelope);
      // Exact signed original result precedes fresh-admission/head checks. Never
      // infer success from a family row, unsigned local view or reused nonce.
      const original = await confirmed(intent, envelope, key, body.nonce); if (original) return original;
      const pending = feature.proxyPublicationController.journal.get(key);
      if (pending && (pending.nonce !== body.nonce || stable(pending.envelope) !== stable(envelope))) fail('operation_conflict');
      if (pending) {
        await feature.submit(key, envelope, { nonce: body.nonce });
        const recovered = await confirmed(intent, envelope, key, body.nonce);
        if (recovered) return recovered;
        return { schema_version: 1, intent_digest: familyIntentDigest(intent), status: 'pending', operation_key: key, result_key: null,
          network: intent.network, admin: intent.admin, family: intent.family, policy_revision: envelope.revision, proof: null };
      }
      const snapshot = await reader.pin();
      try {
        if (snapshot.admin !== intent.admin || stable(networkOf(snapshot.context)) !== stable(intent.network)) fail('identity_changed');
        const head = await snapshot.read('proxy/v1/policy-head');
        if (head?.revision !== intent.expected_policy_revision) fail('revision_conflict');
        if (await snapshot.read(`proxy/v1/family/${intent.family.family_id}`) !== null) fail('already_exists');
        snapshot.assertCanonical();
      } finally { await snapshot.view.close(); }
      // The existing gate repeats expected policy/context validation immediately
      // before dispatch; all policy writes conflict in its durable pending journal.
      await feature.submit(key, envelope, { nonce: body.nonce });
      const result = await confirmed(intent, envelope, key, body.nonce);
      return result ?? { schema_version: 1, intent_digest: familyIntentDigest(intent), status: 'pending',
        operation_key: key, result_key: null, network: intent.network, admin: intent.admin,
        family: intent.family, policy_revision: envelope.revision, proof: null };
    },
  };
}
