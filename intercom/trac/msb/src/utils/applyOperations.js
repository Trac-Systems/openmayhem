import {OperationType} from "./constants.js";

const isCoreAdmin = type => {
    return [
        OperationType.ADD_ADMIN,
        OperationType.DISABLE_INITIALIZATION,
    ].includes(type);
}

const isAdminControl = type => {
    return [
        OperationType.APPEND_WHITELIST,
        OperationType.ADD_INDEXER,
        OperationType.REMOVE_INDEXER,
        OperationType.BAN_VALIDATOR,
    ].includes(type);
}

const isRoleAccess = type => {
    return [
        OperationType.ADD_WRITER,
        OperationType.REMOVE_WRITER,
        OperationType.ADMIN_RECOVERY,
    ].includes(type);
}

const isTransaction = type => {
    return [
        OperationType.TX
    ].includes(type);
}

const isBootstrapDeployment = type => {
    return [
        OperationType.BOOTSTRAP_DEPLOYMENT
    ].includes(type);
}

const isTransfer = type => {
    return [
        OperationType.TRANSFER
    ].includes(type);
}

const isBalanceInitialization = type => {
    return [
        OperationType.BALANCE_INITIALIZATION
    ].includes(type);
}

const operationToPayload = type => {
    const fromTo = [
        {
            condition: isCoreAdmin,
            jsonPath: 'cao'
        },
        {
            condition: isAdminControl,
            jsonPath: 'aco'
        },
        {
            condition: isRoleAccess,
            jsonPath: 'rao'
        },
        {
            condition: isTransaction,
            jsonPath: 'txo'
        },
        {
            condition: isBootstrapDeployment,
            jsonPath: 'bdo'
        },
        {
            condition: isTransfer,
            jsonPath: 'tro'
        },
        {
            condition: isBalanceInitialization,
            jsonPath: 'bio'
        }
    ]
    const match = fromTo.find(entry => !!entry.condition(type))
    return match?.jsonPath
}

export {
    isCoreAdmin,
    isAdminControl,
    isRoleAccess,
    isTransaction,
    isBootstrapDeployment,
    operationToPayload,
    isTransfer,
    isBalanceInitialization
}