/**
 * ProtocolInterface serves as a base class for all network protocol implementations.
 * @interface
 */

class ProtocolInterface {
    
    // TODO: Refactor this so we don't need to pass a reference for the whole network instance
    constructor(_router, _connection, _pendingRequestService, _config) {
        if (new.target === ProtocolInterface) {
            throw new Error('ProtocolInterface cannot be instantiated directly');
        }
    }

    init(_connection) {
        // Abstract method. Need to be implemented by subclasses.
        throw new Error('init() method must be implemented by subclass');
    }

    // TODO: This method is only kept here because of v1, but it should be probably removed from the interface
    // Remove it after we finish refactoring v1 protocol
    decode(_message) {
        // Abstract method. Need to be implemented by subclasses.
        throw new Error('decode() method must be implemented by subclass');
    }

    async send(_message) {
        // Abstract method. Need to be implemented by subclasses.
        throw new Error('send() method must be implemented by subclass');
    }

    sendAndForget(_message) {
        // Abstract method. Need to be implemented by subclasses.
        throw new Error('sendAndForget() method must be implemented by subclass');
    }

    close() {
        // Abstract method. Need to be implemented by subclasses.
        throw new Error('close() method must be implemented by subclass');
    }
}

export default ProtocolInterface;
