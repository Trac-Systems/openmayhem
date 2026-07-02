import b4a from "b4a";
import {bufferToAddress} from "../core/state/utils/address.js";
import { EntryType } from "./constants.js";

//TODO: change file name or split functions below into multiple files (Remember to update imports and tests accordingly)

export function isHexString(string) {
    return typeof string === 'string' && string.length > 1 && /^[0-9a-fA-F]+$/.test(string) && string.length % 2 === 0;
}

export const normalizeHex = (input) => {
    if (input == null) {
        throw new Error('Input cannot be null or undefined');
    }

    if (typeof input === 'string') {
        if (!isHexString(input)) {
            throw new Error('Invalid hex string');
        }
        return b4a.from(input, 'hex');
    }

    if (b4a.isBuffer(input)) {
        return input;
    }
    throw new Error('Input must be a hex string or a Buffer');
};

export async function sleep(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

export const safeJsonStringify = (value) => {
    try {
        return JSON.stringify(value);
    } catch (error) {
        console.error(error);
    }
    return null;
}

export const safeJsonParse = (str) => {
    try {
        return JSON.parse(str);
    } catch (error) {
        console.error(error);
    }
    return undefined;
}

export async function getFormattedIndexersWithAddresses(state, config) {
    const indexers = await state.getIndexersEntry();
    const formatted = indexers.map((entry) => ({
        writingKey: b4a.toString(entry.key, "hex"),
    }));

    const results = await Promise.all(
        formatted.map(async (entry) => {            
            const address = bufferToAddress(
                await state.getSigned(EntryType.WRITER_ADDRESS + entry.writingKey),
                config.addressPrefix
            );

            return {
                ...entry,
                address,
            };
        })
    );

    return results;
}

export function formatIndexersEntry(indexersEntry, addressLength) {

    const count = indexersEntry[0];
    const indexers = [];

    for (let i = 0; i < count; i++) {
        const start = 1 + (i * addressLength);
        const end = start + addressLength;
        const indexerAddr = indexersEntry.subarray(start, end);
        indexers.push(indexerAddr.toString('ascii'));
    }

    return {
        count,
        addresses: indexers
    };
}

export function isTransactionRecordPut(entry) {
    const isPut = entry.type === "put";
    const isHex = isHexString(entry.key);
    const is64 = entry.key.length === 64;
    return isPut && isHex && is64;
}
