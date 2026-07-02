import { test } from 'brittle';
import b4a from 'b4a';
import { randomBuffer, TEN_THOUSAND_VALUE, tokenUnits } from '../stateTestUtils.js';
import { ZERO_BALANCE, decode, encode } from '../../../../src/core/state/utils/nodeEntry.js';
import { WRITER_BYTE_LENGTH, ADMIN_INITIAL_BALANCE, BALANCE_BYTE_LENGTH } from '../../../../src/utils/constants.js';
import { $TNK, toBalance, toTerm, percent } from '../../../../src/core/state/utils/balance.js';
import { bufferToBigInt } from '../../../../src/utils/amountSerialization.js';
import { ZERO_LICENSE } from '../../../../src/core/state/utils/nodeEntry.js';
test('Balance#asHex explicit', t => {
    const val = $TNK(1000n)
    t.is(toBalance(val).asHex(), b4a.toString(val, 'hex'), 'hex matches')
})

test('Balance#add', () => {
    test('with zero', t => {
        const node = {
            wk: randomBuffer(WRITER_BYTE_LENGTH),
            isWhitelisted: true,
            isWriter: true,
            isIndexer: false,
            balance: ZERO_BALANCE,
            license: ZERO_LICENSE,
            stakedBalance: ZERO_BALANCE
        };

        const balance = toBalance(node.balance)
        const addedBalance = balance.add(toBalance(TEN_THOUSAND_VALUE))

        const encoded = encode(node)
        addedBalance.update(encoded)

        const updated = decode(encoded)
        console.log("updated", updated)
        t.ok(b4a.equals(updated.balance, TEN_THOUSAND_VALUE), 'balance matches');
    });

    test('other stuff', t => {
        const node = {
            wk: randomBuffer(WRITER_BYTE_LENGTH),
            isWhitelisted: true,
            isWriter: true,
            isIndexer: false,
            balance: TEN_THOUSAND_VALUE
        };

        const balance = toBalance(node.balance)
        const addedBalance = balance.add(toBalance(TEN_THOUSAND_VALUE))
        t.is(addedBalance.asHex(), '00000000000000000000000000004e20', 'balance matches');
    });

    test('overflow returns NULL_BUFFER', t => {
        const max = b4a.alloc(BALANCE_BYTE_LENGTH, 0xFF);
        const balance = toBalance(max);

        const oneRaw = b4a.alloc(BALANCE_BYTE_LENGTH);
        oneRaw[oneRaw.length - 1] = 1; // +1

        const result = balance.add(toBalance(oneRaw));

        // Should return null-like buffer on overflow
        t.is(result, null, 'overflow returns null');
    });

    test('edge case: max - 1 + 1 = max', t => {
        const maxMinusOne = b4a.alloc(BALANCE_BYTE_LENGTH, 0xFF);
        maxMinusOne[maxMinusOne.length - 1] = 0xFE;
        const balance = toBalance(maxMinusOne);

        const oneRaw = b4a.alloc(BALANCE_BYTE_LENGTH);
        oneRaw[oneRaw.length - 1] = 1;

        const result = balance.add(toBalance(oneRaw));

        // Should equal max value
        const expected = b4a.alloc(BALANCE_BYTE_LENGTH, 0xFF);
        t.ok(b4a.equals(result.value, expected), 'max - 1 + 1 = max');
    });
})

test('Balance#sub', () => {
    test('with zero', t => {
        const node = {
            wk: randomBuffer(WRITER_BYTE_LENGTH),
            isWhitelisted: true,
            isWriter: true,
            isIndexer: false,
            balance: TEN_THOUSAND_VALUE,
            license: ZERO_LICENSE,
            stakedBalance: ZERO_BALANCE
        };

        const balance = toBalance(node.balance)
        const subBalance = balance.sub(toBalance(ZERO_BALANCE))

        const encoded = encode(node)
        subBalance.update(encoded)

        const updated = decode(encoded)
        t.ok(b4a.equals(updated.balance, TEN_THOUSAND_VALUE), 'balance matches');
    });

    test('zero from value', t => {
        const node = {
            wk: randomBuffer(WRITER_BYTE_LENGTH),
            isWhitelisted: true,
            isWriter: true,
            isIndexer: false,
            balance: TEN_THOUSAND_VALUE,
            license: ZERO_LICENSE,
            stakedBalance: ZERO_BALANCE
        };

        const balance = toBalance(node.balance)
        const subBalance = balance.sub(toBalance(ZERO_BALANCE))

        const encoded = encode(node)
        subBalance.update(encoded)

        const updated = decode(encoded)
        t.ok(b4a.equals(updated.balance, TEN_THOUSAND_VALUE), 'balance matches');
    });

    test('underflow returns NULL_BUFFER', t => {
        const a = $TNK(1000n);
        const b = $TNK(2000n);
        const result = toBalance(a).sub(toBalance(b));

        // Should return null-like buffer on underflow
        t.is(result, null, 'overflow returns null');
    });

    test('edge case: equal amounts = zero', t => {
        const a = $TNK(5000n);
        const b = $TNK(5000n);
        const result = toBalance(a).sub(toBalance(b));

        const expected = b4a.alloc(BALANCE_BYTE_LENGTH); // all zeros
        t.ok(b4a.equals(result.value, expected), 'equal amounts subtract to zero');
    });

    test('edge case: one less than a', t => {
        const a = $TNK(1000n);
        const b = $TNK(999n);
        const result = toBalance(a).sub(toBalance(b));

        const expected = $TNK(1n);
        t.ok(b4a.equals(result.value, expected), 'subtracting 999 from 1000 gives 1');
    });
})

test('Balance#mul', () => {
    test('basic multiplication', t => {
        const a = $TNK(1000n);
        const result = toBalance(a).mul(toTerm(4n));

        // Should return null-like buffer on underflow
        t.ok(toBalance($TNK(4000n)).equals(result), 'equal 4000n');
    })

    test('zero multiplication', t => {
        const a = $TNK(4000n);
        const result = toBalance(a).mul(toTerm(0n));

        // Should return null-like buffer on underflow
        t.ok(result.equals(toBalance($TNK(0n))), 'returns zero');
    })

    test('zero value multiplication', t => {
        const a = toBalance($TNK(0n));
        const result = a.mul(toTerm(1000n));

        // Should return null-like buffer on underflow
        t.ok(result.equals(toBalance($TNK(0n))), 'returns zero');
    })

    test('zero value multiplication', t => {
        const a = toBalance($TNK(0n));
        const result = a.mul(toTerm(1000n));

        // Should return null-like buffer on underflow
        t.ok(result.equals(toBalance($TNK(0n))), 'returns zero');
    })

    test('overflow returns NULL_BUFFER', t => {
        const max = b4a.alloc(BALANCE_BYTE_LENGTH, 0xFF);
        const balance = toBalance(max);

        const balance2 = toBalance(max);

        const result = balance.mul(balance2);

        // Should return null-like buffer on overflow
        t.is(result, null, 'overflow returns null');
    });
})

test('Balance#div', () => {
    test('basic division', t => {
        const a = $TNK(4000n);
        const result = toBalance(a).div(toTerm(4n));

        // Should return null-like buffer on underflow
        t.ok(toBalance($TNK(1000n)).equals(result), 'equal 1000n');
    })

    test('zero division', t => {
        const a = $TNK(4000n);
        const result = toBalance(a).div(toTerm(0n));

        // Should return null-like buffer on underflow
        t.is(result, null, 'returns null');
    })

    test('zero value division', t => {
        const a = toBalance($TNK(0n));
        const result = a.div(toTerm(1000n));

        // Should return null-like buffer on underflow
        t.ok(result.equals(toBalance($TNK(0n))), 'returns 0');
    })
})

test('Balance#percentage', () => {
    test('basic percentage', t => {
        const a = $TNK(1000n);
        const result = toBalance(a).percentage(percent(4));

        // Should return null-like buffer on underflow
        t.ok(toBalance($TNK(40n)).equals(result), 'equal 40n');
    })
})

test('toTerm', t => {
    const converted = bufferToBigInt(toTerm(4n))
    t.is(converted, 4n, 'overflow returns null');
})

test('Balance#asBigInt', t => {
    const node = {
        wk: randomBuffer(WRITER_BYTE_LENGTH),
        isWhitelisted: true,
        isWriter: true,
        isIndexer: false,
        balance: TEN_THOUSAND_VALUE
    };

    const balance = toBalance(node.balance)
    const addedBalance = balance.add(toBalance(TEN_THOUSAND_VALUE))
    t.is(addedBalance.asBigInt(), 20_000n, 'balance matches');
});

test('Balance#greaterThan', t => {
    const $TNK1000 = $TNK(1000n)
    const $TNK1001 = $TNK(1001n)

    t.not(toBalance($TNK1000).greaterThan(toBalance($TNK1001)), '1000 not greater than 1001');
    t.ok(toBalance($TNK1001).greaterThan(toBalance($TNK1000)), '1001 greater than 1000');
    t.not(toBalance($TNK1000).greaterThan(toBalance($TNK1000)), '1000 not greater than 1000');
});

test('Balance#lowerThan', t => {
    const $TNK1000 = $TNK(1000n)
    const $TNK1001 = $TNK(1001n)

    t.not(toBalance($TNK1001).lowerThan(toBalance($TNK1000)), '1001 not lower than 1000');
    t.ok(toBalance($TNK1000).lowerThan(toBalance($TNK1001)), '1000 lower than 1001');
    t.not(toBalance($TNK1000).lowerThan(toBalance($TNK1000)), '1000 not lower than 1000');
});

test('Balance#equals', t => {
    const $TNK1000 = $TNK(1000n)

    t.ok(toBalance($TNK1000).equals(toBalance($TNK1000)), '1000 equals 1000');
});

test('Balance $TNK', t => {
    const $TNK300 = $TNK(300n)
    const converted = toBalance($TNK300).asBigInt()
    t.is(converted, tokenUnits(300n), 'balance matches');
});

test('Node entry integration', t => {
    const entry = encode({
        wk: randomBuffer(WRITER_BYTE_LENGTH),
        isWhitelisted: true,
        isWriter: true,
        isIndexer: true,
        balance: ADMIN_INITIAL_BALANCE,
        license: b4a.from([0, 0, 0, 1]),
        stakedBalance: TEN_THOUSAND_VALUE
    });

    const decoded = decode(entry)

    const updated = toBalance(decoded.balance)
        .add(toBalance($TNK(300n)))
        .update(encode(decoded))

    t.is(toBalance(decode(updated).balance).asBigInt(), tokenUnits(1300n), 'balance matches');
});

test('Balance#greaterThanOrEquals', t => {
    const b1000 = toBalance($TNK(1000n))
    const b1001 = toBalance($TNK(1001n))
    const b1000Dup = toBalance($TNK(1000n))

    t.ok(b1001.greaterThanOrEquals(b1000), '1001 >= 1000')
    t.ok(b1000.greaterThanOrEquals(b1000Dup), '1000 >= 1000')
    t.ok(!b1000.greaterThanOrEquals(b1001), '1000 !>= 1001')
})

test('Balance#lowerThanOrEquals', t => {
    const b1000 = toBalance($TNK(1000n))
    const b1001 = toBalance($TNK(1001n))
    const b1000Dup = toBalance($TNK(1000n))

    t.ok(b1000.lowerThanOrEquals(b1001), '1000 <= 1001')
    t.ok(b1000.lowerThanOrEquals(b1000Dup), '1000 <= 1000')
    t.ok(!b1001.lowerThanOrEquals(b1000), '1001 !<= 1000')
})