import { test, hook } from 'brittle';
import fileUtils from '../../../../src/utils/fileUtils.js';
import { errorMessageIncludes } from "../../../helpers/regexHelper.js";
import fs from 'fs';
import PeerWallet from 'trac-wallet';
import { config } from '../../../helpers/config.js';

const DUMMY_PATH_OK = './dummy_balance_ok.csv';
const DUMMY_PATH_DUP = './dummy_balance_dup.csv';
const DUMMY_PATH_INVALID_ADDRESS = './dummy_balance_invalid_address.csv';
const DUMMY_PATH_ADMIN_ADDRESS = './dummy_balance_admin_address.csv';
const DUMMY_PATH_EMPTY = './dummy_balance_empty.csv';
const DUMMY_PATH_WHITESPACE = './dummy_balance_whitespace.csv';
const DUMMY_PATH_INVALID_BALANCE = './dummy_balance_invalid_balance.csv';
const DUMMY_PATH_NEGATIVE = './dummy_balance_negative.csv';
const DUMMY_PATH_MALFORMED = './dummy_balance_malformed.csv';
const DUMMY_PATH_BLANK = './dummy_balance_blank.csv';
const DUMMY_PATH_ZERO = './dummy_balance_zero.csv';
const DUMMY_PATH_BOM = './dummy_balance_bom.csv';
const DUMMY_PATH_LARGE = './dummy_balance_large.csv';

const ADDR1 = 'trac1dguwzsvcsehslh6dgj2mqlsxdn7s5t5vhem56yd0xlg47aq6exzqymhr6u';
const ADDR2 = 'trac123z3gfpr2epjwww7ntm3m6ud2fhmq0tvts27p2f5mx3qkecsutlqfys769';

hook('Initialize dummy balance files', async t => {
    // Happy path
    fs.writeFileSync(DUMMY_PATH_OK, `${ADDR1},1.000000000000000001\n${ADDR2},2.000000000000000000\n`);
    // Edge: duplicated address
    fs.writeFileSync(DUMMY_PATH_DUP, `${ADDR1},1.0\n${ADDR1},2.0\n`);
    // Edge: Empty file
    fs.writeFileSync(DUMMY_PATH_EMPTY, '');
    // Edge: address with whitespace
    fs.writeFileSync(DUMMY_PATH_WHITESPACE, `  ${ADDR1}  , 1.0\n`);
    // Edge: invalid balance format
    fs.writeFileSync(DUMMY_PATH_INVALID_BALANCE, `${ADDR1},notanumber\n`);
    // Edge: negative balance
    fs.writeFileSync(DUMMY_PATH_NEGATIVE, `${ADDR1},-1.0\n`);
    // Edge: malformed line (missing balance)
    fs.writeFileSync(DUMMY_PATH_MALFORMED, `${ADDR1}\n`);
    // Edge: blank lines and whitespace only
    fs.writeFileSync(DUMMY_PATH_BLANK, `\n   \n${ADDR1},1.0\n`);
    // Edge: zero balance
    fs.writeFileSync(DUMMY_PATH_ZERO, `${ADDR1},0.0\n`);
    // Edge: file with BOM
    fs.writeFileSync(DUMMY_PATH_BOM, '\uFEFF' + `${ADDR1},1.0\n`);
    // Edge: large file
    let large = '';
    const randomAddress = async () => {
        const wallet = new PeerWallet();
        await wallet.ready;
        await wallet.generateKeyPair();
        return wallet.address;
    }
    for (let i = 0; i < 1000; i++) {
        const rand = await randomAddress(config.addressPrefix);
        large += `${rand},1.0\n`;
    }
    fs.writeFileSync(DUMMY_PATH_LARGE, large);
});


test('readBalanceMigrationFile - valid file', async (t) => {
    const { addressBalancePair, totalBalance, totalAddresses } = await fileUtils.readBalanceMigrationFile(DUMMY_PATH_OK);
    t.is(typeof addressBalancePair, 'object');
    t.is(addressBalancePair.size, 2);
    t.is(totalAddresses, 2);
    t.ok(totalBalance > 0n);
    t.ok(addressBalancePair.has(ADDR1));
    t.ok(addressBalancePair.has(ADDR2));
});

test('readBalanceMigrationFile - address with whitespace', async (t) => {
    await t.exception(() => fileUtils.readBalanceMigrationFile( DUMMY_PATH_WHITESPACE),
        errorMessageIncludes('Failed to read balance migration file: Invalid line in balance migration file:'))
});

test('readBalanceMigrationFile - duplicate address', async (t) => {
    await t.exception(() => fileUtils.readBalanceMigrationFile(DUMMY_PATH_DUP),
        errorMessageIncludes(`Duplicate address found in balance migration file: '${ADDR1}'. Each address must be unique.`))
});

test('readBalanceMigrationFile - invalid balance format', async (t) => {
    await t.exception(() => fileUtils.readBalanceMigrationFile(DUMMY_PATH_INVALID_BALANCE),
        errorMessageIncludes(`Failed to read balance migration file: Invalid line in balance migration file: `))
});

test('readBalanceMigrationFile - negative balance', async (t) => {
    await t.exception(() => fileUtils.readBalanceMigrationFile(DUMMY_PATH_NEGATIVE),
        errorMessageIncludes(`Failed to read balance migration file: Invalid line in balance migration file: `))
});

test('readBalanceMigrationFile - malformed line', async (t) => {
    await t.exception(() => fileUtils.readBalanceMigrationFile( DUMMY_PATH_MALFORMED),
        errorMessageIncludes(`Failed to read balance migration file: Invalid line in balance migration file: `))
});

test('readBalanceMigrationFile - blank lines and whitespace', async (t) => {
    const { addressBalancePair, totalBalance, totalAddresses } = await fileUtils.readBalanceMigrationFile(DUMMY_PATH_BLANK);
    t.is(addressBalancePair.size, 1);
    t.is(totalAddresses, 1);
    t.ok(addressBalancePair.has(ADDR1));
});

test('readBalanceMigrationFile - zero balance', async (t) => {
    const { addressBalancePair, totalBalance, totalAddresses } = await fileUtils.readBalanceMigrationFile( DUMMY_PATH_ZERO);
    t.is(addressBalancePair.size, 1);
    t.is(totalBalance, 0n);
    t.is(totalAddresses, 1);
    t.ok(addressBalancePair.has(ADDR1));
});

test('readBalanceMigrationFile - BOM at start', async (t) => {
    const { addressBalancePair, totalBalance, totalAddresses } = await fileUtils.readBalanceMigrationFile(DUMMY_PATH_BOM);
    t.is(addressBalancePair.size, 1);
    t.is(totalAddresses, 1);
    t.ok(addressBalancePair.has(ADDR1));
});

test('readBalanceMigrationFile - large file', async (t) => {
    const { addressBalancePair, totalBalance, totalAddresses } = await fileUtils.readBalanceMigrationFile(DUMMY_PATH_LARGE);
    t.is(addressBalancePair.size, 1000);
    t.is(totalAddresses, 1000);
});

test('readBalanceMigrationFile - empty file', async (t) => {
    await t.exception(() => fileUtils.readBalanceMigrationFile(DUMMY_PATH_EMPTY),
        errorMessageIncludes('Balance migration file is empty. File must contain at least one valid address balance pair.'))
});

test('readBalanceMigrationFile - file does not exist', async (t) => {
    const nonExistentPath = './non_existent_balance.csv';
    await t.exception(() => fileUtils.readBalanceMigrationFile(nonExistentPath),
        errorMessageIncludes(`Balance migration file not found: ${nonExistentPath}`))
});

test('readBalanceMigrationFile - invalid file extension', async (t) => {
    const invalidExtensionPath = './dummy_balance.txt';
    await t.exception(() => fileUtils.readBalanceMigrationFile(invalidExtensionPath),
        errorMessageIncludes(`Invalid file format: ${invalidExtensionPath}. Balance migration file must be a CSV file.`))
});

hook('Cleanup dummy balance files', async t => {
    [DUMMY_PATH_OK, DUMMY_PATH_DUP, DUMMY_PATH_EMPTY,
     DUMMY_PATH_WHITESPACE, DUMMY_PATH_INVALID_BALANCE, DUMMY_PATH_NEGATIVE, DUMMY_PATH_MALFORMED, DUMMY_PATH_BLANK,
     DUMMY_PATH_ZERO, DUMMY_PATH_BOM, DUMMY_PATH_LARGE].forEach(path => {
        if (fs.existsSync(path)) fs.unlinkSync(path);
    });
});
