import NetworkMessageDirector from "./NetworkMessageDirector.js";
import NetworkMessageBuilder from "./NetworkMessageBuilder.js";

/**
 * Factory helper to create a director with a fresh builder instance.
 * @param {PeerWallet} wallet
 * @param {object} config
 * @returns {NetworkMessageDirector}
 */
export const networkMessageFactory = (wallet, config) => {
    return new NetworkMessageDirector(new NetworkMessageBuilder(wallet, config))
}
