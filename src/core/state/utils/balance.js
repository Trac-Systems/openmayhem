import b4a from 'b4a';
import { setBalance, ZERO_BALANCE } from './nodeEntry.js';
import { isBufferValid, bigIntToBuffer, NULL_BUFFER } from '../../../utils/buffer.js';
import { BALANCE_BYTE_LENGTH, TOKEN_DECIMALS } from '../../../utils/constants.js';
import { bufferToBigInt } from '../../../utils/amountSerialization.js';
import { FEE } from './transaction.js';

const DOUBLE_LENGTH = BALANCE_BYTE_LENGTH * 2
const PERCENTAGE_TERM = bigIntToBuffer(10_000n, DOUBLE_LENGTH)

/**
 * Converts a decimal to a buffer that can be used along with Balance#percentage. 
 * Should be used to define readable constants.
 * @param {number} value - The % with two decimal digits
 * @returns {Buffer} - Fixed-length buffer representing the percentage to be applied
 */
export const percent = value => {
    const bigint = BigInt(Math.round(value * 100))
    return bigIntToBuffer(bigint, BALANCE_BYTE_LENGTH)
}

/**
 * Short term for 75% to be used with balance. Extracted to avoid buffer conversions during request time.
 */
export const PERCENT_75 = percent(75)
/**
 * Short term for 50% to be used with balance. Extracted to avoid buffer conversions during request time.
 */
export const PERCENT_50 = percent(50)
/**
 * Short term for 25% to be used with balance. Extracted to avoid buffer conversions during request time.
 */
export const PERCENT_25 = percent(25)

/**
 * Converts a bigint amount of tokens into a fixed-length buffer,
 * scaled according to TOKEN_DECIMALS.
 * Note: DO NOT USE IN APPLY FUNCTION.
 * @param {bigint} bigint - The amount of tokens
 * @returns {Buffer} - Fixed-length buffer representing token amount
 */
export const $TNK = bigint => bigIntToBuffer(
    bigint * 10n ** TOKEN_DECIMALS, 
    BALANCE_BYTE_LENGTH
)

/**
 * Converts a bigint into a fixed-length buffer reprenseting a positive number
 * scaled according to TOKEN_DECIMALS.
 * @param {bigint} bigint - The number to be converted
 * @returns {Buffer} - Fixed-length buffer representing token amount
 */
export const toTerm = bigint => bigIntToBuffer(
    bigint, 
    BALANCE_BYTE_LENGTH
)

const truncate = buf => buf.slice(BALANCE_BYTE_LENGTH * -1)

const shiftLeft1 = buf => {
    const res = b4a.alloc(BALANCE_BYTE_LENGTH)
    let carry = 0
    for (let i = BALANCE_BYTE_LENGTH - 1; i >= 0; i--) {
        const val = (buf[i] << 1) | carry
        res[i] = val & 0xff
        carry = val >> 8
    }
    return res
}

const addBuffers = (a, b) => {
    if (a.length !== b.length) return NULL_BUFFER
    const result = b4a.alloc(a.length);
    let carry = 0;
    for (let i = a.length - 1; i >= 0; i--) {
        const sum = a[i] + b[i] + carry;
        result[i] = sum & 0xff;
        carry = sum >> 8;
    }

    if(carry) return NULL_BUFFER // overflow
    return result;
}

const subBuffers = (a, b) => {
    if (a.length !== b.length) return NULL_BUFFER;

    const result = b4a.alloc(a.length);
    let borrow = 0;

    for (let i = a.length - 1; i >= 0; i--) {
        let diff = a[i] - b[i] - borrow;
        if (diff < 0) {
            diff += 256;
            borrow = 1;
        } else {
            borrow = 0;
        }
        result[i] = diff;
    }

    // If we ended with borrow, it means a < b (underflow)
    if (borrow) return NULL_BUFFER;

    return result;
}

const divBuffers = (dividend, divisor) => {
    if (dividend.length !== BALANCE_BYTE_LENGTH || divisor.length !== BALANCE_BYTE_LENGTH) {
        return NULL_BUFFER
    }
    if (divisor.equals(ZERO_BALANCE)) {
        return NULL_BUFFER
    }

    let quotient = b4a.alloc(BALANCE_BYTE_LENGTH)
    let remainder = b4a.alloc(BALANCE_BYTE_LENGTH)

    for (let bit = 0; bit < BALANCE_BYTE_LENGTH * 8; bit++) {
        remainder = shiftLeft1(remainder)
        const byteIndex = Math.floor(bit / 8)
        const bitIndex = 7 - (bit % 8)
        const bitVal = (dividend[byteIndex] >> bitIndex) & 1
        remainder[BALANCE_BYTE_LENGTH - 1] |= bitVal
        if (b4a.compare(remainder, divisor) >= 0) {
            remainder = subBuffers(remainder, divisor)
            quotient[byteIndex] |= (1 << bitIndex)
        }
    }

    return quotient
}

const mulBuffers = (a, b) => {
    if (a.length !== BALANCE_BYTE_LENGTH || b.length !== BALANCE_BYTE_LENGTH) {
      return NULL_BUFFER
    }
  
    const alen = a.length;
    const blen = b.length;
    const result = b4a.alloc(DOUBLE_LENGTH); // up to 32 bytes
  
    for (let i = alen - 1; i >= 0; i--) {
        let carry = 0;
        for (let j = blen - 1; j >= 0; j--) {
        const ai = a[i];
        const bj = b[j];

        const pos = i + j + 1;
        const mul = ai * bj + result[pos] + carry;

        result[pos] = mul & 0xff;   // low byte
        carry = mul >>> 8;          // high byte
        }
        result[i] += carry;
    }

    if (!b4a.equals(result.slice(0, BALANCE_BYTE_LENGTH), ZERO_BALANCE)) {
        return NULL_BUFFER
    }
  
    // Truncate
    return truncate(result)
}

const validateValue = value => {
    if (!isBufferValid(value, BALANCE_BYTE_LENGTH)) {
        throw new Error('Invalid balance') // Should be a qualified exception
    }
}

/**
 * Balance class encapsulates a token balance stored as a fixed-length buffer.
 */
class Balance {
    #value

    /** Returns the internal buffer */
    get value() {
        return this.#value
    }
    
    /**
     * Creates a Balance instance.
     * @param {Buffer} value - Buffer representing the balance
     */
    constructor(value) {
        validateValue(value)
        this.#value = value
    }

    /**
     * Adds another balance to this one.
     * @param {Balance} b 
     * @returns {Balance} - New Balance instance
     */
    add(b) {
        return toBalance(addBuffers(this.#value, b.value))
    }

    /**
     * Subtracts another balance from this one.
     * @param {Balance} b 
     * @returns {Balance} - New Balance instance
     */
    sub(b) {
        return toBalance(subBuffers(this.#value, b.value))
    }

    /**
     * Multiply a balance for a number in bytes
     * @param {Buffer} num - to be used along `toTerm`
     * @returns {Balance} - New Balance instance
     */
    mul(num) {
        if (!isBufferValid(num, BALANCE_BYTE_LENGTH)) return null
        const result = mulBuffers(this.#value, num)
        return toBalance(result)
    }

    /**
     * Reduce to its percentage
     * @param {Buffer} percent - the buffer percentage (with two extra decimals). Expected to be used along with percent function.
     * @returns {Balance} - New Balance instance
     */
    percentage(percent) {
        if (!isBufferValid(percent, BALANCE_BYTE_LENGTH)) return null
        const dividend = mulBuffers(this.#value, percent)
        const result = divBuffers(dividend, PERCENTAGE_TERM)
        return toBalance(result)
    }

    /**
     * Divide a balance by a number in bytes
     * @param {Buffer} b - to be used along `toTerm`
     * @returns {Balance} - New Balance instance
     */
    div(divisor) {
        if (!isBufferValid(divisor, BALANCE_BYTE_LENGTH)) return null
        return toBalance(divBuffers(this.#value, divisor))
    }

    /**
     * Updates a node entry with this balance.
     * @param {Object} nodeEntry 
     */
    update(nodeEntry) {
        return setBalance(nodeEntry, this.#value)
    }

    /** Compares equality with another balance */
    equals(b) {
        return isBufferValid(b?.value, BALANCE_BYTE_LENGTH) && b4a.equals(this.#value, b.value)
    }

    /** Returns true if this balance is greater than another */
    greaterThan(b) {
        return b4a.compare(this.#value, b.value) === 1
    }

    /** Returns true if this balance is lower than another */
    lowerThan(b) {
        return b4a.compare(this.#value, b.value) === -1
    }

    /** Returns true if this balance is greater than or equal to another */
    greaterThanOrEquals(b) {
        const cmp = b4a.compare(this.#value, b.value)
        return cmp === 1 || cmp === 0
    }

    /** Returns true if this balance is lower than or equal to another */
    lowerThanOrEquals(b) {
        const cmp = b4a.compare(this.#value, b.value)
        return cmp === -1 || cmp === 0
    }

    // Note: DO NOT USE IN APPLY FUNCTION.
    /** Returns the hex string representation of the balance buffer */
    asHex() {
        return b4a.toString(this.#value, 'hex');
    }

    // Note: DO NOT USE IN APPLY FUNCTION.
    /** Returns the balance as a BigInt */
    asBigInt() {
        return bufferToBigInt(this.#value)
    }
}

export function toBalance(balance) {
    try{
        return new Balance(balance)
    } catch(e) {
        console.error(e?.message || e)
        return null
    }
}

export const BALANCE_FEE = toBalance(FEE);
export const BALANCE_TO_STAKE = BALANCE_FEE.mul(toTerm(10n));
export const BALANCE_ZERO = toBalance(ZERO_BALANCE);
