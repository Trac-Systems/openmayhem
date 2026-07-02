/**
 * ProtocolInterface serves as a base class for all network protocol implementations.
 * @interface
 */

class ProtocolInterface {
    
    // TODO: Refactor this so we don't need to pass a reference for the whole network instance
    constructor(router, connection, config) {
        if (new.target === ProtocolInterface) {
            throw new Error('ProtocolInterface cannot be instantiated directly');
        }
    }

    init(connection) {
        // Abstract method. Need to be implemented by subclasses.
        throw new Error('init() method must be implemented by subclass');
    }

    send(message) {
        // Abstract method. Need to be implemented by subclasses.
        throw new Error('send() method must be implemented by subclass');
    }

    close() {
        // Abstract method. Need to be implemented by subclasses.
        throw new Error('close() method must be implemented by subclass');
    }
}

export default ProtocolInterface;
