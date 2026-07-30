import { test, hook } from 'brittle';
import fileUtils from '../../../../src/utils/fileUtils.js';
import { errorMessageIncludes } from "../../../helpers/regexHelper.js";
import fs from 'fs';
import { WalletProvider } from 'trac-wallet';
import { config } from '../../../helpers/config.js';
import { asAddress } from '../../../helpers/address.js';

const DUMMY_PATH_OK = './dummy_whitelist_ok.csv';
const DUMMY_PATH_DUP = './dummy_whitelist_dup.csv';
const DUMMY_PATH_EMPTY = './dummy_whitelist_empty.csv';
const DUMMY_PATH_BLANK = './dummy_whitelist_blank.csv';
const DUMMY_PATH_BOM = './dummy_whitelist_bom.csv';
const DUMMY_PATH_LARGE = './dummy_whitelist_large.csv';

const ADDR1 = asAddress('6a38e14198866f0fdf4d4495b07e066cfd0a2e8cbe774d11af37d15f741ac984');
const ADDR2 = asAddress('544514242356432739de9af71deb8d526fb03d6c5c15e0a934d9a20b6710e2fe');

hook('Initialize dummy whitelist files', async () => {
    // Happy path
    fs.writeFileSync(DUMMY_PATH_OK, `${ADDR1}\n${ADDR2}\n`);
    // Edge: duplicated address
    fs.writeFileSync(DUMMY_PATH_DUP, `${ADDR1}\n${ADDR1}\n`);
    // Edge: Empty file
    fs.writeFileSync(DUMMY_PATH_EMPTY, '');
    // Edge: blank lines and whitespace only
    fs.writeFileSync(DUMMY_PATH_BLANK, `\n   \n${ADDR1}\n`);
    // Edge: file with BOM
    fs.writeFileSync(DUMMY_PATH_BOM, '\uFEFF' + `${ADDR1}\n`);
    // Edge: large file
    let large = '';
    const randomAddress = async () => {
        const wallet = await new WalletProvider(config).generate({ derivationPath: config.derivationPath })
        return wallet.address;
    };
    for (let i = 0; i < 1000; i++) {
        const rand = await randomAddress(config.addressPrefix);
        large += `${rand}\n`;
    }
    fs.writeFileSync(DUMMY_PATH_LARGE, large);
});

test('readAddressesFromWhitelistFile - valid file', async (t) => {
    const addresses = await fileUtils.readAddressesFromWhitelistFile(DUMMY_PATH_OK);
    t.is(addresses.length, 2);
    t.ok(addresses.includes(ADDR1));
    t.ok(addresses.includes(ADDR2));
});

test('readAddressesFromWhitelistFile - duplicate address', async (t) => {
    await t.exception(() => fileUtils.readAddressesFromWhitelistFile(DUMMY_PATH_DUP),
        errorMessageIncludes(`Duplicate addresses found in whitelist file: ${ADDR1}`));
});

test('readAddressesFromWhitelistFile - blank lines and whitespace', async (t) => {
    const addresses = await fileUtils.readAddressesFromWhitelistFile(DUMMY_PATH_BLANK);
    t.is(addresses.length, 1);
    t.ok(addresses.includes(ADDR1));
});

test('readAddressesFromWhitelistFile - BOM at start', async (t) => {
    const addresses = await fileUtils.readAddressesFromWhitelistFile(DUMMY_PATH_BOM);
    t.is(addresses.length, 1);
    t.ok(addresses.includes(ADDR1));
});

test('readAddressesFromWhitelistFile - large file', async (t) => {
    const addresses = await fileUtils.readAddressesFromWhitelistFile(DUMMY_PATH_LARGE);
    t.is(addresses.length, 1000);
});

test('readAddressesFromWhitelistFile - empty file', async (t) => {
    await t.exception(() => fileUtils.readAddressesFromWhitelistFile(DUMMY_PATH_EMPTY),
        errorMessageIncludes('The whitelist file is empty. File must contain at least one valid address.'));
});

test('readAddressesFromWhitelistFile - file does not exist', async (t) => {
    const nonExistentPath = './non_existent_whitelist.csv';
    await t.exception(() => fileUtils.readAddressesFromWhitelistFile(nonExistentPath),
        errorMessageIncludes(`Whitelist file not found: ${nonExistentPath}`));
});

test('readAddressesFromWhitelistFile - invalid file extension', async (t) => {
    const invalidExtensionPath = './dummy_whitelist.txt';
    await t.exception(() => fileUtils.readAddressesFromWhitelistFile(invalidExtensionPath),
        errorMessageIncludes(`Invalid file format: ${invalidExtensionPath}. Balance migration file must be a CSV file.`));
});

hook('Cleanup dummy whitelist files', async () => {
    [DUMMY_PATH_OK, DUMMY_PATH_DUP, DUMMY_PATH_EMPTY, DUMMY_PATH_BLANK, DUMMY_PATH_BOM, DUMMY_PATH_LARGE].forEach(path => {
        if (fs.existsSync(path)) fs.unlinkSync(path);
    });
});
