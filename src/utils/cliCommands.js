import { ZERO_WK } from "./buffer.js";
import { bigIntToDecimalString, bufferToBigInt } from "./amountSerialization.js";
import { normalizeDecodedPayloadForJson } from "./normalizers.js";
import { get_confirmed_tx_info, get_unconfirmed_tx_info } from "./cli.js";
import { EntryType } from "./constants.js";
import { bufferToAddress } from "../core/state/utils/address.js";
import deploymentEntryUtils from "../core/state/utils/deploymentEntry.js";
import { safeDecodeApplyOperation } from "./protobuf/operationHelpers.js";

export async function getBalanceCommand(state, address, confirmedFlag) {
    const unconfirmedBalance = confirmedFlag === "false";
    const nodeEntry = unconfirmedBalance
        ? await state.getNodeEntryUnsigned(address)
        : await state.getNodeEntry(address);

    if (nodeEntry) {
        console.log({
            Address: address,
            Balance: bigIntToDecimalString(bufferToBigInt(nodeEntry.balance))
        });
        return {
            address,
            balance: bufferToBigInt(nodeEntry.balance).toString()
        };
    }

    console.log("Node Entry:", {
        WritingKey: ZERO_WK.toString("hex"),
        IsWhitelisted: false,
        IsWriter: false,
        IsIndexer: false,
        balance: bigIntToDecimalString(0n)
    });
}

export async function nodeStatusCommand(state, address) {
    const nodeEntry = await state.getNodeEntry(address);
    if (nodeEntry) {
        const licenseValue = nodeEntry.license.readUInt32BE(0);
        const licenseDisplay = licenseValue === 0 ? "N/A" : licenseValue.toString();
        console.log("Node Status:", {
            Address: address,
            WritingKey: nodeEntry.wk.toString("hex"),
            IsWhitelisted: nodeEntry.isWhitelisted,
            IsWriter: nodeEntry.isWriter,
            IsIndexer: nodeEntry.isIndexer,
            License: licenseDisplay,
            StakedBalance: bigIntToDecimalString(bufferToBigInt(nodeEntry.stakedBalance)),
            Balance: bigIntToDecimalString(bufferToBigInt(nodeEntry.balance))
        });
        return {
            address,
            writingKey: nodeEntry.wk.toString("hex"),
            isWhitelisted: nodeEntry.isWhitelisted,
            isWriter: nodeEntry.isWriter,
            isIndexer: nodeEntry.isIndexer,
            license: licenseDisplay,
            stakedBalance: bigIntToDecimalString(bufferToBigInt(nodeEntry.stakedBalance))
        };
    }

    console.log("Node Status:", {
        WritingKey: ZERO_WK.toString("hex"),
        IsWhitelisted: false,
        IsWriter: false,
        IsIndexer: false,
        license: "N/A",
        stakedBalance: "0"
    });
}

export async function coreInfoCommand(state) {
    const admin = await state.getAdminEntry();
    console.log("Admin:", admin ? {
        address: admin.address,
        writingKey: admin.wk.toString("hex")
    } : null);
    const formattedIndexers = await state.getIndexersEntry().then(entry => entry ? entry : null);
    if (!formattedIndexers || (Array.isArray(formattedIndexers) && formattedIndexers.length === 0)) {
        console.log("Indexers: no indexers");
    } else {
        console.log("Indexers:", formattedIndexers);
    }
}

export async function getValidatorAddressCommand(state, wkHexString, prefix) {
    const payload = await state.getSigned(EntryType.WRITER_ADDRESS + wkHexString);
    if (payload === null) {
        console.log(`No address assigned to the writer key: ${wkHexString}`);
    } else {
        console.log(`Address assigned to the writer key: ${wkHexString} - ${bufferToAddress(payload, prefix)}`);
    }
}

export async function getDeploymentCommand(state, bootstrapHex, addressLength) {
    const deploymentEntry = await state.getRegisteredBootstrapEntry(bootstrapHex);
    console.log(`Searching deployment for bootstrap: ${bootstrapHex}`);
    if (deploymentEntry) {
        const decodedDeploymentEntry = deploymentEntryUtils.decode(deploymentEntry, addressLength);
        const txhash = decodedDeploymentEntry.txHash.toString("hex");
        console.log(`Bootstrap deployed under transaction hash: ${txhash}`);
        const payload = await state.getSigned(txhash);
        if (payload) {
            const decoded = safeDecodeApplyOperation(payload);
            console.log("Decoded Bootstrap Deployment Payload:", decoded);
        } else {
            console.log(`No payload found for transaction hash: ${txhash}`);
        }
    } else {
        console.log(`No deployment found for bootstrap: ${bootstrapHex}`);
    }
}

export async function getTxInfoCommand(state, txHash) {
    const txInfo = await get_confirmed_tx_info(state, txHash);
    if (txInfo) {
        console.log(`Payload for transaction hash ${txHash}:`);
        console.log(txInfo.decoded);
    } else {
        console.log(`No information found for transaction hash: ${txHash}`);
    }
}

export async function getLicenseNumberCommand(state, address) {
    const nodeEntry = await state.getNodeEntry(address);
    if (nodeEntry) {
        console.log({
            Address: address,
            License: bufferToBigInt(nodeEntry.license).toString()
        });
    }
}

export async function getLicenseAddressCommand(state, licenseId) {
    if (isNaN(licenseId) || licenseId < 0) {
        console.log("Invalid license ID. Please provide a valid non-negative number.");
        return;
    }

    const address = await state.getAddressByLicenseId(licenseId);
    if (address) {
        console.log({
            LicenseId: licenseId,
            Address: address
        });
    } else {
        console.log(`No address found for license ID: ${licenseId}`);
    }
}

export async function getLicenseCountCommand(state, isAdminFn) {
    const adminEntry = await state.getAdminEntry();

    if (!adminEntry) {
        throw new Error("Cannot read license count. Admin does not exist");
    }

    if (!isAdminFn(adminEntry)) {
        throw new Error("Cannot perform this operation - you are not the admin!.");
    }

    const licenseCount = await state.getLicenseCount();
    console.log({
        LicensesCount: licenseCount
    });
}

export async function getTxvCommand(state) {
    const txv = await state.getIndexerSequenceState();
    console.log("Current TXV:", txv.toString("hex"));
    return txv;
}

export function getFeeCommand(state) {
    const fee = state.getFee();
    console.log("Current FEE:", bigIntToDecimalString(bufferToBigInt(fee)));
    return bufferToBigInt(fee).toString();
}

export function getConfirmedLengthCommand(state) {
    const confirmedLength = state.getSignedLength();
    console.log("Confirmed_length:", confirmedLength);
    return confirmedLength;
}

export function getUnconfirmedLengthCommand(state) {
    const unconfirmedLength = state.getUnsignedLength();
    console.log("Unconfirmed_length:", unconfirmedLength);
    return unconfirmedLength;
}

export async function getTxPayloadsBulkCommand(state, hashes, config) {
    if (!Array.isArray(hashes) || hashes.length === 0) {
        throw new Error("Missing hash list.");
    }

    if (hashes.length > 1500) {
        throw new Error("Length of input tx hashes exceeded.");
    }

    const res = { results: [], missing: [] };

    const promises = hashes.map(hash => get_confirmed_tx_info(state, hash));
    const results = await Promise.all(promises);

    results.forEach((result, index) => {
        const hash = hashes[index];
        if (result === null || result === undefined) {
            res.missing.push(hash);
        } else {
            const decodedResult = normalizeDecodedPayloadForJson(result.decoded, config);
            res.results.push({ hash, payload: decodedResult });
        }
    });

    return res;
}

export async function getTxHashesCommand(state, start, end) {
    try {
        const hashes = await state.confirmedTransactionsBetween(start, end);
        return { hashes };
    } catch (error) {
        throw new Error("Invalid params to perform the request.", error.message);
    }
}

export async function getTxDetailsCommand(state, hash, config) {
    try {
        const rawPayload = await get_confirmed_tx_info(state, hash);
        if (!rawPayload) {
            console.log(`No payload found for tx hash: ${hash}`);
            return null;
        }
        return normalizeDecodedPayloadForJson(rawPayload.decoded, config);
    } catch (error) {
        throw new Error("Invalid params to perform the request.", error.message);
    }
}

export async function getExtendedTxDetailsCommand(state, hash, confirmed, config) {
    if (confirmed) {
        const rawPayload = await get_confirmed_tx_info(state, hash);
        if (!rawPayload) {
            throw new Error(`No payload found for tx hash: ${hash}`);
        }
        const confirmedLength = await state.getTransactionConfirmedLength(hash);
        if (confirmedLength === null) {
            throw new Error(`No confirmed length found for tx hash: ${hash} in confirmed mode`);
        }
        const normalizedPayload = normalizeDecodedPayloadForJson(rawPayload.decoded, config);
        const fee = state.getFee();
        return {
            txDetails: normalizedPayload,
            confirmed_length: confirmedLength,
            fee: bufferToBigInt(fee).toString()
        };
    }

    const rawPayload = await get_unconfirmed_tx_info(state, hash);
    if (!rawPayload) {
        throw new Error(`No payload found for tx hash: ${hash}`);
    }
    const normalizedPayload = normalizeDecodedPayloadForJson(rawPayload.decoded, config);
    const length = await state.getTransactionConfirmedLength(hash);
    if (length === null) {
        return {
            txDetails: normalizedPayload,
            confirmed_length: 0,
            fee: "0"
        };
    }

    const fee = state.getFee();
    return {
        txDetails: normalizedPayload,
        confirmed_length: length,
        fee: bufferToBigInt(fee).toString()
    };
}
