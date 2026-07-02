import b4a from 'b4a';
import {OperationType} from "../../../../../utils/constants.js";
import {bufferToAddress} from "../../../../state/utils/address.js";
import PartialOperation from './base/PartialOperation.js';
import {bufferToBigInt} from "../../../../../utils/amountSerialization.js";

class PartialRoleAccess extends PartialOperation {
    #config;

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

        // non common validations below
        if (payload.type === OperationType.ADD_WRITER) {
            await this.validateRequesterBalanceForAddWriterOperation(payload);
            await this.validateRequesterBalanceForAddWriterOperation(payload, true);
            await this.validateWriterKey(payload)
        } else {
            await this.validateRequesterBalance(payload, true);
            await this.validateRequesterBalance(payload)
        }
        await this.isRequesterAllowedToChangeRole(payload);

        return true;
    }

    async isRequesterAllowedToChangeRole(payload) {
        const {type} = payload;

        if (type === OperationType.ADD_WRITER) {
            const nodeAddress = bufferToAddress(payload.address, this.#config.addressPrefix);
            const nodeEntry = await this.state.getNodeEntry(nodeAddress);
            if (!nodeEntry) {
                throw new Error(`Node with address ${nodeAddress} entry does not exist.`);
            }

            const isNodeAlreadyWriter = nodeEntry.isWriter;
            if (isNodeAlreadyWriter) {
                throw new Error(`Node with address ${nodeAddress} is already a writer.`);
            }

            const isNodeWhitelisted = nodeEntry.isWhitelisted;
            if (!isNodeWhitelisted) {
                throw new Error(`Node with address ${nodeAddress} is not whitelisted.`);
            }
            return;

        } else if (type === OperationType.REMOVE_WRITER) {
            const nodeAddress = bufferToAddress(payload.address, this.#config.addressPrefix);
            const nodeEntry = await this.state.getNodeEntry(nodeAddress);
            if (!nodeEntry) {
                throw new Error(`Node with address ${nodeAddress} entry does not exist.`);
            }

            const isAlreadyWriter = nodeEntry.isWriter;
            if (!isAlreadyWriter) {
                throw new Error(`Node with address ${nodeAddress} is not a writer.`);
            }

            const isAlreadyIndexer = nodeEntry.isIndexer;
            if (isAlreadyIndexer) {
                throw new Error(`Node with address ${nodeAddress} is an indexer.`);
            }
            return;

        } else if (type === OperationType.ADMIN_RECOVERY) {
            const adminEntry = await this.state.getAdminEntry();
            if (!adminEntry) {
                throw new Error('Admin entry does not exist.');
            }

            const adminAddressBuffer = payload.address;
            const adminAddress = bufferToAddress(adminAddressBuffer, this.#config.addressPrefix);
            const isRecoveryCase = !!(
                adminEntry.address === adminAddress &&
                !b4a.equals(payload.rao.iw, adminEntry.wk)
            );
            if (!isRecoveryCase) {
                throw new Error(`Node with address ${adminAddress} is not a valid recovery case.`);
            }

            return;
        }

        throw new Error(`Unknown role access operation type: ${type}`);
    }

    async validateWriterKey(payload) {
        const requesterAddress = bufferToAddress(payload.address, this.#config.addressPrefix);
        const nodeEntry = await this.state.getNodeEntry(requesterAddress);
        if (!nodeEntry) {
            throw new Error(`Node entry not found for address ${requesterAddress}`);
        }

        const writerKey = payload.rao.iw.toString('hex');
        const addressFromRegisteredWritingKey = await this.state.getRegisteredWriterKey(writerKey);

        if (addressFromRegisteredWritingKey !== null) {
            const isCurrentWk = b4a.equals(nodeEntry.wk, payload.rao.iw);
            const isOwner = b4a.equals(addressFromRegisteredWritingKey, payload.address);

            if (!isCurrentWk || !isOwner) {
                throw new Error('Invalid writer key: either not owned by requester or different from assigned key');
            }
        }
    }

    async validateRequesterBalanceForAddWriterOperation(payload, signed = false) {
        const requesterAddress = bufferToAddress(payload.address, this.#config.addressPrefix);
        let requesterEntry;
        if (signed) {
            requesterEntry = await this.state.getNodeEntry(requesterAddress);
        } else {
            requesterEntry = await this.state.getNodeEntryUnsigned(requesterAddress);
        }

        if (!requesterEntry) {
            throw new Error('Requester address not found in state');
        }
        const requesterBalance = bufferToBigInt(requesterEntry.balance);

        const requiredBalance = this.fee * 11n;
        if (requesterBalance < requiredBalance) {
            throw new Error('Insufficient requester balance to cover role access operation FEE.');
        }
    }
}

export default PartialRoleAccess;
