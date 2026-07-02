import { MessageHeader } from '../../../../utils/protobuf/network.cjs';

class NetworkMessageRouterV1 {
    #config;

    constructor(config) {
        this.#config = config;
    }

    async route(incomingMessage) {
        MessageHeader.decode(incomingMessage);
    }
}

export default NetworkMessageRouterV1;
