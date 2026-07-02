import { test } from 'brittle';
import b4a from 'b4a';

import { config } from '../../../helpers/config.js';
import { randomAddress } from '../../state/stateTestUtils.js';
import { errorMessageIncludes } from '../../../helpers/regexHelper.js';
import { OperationType } from '../../../../src/utils/constants.js';
import {
    normalizeBootstrapDeploymentOperation,
    normalizeDecodedPayloadForJson,
    normalizeRoleAccessOperation,
    normalizeTransactionOperation,
    normalizeTransferOperation
} from '../../../../src/utils/normalizers.js';
import { addressToBuffer } from '../../../../src/core/state/utils/address.js';
import { bigIntTo16ByteBuffer } from '../../../../src/utils/amountSerialization.js';

const hex = (value, bytes) => value.repeat(bytes);
const toBuffer = value => b4a.from(value, 'hex');

test('normalizeTransferOperation normalizes hex strings and addresses', t => {
    const sender = randomAddress(config.addressPrefix);
    const recipient = randomAddress(config.addressPrefix);
    const tx = hex('11', 32);
    const txv = hex('22', 32);
    const nonce = hex('33', 32);
    const amount = hex('44', 16);
    const signature = hex('55', 64);

    const payload = {
        type: OperationType.TRANSFER,
        address: sender,
        tro: {
            tx,
            txv,
            in: nonce,
            to: recipient,
            am: amount,
            is: signature
        }
    };

    const normalized = normalizeTransferOperation(payload, config);

    t.is(normalized.type, OperationType.TRANSFER);
    t.ok(b4a.equals(normalized.address, addressToBuffer(sender, config.addressPrefix)));
    t.ok(b4a.equals(normalized.tro.tx, toBuffer(tx)));
    t.ok(b4a.equals(normalized.tro.txv, toBuffer(txv)));
    t.ok(b4a.equals(normalized.tro.in, toBuffer(nonce)));
    t.ok(b4a.equals(normalized.tro.to, addressToBuffer(recipient, config.addressPrefix)));
    t.ok(b4a.equals(normalized.tro.am, toBuffer(amount)));
    t.ok(b4a.equals(normalized.tro.is, toBuffer(signature)));
});

test('normalizeTransferOperation accepts buffer inputs', t => {
    const sender = randomAddress(config.addressPrefix);
    const recipient = randomAddress(config.addressPrefix);
    const tx = toBuffer(hex('aa', 32));
    const txv = toBuffer(hex('bb', 32));
    const nonce = toBuffer(hex('cc', 32));
    const amount = toBuffer(hex('dd', 16));
    const signature = toBuffer(hex('ee', 64));

    const payload = {
        type: OperationType.TRANSFER,
        address: sender,
        tro: {
            tx,
            txv,
            in: nonce,
            to: recipient,
            am: amount,
            is: signature
        }
    };

    const normalized = normalizeTransferOperation(payload, config);
    t.ok(b4a.equals(normalized.tro.tx, tx));
    t.ok(b4a.equals(normalized.tro.txv, txv));
    t.ok(b4a.equals(normalized.tro.in, nonce));
    t.ok(b4a.equals(normalized.tro.am, amount));
    t.ok(b4a.equals(normalized.tro.is, signature));
});

test('normalizeTransferOperation throws on missing payload fields', t => {
    t.exception(
        () => normalizeTransferOperation({ type: OperationType.TRANSFER }, config),
        errorMessageIncludes('Invalid payload for transfer operation normalization.')
    );

    t.exception(
        () => normalizeTransferOperation({ type: OperationType.TX, address: 'x', tro: {} }, config),
        errorMessageIncludes('Missing required fields in transfer operation payload.')
    );

    const sender = randomAddress(config.addressPrefix);
    const payload = {
        type: OperationType.TRANSFER,
        address: sender,
        tro: {
            tx: hex('11', 32),
            txv: hex('22', 32),
            in: hex('33', 32),
            to: randomAddress(config.addressPrefix),
            am: hex('44', 16)
        }
    };
    t.exception(
        () => normalizeTransferOperation(payload, config),
        errorMessageIncludes('Missing required fields in transfer operation payload.')
    );
});

test('normalizeTransferOperation throws on invalid hex string', t => {
    const sender = randomAddress(config.addressPrefix);
    const payload = {
        type: OperationType.TRANSFER,
        address: sender,
        tro: {
            tx: 'zz',
            txv: hex('22', 32),
            in: hex('33', 32),
            to: randomAddress(config.addressPrefix),
            am: hex('44', 16),
            is: hex('55', 64)
        }
    };
    t.exception(
        () => normalizeTransferOperation(payload, config),
        errorMessageIncludes('Invalid hex string')
    );
});

test('normalizeTransactionOperation normalizes hex strings and addresses', t => {
    const sender = randomAddress(config.addressPrefix);
    const tx = hex('11', 32);
    const txv = hex('22', 32);
    const writerKey = hex('33', 32);
    const contentHash = hex('44', 32);
    const bootstrap = hex('55', 32);
    const msbBootstrap = hex('66', 32);
    const nonce = hex('77', 32);
    const signature = hex('88', 64);

    const payload = {
        type: OperationType.TX,
        address: sender,
        txo: {
            tx,
            txv,
            iw: writerKey,
            ch: contentHash,
            bs: bootstrap,
            mbs: msbBootstrap,
            in: nonce,
            is: signature
        }
    };

    const normalized = normalizeTransactionOperation(payload, config);
    t.is(normalized.type, OperationType.TX);
    t.ok(b4a.equals(normalized.address, addressToBuffer(sender, config.addressPrefix)));
    t.ok(b4a.equals(normalized.txo.tx, toBuffer(tx)));
    t.ok(b4a.equals(normalized.txo.txv, toBuffer(txv)));
    t.ok(b4a.equals(normalized.txo.iw, toBuffer(writerKey)));
    t.ok(b4a.equals(normalized.txo.ch, toBuffer(contentHash)));
    t.ok(b4a.equals(normalized.txo.bs, toBuffer(bootstrap)));
    t.ok(b4a.equals(normalized.txo.mbs, toBuffer(msbBootstrap)));
    t.ok(b4a.equals(normalized.txo.in, toBuffer(nonce)));
    t.ok(b4a.equals(normalized.txo.is, toBuffer(signature)));
});

test('normalizeTransactionOperation accepts buffer inputs', t => {
    const sender = randomAddress(config.addressPrefix);
    const tx = toBuffer(hex('aa', 32));
    const txv = toBuffer(hex('bb', 32));
    const writerKey = toBuffer(hex('cc', 32));
    const contentHash = toBuffer(hex('dd', 32));
    const bootstrap = toBuffer(hex('ee', 32));
    const msbBootstrap = toBuffer(hex('ff', 32));
    const nonce = toBuffer(hex('12', 32));
    const signature = toBuffer(hex('34', 64));

    const payload = {
        type: OperationType.TX,
        address: sender,
        txo: {
            tx,
            txv,
            iw: writerKey,
            ch: contentHash,
            bs: bootstrap,
            mbs: msbBootstrap,
            in: nonce,
            is: signature
        }
    };

    const normalized = normalizeTransactionOperation(payload, config);
    t.ok(b4a.equals(normalized.txo.tx, tx));
    t.ok(b4a.equals(normalized.txo.txv, txv));
    t.ok(b4a.equals(normalized.txo.iw, writerKey));
    t.ok(b4a.equals(normalized.txo.ch, contentHash));
    t.ok(b4a.equals(normalized.txo.bs, bootstrap));
    t.ok(b4a.equals(normalized.txo.mbs, msbBootstrap));
    t.ok(b4a.equals(normalized.txo.in, nonce));
    t.ok(b4a.equals(normalized.txo.is, signature));
});

test('normalizeTransactionOperation throws on missing payload fields', t => {
    t.exception(
        () => normalizeTransactionOperation({ type: OperationType.TX }, config),
        errorMessageIncludes('Invalid payload for transaction operation normalization.')
    );

    t.exception(
        () => normalizeTransactionOperation({ type: OperationType.TRANSFER, address: 'x', txo: {} }, config),
        errorMessageIncludes('Missing required fields in transaction operation payload.')
    );

    const sender = randomAddress(config.addressPrefix);
    const payload = {
        type: OperationType.TX,
        address: sender,
        txo: {
            tx: hex('11', 32),
            txv: hex('22', 32),
            iw: hex('33', 32),
            ch: hex('44', 32),
            bs: hex('55', 32),
            mbs: hex('66', 32),
            in: hex('77', 32)
        }
    };
    t.exception(
        () => normalizeTransactionOperation(payload, config),
        errorMessageIncludes('Missing required fields in transaction operation payload.')
    );
});

test('normalizeTransactionOperation throws on invalid hex string', t => {
    const sender = randomAddress(config.addressPrefix);
    const payload = {
        type: OperationType.TX,
        address: sender,
        txo: {
            tx: 'zz',
            txv: hex('22', 32),
            iw: hex('33', 32),
            ch: hex('44', 32),
            bs: hex('55', 32),
            mbs: hex('66', 32),
            in: hex('77', 32),
            is: hex('88', 64)
        }
    };
    t.exception(
        () => normalizeTransactionOperation(payload, config),
        errorMessageIncludes('Invalid hex string')
    );
});

test('normalizeRoleAccessOperation normalizes hex strings and addresses', t => {
    const sender = randomAddress(config.addressPrefix);
    const payload = {
        type: OperationType.ADD_WRITER,
        address: sender,
        rao: {
            tx: hex('11', 32),
            txv: hex('22', 32),
            iw: hex('33', 32),
            in: hex('44', 32),
            is: hex('55', 64)
        }
    };

    const normalized = normalizeRoleAccessOperation(payload, config);
    t.is(normalized.type, OperationType.ADD_WRITER);
    t.ok(b4a.equals(normalized.address, addressToBuffer(sender, config.addressPrefix)));
    t.ok(b4a.equals(normalized.rao.tx, toBuffer(hex('11', 32))));
    t.ok(b4a.equals(normalized.rao.txv, toBuffer(hex('22', 32))));
    t.ok(b4a.equals(normalized.rao.iw, toBuffer(hex('33', 32))));
    t.ok(b4a.equals(normalized.rao.in, toBuffer(hex('44', 32))));
    t.ok(b4a.equals(normalized.rao.is, toBuffer(hex('55', 64))));
});

test('normalizeRoleAccessOperation accepts buffer inputs', t => {
    const sender = randomAddress(config.addressPrefix);
    const tx = toBuffer(hex('aa', 32));
    const txv = toBuffer(hex('bb', 32));
    const writerKey = toBuffer(hex('cc', 32));
    const nonce = toBuffer(hex('dd', 32));
    const signature = toBuffer(hex('ee', 64));

    const payload = {
        type: OperationType.REMOVE_WRITER,
        address: sender,
        rao: {
            tx,
            txv,
            iw: writerKey,
            in: nonce,
            is: signature
        }
    };

    const normalized = normalizeRoleAccessOperation(payload, config);
    t.ok(b4a.equals(normalized.rao.tx, tx));
    t.ok(b4a.equals(normalized.rao.txv, txv));
    t.ok(b4a.equals(normalized.rao.iw, writerKey));
    t.ok(b4a.equals(normalized.rao.in, nonce));
    t.ok(b4a.equals(normalized.rao.is, signature));
});

test('normalizeRoleAccessOperation throws on missing payload fields', t => {
    t.exception(
        () => normalizeRoleAccessOperation({ type: OperationType.ADD_WRITER }, config),
        errorMessageIncludes('Invalid payload for role access normalization.')
    );

    t.exception(
        () => normalizeRoleAccessOperation({ type: OperationType.ADD_WRITER, address: 'x', rao: {} }, config),
        errorMessageIncludes('Missing required fields in role access payload.')
    );
});

test('normalizeRoleAccessOperation throws on invalid hex string', t => {
    const sender = randomAddress(config.addressPrefix);
    const payload = {
        type: OperationType.ADD_WRITER,
        address: sender,
        rao: {
            tx: 'zz',
            txv: hex('22', 32),
            iw: hex('33', 32),
            in: hex('44', 32),
            is: hex('55', 64)
        }
    };
    t.exception(
        () => normalizeRoleAccessOperation(payload, config),
        errorMessageIncludes('Invalid hex string')
    );
});

test('normalizeBootstrapDeploymentOperation normalizes hex strings and addresses', t => {
    const sender = randomAddress(config.addressPrefix);
    const payload = {
        type: OperationType.BOOTSTRAP_DEPLOYMENT,
        address: sender,
        bdo: {
            tx: hex('11', 32),
            txv: hex('22', 32),
            bs: hex('33', 32),
            ic: hex('44', 32),
            in: hex('55', 32),
            is: hex('66', 64)
        }
    };

    const normalized = normalizeBootstrapDeploymentOperation(payload, config);
    t.is(normalized.type, OperationType.BOOTSTRAP_DEPLOYMENT);
    t.ok(b4a.equals(normalized.address, addressToBuffer(sender, config.addressPrefix)));
    t.ok(b4a.equals(normalized.bdo.tx, toBuffer(hex('11', 32))));
    t.ok(b4a.equals(normalized.bdo.txv, toBuffer(hex('22', 32))));
    t.ok(b4a.equals(normalized.bdo.bs, toBuffer(hex('33', 32))));
    t.ok(b4a.equals(normalized.bdo.ic, toBuffer(hex('44', 32))));
    t.ok(b4a.equals(normalized.bdo.in, toBuffer(hex('55', 32))));
    t.ok(b4a.equals(normalized.bdo.is, toBuffer(hex('66', 64))));
});

test('normalizeBootstrapDeploymentOperation accepts buffer inputs', t => {
    const sender = randomAddress(config.addressPrefix);
    const tx = toBuffer(hex('aa', 32));
    const txv = toBuffer(hex('bb', 32));
    const bootstrap = toBuffer(hex('cc', 32));
    const channel = toBuffer(hex('dd', 32));
    const nonce = toBuffer(hex('ee', 32));
    const signature = toBuffer(hex('ff', 64));

    const payload = {
        type: OperationType.BOOTSTRAP_DEPLOYMENT,
        address: sender,
        bdo: {
            tx,
            txv,
            bs: bootstrap,
            ic: channel,
            in: nonce,
            is: signature
        }
    };

    const normalized = normalizeBootstrapDeploymentOperation(payload, config);
    t.ok(b4a.equals(normalized.bdo.tx, tx));
    t.ok(b4a.equals(normalized.bdo.txv, txv));
    t.ok(b4a.equals(normalized.bdo.bs, bootstrap));
    t.ok(b4a.equals(normalized.bdo.ic, channel));
    t.ok(b4a.equals(normalized.bdo.in, nonce));
    t.ok(b4a.equals(normalized.bdo.is, signature));
});

test('normalizeBootstrapDeploymentOperation throws on missing payload fields', t => {
    t.exception(
        () => normalizeBootstrapDeploymentOperation({ type: OperationType.BOOTSTRAP_DEPLOYMENT }, config),
        errorMessageIncludes('Invalid payload for bootstrap deployment normalization.')
    );

    t.exception(
        () => normalizeBootstrapDeploymentOperation({ type: OperationType.TX, address: 'x', bdo: {} }, config),
        errorMessageIncludes('Missing required fields in bootstrap deployment payload.')
    );
});

test('normalizeBootstrapDeploymentOperation throws on invalid hex string', t => {
    const sender = randomAddress(config.addressPrefix);
    const payload = {
        type: OperationType.BOOTSTRAP_DEPLOYMENT,
        address: sender,
        bdo: {
            tx: 'zz',
            txv: hex('22', 32),
            bs: hex('33', 32),
            ic: hex('44', 32),
            in: hex('55', 32),
            is: hex('66', 64)
        }
    };
    t.exception(
        () => normalizeBootstrapDeploymentOperation(payload, config),
        errorMessageIncludes('Invalid hex string')
    );
});

test('normalizeDecodedPayloadForJson converts buffers to strings', t => {
    const address = randomAddress(config.addressPrefix);
    const addressBuf = addressToBuffer(address, config.addressPrefix);
    const amountBuf = bigIntTo16ByteBuffer(1234n);
    const otherBuf = b4a.from('abcd', 'hex');

    const payload = {
        address: addressBuf,
        am: amountBuf,
        nested: {
            to: addressBuf,
            amount: amountBuf,
            other: otherBuf
        }
    };

    const normalized = normalizeDecodedPayloadForJson(payload, config);
    t.is(normalized.address, address);
    t.is(normalized.am, '1234');
    t.is(normalized.nested.to, address);
    t.is(normalized.nested.amount, '1234');
    t.is(normalized.nested.other, 'abcd');
});

test('normalizeDecodedPayloadForJson falls back to hex for invalid address buffers', t => {
    const bad = b4a.from('deadbeef', 'hex');
    const payload = { address: bad };
    const normalized = normalizeDecodedPayloadForJson(payload, config);
    t.is(normalized.address, b4a.toString(bad, 'hex'));
});

test('normalizeDecodedPayloadForJson returns input for non-objects', t => {
    t.is(normalizeDecodedPayloadForJson(null, config), null);
    t.is(normalizeDecodedPayloadForJson('value', config), 'value');
});
