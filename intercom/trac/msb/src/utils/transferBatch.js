import b4a from 'b4a';
import { addressToBuffer, bufferToAddress, isAddressValid } from '../core/state/utils/address.js';
import { bigIntTo16ByteBuffer, bufferToBigInt, decimalStringToBigInt } from './amountSerialization.js';

const COUNT_BYTES = 4;
const AMOUNT_BYTES = 16;
const MAX_AMOUNT = BigInt('0xffffffffffffffffffffffffffffffff');

export function encodeTransferBatchOutputs(outputs, config) {
    if (!Array.isArray(outputs) || outputs.length === 0) {
        throw new Error('Batch transfer outputs are required.');
    }
    if (outputs.length > 5000) {
        throw new Error('Batch transfer output count exceeds limit.');
    }

    const normalized = outputs.map((output, index) => {
        const to = output?.to;
        const amount = output?.amount ?? output?.tnk ?? output?.tnk_amount;
        if (!isAddressValid(to, config.addressPrefix)) {
            throw new Error(`Invalid batch transfer recipient at index ${index}.`);
        }
        const amountBigInt = typeof amount === 'bigint' ? amount : decimalStringToBigInt(String(amount));
        if (amountBigInt <= 0n || amountBigInt > MAX_AMOUNT) {
            throw new Error(`Invalid batch transfer amount at index ${index}.`);
        }
        return {
            to,
            address: addressToBuffer(to, config.addressPrefix),
            amount: amountBigInt,
            amountBuffer: bigIntTo16ByteBuffer(amountBigInt),
        };
    });

    let total = 0n;
    const seen = new Set();
    for (const output of normalized) {
        if (seen.has(output.to)) throw new Error(`Duplicate batch transfer recipient: ${output.to}`);
        seen.add(output.to);
        total += output.amount;
        if (total > MAX_AMOUNT) throw new Error('Batch transfer total exceeds maximum allowed value.');
    }

    const tupleBytes = config.addressLength + AMOUNT_BYTES;
    const buffer = b4a.alloc(COUNT_BYTES + normalized.length * tupleBytes);
    buffer.writeUInt32BE(normalized.length, 0);
    let offset = COUNT_BYTES;
    for (const output of normalized) {
        b4a.copy(output.address, buffer, offset);
        offset += config.addressLength;
        b4a.copy(output.amountBuffer, buffer, offset);
        offset += AMOUNT_BYTES;
    }

    return {
        buffer,
        totalAmount: bigIntTo16ByteBuffer(total),
        outputs: normalized.map((output) => ({
            to: output.to,
            amount: output.amount.toString(),
        })),
    };
}

export function decodeTransferBatchOutputs(buffer, config) {
    if (!b4a.isBuffer(buffer) || buffer.length < COUNT_BYTES) {
        throw new Error('Invalid batch transfer output buffer.');
    }
    const count = buffer.readUInt32BE(0);
    if (count <= 0 || count > 5000) throw new Error('Invalid batch transfer output count.');
    const tupleBytes = config.addressLength + AMOUNT_BYTES;
    if (buffer.length !== COUNT_BYTES + count * tupleBytes) {
        throw new Error('Invalid batch transfer output buffer length.');
    }

    const outputs = [];
    let total = 0n;
    let offset = COUNT_BYTES;
    for (let index = 0; index < count; index += 1) {
        const addressBuffer = buffer.subarray(offset, offset + config.addressLength);
        offset += config.addressLength;
        const amountBuffer = buffer.subarray(offset, offset + AMOUNT_BYTES);
        offset += AMOUNT_BYTES;
        const to = bufferToAddress(addressBuffer, config.addressPrefix);
        if (!to) throw new Error(`Invalid batch transfer recipient at index ${index}.`);
        const amount = bufferToBigInt(amountBuffer);
        if (amount <= 0n || amount > MAX_AMOUNT) {
            throw new Error(`Invalid batch transfer amount at index ${index}.`);
        }
        total += amount;
        if (total > MAX_AMOUNT) throw new Error('Batch transfer total exceeds maximum allowed value.');
        outputs.push({
            to,
            address: addressBuffer,
            amount,
            amountBuffer,
        });
    }

    return {
        outputs,
        totalAmount: bigIntTo16ByteBuffer(total),
        total,
    };
}
