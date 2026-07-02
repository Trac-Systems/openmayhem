import { decodeBase64Payload, isBase64, sanitizeBulkPayloadsRequestBody, sanitizeTransferPayload, validatePayloadStructure } from "./utils/helpers.js"
import { MAX_SIGNED_LENGTH, ZERO_WK } from "./constants.js";
import { buildRequestUrl } from "./utils/url.js";
import { isHexString } from "../src/utils/helpers.js";
import {
    getBalance,
    getTxv,
    getFee,
    getConfirmedLength,
    getUnconfirmedLength,
    broadcastTransaction,
    getTxHashes,
    getTxDetails,
    fetchBulkTxPayloads,
    getExtendedTxDetails
} from "./rpc_services.js";
import { bufferToBigInt, licenseBufferToBigInt } from "../src/utils/amountSerialization.js";
import { isAddressValid } from "../src/core/state/utils/address.js";
import { getConfirmedParameter } from "./utils/confirmedParameter.js";

export async function handleBalance({ req, respond, msbInstance }) {
    const url = buildRequestUrl(req);
    const parts = url.pathname.split("/").filter(Boolean);
    const address = parts[2];

    const confirmedParam = getConfirmedParameter(url);
    const confirmed = confirmedParam === null ? false : confirmedParam; // invalid -> fallback to unconfirmed

    // TODO: VALIDATION? 
    if (!address) {
        respond(400, { error: 'Wallet address is required' });
        return;
    }

    const nodeInfo = await getBalance(msbInstance, address, confirmed);
    const balance = nodeInfo?.balance || 0;
    respond(200, { address, balance });
}

export async function handleTxv({ msbInstance, respond }) {
    const txv = await getTxv(msbInstance);
    respond(200, { txv });
}

export async function handleFee({ msbInstance, respond }) {
    const fee = await getFee(msbInstance);
    respond(200, { fee });
}

export async function handleConfirmedLength({ msbInstance, respond }) {
    const confirmed_length = await getConfirmedLength(msbInstance);
    respond(200, { confirmed_length });
}

export async function handleBroadcastTransaction({ msbInstance, config, respond, req }) {
    let body = '';
    req.on('data', chunk => {
        body += chunk.toString();
    });

    req.on('end', async () => {
        try {
            const { payload } = JSON.parse(body);
            if (!payload) {
                return respond(400, { error: 'Payload is missing.' });
            }

            if (!isBase64(payload)) {
                return respond(400, { error: 'Payload must be a valid base64 string.' });
            }

            const decodedPayload = decodeBase64Payload(payload);
            validatePayloadStructure(decodedPayload);
            const sanitizedPayload = sanitizeTransferPayload(decodedPayload);
            const result = await broadcastTransaction(msbInstance, config, sanitizedPayload);
            respond(200, { result });
        } catch (error) {
            let code = error instanceof SyntaxError ? 400 : 500;
            let errorMsg = code === 400 ? 'Invalid JSON payload.' : 'An error occurred processing the transaction.'

            if (error.message.includes("Failed to broadcast transaction after multiple attempts.")) {
                code = 429;
                errorMsg = "Failed to broadcast transaction after multiple attempts."
            }

            console.error('Error in handleBroadcastTransaction:', error);
            // Use 400 for client errors (like bad JSON), 500 for server/command errors
            respond(code, { error: errorMsg });
        }
    });

    req.on('error', (err) => {
        console.error('Stream error in handleBroadcastTransaction:', err);
        respond(500, { error: 'Request stream failed during body transfer.' });
    });
}

export async function handleTxHashes({ msbInstance, respond, req }) {
    const url = buildRequestUrl(req);
    const pathParts = url.pathname.split('/').filter(Boolean);
    const startSignedLengthStr = pathParts[2];
    const endSignedLengthStr = pathParts[3];

    const startSignedLength = parseInt(startSignedLengthStr);
    const endSignedLength = parseInt(endSignedLengthStr);

    // 1. Check if the parsed values are valid numbers
    if (isNaN(startSignedLength) || isNaN(endSignedLength)) {
        return respond(400, { error: 'Params must be integer' });
    }

    // 2. Check for non-negative numbers
    // The requirement is "non-negative," which includes 0.
    if (startSignedLength < 0 || endSignedLength < 0) {
        return respond(400, { error: 'Params must be non-negative' });
    }

    // 3. endSignedLength must be >= startSignedLength
    if (endSignedLength < startSignedLength) {
        return respond(400, { error: 'endSignedLength must be greater than or equal to startSignedLength.' });
    }

    if (endSignedLength - startSignedLength > MAX_SIGNED_LENGTH) {
        return respond(400, { error: `The max range for signedLength must be ${MAX_SIGNED_LENGTH}.` });
    }

    // 4. Get current confirmed length
    const currentConfirmedLength = await getConfirmedLength(msbInstance);

    // 5. Adjust the end index to not exceed the confirmed length.
    const adjustedEndLength = Math.min(endSignedLength, currentConfirmedLength)

    // 6. Fetch txs hashes for the adjusted range, assuming the command takes start and end index.
    const { hashes } = await getTxHashes(msbInstance, startSignedLength, adjustedEndLength);
    respond(200, { hashes });
}

export async function handleUnconfirmedLength({ msbInstance, respond }) {
    const unconfirmed_length = await getUnconfirmedLength(msbInstance);
    respond(200, { unconfirmed_length });
}

export async function handleTransactionDetails({ msbInstance, respond, req }) {
    const hash = req.url.split('/')[3];
    const txDetails = await getTxDetails(msbInstance, hash);
    respond(txDetails === null ? 404 : 200 , { txDetails });
}

export async function handleFetchBulkTxPayloads({ msbInstance, respond, req }) {
    let body = ''
    let bytesRead = 0;
    let limitBytes = 1_000_000;
    let headersSent = false; // Add a flag to prevent double response

    req.on('data', chunk => {
        if (headersSent) return; // Stop processing if response has started/errored

        bytesRead += chunk.length;
        if (bytesRead > limitBytes) {
            respond(413, { error: 'Request body too large.' });
            headersSent = true;
            req.destroy(); // Stop receiving data (GOOD PRACTICE)
            return;
        }
        body += chunk.toString();
    });

    req.on('end', async () => {
        if (headersSent) return; // Don't process if an error already occurred


        try {
            if (body === null || body === '') {
                return respond(400, { error: 'Missing payload.' });
            }

            const sanitizedPayload = sanitizeBulkPayloadsRequestBody(body);

            if (sanitizedPayload === null) {
                return respond(400, { error: 'Invalid payload.' });
            }

            const { hashes } = sanitizedPayload;

            if (!Array.isArray(hashes) || hashes.length === 0) {
                return respond(400, { error: 'Missing hash list.' });
            }

            if (hashes.length > 1500) {
                return respond(413, { error: 'Too many hashes. Max 1500 allowed per request.' });
            }

            const uniqueHashes = [...new Set(hashes)];

            const commandResult = await fetchBulkTxPayloads(msbInstance, uniqueHashes);

            const responseString = JSON.stringify(commandResult);
            if (Buffer.byteLength(responseString, 'utf8') > 2_000_000) {
                return respond(413, { error: 'Response too large. Reduce number of hashes.' });
            }

            return respond(200, commandResult);
        } catch (error) {
            console.error('Error in handleFetchBulkTxPayloads:', error);
            // Use 400 for JSON errors, 500 otherwise
            const code = error instanceof SyntaxError ? 400 : 500;
            respond(code, { error: code === 400 ? 'Invalid request body format.' : 'An internal error occurred.' });
        }
    })

    req.on('error', (err) => {
        console.error('Stream error in handleFetchBulkTxPayloads:', err);
        respond(500, { error: 'Request stream failed during body transfer.' });
    });
}

export async function handleTransactionExtendedDetails({ msbInstance, respond, req }) {
    const url = buildRequestUrl(req);
    const pathParts = url.pathname.split('/').filter(Boolean);
    const hash = pathParts[3];

    if (!hash) {
        return respond(400, { error: "Transaction hash is required" });
    }

    if (isHexString(hash) === false || hash.length !== 64) {
        return respond(400, { error: "Invalid transaction hash format" });
    }

    const confirmed = getConfirmedParameter(url);
    if (confirmed === null) {
        return respond(400, { error: 'Parameter "confirmed" must be exactly "true" or "false"' });
    }

    try {
        const details = await getExtendedTxDetails(msbInstance, hash, confirmed);
        respond(200, details);
    } catch (error) {
        if (error.message?.includes('No payload found for tx hash')) {
            respond(404, { error: error.message });
        } else {
            console.error('Error in handleTransactionDetails:', error);
            respond(500, { error: 'An error occurred processing the request.' });
        }
    }
}

export async function handleAccountDetails({ msbInstance, respond, req }) {
    const url = buildRequestUrl(req);
    const address = url.pathname.split('/').filter(Boolean)[2];

    if (!address) {
        return respond(400, { error: "Account address is required" });
    }

    const confirmed = getConfirmedParameter(url);
    if (confirmed === null) {
        return respond(400, { error: 'Parameter "confirmed" must be exactly "true" or "false"' });
    }

    if (!isAddressValid(address, msbInstance.config.addressPrefix)) {
        return respond(400, { error: "Invalid account address format" });
    }

    const defaultAccountState = {
        address,
        writingKey: ZERO_WK.toString('hex'),
        isWhitelisted: false,
        isValidator: false,
        isIndexer: false,
        license: null,
        balance: '0',
        stakedBalance: '0',
    };

    const nodeEntry = confirmed
        ? await msbInstance.state.getNodeEntry(address)
        : await msbInstance.state.getNodeEntryUnsigned(address);
    if (!nodeEntry) {
        return respond(200, defaultAccountState);
    }

    const licenseValue = licenseBufferToBigInt(nodeEntry.license);

    return respond(200, {
        ...defaultAccountState,
        writingKey: nodeEntry.wk.toString('hex'),
        isWhitelisted: nodeEntry.isWhitelisted,
        isValidator: nodeEntry.isWriter,
        isIndexer: nodeEntry.isIndexer,
        license: licenseValue === 0n ? null : licenseValue.toString(),
        balance: bufferToBigInt(nodeEntry.balance).toString(),
        stakedBalance: bufferToBigInt(nodeEntry.stakedBalance).toString(),
    });
}
