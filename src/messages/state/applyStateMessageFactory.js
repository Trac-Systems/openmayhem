import ApplyStateMessageDirector from "./ApplyStateMessageDirector.js";
import ApplyStateMessageBuilder from "./ApplyStateMessageBuilder.js";

/**
 * Factory helper to create a director with a builder instance.
 * @param {PeerWallet} wallet
 * @param {object} config
 * @returns {ApplyStateMessageDirector}
 */
export const applyStateMessageFactory = (wallet, config) =>{
    return new ApplyStateMessageDirector(new ApplyStateMessageBuilder(wallet, config))
}
