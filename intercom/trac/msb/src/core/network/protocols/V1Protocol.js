import Protomux from 'protomux';
import ProtocolInterface from './ProtocolInterface.js';
import c from 'compact-encoding';
import {encodeV1networkOperation, decodeV1networkOperation} from '../../../utils/protobuf/operationHelpers.js';

class V1Protocol extends ProtocolInterface {
    #channel;
    #session;
    #router;
    #publicKeyHex;
    #pendingRequestService;

    constructor(router, connection, pendingRequestService, config) {
        super(router, connection, pendingRequestService, config);
        this.#router = router;
        this.#publicKeyHex = connection.remotePublicKey.toString('hex');
        this.#pendingRequestService = pendingRequestService;
        this.init(connection);
    }

    get channel() {
        return this.#channel;
    }

    get session() {
        return this.#session;
    }

    init(connection) {
        const mux = Protomux.from(connection);
        connection.userData = mux;

        this.#channel = mux.createChannel({
            protocol: 'network/v1',
            onopen() {
            },
            onclose() {
            }
        });

        this.#channel.open();

        this.#session = this.#channel.addMessage({
            encoding: c.raw,
            onmessage: (incomingMessage) => {
                this.#router.route(incomingMessage, connection).catch((err) => {
                    console.error(`V1Protocol: unhandled router error: ${err.message}`);
                    try {
                        connection.end();
                    } catch {
                    }
                });
            }
        });
    }

    // TODO: Consider making this method private/internal-only, just like 'encode'
    // NOTE: This method might be moved to v1/NetworkMessageRouter.js as it is only used there
    decode(message) {
        return decodeV1networkOperation(message);
    }

    async send(message) {
        const encodedMessage = encodeV1networkOperation(message);
        const msgReplyPromise = this.#pendingRequestService.registerPendingRequest(this.#publicKeyHex, message);
        try {
            this.#session.send(encodedMessage);
        } catch (error) {
            this.#pendingRequestService.rejectPendingRequest(message.id, error);
        }
        return msgReplyPromise;
    }

    sendAndForget(message) {
        this.#session.send(encodeV1networkOperation(message));
    }

    close() {
        this.#channel.close();
    }
}

export default V1Protocol;