import {
    handleBalance,
    handleTxv,
    handleFee,
    handleConfirmedLength,
    handleBroadcastTransaction,
    handleTxHashes,
    handleUnconfirmedLength,
    handleTransactionDetails,
    handleFetchBulkTxPayloads,
    handleTransactionExtendedDetails,
    handleAccountDetails
} from '../handlers.js';

export const v1Routes = [
    { method: 'GET', path: '/balance', handler: handleBalance },
    { method: 'GET', path: '/txv', handler: handleTxv },
    { method: 'GET', path: '/fee', handler: handleFee },
    { method: 'GET', path: '/confirmed-length', handler: handleConfirmedLength },
    { method: 'POST', path: '/broadcast-transaction', handler: handleBroadcastTransaction },
    { method: 'GET', path: '/tx-hashes/', handler: handleTxHashes },
    { method: 'GET', path: '/unconfirmed-length', handler: handleUnconfirmedLength },
    { method: 'GET', path: '/tx', handler: handleTransactionDetails },
    { method: 'POST', path: '/tx-payloads-bulk', handler: handleFetchBulkTxPayloads },
    { method: 'GET', path: '/tx/details', handler: handleTransactionExtendedDetails },
    { method: 'GET', path: '/account', handler: handleAccountDetails },
];
