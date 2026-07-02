import { BaseCheck } from '../../base/check.js';
import b4a from 'b4a';
import { safeDecodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { jsonStringify, safeClone, createHash } from '../../utils/types.js';

export class TxOperation {
    #validator
    #wallet
    #protocolInstance
    #contractInstance
    #msbClient
    #config

    constructor(validator, {
        wallet,
        protocolInstance,
        contractInstance,
        msbClient,
        config
    }) {
        this.#validator = validator
        this.#wallet = wallet
        this.#protocolInstance = protocolInstance
        this.#contractInstance = contractInstance
        this.#msbClient = msbClient
        this.#config = config
    }
    async handle(op, batch, base, node) {
        if(false === this.#validator.validateNode(node)) return;
        // TX apply: only accept subnet TXs that are confirmed on MSB, then execute contract logic
        // deterministically (same ordered log => same state everywhere).

        // Payload size guard (protect apply from huge JSON ops)
        if(b4a.byteLength(jsonStringify(op)) > this.#protocolInstance.txMaxBytes()) return;
        // Schema validation (required fields / types)
        if(false === this.#validator.validate(op)) return;
        // Stall guard: don't allow a writer to pin apply waiting on an absurd MSB height
        if (op.value.msbsl > this.#config.maxMsbSignedLength) return;
        // Wait for local MSB view to reach the referenced signed length
        await this.#msbClient.waitForSignedLengthAtLeast(op.value.msbsl);
        // Fetch MSB apply-op at msbsl by tx key (op.key = tx hash)
        const msbTxEntry = await this.#msbClient.getSignedAtLength(op.key, op.value.msbsl);
        // MSB entry shape/size guards (protect protobuf decode + keep apply bounded)
        if (null === msbTxEntry || false === b4a.isBuffer(msbTxEntry.value)) return;
        if (msbTxEntry.value.byteLength > this.#config.maxMsbApplyOperationBytes) return;
        // Decode MSB operation and ensure it's a TX (type 12) with required fields
        const decoded = safeDecodeApplyOperation(msbTxEntry.value);
        if (null === decoded || decoded.txo === undefined) return;
        if (decoded.type !== 12) return;
        // Cross-check: tx hash matches op.key
        if (null === decoded.txo.tx || decoded.txo.tx.toString('hex') !== op.key) return;
        // Cross-check: MSB tx targets this subnet + this MSB network
        const subnetBootstrapHex = (b4a.isBuffer(this.#config.bootstrap) ? this.#config.bootstrap.toString('hex') : `${this.#config.bootstrap}`).toLowerCase();
        if (null === decoded.txo.bs || decoded.txo.bs.toString('hex') !== subnetBootstrapHex) return;
        if (null === decoded.txo.mbs || decoded.txo.mbs.toString('hex') !== this.#msbClient.bootstrapHex) return;
        // Cross-check: content hash matches the subnet dispatch payload (blake3)
        const contentHash = await createHash(jsonStringify(op.value.dispatch));
        if (null === decoded.txo.ch || decoded.txo.ch.toString('hex') !== contentHash) return;
        // Cross-check: requester identity matches ipk
        const invokerAddress = decoded.address ? decoded.address.toString('ascii') : null;
        const invokerPubKeyHex = invokerAddress ? this.#msbClient.addressToPubKeyHex(invokerAddress) : null;
        if (null === invokerPubKeyHex || invokerPubKeyHex !== `${op.value.ipk}`.toLowerCase()) return;
        // Cross-check: validator identity matches wp
        const validatorAddress = decoded.txo.va ? decoded.txo.va.toString('ascii') : null;
        const validatorPubKeyHex = validatorAddress ? this.#msbClient.addressToPubKeyHex(validatorAddress) : null;
        if (null === validatorPubKeyHex || validatorPubKeyHex !== `${op.value.wp}`.toLowerCase()) return;
        // Transactions enabled gate (default: enabled if missing)
        const enabled = await batch.get('txen');
        if (!(enabled === null || enabled.value === true)) return;
        // Replay protection: ignore already-indexed TXs
        if (null !== await batch.get(`tx/${op.key}`)) return;
        // Execute contract and index deterministic result into subnet state
        const err = this.#protocolInstance.getError(
            await this.#contractInstance.execute(op, batch)
        );
        let errValue = null;
        if(null !== err) {
            if(err.constructor.name === 'UnknownContractOperationType') return;
            const errMsg = parseInt(err.message);
            errValue = isNaN(errMsg) ? `${err.message}` : errMsg;
        }
        let len = await batch.get('txl');
        if(null === len) {
            len = 0;
        } else {
            len = len.value;
        }
        const dta = {};
        dta['val'] = safeClone(op.value.dispatch);
        dta['err'] = errValue;
        dta['tx'] = op.key;
        dta['ipk'] = op.value.ipk;
        dta['wp'] = op.value.wp;
        await batch.put(`txi/${len}`, dta);
        await batch.put('txl', len + 1);
        await batch.put(`tx/${op.key}`, len);
        let ulen = await batch.get(`utxl/${op.value.ipk}`);
        if(null === ulen) {
            ulen = 0;
        } else {
            ulen = ulen.value;
        }
        await batch.put(`utxi/${op.value.ipk}/${ulen}`, len);
        await batch.put(`utxl/${op.value.ipk}`, ulen + 1);
        if(true === this.#config.enableTxlogs){
            console.log(`${op.key} appended. Signed length: ${base.view.core.signedLength}, tx length: ${len + 1}`);
        }
    }
}

export class TxCheck extends BaseCheck {
    #validate

    constructor() {
        super()
        this.#validate = this.#compile()
    }

    #compile() {
        const schema = {
            key: { type : "is_hex" },
            value : {
                $$type: "object",
                dispatch : {
                    $$strict : true,
                    $$type : "object",
                    type : { type : "string", min : 1, max : 256 },
                    value : { type : "any", nullable : true }
                },
                msbsl : { type : "number", integer : true, min : 0, max : Number.MAX_SAFE_INTEGER },
                ipk : { type : "is_hex" },
                wp : { type : "is_hex" }
            }
        };

        return this.validator.compile(schema)
    }

    validate(op) {
        return this.#validate(op) === true
    }
}
