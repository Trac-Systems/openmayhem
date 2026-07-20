import b4a from 'b4a';
import PeerWallet from 'trac-wallet';

import { createMessage } from '../../utils/buffer.js';
import { OperationType } from '../../utils/constants.js';
import { addressToBuffer, bufferToAddress } from '../../core/state/utils/address.js';
import { isAddressValid } from "../../core/state/utils/address.js";
import {
    isAdminControl,
    isBalanceInitialization,
    isBootstrapDeployment,
    isCoreAdmin,
    isRoleAccess,
    isTransaction,
    isTransfer,
    operationToPayload
} from '../../utils/applyOperations.js';
import { isHexString } from '../../utils/helpers.js';

// Single use per transaction: reuse of this instance needs mutex/queue or fail-fast and can delay validation or break validation rule.
// A fresh instance is effectively zero-cost, so no reset() is provided.

/**
 * Builder for partial/complete ApplyState messages.
 * @param {PeerWallet} wallet
 * @param {object} config
 */
class ApplyStateMessageBuilder {
    #address;
    #amount;
    #channel;
    #config
    #contentHash;
    #externalBootstrap;
    #incomingAddress;
    #incomingNonce;
    #incomingSignature;
    #incomingWriterKey;
    #msbBootstrap;
    #operationType;
    #payload;
    #txHash;
    #txValidity;
    #wallet;
    #writingKey;
    #phase;
    #output;
    #payloadKey;
    #built=false;

    constructor(wallet, config) {
        this.#config = config;
        if (!wallet || typeof wallet !== 'object') {
            throw new Error('Wallet must be a valid wallet object');
        }
        if (!isAddressValid(wallet.address, this.#config.addressPrefix)) {
            throw new Error('Wallet should have a valid TRAC address.');
        }

        this.#wallet = wallet;
    }

    setPhase(phase) {
        if (!['partial', 'complete'].includes(phase)) {
            throw new Error(`Invalid phase: ${phase}`);
        }
        this.#phase = phase;
        return this;
    }

    setOutput(output) {
        if (!['json', 'buffer'].includes(output)) {
            throw new Error(`Invalid output format: ${output}`);
        }
        this.#output = output;
        return this;
    }

    setOperationType(operationType) {
        if (!Object.values(OperationType).includes(operationType)) {
            throw new Error(`Invalid operation type: ${operationType}`);
        }
        this.#operationType = operationType;
        return this;
    }

    setAddress(address) {
        const addressBuffer = this.#normalizeAddress(address);
        if (!addressBuffer) {
            throw new Error(`Address field must be a valid TRAC bech32m address with length ${this.#config.addressLength}.`);
        }

        this.#address = addressBuffer;
        return this;
    }

    setWriterKey(writingKey) {
        this.#writingKey = this.#normalizeHexBuffer(writingKey, 32, 'Writer key');
        return this;
    }

    setTxHash(txHash) {
        this.#txHash = this.#normalizeHexBuffer(txHash, 32, 'Transaction hash');
        return this;
    }

    setIncomingAddress(address) {
        const addressBuffer = this.#normalizeAddress(address);
        if (!addressBuffer) {
            throw new Error(`Address field must be a valid TRAC bech32m address with length ${this.#config.addressLength}.`);
        }

        this.#incomingAddress = addressBuffer;
        return this;
    }

    setIncomingWriterKey(writerKey) {
        this.#incomingWriterKey = this.#normalizeHexBuffer(writerKey, 32, 'Incoming writer key');
        return this;
    }

    setIncomingNonce(nonce) {
        this.#incomingNonce = this.#normalizeHexBuffer(nonce, 32, 'Incoming nonce');
        return this;
    }

    setContentHash(contentHash) {
        this.#contentHash = this.#normalizeHexBuffer(contentHash, 32, 'Content hash');
        return this;
    }

    setIncomingSignature(signature) {
        this.#incomingSignature = this.#normalizeHexBuffer(signature, 64, 'Incoming signature');
        return this;
    }

    setExternalBootstrap(bootstrapKey) {
        this.#externalBootstrap = this.#normalizeHexBuffer(bootstrapKey, 32, 'Bootstrap key');
        return this;
    }

    setMsbBootstrap(msbBootstrap) {
        this.#msbBootstrap = this.#normalizeHexBuffer(msbBootstrap, 32, 'MSB bootstrap');
        return this;
    }

    setChannel(channel) {
        this.#channel = this.#normalizeHexBuffer(channel, 32, 'Channel');
        return this;
    }

    setTxValidity(txValidity) {
        this.#txValidity = this.#normalizeHexBuffer(txValidity, 32, 'Transaction validity');
        return this;
    }

    setAmount(amount) {
        this.#amount = this.#normalizeHexBuffer(amount, 16, 'Amount');
        return this;
    }

    #requireFields(fields) {
        for (const [value, name] of fields) {
            if (!value) {
                throw new Error(`${name} must be set before build.`);
            }
        }
    }

    async build() {
        this.#assertPhaseAndOutput();

        if (!this.#operationType) {
            throw new Error('Operation type must be set before build.');
        }

        if (!this.#address) {
            throw new Error('Address must be set before build.');
        }

        const payloadKey = operationToPayload(this.#operationType);
        if (!payloadKey) {
            throw new Error(`Unsupported operation type: ${this.#operationType}`);
        }

        let body;
        if (this.#phase === 'partial') {
            body = await this.#buildPartialBody();
        } else {
            body = await this.#buildCompleteBody();
        }

        this.#payloadKey = payloadKey;
        this.#payload = {
            type: this.#operationType,
            address: this.#address,
            [payloadKey]: body
        };
        this.#built = true;
        return this;
    }

    getPayload() {
        if (!this.#built || !this.#payload) {
            throw new Error('Payload has not been built.');
        }
        return this.#output === 'json' ? this.#encodePayloadJson(this.#payload) : this.#payload;
    }

    #assertPhaseAndOutput() {
        if (!this.#phase) {
            throw new Error('Phase must be set before build.');
        }

        if (!this.#output) {
            throw new Error('Output format must be set before build.');
        }

        // We assume that complete phase only supports buffer output. So this check will be enforced
        if (this.#phase === 'complete' && this.#output !== 'buffer') {
            throw new Error('Complete phase only supports buffer output.');
        }
    }

    #normalizeHexBuffer(value, expectedBytes, fieldName) {
        // with normalizer built in builder, we can remove other normalizers later.
        if (b4a.isBuffer(value)) {
            if (value.length !== expectedBytes) {
                throw new Error(`${fieldName} must be a ${expectedBytes}-byte buffer.`);
            }
            return value;
        }
        if (typeof value === 'string') {
            const expectedLength = expectedBytes * 2;
            if (!isHexString(value) || value.length !== expectedLength) {
                throw new Error(`${fieldName} must be a ${expectedLength}-length hexstring.`);
            }
            return b4a.from(value, 'hex');
        }
        throw new Error(`${fieldName} must be a ${expectedBytes}-byte buffer or ${expectedBytes * 2}-length hexstring.`);
    }

    #normalizeAddress(address) {
        if (b4a.isBuffer(address)) {
            const addr = bufferToAddress(address, this.#config.addressPrefix);
            return addr ? address : null;
        }
        if (!isAddressValid(address, this.#config.addressPrefix)) {
            return null;
        }
        return addressToBuffer(address, this.#config.addressPrefix);
    }

    async #buildPartialBody() {
        if (!isRoleAccess(this.#operationType) && !isTransaction(this.#operationType) &&
            !isBootstrapDeployment(this.#operationType) && !isTransfer(this.#operationType)) {
            throw new Error(`Operation type ${this.#operationType} is not supported for partial build.`);
        }

        const nonce = PeerWallet.generateNonce();
        let msg;

        switch (this.#operationType) {
            case OperationType.ADD_WRITER:
            case OperationType.REMOVE_WRITER:
            case OperationType.ADMIN_RECOVERY:
                this.#requireFields([
                    [this.#txValidity, 'Transaction validity'],
                    [this.#writingKey, 'Writer key']
                ]);
                msg = createMessage(
                    this.#config.networkId,
                    this.#txValidity,
                    this.#writingKey,
                    nonce,
                    this.#operationType
                );
                break;
            case OperationType.BOOTSTRAP_DEPLOYMENT:
                this.#requireFields([
                    [this.#txValidity, 'Transaction validity'],
                    [this.#externalBootstrap, 'External bootstrap'],
                    [this.#channel, 'Channel']
                ]);
                msg = createMessage(
                    this.#config.networkId,
                    this.#txValidity,
                    this.#externalBootstrap,
                    this.#channel,
                    nonce,
                    OperationType.BOOTSTRAP_DEPLOYMENT
                );
                break;
            case OperationType.TX:
                this.#requireFields([
                    [this.#txValidity, 'Transaction validity'],
                    [this.#writingKey, 'Writer key'],
                    [this.#contentHash, 'Content hash'],
                    [this.#externalBootstrap, 'External bootstrap'],
                    [this.#msbBootstrap, 'MSB bootstrap']
                ]);
                msg = createMessage(
                    this.#config.networkId,
                    this.#txValidity,
                    this.#writingKey,
                    this.#contentHash,
                    this.#externalBootstrap,
                    this.#msbBootstrap,
                    nonce,
                    OperationType.TX
                );
                break;
            case OperationType.TRANSFER:
                this.#requireFields([
                    [this.#txValidity, 'Transaction validity'],
                    [this.#incomingAddress, 'Incoming address'],
                    [this.#amount, 'Amount']
                ]);
                msg = createMessage(
                    this.#config.networkId,
                    this.#txValidity,
                    this.#incomingAddress,
                    this.#amount,
                    nonce,
                    OperationType.TRANSFER
                );
                break;
            default:
                throw new Error(`Unsupported operation type: ${this.#operationType}`);
        }

        const tx = await PeerWallet.blake3(msg);
        const signature = this.#wallet.sign(tx);

        if (isBootstrapDeployment(this.#operationType)) {
            return {
                tx,
                txv: this.#txValidity,
                bs: this.#externalBootstrap,
                ic: this.#channel,
                in: nonce,
                is: signature
            };
        }
        if (isRoleAccess(this.#operationType)) {
            return {
                tx,
                txv: this.#txValidity,
                iw: this.#writingKey,
                in: nonce,
                is: signature
            };
        }
        if (isTransaction(this.#operationType)) {
            return {
                tx,
                txv: this.#txValidity,
                iw: this.#writingKey,
                ch: this.#contentHash,
                bs: this.#externalBootstrap,
                mbs: this.#msbBootstrap,
                in: nonce,
                is: signature,
            };
        }
        if (isTransfer(this.#operationType)) {
            return {
                tx,
                txv: this.#txValidity,
                to: this.#incomingAddress,
                am: this.#amount,
                in: nonce,
                is: signature
            };
        }

        throw new Error(`No corresponding value type for operation: ${this.#operationType}`);
    }

    async #buildCompleteBody() {
        const nonce = PeerWallet.generateNonce();
        let msg;

        switch (this.#operationType) {
            case OperationType.ADD_ADMIN:
            case OperationType.DISABLE_INITIALIZATION:
                this.#requireFields([
                    [this.#txValidity, 'Transaction validity'],
                    [this.#writingKey, 'Writer key']
                ]);
                msg = createMessage(
                    this.#config.networkId,
                    this.#txValidity,
                    this.#writingKey,
                    nonce,
                    this.#operationType
                );
                break;
            case OperationType.BALANCE_INITIALIZATION:
                this.#requireFields([
                    [this.#txValidity, 'Transaction validity'],
                    [this.#incomingAddress, 'Incoming address'],
                    [this.#amount, 'Amount']
                ]);
                msg = createMessage(
                    this.#config.networkId,
                    this.#txValidity,
                    this.#incomingAddress,
                    this.#amount,
                    nonce,
                    this.#operationType
                );
                break;
            case OperationType.APPEND_WHITELIST:
            case OperationType.ADD_INDEXER:
            case OperationType.REMOVE_INDEXER:
            case OperationType.BAN_VALIDATOR: {
                this.#requireFields([
                    [this.#txValidity, 'Transaction validity'],
                    [this.#incomingAddress, 'Incoming address']
                ]);
                const incomingAddress = bufferToAddress(this.#incomingAddress, this.#config.addressPrefix);
                if (incomingAddress && this.#wallet.address === incomingAddress) {
                    throw new Error('Address must not be the same as the wallet address for basic operations.');
                }
                msg = createMessage(
                    this.#config.networkId,
                    this.#txValidity,
                    this.#incomingAddress,
                    nonce,
                    this.#operationType
                );
                break;
            }
            case OperationType.ADD_WRITER:
            case OperationType.REMOVE_WRITER:
            case OperationType.ADMIN_RECOVERY:
                this.#requireFields([
                    [this.#txHash, 'Transaction hash'],
                    [this.#txValidity, 'Transaction validity'],
                    [this.#incomingWriterKey, 'Incoming writer key'],
                    [this.#incomingNonce, 'Incoming nonce'],
                    [this.#incomingSignature, 'Incoming signature']
                ]);
                msg = createMessage(
                    this.#config.networkId,
                    this.#txHash,
                    nonce,
                    this.#operationType
                );
                break;
            case OperationType.BOOTSTRAP_DEPLOYMENT:
                this.#requireFields([
                    [this.#txHash, 'Transaction hash'],
                    [this.#txValidity, 'Transaction validity'],
                    [this.#externalBootstrap, 'External bootstrap'],
                    [this.#channel, 'Channel'],
                    [this.#incomingNonce, 'Incoming nonce'],
                    [this.#incomingSignature, 'Incoming signature']
                ]);
                msg = createMessage(
                    this.#config.networkId,
                    this.#txHash,
                    nonce,
                    this.#operationType
                );
                break;
            case OperationType.TX:
                this.#requireFields([
                    [this.#txHash, 'Transaction hash'],
                    [this.#txValidity, 'Transaction validity'],
                    [this.#incomingWriterKey, 'Incoming writer key'],
                    [this.#incomingNonce, 'Incoming nonce'],
                    [this.#incomingSignature, 'Incoming signature'],
                    [this.#contentHash, 'Content hash'],
                    [this.#externalBootstrap, 'External bootstrap'],
                    [this.#msbBootstrap, 'MSB bootstrap']
                ]);
                msg = createMessage(
                    this.#config.networkId,
                    this.#txHash,
                    nonce,
                    this.#operationType
                );
                break;
            case OperationType.TRANSFER:
                this.#requireFields([
                    [this.#txHash, 'Transaction hash'],
                    [this.#txValidity, 'Transaction validity'],
                    [this.#incomingAddress, 'Incoming address'],
                    [this.#amount, 'Amount'],
                    [this.#incomingNonce, 'Incoming nonce'],
                    [this.#incomingSignature, 'Incoming signature']
                ]);
                msg = createMessage(
                    this.#config.networkId,
                    this.#txHash,
                    nonce,
                    this.#operationType
                );
                break;
            default:
                throw new Error(`Unsupported operation type: ${this.#operationType}`);
        }

        const tx = await PeerWallet.blake3(msg);
        const signature = this.#wallet.sign(tx);
        const validatorAddress = addressToBuffer(this.#wallet.address, this.#config.addressPrefix);

        if (isCoreAdmin(this.#operationType)) {
            return {
                tx,
                txv: this.#txValidity,
                iw: this.#writingKey,
                in: nonce,
                is: signature
            };
        }
        if (isAdminControl(this.#operationType)) {
            return {
                tx,
                txv: this.#txValidity,
                ia: this.#incomingAddress,
                in: nonce,
                is: signature
            };
        }
        if (isRoleAccess(this.#operationType)) {
            return {
                tx: this.#txHash,
                txv: this.#txValidity,
                iw: this.#incomingWriterKey,
                in: this.#incomingNonce,
                is: this.#incomingSignature,
                va: validatorAddress,
                vn: nonce,
                vs: signature,
            };
        }
        if (isTransaction(this.#operationType)) {
            return {
                tx: this.#txHash,
                txv: this.#txValidity,
                iw: this.#incomingWriterKey,
                ch: this.#contentHash,
                bs: this.#externalBootstrap,
                mbs: this.#msbBootstrap,
                in: this.#incomingNonce,
                is: this.#incomingSignature,
                va: validatorAddress,
                vn: nonce,
                vs: signature,
            };
        }
        if (isBootstrapDeployment(this.#operationType)) {
            return {
                tx: this.#txHash,
                txv: this.#txValidity,
                bs: this.#externalBootstrap,
                ic: this.#channel,
                in: this.#incomingNonce,
                is: this.#incomingSignature,
                va: validatorAddress,
                vn: nonce,
                vs: signature
            };
        }
        if (isTransfer(this.#operationType)) {
            return {
                tx: this.#txHash,
                txv: this.#txValidity,
                to: this.#incomingAddress,
                am: this.#amount,
                in: this.#incomingNonce,
                is: this.#incomingSignature,
                va: validatorAddress,
                vn: nonce,
                vs: signature
            };
        }
        if (isBalanceInitialization(this.#operationType)) {
            return {
                tx,
                txv: this.#txValidity,
                ia: this.#incomingAddress,
                am: this.#amount,
                in: nonce,
                is: signature
            };
        }

        throw new Error(`No corresponding value type for operation: ${this.#operationType}`);
    }

    #encodePayloadJson(payload) {
        const toHex = buffer => buffer.toString('hex');
        const address = bufferToAddress(payload.address, this.#config.addressPrefix);
        if (!address) {
            throw new Error('Payload address is invalid.');
        }

        const body = payload[this.#payloadKey];
        const base = { type: payload.type, address };

        switch (this.#payloadKey) {
            case 'rao':
                return {
                    ...base,
                    rao: {
                        tx: toHex(body.tx),
                        txv: toHex(body.txv),
                        iw: toHex(body.iw),
                        in: toHex(body.in),
                        is: toHex(body.is)
                    }
                };
            case 'txo':
                return {
                    ...base,
                    txo: {
                        tx: toHex(body.tx),
                        txv: toHex(body.txv),
                        iw: toHex(body.iw),
                        ch: toHex(body.ch),
                        bs: toHex(body.bs),
                        mbs: toHex(body.mbs),
                        in: toHex(body.in),
                        is: toHex(body.is)
                    }
                };
            case 'bdo':
                return {
                    ...base,
                    bdo: {
                        tx: toHex(body.tx),
                        txv: toHex(body.txv),
                        bs: toHex(body.bs),
                        ic: toHex(body.ic),
                        in: toHex(body.in),
                        is: toHex(body.is)
                    }
                };
            case 'tro':
                return {
                    ...base,
                    tro: {
                        tx: toHex(body.tx),
                        txv: toHex(body.txv),
                        to: bufferToAddress(body.to, this.#config.addressPrefix),
                        am: toHex(body.am),
                        in: toHex(body.in),
                        is: toHex(body.is)
                    }
                };
            default:
                throw new Error(`JSON output is not supported for payload ${this.#payloadKey}.`);
        }
    }
}

export default ApplyStateMessageBuilder;
