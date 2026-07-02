import { OperationType as ApplyOperationType } from './protobuf/applyOperations.cjs';
import { MessageType as NetworkMessageType, ResultCode as NetworkResultCode } from './protobuf/network.cjs';
import b4a from 'b4a'
// TODO: We are going to have a lot of contstants. It would be good, to separate them into different files.

//ATTENTION - THIS IS USED IN THE APPLY FUNCTION!
export const EntryType = Object.freeze({
    ADMIN: 'admin',
    INDEXERS: 'indexers',
    WRITERS_LENGTH: 'wrl',
    WRITERS_INDEX: 'wri/',
    WRITER_ADDRESS: 'wka/',
    DEPLOYMENT: 'deployment/',
    INITIALIZATION: 'init',
    LICENSE_COUNT: 'lc',
    LICENSE_INDEX: 'li/',
});

//ATTENTION - THIS IS USED IN THE APPLY FUNCTION!
export const OperationType = Object.freeze({
    ADD_ADMIN: ApplyOperationType.ADD_ADMIN,
    DISABLE_INITIALIZATION: ApplyOperationType.DISABLE_INITIALIZATION,
    BALANCE_INITIALIZATION: ApplyOperationType.BALANCE_INITIALIZATION,
    APPEND_WHITELIST: ApplyOperationType.APPEND_WHITELIST,
    ADD_WRITER: ApplyOperationType.ADD_WRITER,
    REMOVE_WRITER: ApplyOperationType.REMOVE_WRITER,
    ADMIN_RECOVERY: ApplyOperationType.ADMIN_RECOVERY,
    ADD_INDEXER: ApplyOperationType.ADD_INDEXER,
    REMOVE_INDEXER: ApplyOperationType.REMOVE_INDEXER,
    BAN_VALIDATOR: ApplyOperationType.BAN_VALIDATOR,
    BOOTSTRAP_DEPLOYMENT: ApplyOperationType.BOOTSTRAP_DEPLOYMENT,
    TX: ApplyOperationType.TX,
    TRANSFER: ApplyOperationType.TRANSFER,
});

export const NetworkOperationType = Object.freeze({
    VALIDATOR_CONNECTION_REQUEST: NetworkMessageType.MESSAGE_TYPE_VALIDATOR_CONNECTION_REQUEST,
    VALIDATOR_CONNECTION_RESPONSE: NetworkMessageType.MESSAGE_TYPE_VALIDATOR_CONNECTION_RESPONSE,
    LIVENESS_REQUEST: NetworkMessageType.MESSAGE_TYPE_LIVENESS_REQUEST,
    LIVENESS_RESPONSE: NetworkMessageType.MESSAGE_TYPE_LIVENESS_RESPONSE,
    BROADCAST_TRANSACTION_REQUEST: NetworkMessageType.MESSAGE_TYPE_BROADCAST_TRANSACTION_REQUEST,
    BROADCAST_TRANSACTION_RESPONSE: NetworkMessageType.MESSAGE_TYPE_BROADCAST_TRANSACTION_RESPONSE,
});

export const ResultCode = Object.freeze({
    OK: NetworkResultCode.RESULT_CODE_OK,
    INVALID_PAYLOAD: NetworkResultCode.RESULT_CODE_INVALID_PAYLOAD,
    UNSUPPORTED_VERSION: NetworkResultCode.RESULT_CODE_UNSUPPORTED_VERSION,
    RATE_LIMITED: NetworkResultCode.RESULT_CODE_RATE_LIMITED,
    TIMEOUT: NetworkResultCode.RESULT_CODE_TIMEOUT,
    SIGNATURE_INVALID: NetworkResultCode.RESULT_CODE_SIGNATURE_INVALID,
});

// Role managment constants
export const EventType = Object.freeze({
    WRITER_EVENT: 'writer-event',
    IS_INDEXER: 'is-indexer',
    IS_NON_INDEXER: 'is-non-indexer',
    WRITABLE: 'writable',
    UNWRITABLE: 'unwritable',
    WARNING: 'warning',
    VALIDATOR_CONNECTION_READY: 'validator-connection-ready',
    VALIDATOR_CONNECTION_TIMEOUT: 'validator-connection-timeout',
});

// Role managment constants
export const CustomEventType = Object.freeze({
    IS_INDEXER: 'msb:is-indexer',
    UNWRITABLE: 'msb:unwritable'
});

// Token
export const TOKEN_DECIMALS = 18n
export const ADMIN_INITIAL_BALANCE = b4a.from([0, 0, 0, 0, 0, 0, 0, 54, 53, 201, 173, 197, 222, 160, 0, 0]) // 1000 in a 18 decimals token
export const ADMIN_INITIAL_STAKED_BALANCE = b4a.from([0, 0, 0, 0, 0, 0, 0, 0,4, 41, 208, 105, 24, 158, 0, 0]) // 0.3 in a 18 decimals token

export const WHITELIST_FILEPATH = './whitelist/addresses.csv';
export const BALANCE_MIGRATION_FILEPATH = './migration/initial_balances.csv';
export const BALANCE_MIGRATED_DIR = './migration/migrated/';
export const WHITELIST_MIGRATION_DIR = './whitelist/migrated/';
export const TRAC_NAMESPACE = 'TracNetwork';
export const WHITELIST_SLEEP_INTERVAL = 1_000;
export const BALANCE_MIGRATION_SLEEP_INTERVAL = 500;

// Connectivity constants
export const MAX_PEERS = 64;
export const MAX_PARALLEL = 64;
export const MAX_SERVER_CONNECTIONS = Infinity;
export const MAX_CLIENT_CONNECTIONS = Infinity;
export const MAX_WRITERS_FOR_ADMIN_INDEXER_CONNECTION = 10;

// State
export const ACK_INTERVAL = 1_000;
export const AUTOBASE_VALUE_ENCODING = 'binary';
export const HYPERBEE_KEY_ENCODING = 'ascii';
export const HYPERBEE_VALUE_ENCODING = 'binary';
// check.js

//ATTENTION - THIS IS USED IN THE APPLY FUNCTION!
export const WRITER_BYTE_LENGTH = 32;
export const BOOTSTRAP_BYTE_LENGTH = 32;
export const CHANNEL_BYTE_LENGTH = 32;
export const NONCE_BYTE_LENGTH = 32;
export const HASH_BYTE_LENGTH = 32;
export const BALANCE_BYTE_LENGTH = 16;
export const SIGNATURE_BYTE_LENGTH = 64;
export const AMOUNT_BYTE_LENGTH = 16;
export const MIN_SAFE_VALIDATION_INTEGER = 0x00000001;
export const MAX_SAFE_VALIDATION_INTEGER = 0xFFFFFFFF;

export const LICENSE_BYTE_LENGTH = 4;

// index.js
export const BOOTSTRAP_HEXSTRING_LENGTH = 64;

// Pool constants
export const BATCH_SIZE = 10;
export const PROCESS_INTERVAL_MS = 50;

// Rate limiting constants
export const CLEANUP_INTERVAL_MS = 120_000;
export const CONNECTION_TIMEOUT_MS = 60_000;
export const MAX_TRANSACTIONS_PER_SECOND = 50;

// Operation handler constants
export const MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE = 3072;
export const TRANSACTION_POOL_SIZE = 1000;

// Network message constants
export const NETWORK_MESSAGE_TYPES = Object.freeze({
    GET: {
        VALIDATOR: 'get_validator_pop',
        ADMIN: 'get_admin_pop',
        NODE: 'get_node_pop'
    },

    RESPONSE: {
        VALIDATOR: 'validatorResponse',
        ADMIN: 'adminResponse',
        NODE: 'nodeResponse'
    },
});
