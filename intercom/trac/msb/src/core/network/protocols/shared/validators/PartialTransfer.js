import PeerWallet from 'trac-wallet';

import {bufferToAddress} from "../../../../state/utils/address.js";
import {bufferToBigInt} from "../../../../../utils/amountSerialization.js";
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
        this.#validateRecipientAddress(payload)
        await this.#validateStateBalances(payload)

        return true;
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
