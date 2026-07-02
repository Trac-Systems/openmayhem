import { test, hook } from 'brittle';
import migrationUtils from '../../../../src/utils/migrationUtils.js';
import { errorMessageIncludes } from "../../../helpers/regexHelper.js";
import { ZERO_LICENSE } from '../../../../src/core/state/utils/nodeEntry.js';
import b4a from 'b4a';
import { config } from '../../../helpers/config.js';

const VALID_ADDRESS = 'trac1dguwzsvcsehslh6dgj2mqlsxdn7s5t5vhem56yd0xlg47aq6exzqymhr6u';
const ADMIN_ADDRESS = 'trac1yva2pduhz5yst8jgzmrc9ve0as5mx7tcw6le9srj6xcwqkx9hacqxxhsf9';
const INVALID_ADDRESS = 'notanaddress';
const LICENSE_NUMBER_ONE = b4a.alloc(4, 1);

const mockStateInstance = {
    getNodeEntryUnsigned: async () => ({
        isWhitelisted: false,
        license: ZERO_LICENSE
    }),
    getAdminEntry: async () => ({
        address: ADMIN_ADDRESS
    })
};

const mockStateInstanceWhitelisted = {
    getNodeEntryUnsigned: async () => ({
        isWhitelisted: true,
        license: ZERO_LICENSE
    }),
    getAdminEntry: async () => ({
        address: ADMIN_ADDRESS
    })
};

const mockStateInstanceBanned = {
    getNodeEntryUnsigned: async () => ({
        isWhitelisted: false,
        license: LICENSE_NUMBER_ONE
    }),
    getAdminEntry: async () => ({
        address: ADMIN_ADDRESS
    })
};

test('validateAddressFromIncomingFile - valid address', async (t) => {
    await migrationUtils.validateAddressFromIncomingFile(
        mockStateInstance,
        config,
        VALID_ADDRESS,
        { address: ADMIN_ADDRESS }
    );
    t.pass('Address validation succeeded as expected');
});

test('validateAddressFromIncomingFile - invalid address format', async (t) => {
    await t.exception(
        () => migrationUtils.validateAddressFromIncomingFile(
            mockStateInstance,
            config,
            INVALID_ADDRESS,
            { address: ADMIN_ADDRESS }
        ),
        errorMessageIncludes('Invalid address format')
    );
});

test('validateAddressFromIncomingFile - admin address', async (t) => {
    await t.exception(
        () => migrationUtils.validateAddressFromIncomingFile(
            mockStateInstance,
            config,
            ADMIN_ADDRESS,
            { address: ADMIN_ADDRESS }
        ),
        errorMessageIncludes(`The admin address '${ADMIN_ADDRESS}' cannot be included in the current operation`)
    );
});

test('validateAddressFromIncomingFile - whitelisted node', async (t) => {
    await t.exception(
        () => migrationUtils.validateAddressFromIncomingFile(
            mockStateInstanceWhitelisted,
            config,
            VALID_ADDRESS,
            { address: ADMIN_ADDRESS }
        ),
        errorMessageIncludes(`Whitelisted node address '${VALID_ADDRESS}' cannot be included in the current operation`)
    );
});

test('validateAddressFromIncomingFile - banned/previously whitelisted address', async (t) => {
    await t.exception(
        () => migrationUtils.validateAddressFromIncomingFile(
            mockStateInstanceBanned,
            config,
            VALID_ADDRESS,
            { address: ADMIN_ADDRESS }
        ),
        errorMessageIncludes(`Address '${VALID_ADDRESS}' has been banned/whitelisted in the past`)
    );
});
