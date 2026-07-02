import Protomux from 'protomux';
import ProtocolInterface from './ProtocolInterface.js';
import b4a from 'b4a';
import c from 'compact-encoding';

class LegacyProtocol extends ProtocolInterface {
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
        // TODO: Abstract in a separate function
        const mux = Protomux.from(connection);
        connection.userData = mux;

        this.#channel = mux.createChannel({
            protocol: b4a.toString(this.#config.channel, 'utf8'),
            onopen() { },
            onclose() { }
        });

        this.#channel.open();

        // Todo: Abstract in a separate function
        this.#session = this.#channel.addMessage({
            encoding: c.json,
            onmessage: async (incomingMessage) => {
                try {
                    if (typeof incomingMessage === 'object' || typeof incomingMessage === 'string') {
                        await this.#router.route(incomingMessage, connection, this.#session);
                    } else {
                        throw new Error('NetworkMessages: Received message is undefined');
                    }
                } catch (error) {
                    console.error(`NetworkMessages: Failed to handle incoming message: ${error.message}`);
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

export default LegacyProtocol;