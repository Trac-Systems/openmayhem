export class Logger {
    #config
    constructor(config) {
        this.#config = config;
    }

    info(message) {
        console.log("i: " + message);
    }

    debug(message) {
        if (this.#config.debug) {
            console.debug("d: " + message);
        }
    }

    error(message) {
        console.error("e: " + message);
    }

    warn(message) {
        console.warn("w: " + message);
    }

}
