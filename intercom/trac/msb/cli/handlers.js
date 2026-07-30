import { ZERO_WK } from "../src/utils/buffer.js";
import { bigIntToDecimalString, bufferToBigInt } from "../src/utils/amountSerialization.js";
import { EntryType } from "../src/utils/constants.js";
import { bufferToAddress } from "../src/core/state/utils/address.js";
import deploymentEntryUtils from "../src/core/state/utils/deploymentEntry.js";
import { safeDecodeApplyOperation } from "../src/utils/protobuf/operationHelpers.js";

export class Handlers {
    #msb
    #config

    constructor(msb, config) {
        this.#msb = msb
        this.#config = config
    }

    async handleNodeStatus(address) {
        const nodeEntry = await this.#msb.state.getNodeEntry(address)
        if (nodeEntry) {
            const licenseValue = nodeEntry.license.readUInt32BE(0)
            const licenseDisplay = licenseValue === 0 ? "N/A" : licenseValue.toString()
            console.log("Node Status:", {
                Address: address,
                WritingKey: nodeEntry.wk.toString("hex"),
                IsWhitelisted: nodeEntry.isWhitelisted,
                IsWriter: nodeEntry.isWriter,
                IsIndexer: nodeEntry.isIndexer,
                License: licenseDisplay,
                StakedBalance: bigIntToDecimalString(bufferToBigInt(nodeEntry.stakedBalance)),
                Balance: bigIntToDecimalString(bufferToBigInt(nodeEntry.balance))
            })
            return {
                address,
                writingKey: nodeEntry.wk.toString("hex"),
                isWhitelisted: nodeEntry.isWhitelisted,
                isWriter: nodeEntry.isWriter,
                isIndexer: nodeEntry.isIndexer,
                license: licenseDisplay,
                stakedBalance: bigIntToDecimalString(bufferToBigInt(nodeEntry.stakedBalance)),
                balance: bigIntToDecimalString(bufferToBigInt(nodeEntry.balance))
            }
        }

        console.log("Node Status:", {
            WritingKey: ZERO_WK.toString("hex"),
            IsWhitelisted: false,
            IsWriter: false,
            IsIndexer: false,
            license: "N/A",
            stakedBalance: "0"
        })
    }

    async handleCoreInfo() {
        const admin = await this.#msb.state.getAdminEntry()
        console.log("Admin:", admin ? {
            address: admin.address,
            writingKey: admin.wk.toString("hex")
        } : null)

        const formattedIndexers = await this.#msb.state.getIndexersEntry().then(entry => entry ? entry : null)
        if (!formattedIndexers || (Array.isArray(formattedIndexers) && formattedIndexers.length === 0)) {
            console.log("Indexers: no indexers")
        } else {
            console.log("Indexers:", formattedIndexers)
        }
    }

    async handleValidatorAddress(wkHexString) {
        const payload = await this.#msb.state.getSigned(EntryType.WRITER_ADDRESS + wkHexString)
        if (payload === null) {
            console.log(`No address assigned to the writer key: ${wkHexString}`)
        } else {
            console.log(
                `Address assigned to the writer key: ${wkHexString} - ${bufferToAddress(payload, this.#config.addressPrefix)}`
            )
        }
    }

    async handleDeployment(bootstrapHex) {
        const deploymentEntry = await this.#msb.state.getRegisteredBootstrapEntry(bootstrapHex)
        if (deploymentEntry) {
            const decodedDeploymentEntry = deploymentEntryUtils.decode(deploymentEntry, this.#config.addressLength)
            const txhash = decodedDeploymentEntry.txHash.toString("hex")
            console.log(`Bootstrap deployed under transaction hash: ${txhash}`)
            const payload = await this.#msb.state.getSigned(txhash)
            if (payload) {
                const decoded = safeDecodeApplyOperation(payload)
                console.log("Decoded Bootstrap Deployment Payload:", decoded)
            } else {
                console.log(`No payload found for transaction hash: ${txhash}`)
            }
        } else {
            console.log(`No deployment found for bootstrap: ${bootstrapHex}`)
        }
    }

    async handleTxInfo(txHash) {
        const txInfo = await this.#msb.getConfirmedTxInfo(txHash)
        if (txInfo) {
            console.log(`Payload for transaction hash ${txHash}:`, txInfo.decoded)
        } else {
            console.log(`No information found for transaction hash: ${txHash}`)
        }
    }

    async handleLicenseNumber(address) {
        const nodeEntry = await this.#msb.state.getNodeEntry(address)
        if (nodeEntry) {
            console.log({
                Address: address,
                License: bufferToBigInt(nodeEntry.license).toString()
            })
        }
    }

    async handleLicenseAddress(licenseId) {
        if (isNaN(licenseId) || licenseId < 0) {
            console.log("Invalid license ID. Please provide a valid non-negative number.")
            return
        }

        const address = await this.#msb.state.getAddressByLicenseId(licenseId)
        if (address) {
            console.log({
                LicenseId: licenseId,
                Address: address
            })
        } else {
            console.log(`No address found for license ID: ${licenseId}`)
        }
    }

    async handleLicenseCount() {
        if (!await this.#msb.state.isAdmin()) {
            throw new Error("Cannot perform this operation - you are not the admin!.")
        }

        const licenseCount = await this.#msb.state.getLicenseCount()
        console.log({
            LicensesCount: licenseCount
        })
    }

    handleFee() {
        const fee = this.#msb.handleGetFee()
        console.log("Current FEE:", bigIntToDecimalString(fee));
    }

    async handleBalance(address, confirmedFlag) {
        const nodeInfo = await this.#msb.getBalance(address, confirmedFlag !== "false");

        if (nodeInfo) {
            console.log({
                Address: address,
                Balance: bigIntToDecimalString(BigInt(nodeInfo.balance))
            });
            return nodeInfo;
        }

        console.log("Node Entry:", {
            WritingKey: ZERO_WK.toString("hex"),
            IsWhitelisted: false,
            IsWriter: false,
            IsIndexer: false,
            balance: bigIntToDecimalString(0n)
        });
    }

    async handleTxv() {
        const txv = await this.#msb.getTxv();
        console.log("Current TXV:", txv);
        return txv;
    }

    handleConfirmedLength() {
        const confirmedLength = this.#msb.getConfirmedLength();
        console.log("Confirmed_length:", confirmedLength);
        return confirmedLength;
    }

    handleUnconfirmedLength() {
        const unconfirmedLength = this.#msb.getUnconfirmedLength();
        console.log("Unconfirmed_length:", unconfirmedLength);
        return unconfirmedLength;
    }

    async handleTxHashes(start, end) {
        return this.#msb.getTxHashes(start, end);
    }

    async handleTxDetails(hash) {
        const txDetails = await this.#msb.getTxDetails(hash);
        if (!txDetails) {
            console.log(`No payload found for tx hash: ${hash}`);
            return null;
        }

        return txDetails;
    }

    async handleExtendedTxDetails(hash, confirmed) {
        const txDetails = await this.#msb.getExtendedTxDetails(hash, confirmed);
        if (!txDetails) {
            throw new Error(`No payload found for tx hash: ${hash}`);
        }

        return txDetails;
    }
}
