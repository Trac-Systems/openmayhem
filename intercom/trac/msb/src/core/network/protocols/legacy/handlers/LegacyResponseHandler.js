import ValidatorResponse from '../validators/ValidatorResponse.js';

class LegacyResponseHandler {
    #responseValidator;

    constructor(state, wallet, config) {
        this.#responseValidator = new ValidatorResponse(state, wallet, config);

    }

    async handle(message, channelString) {
        await this.#handleValidatorResponse(message, channelString);
    }

    async #handleValidatorResponse(message, channelString) {
        const isValid = await this.#responseValidator.validate(message, channelString);
        if (!isValid) {
            throw new Error("Validator response verification failed");
        }
    }
}

export default LegacyResponseHandler;
