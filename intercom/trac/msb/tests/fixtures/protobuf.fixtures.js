import b4a from 'b4a';
import { OperationType } from '../../src/utils/constants.js';
import { addressToBuffer } from '../../src/core/state/utils/address.js';
import { config } from '../helpers/config.js';

const validTransferOperation = {
    type: OperationType.TRANSFER,
    address: addressToBuffer('trac123z3gfpr2epjwww7ntm3m6ud2fhmq0tvts27p2f5mx3qkecsutlqfys769', config.addressPrefix),
    tro: {
        tx: b4a.from('c59f70942febb1de32fcb59febe84560416265d39f39b48fae676592910a98f4', 'hex'),
        txv: b4a.from('eb59a3e756d1c9597e46b33bcea91e262f8f73e94c238bdf70854aa2e8c42608', 'hex'),
        to: addressToBuffer('trac1mqktwme8fvklrds4hlhfy6lhmsu9qgfn3c3kuhz7c5zwjt8rc3dqj9tx7h', config.addressPrefix),
        am: b4a.from('00000000000000015af1d78b58c40001', 'hex'),
        in: b4a.from('863fef21f5146553b0396b2ee1a93a8dbfce240411b71ccdcfc504504a6b9b50', 'hex'),
        is: b4a.from('06acd7faecd5159221259ebb1d7e98eccd7c6e2884de9de45097e6d9d8c37192602901c74dde6bb2f48f6f665edc84140627f6e9c42f774a0e9f55ef3b348e06', 'hex'),
        va: addressToBuffer('trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p', config.addressPrefix),
        vn: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        vs: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
    }
}

const validPartialTransferOperation = {
    type: OperationType.TRANSFER,
    address: addressToBuffer('trac123z3gfpr2epjwww7ntm3m6ud2fhmq0tvts27p2f5mx3qkecsutlqfys769'),
    tro: {
        tx: b4a.from('c59f70942febb1de32fcb59febe84560416265d39f39b48fae676592910a98f4', 'hex'),
        txv: b4a.from('eb59a3e756d1c9597e46b33bcea91e262f8f73e94c238bdf70854aa2e8c42608', 'hex'),
        to: addressToBuffer('trac1mqktwme8fvklrds4hlhfy6lhmsu9qgfn3c3kuhz7c5zwjt8rc3dqj9tx7h'),
        am: b4a.from('00000000000000015af1d78b58c40001', 'hex'),
        in: b4a.from('863fef21f5146553b0396b2ee1a93a8dbfce240411b71ccdcfc504504a6b9b50', 'hex'),
        is: b4a.from('06acd7faecd5159221259ebb1d7e98eccd7c6e2884de9de45097e6d9d8c37192602901c74dde6bb2f48f6f665edc84140627f6e9c42f774a0e9f55ef3b348e06', 'hex'),
        va: null,
        vn: null,
        vs: null
    }
}

const validTransactionOperation = {
    type: OperationType.TX,
    address: addressToBuffer('trac1c232xtkvyg08zyeurn7l0wrarc4y36fzq5vhcdsgkxe6hdpzuslsm63dw8', config.addressPrefix),
    txo: {
        tx: b4a.from('6fb7f6e7f6970477977080f2b46cc837d48605e67691d30bf7511a1417d17ed7', 'hex'),
        txv: b4a.from('6fb7f6e7f6970477977080f2b46cc837d48605e67691d30bf7511a1417d17ed7', 'hex'),
        iw: b4a.from('79ef7be837aa9fd8a446a120e1bc1e6bdd99fb5393dc4fa8299d9d5043a7cd98', 'hex'),
        ch: b4a.from('6ee7b29ce494875c1ea0dc0f9c2997d1aeeb8d21c67809950e145822989c8b2e', 'hex'),
        bs: b4a.from('f68bac445eee3d9276be46aef58328a543e4b7ecc2b5c98c387c1b3ca1a7e85d', 'hex'),
        mbs: b4a.from('5f3b9a6a516066de365e5e75a7ac0feb55ab7cd4a29facbb028a047fc3f3956e', 'hex'),
        in: b4a.from('8bcef53a043f42ac7c17344f0c0d56af5b335e412d4042124f27733911169e4f', 'hex'),
        is: b4a.from('d8626ea0552bf302921de3536e877796ef131368c9854119660c9c77a4196d4735d60bb87c6a89bbff7d5f8d72a70610d6ee73d62bc5144874cdf23f88e28a05', 'hex'),
        va: addressToBuffer('trac1xvqvlzx4w2q2pfqrmycew87kq4rv0q0cewxk68ddvddgk2xm09cqvpc4jc', config.addressPrefix),
        vn: b4a.from('9027192c6de13b683bc0c0fbcfe09c4e55d47c12c46b122d988f06c282a4be5e', 'hex'),
        vs: b4a.from('8fb8a3ba30e00c347bca5a8554c47e167f63b248c87e1ea5532eebbad1bc036184fe8872ff65a9e63acfee68d2213a187466c13ff6687d3ab57e5209abd4fb01', 'hex')
    }
};

const validPartialTransactionOperation = {
    type: OperationType.TX,
    address: addressToBuffer('trac1c232xtkvyg08zyeurn7l0wrarc4y36fzq5vhcdsgkxe6hdpzuslsm63dw8'),
    txo: {
        tx: b4a.from('6fb7f6e7f6970477977080f2b46cc837d48605e67691d30bf7511a1417d17ed7', 'hex'),
        txv: b4a.from('6fb7f6e7f6970477977080f2b46cc837d48605e67691d30bf7511a1417d17ed7', 'hex'),
        iw: b4a.from('79ef7be837aa9fd8a446a120e1bc1e6bdd99fb5393dc4fa8299d9d5043a7cd98', 'hex'),
        ch: b4a.from('6ee7b29ce494875c1ea0dc0f9c2997d1aeeb8d21c67809950e145822989c8b2e', 'hex'),
        bs: b4a.from('f68bac445eee3d9276be46aef58328a543e4b7ecc2b5c98c387c1b3ca1a7e85d', 'hex'),
        mbs: b4a.from('5f3b9a6a516066de365e5e75a7ac0feb55ab7cd4a29facbb028a047fc3f3956e', 'hex'),
        in: b4a.from('8bcef53a043f42ac7c17344f0c0d56af5b335e412d4042124f27733911169e4f', 'hex'),
        is: b4a.from('d8626ea0552bf302921de3536e877796ef131368c9854119660c9c77a4196d4735d60bb87c6a89bbff7d5f8d72a70610d6ee73d62bc5144874cdf23f88e28a05', 'hex'),
        va: null,
        vn: null,
        vs: null
    }
};

const validAddIndexer = {
    type: OperationType.ADD_INDEXER,
    address: addressToBuffer('trac1jnafrn8xl3c59s4ml6jusufp2vzm6egkyenrcqvd907j8c9v5j2qnx7wvt', config.addressPrefix),
    aco: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        ia: addressToBuffer('trac1xvqvlzx4w2q2pfqrmycew87kq4rv0q0cewxk68ddvddgk2xm09cqvpc4jc', config.addressPrefix),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
    }
};

const validRemoveIndexer = {
    type: OperationType.REMOVE_INDEXER,
    address: addressToBuffer('trac1jnafrn8xl3c59s4ml6jusufp2vzm6egkyenrcqvd907j8c9v5j2qnx7wvt', config.addressPrefix),
    aco: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        ia: addressToBuffer('trac1xvqvlzx4w2q2pfqrmycew87kq4rv0q0cewxk68ddvddgk2xm09cqvpc4jc', config.addressPrefix),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
    }
};

const validAppendWhitelist = {
    type: OperationType.APPEND_WHITELIST,
    address: addressToBuffer('trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p', config.addressPrefix),
    aco: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        ia: addressToBuffer('trac1xvqvlzx4w2q2pfqrmycew87kq4rv0q0cewxk68ddvddgk2xm09cqvpc4jc', config.addressPrefix),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
    }
};

const validBanValidator = {
    type: OperationType.BAN_VALIDATOR,
    address: addressToBuffer('trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p', config.addressPrefix),
    aco: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        ia: addressToBuffer('trac1xvqvlzx4w2q2pfqrmycew87kq4rv0q0cewxk68ddvddgk2xm09cqvpc4jc', config.addressPrefix),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
    }
};

const validAddAdmin = {
    type: OperationType.ADD_ADMIN,
    address: addressToBuffer('trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p', config.addressPrefix),
    cao: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
    }
};

const validPartialAddWriter = {
    type: OperationType.ADD_WRITER,
    address: addressToBuffer('trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p', config.addressPrefix),
    rao: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
        va: null,
        vn: null,
        vs: null
    }
};

const validCompleteAddWriter = {
    type: OperationType.ADD_WRITER,
    address: addressToBuffer('trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p', config.addressPrefix),
    rao: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
        va: addressToBuffer('trac1xvqvlzx4w2q2pfqrmycew87kq4rv0q0cewxk68ddvddgk2xm09cqvpc4jc', config.addressPrefix),
        vn: b4a.from('9027192c6de13b683bc0c0fbcfe09c4e55d47c12c46b122d988f06c282a4be5e', 'hex'),
        vs: b4a.from('8fb8a3ba30e00c347bca5a8554c47e167f63b248c87e1ea5532eebbad1bc036184fe8872ff65a9e63acfee68d2213a187466c13ff6687d3ab57e5209abd4fb01', 'hex')
    }
};

const validPartialRemoveWriter = {
    type: OperationType.REMOVE_WRITER,
    address: addressToBuffer('trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p', config.addressPrefix),
    rao: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
        va: null,
        vn: null,
        vs: null
    }
};

const validCompleteRemoveWriter = {
    type: OperationType.REMOVE_WRITER,
    address: addressToBuffer('trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p', config.addressPrefix),
    rao: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
        va: addressToBuffer('trac1xvqvlzx4w2q2pfqrmycew87kq4rv0q0cewxk68ddvddgk2xm09cqvpc4jc', config.addressPrefix),
        vn: b4a.from('9027192c6de13b683bc0c0fbcfe09c4e55d47c12c46b122d988f06c282a4be5e', 'hex'),
        vs: b4a.from('8fb8a3ba30e00c347bca5a8554c47e167f63b248c87e1ea5532eebbad1bc036184fe8872ff65a9e63acfee68d2213a187466c13ff6687d3ab57e5209abd4fb01', 'hex')
    }
};

const validPartialAdminRecovery = {
    type: OperationType.ADMIN_RECOVERY,
    address: addressToBuffer('trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p', config.addressPrefix),
    rao: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
        va: null,
        vn: null,
        vs: null

    }
};

const validCompleteAdminRecovery = {
    type: OperationType.ADMIN_RECOVERY,
    address: addressToBuffer('trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p', config.addressPrefix),
    rao: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex'),
        va: addressToBuffer('trac1xvqvlzx4w2q2pfqrmycew87kq4rv0q0cewxk68ddvddgk2xm09cqvpc4jc', config.addressPrefix),
        vn: b4a.from('9027192c6de13b683bc0c0fbcfe09c4e55d47c12c46b122d988f06c282a4be5e', 'hex'),
        vs: b4a.from('8fb8a3ba30e00c347bca5a8554c47e167f63b248c87e1ea5532eebbad1bc036184fe8872ff65a9e63acfee68d2213a187466c13ff6687d3ab57e5209abd4fb01', 'hex')
    }
};

const validBalanceInitOperation = {
    type: OperationType.BALANCE_INITIALIZATION,
    address: addressToBuffer('trac123z3gfpr2epjwww7ntm3m6ud2fhmq0tvts27p2f5mx3qkecsutlqfys769', config.addressPrefix),
    bio: {
        tx: b4a.from('c59f70942febb1de32fcb59febe84560416265d39f39b48fae676592910a98f4', 'hex'),
        txv: b4a.from('eb59a3e756d1c9597e46b33bcea91e262f8f73e94c238bdf70854aa2e8c42608', 'hex'),
        ia: addressToBuffer('trac1xf5sa6k8ykee2dmawpqawj0yjxfx42arx7924eh6k7edf72wrn7seev3pa', config.addressPrefix),
        am: b4a.from('00000000000000015af1d78b58c40000', 'hex'),
        in: b4a.from('863fef21f5146553b0396b2ee1a93a8dbfce240411b71ccdcfc504504a6b9b50', 'hex'),
        is: b4a.from('c8c21d99f30162c66d3ad0e4efcf59907356860f58bab8306e3648f6104a2ad02601f114406308fe592fb4ace547f5e4c35c52f47c70af6d24074b2c417dc405', 'hex')
    }
};

const validDisableInitialization = {
    type: OperationType.DISABLE_INITIALIZATION,
    address: addressToBuffer('trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p'),
    cao: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
    }
};

const validPartialBootstrapDeployment = {
    type: OperationType.BOOTSTRAP_DEPLOYMENT,
    address: addressToBuffer('trac1c232xtkvyg08zyeurn7l0wrarc4y36fzq5vhcdsgkxe6hdpzuslsm63dw8'),
    bdo: {
        tx: b4a.from('6fb7f6e7f6970477977080f2b46cc837d48605e67691d30bf7511a1417d17ed7', 'hex'),
        txv: b4a.from('eb59a3e756d1c9597e46b33bcea91e262f8f73e94c238bdf70854aa2e8c42608', 'hex'),
        bs: b4a.from('f68bac445eee3d9276be46aef58328a543e4b7ecc2b5c98c387c1b3ca1a7e85d', 'hex'),
        ic: b4a.from('79ef7be837aa9fd8a446a120e1bc1e6bdd99fb5393dc4fa8299d9d5043a7cd98', 'hex'),
        in: b4a.from('8bcef53a043f42ac7c17344f0c0d56af5b335e412d4042124f27733911169e4f', 'hex'),
        is: b4a.from('d8626ea0552bf302921de3536e877796ef131368c9854119660c9c77a4196d4735d60bb87c6a89bbff7d5f8d72a70610d6ee73d62bc5144874cdf23f88e28a05', 'hex'),
        va: null,
        vn: null,
        vs: null
    }
};

const validCompleteBootstrapDeployment = {
    type: OperationType.BOOTSTRAP_DEPLOYMENT,
    address: addressToBuffer('trac1c232xtkvyg08zyeurn7l0wrarc4y36fzq5vhcdsgkxe6hdpzuslsm63dw8'),
    bdo: {
        tx: b4a.from('6fb7f6e7f6970477977080f2b46cc837d48605e67691d30bf7511a1417d17ed7', 'hex'),
        txv: b4a.from('eb59a3e756d1c9597e46b33bcea91e262f8f73e94c238bdf70854aa2e8c42608', 'hex'),
        bs: b4a.from('f68bac445eee3d9276be46aef58328a543e4b7ecc2b5c98c387c1b3ca1a7e85d', 'hex'),
        ic: b4a.from('79ef7be837aa9fd8a446a120e1bc1e6bdd99fb5393dc4fa8299d9d5043a7cd98', 'hex'),
        in: b4a.from('8bcef53a043f42ac7c17344f0c0d56af5b335e412d4042124f27733911169e4f', 'hex'),
        is: b4a.from('d8626ea0552bf302921de3536e877796ef131368c9854119660c9c77a4196d4735d60bb87c6a89bbff7d5f8d72a70610d6ee73d62bc5144874cdf23f88e28a05', 'hex'),
        va: addressToBuffer('trac1xvqvlzx4w2q2pfqrmycew87kq4rv0q0cewxk68ddvddgk2xm09cqvpc4jc'),
        vn: b4a.from('9027192c6de13b683bc0c0fbcfe09c4e55d47c12c46b122d988f06c282a4be5e', 'hex'),
        vs: b4a.from('8fb8a3ba30e00c347bca5a8554c47e167f63b248c87e1ea5532eebbad1bc036184fe8872ff65a9e63acfee68d2213a187466c13ff6687d3ab57e5209abd4fb01', 'hex')
    }
};


const invalidPayloads = [
    null,
    undefined,
    true,
    false,
    0,
    1,
    123,
    NaN,
    Infinity,
    -1,
    '',
    'string',
    Symbol('sym'),
    BigInt(10),
    [],
    {},
    () => {
    },
    new Date(),
    { foo: 'bar' },
    { type: '1', key: b4a.from('01', 'hex') },
    { type: 1, key: null },
    { type: 1 },
    { key: b4a.from('01', 'hex') },
    { type: 5, key: [] },
    { type: 1, key: b4a.from('01', 'hex'), bko: null },
    { type: 1, key: b4a.from('01', 'hex'), bko: 123 },
    { type: 1, key: b4a.from('01', 'hex'), bko: b4a.from('01', 'hex'), data: 'string' },
    { type: 1, key: b4a.from('01', 'hex'), bko: b4a.from('01', 'hex'), data: null },
    { type: 1, key: b4a.from('01', 'hex'), memo: 123 },
    { type: 1, key: 'string' },
    { type: 1, key: {}, bko: {} },
    { type: 'foo', key: [], bko: 'bar' },
    b4a.from([0xFF, 0xAA, 0x55, 0x00, 0x99]),
    (() => {
        const a = {};
        a.self = a;
        return a;
    })(),
    Object.create(null),
    new Map(),
    new Set(),
    new Float64Array([1.1, 2.2]),
    new Int16Array([1, 2, 3]),
    { type: 9999, key: b4a.from('01', 'hex') },
    { type: 1, key: 'not-a-buffer' },
    Number.MAX_SAFE_INTEGER + 1,
    -Number.MAX_SAFE_INTEGER - 1,
    {
        type: 1, key: b4a.from('01', 'hex'), callback: () => {
        }
    },
    b4a.alloc(0),
    b4a.alloc(1, 0x00),
    b4a.alloc(1, 0xFF),
    b4a.alloc(10, 0x01),
    b4a.from([0, 255, 127, 128, 64]),
    b4a.from('not serialized data?'),
    b4a.from(Array.from({ length: 256 }, (_, i) => i)),
    b4a.from(Array.from({ length: 1024 * 1024 }, () => Math.floor(Math.random() * 256))), // 1MB
];

const invalidPayloadWithMultipleOneOfKeys = {
    type: OperationType.ADD_WRITER,
    address: addressToBuffer('trac18qq7h503y3326v6msgvq0jwc0e8jp4t4q53z9p9jvd98arj7mtpqfac04p', config.addressPrefix),
    cao: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
    },
    rao: {
        tx: b4a.from('1bd4f96adeffba9c04943a82993c5b19660c3a5f572620d82a67464f381640e2', 'hex'),
        txv: b4a.from('f24e61cf7941256b080be2133bccb520414c78021215edfcb781622da526c414', 'hex'),
        iw: b4a.from('71c53657a8738b48772f0940398d4f4b01dc56cb32cd2fd84c30359f0cbb08f1', 'hex'),
        in: b4a.from('0ad7fe36a35a27ea4df932b800200823a97d4db31bca247f43ad7523b0493645', 'hex'),
        is: b4a.from('5b534be7a374148962c271d194c26cf5b1ad705ab218a87709a33fe74f9d1b811772447c939b17b2f803e3da7648f49b666b929fbb20e458ced952f147162c08', 'hex')
    }
};

export default {
    validTransactionOperation,
    validPartialTransactionOperation,
    validAddIndexer,
    validRemoveIndexer,
    validAppendWhitelist,
    validBanValidator,
    validAddAdmin,
    validDisableInitialization,
    validTransferOperation,
    validPartialTransferOperation,
    validBalanceInitOperation,
    validPartialAddWriter,
    validCompleteAddWriter,
    validCompleteAdminRecovery,
    validPartialAdminRecovery,
    validCompleteRemoveWriter,
    validPartialRemoveWriter,
    validPartialBootstrapDeployment,
    validCompleteBootstrapDeployment,
    invalidPayloads,
    invalidPayloadWithMultipleOneOfKeys
};
