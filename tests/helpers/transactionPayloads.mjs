import b4a from "b4a";
import tracCrypto from "trac-crypto-api";
import PeerWallet from "trac-wallet";
import { $TNK } from "../../src/core/state/utils/balance.js";
import { createMessage } from "../../src/utils/buffer.js";
import { OperationType } from "../../src/utils/constants.js";
import { addressToBuffer } from "../../src/core/state/utils/address.js";
import { config } from '../helpers/config.js'
import { sleep } from "../../src/utils/helpers.js";

export const waitForConnection = async node => {
    let attempts = 0
    while (attempts < 60) {
        const count = node.network.validatorConnectionManager.connectionCount()
        if (count > 0) {
            break
        }
        
        await sleep(300);
        attempts++;
    }
}

/**
 * Build a base64-encoded transfer payload and matching tx hash
 * that are compatible with MSB's PartialTransfer validator.
 *
 * This helper mirrors the hashing/signing logic used by
 * PartialOperation.validateSignature, so that tests broadcast
 * transactions the node will accept without touching consensus code.
 *
 * @param {object} context - General context
 * @param {import("../../src/core/state/State.js").default} state - MSB state instance.
 * @param {bigint} [amountTnk=1n] - Transfer amount in TNK units.
 * @returns {Promise<{ payload: string, txHashHex: string }>}
 */
export async function buildRpcSelfTransferPayload(context, state, amountTnk = 1n) {
    const txvBuffer = await state.getIndexerSequenceState();
    const txvHex = b4a.toString(txvBuffer, "hex");

    const txData = await tracCrypto.transaction.preBuild(
        context.wallet.address,
        context.wallet.address,
        b4a.toString($TNK(amountTnk), "hex"),
        txvHex
    );

    const nonceHex = b4a.toString(txData.nonce, "hex");
    const amountHex = txData.amount;
    const toAddress = txData.to;

    const txvBuf = b4a.from(txData.validity, "hex");
    const nonceBuf = b4a.from(nonceHex, "hex");
    const amountBuf = b4a.from(amountHex, "hex");
    const toBuf = addressToBuffer(toAddress, config.addressPrefix);

    const message = createMessage(
        config.networkId,
        txvBuf,
        toBuf,
        amountBuf,
        nonceBuf,
        OperationType.TRANSFER
    );

    const messageHash = await PeerWallet.blake3(message);
    const signature = context.wallet.sign(messageHash);

    const payloadObject = {
        type: OperationType.TRANSFER,
        address: context.wallet.address,
        tro: {
            tx: b4a.toString(messageHash, "hex"),
            txv: txData.validity,
            in: nonceHex,
            to: toAddress,
            am: amountHex,
            is: b4a.toString(signature, "hex")
        }
    };

    const payload = b4a.toString(
        b4a.from(JSON.stringify(payloadObject)),
        "base64"
    );

    return {
        payload,
        txHashHex: b4a.toString(messageHash, "hex")
    };
}

