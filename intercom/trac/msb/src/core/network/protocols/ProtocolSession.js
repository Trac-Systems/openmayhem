// ProtocolSession is a per-peer (per Hyperswarm connection) wrapper that exposes the available
// protocol messengers (legacy JSON, v1 binary) in one place.
//
// Why it exists:
// - `setupProtomuxMessages(connection)` creates Protomux channels/messages for a specific peer.
class ProtocolSession {
    #legacyProtocol;
    #v1Protocol;

    constructor(legacyProtocol, v1Protocol) {
        // These are Protomux "message" objects (returned by channel.addMessage).
        // They are connection-scoped and expose .send(...), already wired to the channel's encoding.
        this.#legacyProtocol = legacyProtocol;
        this.#v1Protocol = v1Protocol;
    }

    getLegacy() {
        return this.#legacyProtocol;
    }

    getV1() {
        return this.#v1Protocol;
    }

    get(protocol) {
        if (protocol === 'legacy') return this.#legacyProtocol;
        if (protocol === 'v1') return this.#v1Protocol;
        return null;
    }

    has(protocol) {
        return Boolean(this.get(protocol));
    }

    send(message) {
        // TODO: Support v1 messages
        this.#legacyProtocol.send(message);
    }

    close() {
        if (this.#legacyProtocol) {
            try {
                this.#legacyProtocol.close();
            } catch (e) {
                console.error('Failed to close legacy channel:', e); // TODO: Think about throwing instead
            }
        }

        if (this.#v1Protocol) {
            try {
                this.#v1Protocol.close();
            } catch (e) {
                console.error('Failed to close v1 channel:', e); // TODO: Think about throwing instead
            }
        }
    }
}

export default ProtocolSession;
