import { Contract } from 'trac-peer';

const CONTRACT_VERSION = 1;
const CURRENT_RULES_KEY = 'rules/current';

export const consentMessage = (ver, hash) => `mayhem-consent${ver}${hash}`;

class MayhemContract extends Contract {
  constructor(protocol, options = {}) {
    super(protocol, options);

    this.addSchema('noop', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 32 },
      },
    });

    this.addSchema('gatedNoop', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 32 },
      },
    });

    this.addSchema('readKey', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        key: { type: 'string', min: 1, max: 256 },
      },
    });

    this.addSchema('setRules', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        ver: { type: 'number', integer: true, min: 1 },
        hash: { type: 'string', min: 1, max: 128 },
      },
    });

    this.addSchema('consent', {
      value: {
        $$strict: true,
        $$type: 'object',
        op: { type: 'string', min: 1, max: 64 },
        ver: { type: 'number', integer: true, min: 1 },
        hash: { type: 'string', min: 1, max: 128 },
        sig: { type: 'string', min: 1, max: 256 },
      },
    });
  }

  async noop() {
    const result = {
      ok: true,
      op: 'noop',
      contract: 'mayhem',
      version: CONTRACT_VERSION,
      address: this.address,
    };
    console.log('mayhem noop', result);
    return result;
  }

  async gatedNoop() {
    const consentError = await this.requireConsent();
    if (consentError) return consentError;

    const result = {
      ok: true,
      op: 'gatedNoop',
      contract: 'mayhem',
      version: CONTRACT_VERSION,
      address: this.address,
    };
    console.log('mayhem gatedNoop', result);
    return result;
  }

  async readKey() {
    const key = this.value?.key;
    const value = typeof key === 'string' ? await this.get(key) : null;
    console.log('mayhem readKey', key, '=>', value);
    return value;
  }

  async setRules() {
    const adminError = await this.requireAdmin();
    if (adminError) return adminError;

    const current = await this.currentRules();
    if (current && this.value.ver <= current.ver) {
      return new Error('Rules version must increase.');
    }

    const rules = {
      ver: this.value.ver,
      hash: this.value.hash,
      activated_at: this.tx,
    };
    await this.put(`rules/${rules.ver}`, rules);
    await this.put(CURRENT_RULES_KEY, rules);
    console.log('mayhem setRules', rules);
    return { ok: true, op: 'setRules', rules };
  }

  async consent() {
    const rules = await this.currentRules();
    if (!rules) return new Error('Rules are not set.');
    if (this.value.ver !== rules.ver || this.value.hash !== rules.hash) {
      return new Error('Consent must match the current rules.');
    }
    if (!this.verifyConsentSignature(this.address, this.value.ver, this.value.hash, this.value.sig)) {
      return new Error('Invalid consent signature.');
    }

    const record = {
      ver: this.value.ver,
      hash: this.value.hash,
      at: this.tx,
    };
    await this.put(`consent/${this.address}`, record);
    console.log('mayhem consent', { address: this.address, ...record });
    return { ok: true, op: 'consent', address: this.address, ...record };
  }

  async currentRules() {
    return await this.get(CURRENT_RULES_KEY);
  }

  async requireAdmin(sender = this.address) {
    const admin = await this.get('admin');
    if (admin === null || admin === sender) return null;
    return new Error('Admin required.');
  }

  async requireConsent(sender = this.address) {
    if (!sender) return new Error('Consent required.');

    const rules = await this.currentRules();
    if (!rules) return new Error('Rules are not set.');

    const consent = await this.get(`consent/${sender}`);
    if (!consent || consent.ver !== rules.ver || consent.hash !== rules.hash) {
      return new Error(`Consent required for rules version ${rules.ver}.`);
    }
    return null;
  }

  verifyConsentSignature(sender, ver, hash, sig) {
    const verify = this.protocol?.peer?.wallet?.verify;
    if (typeof verify !== 'function') return false;
    return verify.call(this.protocol.peer.wallet, sig, consentMessage(ver, hash), sender) === true;
  }
}

export default MayhemContract;
