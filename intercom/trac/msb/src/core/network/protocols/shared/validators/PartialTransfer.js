import PeerWallet from 'trac-wallet';

import {bufferToAddress} from "../../../../state/utils/address.js";
import {bufferToBigInt} from "../../../../../utils/amountSerialization.js";
import {decodeTransferBatchOutputs} from "../../../../../utils/transferBatch.js";
import PartialOperation from './base/PartialOperation.js';

class PartialTransfer extends PartialOperation {
    #config

    constructor(state, selfAddress, config) {
        super(state, selfAddress, config);
        this.#config = config
    }

    async validate(payload) {
        this.isPayloadSchemaValid(payload);
        this.validateNoSelfValidation(payload);
        this.validateRequesterAddress(payload);
        await this.validateTransactionUniqueness(payload);
        await this.validateSignature(payload);
        await this.validateTransactionValidity(payload);
        this.isOperationNotCompleted(payload);

        // uncommon validations below
        if (payload.tro.bo) {
            this.#validateBatchRecipients(payload)
            await this.#validateBatchStateBalances(payload)
        } else {
            this.#validateRecipientAddress(payload)
            await this.#validateStateBalances(payload)
        }

        return true;
    }

    #decodeBatch(payload) {
        return decodeTransferBatchOutputs(payload.tro.bo, this.#config);
    }

    #validateBatchRecipients(payload) {
        const senderAddress = bufferToAddress(payload.address, this.#config.addressPrefix);
        const batch = this.#decodeBatch(payload);
        for (const output of batch.outputs) {
            if (output.to === senderAddress) {
                throw new Error('Batch transfer must not include a self-recipient.');
            }
            const incomingPublicKey = PeerWallet.decodeBech32mSafe(output.to);
            if (incomingPublicKey === null) {
                throw new Error('Invalid recipient public key in batch transfer payload.');
            }
        }
        if (bufferToBigInt(payload.tro.ba) !== batch.total) {
            throw new Error('Batch transfer total does not match outputs.');
        }
    }

    async #validateBatchStateBalances(payload) {
        const senderAddress = bufferToAddress(payload.address, this.#config.addressPrefix);
        const batch = this.#decodeBatch(payload);
        const totalDeductedAmount = batch.total + this.fee;

        const senderEntry = await this.state.getNodeEntryUnsigned(senderAddress);
        if (!senderEntry) {
            throw new Error('Sender account not found');
        }

        const senderBalance = bufferToBigInt(senderEntry.balance);
        if (!(senderBalance >= totalDeductedAmount)) {
            throw new Error('Insufficient balance for batch transfer + fee');
        }

        for (const output of batch.outputs) {
            const recipientEntry = await this.state.getNodeEntryUnsigned(output.to);
            if (recipientEntry) {
                const recipientBalance = bufferToBigInt(recipientEntry.balance);
                const newRecipientBalance = recipientBalance + output.amount;
                if (newRecipientBalance > this.max_amount) {
                    throw new Error('Batch transfer would cause recipient balance to exceed maximum allowed value');
                }
            }
        }
    }

    #validateRecipientAddress(payload) {
        const incomingAddress = bufferToAddress(payload.tro.to, this.#config.addressPrefix);
        if (!incomingAddress) {
            throw new Error('Invalid recipient address in transfer payload.');
        }

        const incomingPublicKey = PeerWallet.decodeBech32mSafe(incomingAddress);
        if (incomingPublicKey === null) {
            throw new Error('Invalid recipient public key in transfer payload.');
        }

    }

    async #validateStateBalances(payload) {
        const senderAddress = bufferToAddress(payload.address, this.#config.addressPrefix);
        const recipientAddress = bufferToAddress(payload.tro.to, this.#config.addressPrefix);

        const transferAmount = bufferToBigInt(payload.tro.am);
        if (transferAmount > this.max_amount) {
            throw new Error('Transfer amount exceeds maximum allowed value');
        }

        const isSelfTransfer = senderAddress === recipientAddress;
        const totalDeductedAmount = isSelfTransfer ? this.fee : (transferAmount + this.fee);

        const senderEntry = await this.state.getNodeEntryUnsigned(senderAddress);
        if (!senderEntry) {
            throw new Error('Sender account not found');
        }

        const senderBalance = bufferToBigInt(senderEntry.balance);
        if (!(senderBalance >= totalDeductedAmount)) {
            throw new Error('Insufficient balance for transfer' + (isSelfTransfer ? ' fee' : ' + fee'));
        }

        if (!isSelfTransfer) {
            const recipientEntry = await this.state.getNodeEntryUnsigned(recipientAddress);
            if (recipientEntry) {
                const recipientBalance = bufferToBigInt(recipientEntry.balance);
                const newRecipientBalance = recipientBalance + transferAmount;
                if (newRecipientBalance > this.max_amount) {
                    throw new Error('Transfer would cause recipient balance to exceed maximum allowed value');
                }
            }
        }
    }
}

export default PartialTransfer;
