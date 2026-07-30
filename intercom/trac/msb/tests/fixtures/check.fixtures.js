import b4a from 'b4a';

import { addressToBuffer } from '../../src/core/state/utils/address.js';
import {
    HASH_BYTE_LENGTH,
    SIGNATURE_BYTE_LENGTH,
    WRITER_BYTE_LENGTH,
    BOOTSTRAP_BYTE_LENGTH,
    NONCE_BYTE_LENGTH,
    OperationType, AMOUNT_BYTE_LENGTH
} from '../../src/utils/constants.js';
import { config } from '../helpers/config.js'
import { asAddress } from '../helpers/address.js';

export const TRO = {
    valid_partial_transfer: {
        type: OperationType.TRANSFER,
        address: addressToBuffer(asAddress('544514242356432739de9af71deb8d526fb03d6c5c15e0a934d9a20b6710e2fe'), config.addressPrefix),
        tro: {
            tx: b4a.from('c59f70942febb1de32fcb59febe84560416265d39f39b48fae676592910a98f4', 'hex'),
            txv: b4a.from('eb59a3e756d1c9597e46b33bcea91e262f8f73e94c238bdf70854aa2e8c42608', 'hex'),
            in: b4a.from('863fef21f5146553b0396b2ee1a93a8dbfce240411b71ccdcfc504504a6b9b50', 'hex'),
            to: addressToBuffer(asAddress('d82cb76f274b2df1b615bfee926bf7dc385021338e236e5c5ec504e92ce3c45a'), config.addressPrefix),
            am: b4a.from('00000000000000015af1d78b58c40001', 'hex'),
            is: b4a.from('06acd7faecd5159221259ebb1d7e98eccd7c6e2884de9de45097e6d9d8c37192602901c74dde6bb2f48f6f665edc84140627f6e9c42f774a0e9f55ef3b348e06', 'hex')
        }
    },
    valid_complete_transfer: {
        type: OperationType.TRANSFER,
        address: addressToBuffer(asAddress('544514242356432739de9af71deb8d526fb03d6c5c15e0a934d9a20b6710e2fe'), config.addressPrefix),
        tro: {
            tx: b4a.from('c59f70942febb1de32fcb59febe84560416265d39f39b48fae676592910a98f4', 'hex'),
            txv: b4a.from('eb59a3e756d1c9597e46b33bcea91e262f8f73e94c238bdf70854aa2e8c42608', 'hex'),
            in: b4a.from('863fef21f5146553b0396b2ee1a93a8dbfce240411b71ccdcfc504504a6b9b50', 'hex'),
            to: addressToBuffer(asAddress('d82cb76f274b2df1b615bfee926bf7dc385021338e236e5c5ec504e92ce3c45a'), config.addressPrefix),
            am: b4a.from('00000000000000015af1d78b58c40001', 'hex'),
            is: b4a.from('06acd7faecd5159221259ebb1d7e98eccd7c6e2884de9de45097e6d9d8c37192602901c74dde6bb2f48f6f665edc84140627f6e9c42f774a0e9f55ef3b348e06', 'hex'),
            vn: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            vs: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
            va: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        }
    },
    top_fields_transfer: ['type', 'address', 'tro'],
    partial_transfer_value_fields: ['tx', 'txv', 'in', 'to', 'am', 'is'],
    complete_transfer_value_fields: ['tx', 'txv', 'in', 'to', 'am', 'is', 'va', 'vn', 'vs'],
    required_length_of_fields_for_partial_transfer: {
        tx: HASH_BYTE_LENGTH,
        txv: HASH_BYTE_LENGTH,
        in: NONCE_BYTE_LENGTH,
        to: config.addressLength,
        am: AMOUNT_BYTE_LENGTH,
    },
    required_length_of_fields_for_complete_transfer: {
        tx: HASH_BYTE_LENGTH,
        txv: HASH_BYTE_LENGTH,
        in: NONCE_BYTE_LENGTH,
        to: config.addressLength,
        am: AMOUNT_BYTE_LENGTH,
        is: SIGNATURE_BYTE_LENGTH,
        vn: NONCE_BYTE_LENGTH,
        vs: SIGNATURE_BYTE_LENGTH,
        va: config.addressLength
    },

}

export const BDO = {
    valid_partial_bootstrap_deployment: {
        type: OperationType.BOOTSTRAP_DEPLOYMENT,
        address: addressToBuffer(asAddress('c643a93b097a99b766c3ac1f05ac7d033d1d30f34a916ee642529f115d3e97b1'), config.addressPrefix),
        bdo: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            bs: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            ic: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
        }
    },

    valid_complete_bootstrap_deployment: {
        type: OperationType.BOOTSTRAP_DEPLOYMENT,
        address: addressToBuffer(asAddress('c643a93b097a99b766c3ac1f05ac7d033d1d30f34a916ee642529f115d3e97b1'), config.addressPrefix),
        bdo: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            bs: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            ic: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
            va: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
            vn: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            vs: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
        }
    },

    top_fields_bootstrap_deployment: ['type', 'address', 'bdo'],
    complete_bootstrap_deployment_value_fields: ['tx', 'txv', 'bs', 'in', 'is', 'vn', 'vs', 'va'],
    partial_bootstrap_deployment_value_fields: ['tx', 'txv', 'bs', 'in', 'is'],
    required_length_of_fields_for_complete_bootstrap_deployment: {
        tx: HASH_BYTE_LENGTH,
        txv: HASH_BYTE_LENGTH,
        bs: BOOTSTRAP_BYTE_LENGTH,
        in: NONCE_BYTE_LENGTH,
        is: SIGNATURE_BYTE_LENGTH,
        vn: NONCE_BYTE_LENGTH,
        vs: SIGNATURE_BYTE_LENGTH,
        va: config.addressLength
    },
    required_length_of_fields_for_partial_bootstrap_deployment: {
        tx: HASH_BYTE_LENGTH,
        txv: HASH_BYTE_LENGTH,
        bs: BOOTSTRAP_BYTE_LENGTH,
        in: NONCE_BYTE_LENGTH,
        is: SIGNATURE_BYTE_LENGTH,
    }
}

export const TXO = {
    valid_complete_transaction_operation: {
        type: OperationType.TX,
        address: addressToBuffer(asAddress('c2a2a32ecc221e71133c1cfdf7b87d1e2a48e92205197c3608b1b3abb422e43f'), config.addressPrefix),
        txo: {
            tx: b4a.from('6fb7f6e7f6970477977080f2b46cc837d48605e67691d30bf7511a1417d17ed7', 'hex'),
            txv: b4a.from('6fb7f6e7f6970477977080f2b46cc837d48605e67691d30bf7511a1417d17ed7', 'hex'),
            iw: b4a.from('79ef7be837aa9fd8a446a120e1bc1e6bdd99fb5393dc4fa8299d9d5043a7cd98', 'hex'),
            in: b4a.from('8bcef53a043f42ac7c17344f0c0d56af5b335e412d4042124f27733911169e4f', 'hex'),
            ch: b4a.from('6ee7b29ce494875c1ea0dc0f9c2997d1aeeb8d21c67809950e145822989c8b2e', 'hex'),
            is: b4a.from('d8626ea0552bf302921de3536e877796ef131368c9854119660c9c77a4196d4735d60bb87c6a89bbff7d5f8d72a70610d6ee73d62bc5144874cdf23f88e28a05', 'hex'),
            bs: b4a.from('f68bac445eee3d9276be46aef58328a543e4b7ecc2b5c98c387c1b3ca1a7e85d', 'hex'),
            mbs: b4a.from('5f3b9a6a516066de365e5e75a7ac0feb55ab7cd4a29facbb028a047fc3f3956e', 'hex'),
            vn: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            vs: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
            va: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        }
    },
    valid_partial_transaction_operation: {
        type: OperationType.TX,
        address: addressToBuffer(asAddress('c2a2a32ecc221e71133c1cfdf7b87d1e2a48e92205197c3608b1b3abb422e43f'), config.addressPrefix),
        txo: {
            tx: b4a.from('6fb7f6e7f6970477977080f2b46cc837d48605e67691d30bf7511a1417d17ed7', 'hex'),
            txv: b4a.from('6fb7f6e7f6970477977080f2b46cc837d48605e67691d30bf7511a1417d17ed7', 'hex'),
            iw: b4a.from('79ef7be837aa9fd8a446a120e1bc1e6bdd99fb5393dc4fa8299d9d5043a7cd98', 'hex'),
            in: b4a.from('8bcef53a043f42ac7c17344f0c0d56af5b335e412d4042124f27733911169e4f', 'hex'),
            ch: b4a.from('6ee7b29ce494875c1ea0dc0f9c2997d1aeeb8d21c67809950e145822989c8b2e', 'hex'),
            is: b4a.from('d8626ea0552bf302921de3536e877796ef131368c9854119660c9c77a4196d4735d60bb87c6a89bbff7d5f8d72a70610d6ee73d62bc5144874cdf23f88e28a05', 'hex'),
            bs: b4a.from('f68bac445eee3d9276be46aef58328a543e4b7ecc2b5c98c387c1b3ca1a7e85d', 'hex'),
            mbs: b4a.from('5f3b9a6a516066de365e5e75a7ac0feb55ab7cd4a29facbb028a047fc3f3956e', 'hex'),

        }
    },
    top_fields_transaction_operation: ['type', 'address', 'txo'],
    complete_transaction_operation_value_fields: ['tx', 'txv', 'iw', 'in', 'ch', 'is', 'bs', 'mbs', 'vs', 'vn'],
    partial_transaction_operation_value_fields: ['tx', 'txv', 'iw', 'in', 'ch', 'is', 'bs', 'mbs'],
    required_length_of_fields_for_complete_transaction_operation: {
        tx: HASH_BYTE_LENGTH,
        txv: HASH_BYTE_LENGTH,
        iw: WRITER_BYTE_LENGTH,
        in: NONCE_BYTE_LENGTH,
        ch: HASH_BYTE_LENGTH,
        is: SIGNATURE_BYTE_LENGTH,
        bs: BOOTSTRAP_BYTE_LENGTH,
        mbs: BOOTSTRAP_BYTE_LENGTH,
        vn: NONCE_BYTE_LENGTH,
        vs: SIGNATURE_BYTE_LENGTH,
        va: config.addressLength

    },
    required_length_of_fields_for_partial_transaction_operation: {
        tx: HASH_BYTE_LENGTH,
        txv: HASH_BYTE_LENGTH,
        iw: WRITER_BYTE_LENGTH,
        in: NONCE_BYTE_LENGTH,
        ch: HASH_BYTE_LENGTH,
        is: SIGNATURE_BYTE_LENGTH,
        bs: BOOTSTRAP_BYTE_LENGTH,
        mbs: BOOTSTRAP_BYTE_LENGTH,
    }

}

export const CAO = {
    validAddAdminOperation: {
        type: OperationType.ADD_ADMIN,
        address: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        cao: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
        }
    },
    validDisableInitializationOperation: {
        type: OperationType.DISABLE_INITIALIZATION,
        address: addressToBuffer(asAddress('80233910c26abc3810e2c5a0862a5bc43e56fe1bb7dce00f973154de2611c676'), config.addressPrefix),
        cao: {
            tx: b4a.from('0fc518b31505d163a696555df8dceae415032773f85e578a9a1810ad5c99cf0c', 'hex'),
            txv: b4a.from('dd6b3809673cbca08ee60c32971e9ed9d39fb962c53ab8ef49cd6b467d6977f3', 'hex'),
            iw: b4a.from('b65d816367a32c723f8d221528a3a3cf986d0b1ac8dbb700b5bbbe322563b3ba', 'hex'),
            in: b4a.from('e46559a01f1f38f2305e59888d55c23b221fd8fc89fa43ec2f9cbf888b8b5fae', 'hex'),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
        }
    },
    topFieldsCoreAdmin: ['type', 'address', 'cao'],
    coreAdminOperationValuesFields: ['tx', 'txv', 'iw', 'in', 'is'],
    requiredLengthOfFieldsForCoreAdmin: {
        tx: HASH_BYTE_LENGTH,
        txv: HASH_BYTE_LENGTH,
        iw: WRITER_BYTE_LENGTH,
        in: NONCE_BYTE_LENGTH,
        is: SIGNATURE_BYTE_LENGTH
    }
};

export const ACO = {
    validAppendWhitelistOperation: {
        type: OperationType.APPEND_WHITELIST,
        address: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        aco: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            ia: addressToBuffer(asAddress('3300cf88d57280a0a403d931971fd60546c781f8cb8d6d1dad635a8b28db7970'), config.addressPrefix),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
        }
    },

    validAddIndexerOperation: {
        type: OperationType.ADD_INDEXER,
        address: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        aco: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            ia: addressToBuffer(asAddress('3300cf88d57280a0a403d931971fd60546c781f8cb8d6d1dad635a8b28db7970'), config.addressPrefix),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
        }
    },

    validRemoveIndexerOperation: {
        type: OperationType.REMOVE_INDEXER,
        address: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        aco: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            ia: addressToBuffer(asAddress('3300cf88d57280a0a403d931971fd60546c781f8cb8d6d1dad635a8b28db7970'), config.addressPrefix),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
        }
    },

    validBanValidatorOperation: {
        type: OperationType.BAN_VALIDATOR,
        address: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        aco: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            ia: addressToBuffer(asAddress('3300cf88d57280a0a403d931971fd60546c781f8cb8d6d1dad635a8b28db7970'), config.addressPrefix),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
        }
    },

    topFieldsAdminControl: ['type', 'address', 'aco'],
    adminControlValueFields: ['tx', 'txv', 'in', 'ia', 'is'],
    requiredLengthOfFieldsForAdminControl: {
        tx: HASH_BYTE_LENGTH,
        txv: HASH_BYTE_LENGTH,
        in: NONCE_BYTE_LENGTH,
        ia: config.addressLength,
        is: SIGNATURE_BYTE_LENGTH
    }
};

export const RAO = {
    valid_complete_add_writer: {
        type: OperationType.ADD_WRITER,
        address: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        rao: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
            va: addressToBuffer(asAddress('3300cf88d57280a0a403d931971fd60546c781f8cb8d6d1dad635a8b28db7970'), config.addressPrefix),
            vn: b4a.from('9027192c6de13b683bc0c0fbcfe09c4e55d47c12c46b122d988f06c282a4be5e', 'hex'),
            vs: b4a.from('8fb8a3ba30e00c347bca5a8554c47e167f63b248c87e1ea5532eebbad1bc036184fe8872ff65a9e63acfee68d2213a187466c13ff6687d3ab57e5209abd4fb01', 'hex')
        }
    },
    valid_partial_add_writer: {
        type: OperationType.ADD_WRITER,
        address: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        rao: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')

        }
    },

    valid_complete_remove_writer: {
        type: OperationType.REMOVE_WRITER,
        address: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        rao: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
            va: addressToBuffer(asAddress('3300cf88d57280a0a403d931971fd60546c781f8cb8d6d1dad635a8b28db7970'), config.addressPrefix),
            vn: b4a.from('9027192c6de13b683bc0c0fbcfe09c4e55d47c12c46b122d988f06c282a4be5e', 'hex'),
            vs: b4a.from('8fb8a3ba30e00c347bca5a8554c47e167f63b248c87e1ea5532eebbad1bc036184fe8872ff65a9e63acfee68d2213a187466c13ff6687d3ab57e5209abd4fb01', 'hex')
        }
    },
    valid_partial_remove_writer: {
        type: OperationType.REMOVE_WRITER,
        address: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        rao: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
        }
    },
    valid_complete_admin_recovery: {
        type: OperationType.ADMIN_RECOVERY,
        address: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        rao: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
            va: addressToBuffer(asAddress('3300cf88d57280a0a403d931971fd60546c781f8cb8d6d1dad635a8b28db7970'), config.addressPrefix),
            vn: b4a.from('9027192c6de13b683bc0c0fbcfe09c4e55d47c12c46b122d988f06c282a4be5e', 'hex'),
            vs: b4a.from('8fb8a3ba30e00c347bca5a8554c47e167f63b248c87e1ea5532eebbad1bc036184fe8872ff65a9e63acfee68d2213a187466c13ff6687d3ab57e5209abd4fb01', 'hex')
        }
    },
    valid_partial_admin_recovery: {
        type: OperationType.ADMIN_RECOVERY,
        address: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        rao: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
        }
    },

    top_fields_role_access: ['type', 'address', 'rao'],
    complete_role_access_value_fields: ['tx', 'txv', 'iw', 'in', 'is', 'va', 'vn', 'vs'],
    partial_role_access_value_fields: ['tx', 'txv', 'iw', 'in', 'is'],
    required_length_of_fields_for_complete_role_access: {
        tx: HASH_BYTE_LENGTH,
        txv: HASH_BYTE_LENGTH,
        iw: WRITER_BYTE_LENGTH,
        in: NONCE_BYTE_LENGTH,
        is: SIGNATURE_BYTE_LENGTH,
        va: config.addressLength,
        vn: NONCE_BYTE_LENGTH,
        vs: SIGNATURE_BYTE_LENGTH
    },
    required_length_of_fields_for_partial_role_access: {
        tx: HASH_BYTE_LENGTH,
        txv: HASH_BYTE_LENGTH,
        iw: WRITER_BYTE_LENGTH,
        in: NONCE_BYTE_LENGTH,
        is: SIGNATURE_BYTE_LENGTH,
    }


};

export const BIO = {
    valid_balance_initialization_operation: {
        type: OperationType.BALANCE_INITIALIZATION,
        address: addressToBuffer(asAddress('3801ebd1f12462ad335b821807c9d87e4f20d57505222284b2634a7e8e5edac2'), config.addressPrefix),
        bio: {
            tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
            txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
            in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
            ia: addressToBuffer(asAddress('3300cf88d57280a0a403d931971fd60546c781f8cb8d6d1dad635a8b28db7970'), config.addressPrefix),
            am: b4a.from('00000000000000015af1d78b58c40001', 'hex'),
            is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
        }
    },

    top_fields_balance_initialization: ['type', 'address', 'bio'],
    balance_initialization_operation_value_fields: ['tx', 'txv', 'in', 'ia', 'am', 'is'],
    required_length_of_fields_for_balance_initialization: {
        tx: HASH_BYTE_LENGTH,
        txv: HASH_BYTE_LENGTH,
        in: NONCE_BYTE_LENGTH,
        ia: config.addressLength,
        am: AMOUNT_BYTE_LENGTH,
        is: SIGNATURE_BYTE_LENGTH
    }
}

export const partial_operation_value_type = ['bdo', 'tto', 'txo', 'rao']

export const not_allowed_data_types = [
    997,
    true,
    null,
    undefined,
    Infinity,
    {},
    [],
    () => { },
    "string",
    BigInt(997),
    new Date(),
    NaN,
    new Map(),
    new Set(),
    /abc/,
    new Error('fail'),
    Promise.resolve(),
    new Uint8Array([1, 2, 3]),
    new Float32Array([1.1, 2.2]),
    b4a.alloc(10)
];
