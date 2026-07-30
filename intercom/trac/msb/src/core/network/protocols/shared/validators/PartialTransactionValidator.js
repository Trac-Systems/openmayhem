import b4a from 'b4a';
import {safeDecodeApplyOperation} from "../../../../../utils/protobuf/operationHelpers.js";
import deploymentEntryUtils from "../../../../state/utils/deploymentEntry.js";
import PartialOperationValidator from './PartialOperationValidator.js';
import {ResultCode} from "../../../../../utils/constants.js";
import {V1ProtocolError} from '../../v1/V1ProtocolError.js';

class PartialTransactionValidator extends PartialOperationValidator {
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
        await this.validateRequesterBalance(payload);
        await this.validateRequesterBalance(payload, true);
        this.validateSubnetworkBootstrapEquality(payload);

        // non common validations below
        this.validateMsbBootstrap(payload);
        await this.validateIfExternalBootstrapHasBeenDeployed(payload);

        return true;
    }

    validateMsbBootstrap(payload) {
        if (!b4a.equals(this.#config.bootstrap, payload.txo.mbs)) {
            throw new V1ProtocolError(
                ResultCode.MSB_BOOTSTRAP_MISMATCH,
                `Declared MSB bootstrap is different than network bootstrap in transaction operation: ${payload.txo.mbs.toString('hex')}`
            );
        }
    }

    async validateIfExternalBootstrapHasBeenDeployed(payload) {
        const externalBootstrapResult = await this.state.getRegisteredBootstrapEntry(payload.txo.bs.toString('hex'));
        if (externalBootstrapResult === null) {
            throw new V1ProtocolError(
                ResultCode.EXTERNAL_BOOTSTRAP_NOT_DEPLOYED,
                `External bootstrap with hash ${payload.txo.bs.toString('hex')} is not registered as deployment entry.`
            );
        }

        const decodedPayload = deploymentEntryUtils.decode(externalBootstrapResult, this.#config.addressLength);
        const txHash = decodedPayload.txHash
        const getBootstrapTransactionTxPayload = await this.state.get(txHash.toString('hex'));

        if (getBootstrapTransactionTxPayload === null) {
            throw new V1ProtocolError(
                ResultCode.EXTERNAL_BOOTSTRAP_TX_MISSING,
                `External bootstrap is not registered as usual tx ${externalBootstrapResult.toString('hex')}: ${payload}`
            );
        }

        const decodedBootstrapDeployment = safeDecodeApplyOperation(getBootstrapTransactionTxPayload)

        // edge case
        if (!b4a.equals(decodedBootstrapDeployment.bdo.bs, payload.txo.bs)) {
            throw new V1ProtocolError(
                ResultCode.EXTERNAL_BOOTSTRAP_MISMATCH,
                `External bootstrap does not match the one in the transaction payload: ${decodedBootstrapDeployment.bdo.bs.toString('hex')} !== ${payload.txo.bs.toString('hex')}`
            );
        }
    }

}

export default PartialTransactionValidator;
