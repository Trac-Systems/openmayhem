import b4a from "b4a";
import { blake3 } from '@tracsystems/blake3';
import { bigIntToDecimalString, decimalStringToBigInt } from 'trac-msb/src/utils/amountSerialization.js';

export function resolveNumberString(number, decimals){
    const raw = (number ?? '').toString().trim();
    const safeDecimals = isNaN(decimals) ? 18 : parseInt(decimals);
    const match = raw.match(/^(\d*)(?:\.(\d*))?$/);
    if (!match) return '0';
    const integerPart = match[1] && match[1].length ? match[1] : '0';
    let fractionalPart = match[2] ?? '';
    if (safeDecimals <= 0) {
        return decimalStringToBigInt(integerPart, 0).toString();
    }
    if (fractionalPart.length > safeDecimals) fractionalPart = fractionalPart.slice(0, safeDecimals);
    const normalized = `${integerPart}${fractionalPart.length ? '.' + fractionalPart : ''}`;
    return decimalStringToBigInt(normalized, safeDecimals).toString();
}

export function formatNumberString(string, decimals) {
    const raw = (string ?? '').toString().trim();
    const safeDecimals = isNaN(decimals) ? 18 : parseInt(decimals);
    if (!raw) return '0';
    try {
        // Accept either an already-decimal string or an integer-encoded string.
        const asBigInt = raw.includes('.')
            ? decimalStringToBigInt(raw, safeDecimals)
            : BigInt(raw);
        return bigIntToDecimalString(asBigInt, safeDecimals);
    } catch (_e) {
        return '0';
    }
}

export function removeTrailingZeros(value) {
    value = value.toString();
    if (value.indexOf('.') === -1) {
        return value;
    }

    while((value.slice(-1) === '0' || value.slice(-1) === '.') && value.indexOf('.') !== -1) {
        value = value.substr(0, value.length - 1);
    }
    return value;
}

export function visibleLength(str) {
    return b4a.byteLength(str);
}

export function jsonStringify(value){
    try {
        return JSON.stringify(value);
    } catch(e){
        console.log(e);
    }
    return null;
}

export function jsonParse(str){
    try {
        return JSON.parse(str);
    } catch(e){
        console.log(e);
    }
    return undefined;
}

export function safeClone(obj){
    if(typeof obj !== 'object') return null;
    const str = jsonStringify(obj);
    if(str === null) return null;
    const obj2 = jsonParse(str);
    if(obj2 === undefined) return null;
    return obj2;
}

export async function createHash(message) {
    const out = await blake3(message);
    return b4a.toString(out, 'hex');
}
