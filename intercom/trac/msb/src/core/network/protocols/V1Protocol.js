import Protomux from 'protomux';
import ProtocolInterface from './ProtocolInterface.js';
import b4a from 'b4a';
import c from 'compact-encoding';

class V1Protocol extends ProtocolInterface {
    #channel;
    #session;
    #config;
    #router;

    constructor(router, connection, config) {
        super(router, connection, config);
        this.#config = config;
        this.#router = router;
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
            onopen() { },
            onclose() { }
        });

        this.#channel.open();

        this.#session = this.#channel.addMessage({
            encoding: c.raw,
            onmessage: async (incomingMessage) => {
                try {
                    if (b4a.isBuffer(incomingMessage)) {
                        await this.#router.route(incomingMessage, connection, this.#session);
                    } else {
                        throw new Error('NetworkMessages: v1 message must be a buffer');
                    }
                } catch (error) {
                    console.error(`NetworkMessages: Failed to handle incoming v1 message: ${error.message}`);
                }
            }
        });
    }

    send(message) {
        this.#session.send(message);
    }

    close() {
        this.#channel.close();
    }
}

export default V1Protocol;