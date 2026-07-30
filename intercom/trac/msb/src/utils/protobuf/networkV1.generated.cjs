/*eslint-disable block-scoped-var, id-length, no-control-regex, no-magic-numbers, no-prototype-builtins, no-redeclare, no-shadow, no-var, sort-vars*/
"use strict";
if (typeof globalThis !== 'undefined' && typeof globalThis.self === 'undefined') {
  globalThis.self = globalThis;
}


var $protobuf = require("protobufjs/minimal");

// Common aliases
var $Reader = $protobuf.Reader, $Writer = $protobuf.Writer, $util = $protobuf.util;

// Exported root namespace
var $root = $protobuf.roots["default"] || ($protobuf.roots["default"] = {});

$root.network = (function() {

    /**
     * Namespace network.
     * @exports network
     * @namespace
     */
    var network = {};

    network.v1 = (function() {

        /**
         * Namespace v1.
         * @memberof network
         * @namespace
         */
        var v1 = {};

        v1.MessageHeader = (function() {

            /**
             * Properties of a MessageHeader.
             * @memberof network.v1
             * @interface IMessageHeader
             * @property {network.v1.MessageType|null} [type] MessageHeader type
             * @property {string|null} [id] MessageHeader id
             * @property {number|Long|null} [timestamp] MessageHeader timestamp
             * @property {network.v1.ILivenessRequest|null} [liveness_request] MessageHeader liveness_request
             * @property {network.v1.ILivenessResponse|null} [liveness_response] MessageHeader liveness_response
             * @property {network.v1.IBroadcastTransactionRequest|null} [broadcast_transaction_request] MessageHeader broadcast_transaction_request
             * @property {network.v1.IBroadcastTransactionResponse|null} [broadcast_transaction_response] MessageHeader broadcast_transaction_response
             * @property {Array.<string>|null} [capabilities] MessageHeader capabilities
             */

            /**
             * Constructs a new MessageHeader.
             * @memberof network.v1
             * @classdesc Represents a MessageHeader.
             * @implements IMessageHeader
             * @constructor
             * @param {network.v1.IMessageHeader=} [properties] Properties to set
             */
            function MessageHeader(properties) {
                this.capabilities = [];
                if (properties)
                    for (var keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                        if (properties[keys[i]] != null)
                            this[keys[i]] = properties[keys[i]];
            }

            /**
             * MessageHeader type.
             * @member {network.v1.MessageType} type
             * @memberof network.v1.MessageHeader
             * @instance
             */
            MessageHeader.prototype.type = 0;

            /**
             * MessageHeader id.
             * @member {string} id
             * @memberof network.v1.MessageHeader
             * @instance
             */
            MessageHeader.prototype.id = "";

            /**
             * MessageHeader timestamp.
             * @member {number|Long} timestamp
             * @memberof network.v1.MessageHeader
             * @instance
             */
            MessageHeader.prototype.timestamp = $util.Long ? $util.Long.fromBits(0,0,true) : 0;

            /**
             * MessageHeader liveness_request.
             * @member {network.v1.ILivenessRequest|null|undefined} liveness_request
             * @memberof network.v1.MessageHeader
             * @instance
             */
            MessageHeader.prototype.liveness_request = null;

            /**
             * MessageHeader liveness_response.
             * @member {network.v1.ILivenessResponse|null|undefined} liveness_response
             * @memberof network.v1.MessageHeader
             * @instance
             */
            MessageHeader.prototype.liveness_response = null;

            /**
             * MessageHeader broadcast_transaction_request.
             * @member {network.v1.IBroadcastTransactionRequest|null|undefined} broadcast_transaction_request
             * @memberof network.v1.MessageHeader
             * @instance
             */
            MessageHeader.prototype.broadcast_transaction_request = null;

            /**
             * MessageHeader broadcast_transaction_response.
             * @member {network.v1.IBroadcastTransactionResponse|null|undefined} broadcast_transaction_response
             * @memberof network.v1.MessageHeader
             * @instance
             */
            MessageHeader.prototype.broadcast_transaction_response = null;

            /**
             * MessageHeader capabilities.
             * @member {Array.<string>} capabilities
             * @memberof network.v1.MessageHeader
             * @instance
             */
            MessageHeader.prototype.capabilities = $util.emptyArray;

            // OneOf field names bound to virtual getters and setters
            var $oneOfFields;

            /**
             * MessageHeader field.
             * @member {"liveness_request"|"liveness_response"|"broadcast_transaction_request"|"broadcast_transaction_response"|undefined} field
             * @memberof network.v1.MessageHeader
             * @instance
             */
            Object.defineProperty(MessageHeader.prototype, "field", {
                get: $util.oneOfGetter($oneOfFields = ["liveness_request", "liveness_response", "broadcast_transaction_request", "broadcast_transaction_response"]),
                set: $util.oneOfSetter($oneOfFields)
            });

            /**
             * Creates a new MessageHeader instance using the specified properties.
             * @function create
             * @memberof network.v1.MessageHeader
             * @static
             * @param {network.v1.IMessageHeader=} [properties] Properties to set
             * @returns {network.v1.MessageHeader} MessageHeader instance
             */
            MessageHeader.create = function create(properties) {
                return new MessageHeader(properties);
            };

            /**
             * Encodes the specified MessageHeader message. Does not implicitly {@link network.v1.MessageHeader.verify|verify} messages.
             * @function encode
             * @memberof network.v1.MessageHeader
             * @static
             * @param {network.v1.IMessageHeader} message MessageHeader message or plain object to encode
             * @param {$protobuf.Writer} [writer] Writer to encode to
             * @returns {$protobuf.Writer} Writer
             */
            MessageHeader.encode = function encode(message, writer) {
                if (!writer)
                    writer = $Writer.create();
                if (message.type != null && Object.hasOwnProperty.call(message, "type"))
                    writer.uint32(/* id 1, wireType 0 =*/8).int32(message.type);
                if (message.id != null && Object.hasOwnProperty.call(message, "id"))
                    writer.uint32(/* id 2, wireType 2 =*/18).string(message.id);
                if (message.timestamp != null && Object.hasOwnProperty.call(message, "timestamp"))
                    writer.uint32(/* id 3, wireType 0 =*/24).uint64(message.timestamp);
                if (message.liveness_request != null && Object.hasOwnProperty.call(message, "liveness_request"))
                    $root.network.v1.LivenessRequest.encode(message.liveness_request, writer.uint32(/* id 4, wireType 2 =*/34).fork()).ldelim();
                if (message.liveness_response != null && Object.hasOwnProperty.call(message, "liveness_response"))
                    $root.network.v1.LivenessResponse.encode(message.liveness_response, writer.uint32(/* id 5, wireType 2 =*/42).fork()).ldelim();
                if (message.broadcast_transaction_request != null && Object.hasOwnProperty.call(message, "broadcast_transaction_request"))
                    $root.network.v1.BroadcastTransactionRequest.encode(message.broadcast_transaction_request, writer.uint32(/* id 6, wireType 2 =*/50).fork()).ldelim();
                if (message.broadcast_transaction_response != null && Object.hasOwnProperty.call(message, "broadcast_transaction_response"))
                    $root.network.v1.BroadcastTransactionResponse.encode(message.broadcast_transaction_response, writer.uint32(/* id 7, wireType 2 =*/58).fork()).ldelim();
                if (message.capabilities != null && message.capabilities.length)
                    for (var i = 0; i < message.capabilities.length; ++i)
                        writer.uint32(/* id 8, wireType 2 =*/66).string(message.capabilities[i]);
                return writer;
            };

            /**
             * Encodes the specified MessageHeader message, length delimited. Does not implicitly {@link network.v1.MessageHeader.verify|verify} messages.
             * @function encodeDelimited
             * @memberof network.v1.MessageHeader
             * @static
             * @param {network.v1.IMessageHeader} message MessageHeader message or plain object to encode
             * @param {$protobuf.Writer} [writer] Writer to encode to
             * @returns {$protobuf.Writer} Writer
             */
            MessageHeader.encodeDelimited = function encodeDelimited(message, writer) {
                return this.encode(message, writer).ldelim();
            };

            /**
             * Decodes a MessageHeader message from the specified reader or buffer.
             * @function decode
             * @memberof network.v1.MessageHeader
             * @static
             * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
             * @param {number} [length] Message length if known beforehand
             * @returns {network.v1.MessageHeader} MessageHeader
             * @throws {Error} If the payload is not a reader or valid buffer
             * @throws {$protobuf.util.ProtocolError} If required fields are missing
             */
            MessageHeader.decode = function decode(reader, length, error) {
                if (!(reader instanceof $Reader))
                    reader = $Reader.create(reader);
                var end = length === undefined ? reader.len : reader.pos + length, message = new $root.network.v1.MessageHeader();
                while (reader.pos < end) {
                    var tag = reader.uint32();
                    if (tag === error)
                        break;
                    switch (tag >>> 3) {
                    case 1: {
                            message.type = reader.int32();
                            break;
                        }
                    case 2: {
                            message.id = reader.string();
                            break;
                        }
                    case 3: {
                            message.timestamp = reader.uint64();
                            break;
                        }
                    case 4: {
                            message.liveness_request = $root.network.v1.LivenessRequest.decode(reader, reader.uint32());
                            break;
                        }
                    case 5: {
                            message.liveness_response = $root.network.v1.LivenessResponse.decode(reader, reader.uint32());
                            break;
                        }
                    case 6: {
                            message.broadcast_transaction_request = $root.network.v1.BroadcastTransactionRequest.decode(reader, reader.uint32());
                            break;
                        }
                    case 7: {
                            message.broadcast_transaction_response = $root.network.v1.BroadcastTransactionResponse.decode(reader, reader.uint32());
                            break;
                        }
                    case 8: {
                            if (!(message.capabilities && message.capabilities.length))
                                message.capabilities = [];
                            message.capabilities.push(reader.string());
                            break;
                        }
                    default:
                        reader.skipType(tag & 7);
                        break;
                    }
                }
                return message;
            };

            /**
             * Decodes a MessageHeader message from the specified reader or buffer, length delimited.
             * @function decodeDelimited
             * @memberof network.v1.MessageHeader
             * @static
             * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
             * @returns {network.v1.MessageHeader} MessageHeader
             * @throws {Error} If the payload is not a reader or valid buffer
             * @throws {$protobuf.util.ProtocolError} If required fields are missing
             */
            MessageHeader.decodeDelimited = function decodeDelimited(reader) {
                if (!(reader instanceof $Reader))
                    reader = new $Reader(reader);
                return this.decode(reader, reader.uint32());
            };

            /**
             * Verifies a MessageHeader message.
             * @function verify
             * @memberof network.v1.MessageHeader
             * @static
             * @param {Object.<string,*>} message Plain object to verify
             * @returns {string|null} `null` if valid, otherwise the reason why it is not
             */
            MessageHeader.verify = function verify(message) {
                if (typeof message !== "object" || message === null)
                    return "object expected";
                var properties = {};
                if (message.type != null && message.hasOwnProperty("type"))
                    switch (message.type) {
                    default:
                        return "type: enum value expected";
                    case 0:
                    case 1:
                    case 2:
                    case 3:
                    case 4:
                        break;
                    }
                if (message.id != null && message.hasOwnProperty("id"))
                    if (!$util.isString(message.id))
                        return "id: string expected";
                if (message.timestamp != null && message.hasOwnProperty("timestamp"))
                    if (!$util.isInteger(message.timestamp) && !(message.timestamp && $util.isInteger(message.timestamp.low) && $util.isInteger(message.timestamp.high)))
                        return "timestamp: integer|Long expected";
                if (message.liveness_request != null && message.hasOwnProperty("liveness_request")) {
                    properties.field = 1;
                    {
                        var error = $root.network.v1.LivenessRequest.verify(message.liveness_request);
                        if (error)
                            return "liveness_request." + error;
                    }
                }
                if (message.liveness_response != null && message.hasOwnProperty("liveness_response")) {
                    if (properties.field === 1)
                        return "field: multiple values";
                    properties.field = 1;
                    {
                        var error = $root.network.v1.LivenessResponse.verify(message.liveness_response);
                        if (error)
                            return "liveness_response." + error;
                    }
                }
                if (message.broadcast_transaction_request != null && message.hasOwnProperty("broadcast_transaction_request")) {
                    if (properties.field === 1)
                        return "field: multiple values";
                    properties.field = 1;
                    {
                        var error = $root.network.v1.BroadcastTransactionRequest.verify(message.broadcast_transaction_request);
                        if (error)
                            return "broadcast_transaction_request." + error;
                    }
                }
                if (message.broadcast_transaction_response != null && message.hasOwnProperty("broadcast_transaction_response")) {
                    if (properties.field === 1)
                        return "field: multiple values";
                    properties.field = 1;
                    {
                        var error = $root.network.v1.BroadcastTransactionResponse.verify(message.broadcast_transaction_response);
                        if (error)
                            return "broadcast_transaction_response." + error;
                    }
                }
                if (message.capabilities != null && message.hasOwnProperty("capabilities")) {
                    if (!Array.isArray(message.capabilities))
                        return "capabilities: array expected";
                    for (var i = 0; i < message.capabilities.length; ++i)
                        if (!$util.isString(message.capabilities[i]))
                            return "capabilities: string[] expected";
                }
                return null;
            };

            /**
             * Creates a MessageHeader message from a plain object. Also converts values to their respective internal types.
             * @function fromObject
             * @memberof network.v1.MessageHeader
             * @static
             * @param {Object.<string,*>} object Plain object
             * @returns {network.v1.MessageHeader} MessageHeader
             */
            MessageHeader.fromObject = function fromObject(object) {
                if (object instanceof $root.network.v1.MessageHeader)
                    return object;
                var message = new $root.network.v1.MessageHeader();
                switch (object.type) {
                default:
                    if (typeof object.type === "number") {
                        message.type = object.type;
                        break;
                    }
                    break;
                case "MESSAGE_TYPE_UNSPECIFIED":
                case 0:
                    message.type = 0;
                    break;
                case "MESSAGE_TYPE_LIVENESS_REQUEST":
                case 1:
                    message.type = 1;
                    break;
                case "MESSAGE_TYPE_LIVENESS_RESPONSE":
                case 2:
                    message.type = 2;
                    break;
                case "MESSAGE_TYPE_BROADCAST_TRANSACTION_REQUEST":
                case 3:
                    message.type = 3;
                    break;
                case "MESSAGE_TYPE_BROADCAST_TRANSACTION_RESPONSE":
                case 4:
                    message.type = 4;
                    break;
                }
                if (object.id != null)
                    message.id = String(object.id);
                if (object.timestamp != null)
                    if ($util.Long)
                        (message.timestamp = $util.Long.fromValue(object.timestamp)).unsigned = true;
                    else if (typeof object.timestamp === "string")
                        message.timestamp = parseInt(object.timestamp, 10);
                    else if (typeof object.timestamp === "number")
                        message.timestamp = object.timestamp;
                    else if (typeof object.timestamp === "object")
                        message.timestamp = new $util.LongBits(object.timestamp.low >>> 0, object.timestamp.high >>> 0).toNumber(true);
                if (object.liveness_request != null) {
                    if (typeof object.liveness_request !== "object")
                        throw TypeError(".network.v1.MessageHeader.liveness_request: object expected");
                    message.liveness_request = $root.network.v1.LivenessRequest.fromObject(object.liveness_request);
                }
                if (object.liveness_response != null) {
                    if (typeof object.liveness_response !== "object")
                        throw TypeError(".network.v1.MessageHeader.liveness_response: object expected");
                    message.liveness_response = $root.network.v1.LivenessResponse.fromObject(object.liveness_response);
                }
                if (object.broadcast_transaction_request != null) {
                    if (typeof object.broadcast_transaction_request !== "object")
                        throw TypeError(".network.v1.MessageHeader.broadcast_transaction_request: object expected");
                    message.broadcast_transaction_request = $root.network.v1.BroadcastTransactionRequest.fromObject(object.broadcast_transaction_request);
                }
                if (object.broadcast_transaction_response != null) {
                    if (typeof object.broadcast_transaction_response !== "object")
                        throw TypeError(".network.v1.MessageHeader.broadcast_transaction_response: object expected");
                    message.broadcast_transaction_response = $root.network.v1.BroadcastTransactionResponse.fromObject(object.broadcast_transaction_response);
                }
                if (object.capabilities) {
                    if (!Array.isArray(object.capabilities))
                        throw TypeError(".network.v1.MessageHeader.capabilities: array expected");
                    message.capabilities = [];
                    for (var i = 0; i < object.capabilities.length; ++i)
                        message.capabilities[i] = String(object.capabilities[i]);
                }
                return message;
            };

            /**
             * Creates a plain object from a MessageHeader message. Also converts values to other types if specified.
             * @function toObject
             * @memberof network.v1.MessageHeader
             * @static
             * @param {network.v1.MessageHeader} message MessageHeader
             * @param {$protobuf.IConversionOptions} [options] Conversion options
             * @returns {Object.<string,*>} Plain object
             */
            MessageHeader.toObject = function toObject(message, options) {
                if (!options)
                    options = {};
                var object = {};
                if (options.arrays || options.defaults)
                    object.capabilities = [];
                if (options.defaults) {
                    object.type = options.enums === String ? "MESSAGE_TYPE_UNSPECIFIED" : 0;
                    object.id = "";
                    if ($util.Long) {
                        var long = new $util.Long(0, 0, true);
                        object.timestamp = options.longs === String ? long.toString() : options.longs === Number ? long.toNumber() : long;
                    } else
                        object.timestamp = options.longs === String ? "0" : 0;
                }
                if (message.type != null && message.hasOwnProperty("type"))
                    object.type = options.enums === String ? $root.network.v1.MessageType[message.type] === undefined ? message.type : $root.network.v1.MessageType[message.type] : message.type;
                if (message.id != null && message.hasOwnProperty("id"))
                    object.id = message.id;
                if (message.timestamp != null && message.hasOwnProperty("timestamp"))
                    if (typeof message.timestamp === "number")
                        object.timestamp = options.longs === String ? String(message.timestamp) : message.timestamp;
                    else
                        object.timestamp = options.longs === String ? $util.Long.prototype.toString.call(message.timestamp) : options.longs === Number ? new $util.LongBits(message.timestamp.low >>> 0, message.timestamp.high >>> 0).toNumber(true) : message.timestamp;
                if (message.liveness_request != null && message.hasOwnProperty("liveness_request")) {
                    object.liveness_request = $root.network.v1.LivenessRequest.toObject(message.liveness_request, options);
                    if (options.oneofs)
                        object.field = "liveness_request";
                }
                if (message.liveness_response != null && message.hasOwnProperty("liveness_response")) {
                    object.liveness_response = $root.network.v1.LivenessResponse.toObject(message.liveness_response, options);
                    if (options.oneofs)
                        object.field = "liveness_response";
                }
                if (message.broadcast_transaction_request != null && message.hasOwnProperty("broadcast_transaction_request")) {
                    object.broadcast_transaction_request = $root.network.v1.BroadcastTransactionRequest.toObject(message.broadcast_transaction_request, options);
                    if (options.oneofs)
                        object.field = "broadcast_transaction_request";
                }
                if (message.broadcast_transaction_response != null && message.hasOwnProperty("broadcast_transaction_response")) {
                    object.broadcast_transaction_response = $root.network.v1.BroadcastTransactionResponse.toObject(message.broadcast_transaction_response, options);
                    if (options.oneofs)
                        object.field = "broadcast_transaction_response";
                }
                if (message.capabilities && message.capabilities.length) {
                    object.capabilities = [];
                    for (var j = 0; j < message.capabilities.length; ++j)
                        object.capabilities[j] = message.capabilities[j];
                }
                return object;
            };

            /**
             * Converts this MessageHeader to JSON.
             * @function toJSON
             * @memberof network.v1.MessageHeader
             * @instance
             * @returns {Object.<string,*>} JSON object
             */
            MessageHeader.prototype.toJSON = function toJSON() {
                return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
            };

            /**
             * Gets the default type url for MessageHeader
             * @function getTypeUrl
             * @memberof network.v1.MessageHeader
             * @static
             * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
             * @returns {string} The default type url
             */
            MessageHeader.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
                if (typeUrlPrefix === undefined) {
                    typeUrlPrefix = "type.googleapis.com";
                }
                return typeUrlPrefix + "/network.v1.MessageHeader";
            };

            return MessageHeader;
        })();

        /**
         * MessageType enum.
         * @name network.v1.MessageType
         * @enum {number}
         * @property {number} MESSAGE_TYPE_UNSPECIFIED=0 MESSAGE_TYPE_UNSPECIFIED value
         * @property {number} MESSAGE_TYPE_LIVENESS_REQUEST=1 MESSAGE_TYPE_LIVENESS_REQUEST value
         * @property {number} MESSAGE_TYPE_LIVENESS_RESPONSE=2 MESSAGE_TYPE_LIVENESS_RESPONSE value
         * @property {number} MESSAGE_TYPE_BROADCAST_TRANSACTION_REQUEST=3 MESSAGE_TYPE_BROADCAST_TRANSACTION_REQUEST value
         * @property {number} MESSAGE_TYPE_BROADCAST_TRANSACTION_RESPONSE=4 MESSAGE_TYPE_BROADCAST_TRANSACTION_RESPONSE value
         */
        v1.MessageType = (function() {
            var valuesById = {}, values = Object.create(valuesById);
            values[valuesById[0] = "MESSAGE_TYPE_UNSPECIFIED"] = 0;
            values[valuesById[1] = "MESSAGE_TYPE_LIVENESS_REQUEST"] = 1;
            values[valuesById[2] = "MESSAGE_TYPE_LIVENESS_RESPONSE"] = 2;
            values[valuesById[3] = "MESSAGE_TYPE_BROADCAST_TRANSACTION_REQUEST"] = 3;
            values[valuesById[4] = "MESSAGE_TYPE_BROADCAST_TRANSACTION_RESPONSE"] = 4;
            return values;
        })();

        v1.LivenessRequest = (function() {

            /**
             * Properties of a LivenessRequest.
             * @memberof network.v1
             * @interface ILivenessRequest
             * @property {Uint8Array|null} [nonce] LivenessRequest nonce
             * @property {Uint8Array|null} [signature] LivenessRequest signature
             */

            /**
             * Constructs a new LivenessRequest.
             * @memberof network.v1
             * @classdesc Represents a LivenessRequest.
             * @implements ILivenessRequest
             * @constructor
             * @param {network.v1.ILivenessRequest=} [properties] Properties to set
             */
            function LivenessRequest(properties) {
                if (properties)
                    for (var keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                        if (properties[keys[i]] != null)
                            this[keys[i]] = properties[keys[i]];
            }

            /**
             * LivenessRequest nonce.
             * @member {Uint8Array} nonce
             * @memberof network.v1.LivenessRequest
             * @instance
             */
            LivenessRequest.prototype.nonce = $util.newBuffer([]);

            /**
             * LivenessRequest signature.
             * @member {Uint8Array} signature
             * @memberof network.v1.LivenessRequest
             * @instance
             */
            LivenessRequest.prototype.signature = $util.newBuffer([]);

            /**
             * Creates a new LivenessRequest instance using the specified properties.
             * @function create
             * @memberof network.v1.LivenessRequest
             * @static
             * @param {network.v1.ILivenessRequest=} [properties] Properties to set
             * @returns {network.v1.LivenessRequest} LivenessRequest instance
             */
            LivenessRequest.create = function create(properties) {
                return new LivenessRequest(properties);
            };

            /**
             * Encodes the specified LivenessRequest message. Does not implicitly {@link network.v1.LivenessRequest.verify|verify} messages.
             * @function encode
             * @memberof network.v1.LivenessRequest
             * @static
             * @param {network.v1.ILivenessRequest} message LivenessRequest message or plain object to encode
             * @param {$protobuf.Writer} [writer] Writer to encode to
             * @returns {$protobuf.Writer} Writer
             */
            LivenessRequest.encode = function encode(message, writer) {
                if (!writer)
                    writer = $Writer.create();
                if (message.nonce != null && Object.hasOwnProperty.call(message, "nonce"))
                    writer.uint32(/* id 1, wireType 2 =*/10).bytes(message.nonce);
                if (message.signature != null && Object.hasOwnProperty.call(message, "signature"))
                    writer.uint32(/* id 2, wireType 2 =*/18).bytes(message.signature);
                return writer;
            };

            /**
             * Encodes the specified LivenessRequest message, length delimited. Does not implicitly {@link network.v1.LivenessRequest.verify|verify} messages.
             * @function encodeDelimited
             * @memberof network.v1.LivenessRequest
             * @static
             * @param {network.v1.ILivenessRequest} message LivenessRequest message or plain object to encode
             * @param {$protobuf.Writer} [writer] Writer to encode to
             * @returns {$protobuf.Writer} Writer
             */
            LivenessRequest.encodeDelimited = function encodeDelimited(message, writer) {
                return this.encode(message, writer).ldelim();
            };

            /**
             * Decodes a LivenessRequest message from the specified reader or buffer.
             * @function decode
             * @memberof network.v1.LivenessRequest
             * @static
             * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
             * @param {number} [length] Message length if known beforehand
             * @returns {network.v1.LivenessRequest} LivenessRequest
             * @throws {Error} If the payload is not a reader or valid buffer
             * @throws {$protobuf.util.ProtocolError} If required fields are missing
             */
            LivenessRequest.decode = function decode(reader, length, error) {
                if (!(reader instanceof $Reader))
                    reader = $Reader.create(reader);
                var end = length === undefined ? reader.len : reader.pos + length, message = new $root.network.v1.LivenessRequest();
                while (reader.pos < end) {
                    var tag = reader.uint32();
                    if (tag === error)
                        break;
                    switch (tag >>> 3) {
                    case 1: {
                            message.nonce = reader.bytes();
                            break;
                        }
                    case 2: {
                            message.signature = reader.bytes();
                            break;
                        }
                    default:
                        reader.skipType(tag & 7);
                        break;
                    }
                }
                return message;
            };

            /**
             * Decodes a LivenessRequest message from the specified reader or buffer, length delimited.
             * @function decodeDelimited
             * @memberof network.v1.LivenessRequest
             * @static
             * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
             * @returns {network.v1.LivenessRequest} LivenessRequest
             * @throws {Error} If the payload is not a reader or valid buffer
             * @throws {$protobuf.util.ProtocolError} If required fields are missing
             */
            LivenessRequest.decodeDelimited = function decodeDelimited(reader) {
                if (!(reader instanceof $Reader))
                    reader = new $Reader(reader);
                return this.decode(reader, reader.uint32());
            };

            /**
             * Verifies a LivenessRequest message.
             * @function verify
             * @memberof network.v1.LivenessRequest
             * @static
             * @param {Object.<string,*>} message Plain object to verify
             * @returns {string|null} `null` if valid, otherwise the reason why it is not
             */
            LivenessRequest.verify = function verify(message) {
                if (typeof message !== "object" || message === null)
                    return "object expected";
                if (message.nonce != null && message.hasOwnProperty("nonce"))
                    if (!(message.nonce && typeof message.nonce.length === "number" || $util.isString(message.nonce)))
                        return "nonce: buffer expected";
                if (message.signature != null && message.hasOwnProperty("signature"))
                    if (!(message.signature && typeof message.signature.length === "number" || $util.isString(message.signature)))
                        return "signature: buffer expected";
                return null;
            };

            /**
             * Creates a LivenessRequest message from a plain object. Also converts values to their respective internal types.
             * @function fromObject
             * @memberof network.v1.LivenessRequest
             * @static
             * @param {Object.<string,*>} object Plain object
             * @returns {network.v1.LivenessRequest} LivenessRequest
             */
            LivenessRequest.fromObject = function fromObject(object) {
                if (object instanceof $root.network.v1.LivenessRequest)
                    return object;
                var message = new $root.network.v1.LivenessRequest();
                if (object.nonce != null)
                    if (typeof object.nonce === "string")
                        $util.base64.decode(object.nonce, message.nonce = $util.newBuffer($util.base64.length(object.nonce)), 0);
                    else if (object.nonce.length >= 0)
                        message.nonce = object.nonce;
                if (object.signature != null)
                    if (typeof object.signature === "string")
                        $util.base64.decode(object.signature, message.signature = $util.newBuffer($util.base64.length(object.signature)), 0);
                    else if (object.signature.length >= 0)
                        message.signature = object.signature;
                return message;
            };

            /**
             * Creates a plain object from a LivenessRequest message. Also converts values to other types if specified.
             * @function toObject
             * @memberof network.v1.LivenessRequest
             * @static
             * @param {network.v1.LivenessRequest} message LivenessRequest
             * @param {$protobuf.IConversionOptions} [options] Conversion options
             * @returns {Object.<string,*>} Plain object
             */
            LivenessRequest.toObject = function toObject(message, options) {
                if (!options)
                    options = {};
                var object = {};
                if (options.defaults) {
                    if (options.bytes === String)
                        object.nonce = "";
                    else {
                        object.nonce = [];
                        if (options.bytes !== Array)
                            object.nonce = $util.newBuffer(object.nonce);
                    }
                    if (options.bytes === String)
                        object.signature = "";
                    else {
                        object.signature = [];
                        if (options.bytes !== Array)
                            object.signature = $util.newBuffer(object.signature);
                    }
                }
                if (message.nonce != null && message.hasOwnProperty("nonce"))
                    object.nonce = options.bytes === String ? $util.base64.encode(message.nonce, 0, message.nonce.length) : options.bytes === Array ? Array.prototype.slice.call(message.nonce) : message.nonce;
                if (message.signature != null && message.hasOwnProperty("signature"))
                    object.signature = options.bytes === String ? $util.base64.encode(message.signature, 0, message.signature.length) : options.bytes === Array ? Array.prototype.slice.call(message.signature) : message.signature;
                return object;
            };

            /**
             * Converts this LivenessRequest to JSON.
             * @function toJSON
             * @memberof network.v1.LivenessRequest
             * @instance
             * @returns {Object.<string,*>} JSON object
             */
            LivenessRequest.prototype.toJSON = function toJSON() {
                return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
            };

            /**
             * Gets the default type url for LivenessRequest
             * @function getTypeUrl
             * @memberof network.v1.LivenessRequest
             * @static
             * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
             * @returns {string} The default type url
             */
            LivenessRequest.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
                if (typeUrlPrefix === undefined) {
                    typeUrlPrefix = "type.googleapis.com";
                }
                return typeUrlPrefix + "/network.v1.LivenessRequest";
            };

            return LivenessRequest;
        })();

        v1.LivenessResponse = (function() {

            /**
             * Properties of a LivenessResponse.
             * @memberof network.v1
             * @interface ILivenessResponse
             * @property {Uint8Array|null} [nonce] LivenessResponse nonce
             * @property {Uint8Array|null} [signature] LivenessResponse signature
             * @property {network.v1.ResultCode|null} [result] LivenessResponse result
             */

            /**
             * Constructs a new LivenessResponse.
             * @memberof network.v1
             * @classdesc Represents a LivenessResponse.
             * @implements ILivenessResponse
             * @constructor
             * @param {network.v1.ILivenessResponse=} [properties] Properties to set
             */
            function LivenessResponse(properties) {
                if (properties)
                    for (var keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                        if (properties[keys[i]] != null)
                            this[keys[i]] = properties[keys[i]];
            }

            /**
             * LivenessResponse nonce.
             * @member {Uint8Array} nonce
             * @memberof network.v1.LivenessResponse
             * @instance
             */
            LivenessResponse.prototype.nonce = $util.newBuffer([]);

            /**
             * LivenessResponse signature.
             * @member {Uint8Array} signature
             * @memberof network.v1.LivenessResponse
             * @instance
             */
            LivenessResponse.prototype.signature = $util.newBuffer([]);

            /**
             * LivenessResponse result.
             * @member {network.v1.ResultCode} result
             * @memberof network.v1.LivenessResponse
             * @instance
             */
            LivenessResponse.prototype.result = 0;

            /**
             * Creates a new LivenessResponse instance using the specified properties.
             * @function create
             * @memberof network.v1.LivenessResponse
             * @static
             * @param {network.v1.ILivenessResponse=} [properties] Properties to set
             * @returns {network.v1.LivenessResponse} LivenessResponse instance
             */
            LivenessResponse.create = function create(properties) {
                return new LivenessResponse(properties);
            };

            /**
             * Encodes the specified LivenessResponse message. Does not implicitly {@link network.v1.LivenessResponse.verify|verify} messages.
             * @function encode
             * @memberof network.v1.LivenessResponse
             * @static
             * @param {network.v1.ILivenessResponse} message LivenessResponse message or plain object to encode
             * @param {$protobuf.Writer} [writer] Writer to encode to
             * @returns {$protobuf.Writer} Writer
             */
            LivenessResponse.encode = function encode(message, writer) {
                if (!writer)
                    writer = $Writer.create();
                if (message.nonce != null && Object.hasOwnProperty.call(message, "nonce"))
                    writer.uint32(/* id 1, wireType 2 =*/10).bytes(message.nonce);
                if (message.signature != null && Object.hasOwnProperty.call(message, "signature"))
                    writer.uint32(/* id 2, wireType 2 =*/18).bytes(message.signature);
                if (message.result != null && Object.hasOwnProperty.call(message, "result"))
                    writer.uint32(/* id 3, wireType 0 =*/24).int32(message.result);
                return writer;
            };

            /**
             * Encodes the specified LivenessResponse message, length delimited. Does not implicitly {@link network.v1.LivenessResponse.verify|verify} messages.
             * @function encodeDelimited
             * @memberof network.v1.LivenessResponse
             * @static
             * @param {network.v1.ILivenessResponse} message LivenessResponse message or plain object to encode
             * @param {$protobuf.Writer} [writer] Writer to encode to
             * @returns {$protobuf.Writer} Writer
             */
            LivenessResponse.encodeDelimited = function encodeDelimited(message, writer) {
                return this.encode(message, writer).ldelim();
            };

            /**
             * Decodes a LivenessResponse message from the specified reader or buffer.
             * @function decode
             * @memberof network.v1.LivenessResponse
             * @static
             * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
             * @param {number} [length] Message length if known beforehand
             * @returns {network.v1.LivenessResponse} LivenessResponse
             * @throws {Error} If the payload is not a reader or valid buffer
             * @throws {$protobuf.util.ProtocolError} If required fields are missing
             */
            LivenessResponse.decode = function decode(reader, length, error) {
                if (!(reader instanceof $Reader))
                    reader = $Reader.create(reader);
                var end = length === undefined ? reader.len : reader.pos + length, message = new $root.network.v1.LivenessResponse();
                while (reader.pos < end) {
                    var tag = reader.uint32();
                    if (tag === error)
                        break;
                    switch (tag >>> 3) {
                    case 1: {
                            message.nonce = reader.bytes();
                            break;
                        }
                    case 2: {
                            message.signature = reader.bytes();
                            break;
                        }
                    case 3: {
                            message.result = reader.int32();
                            break;
                        }
                    default:
                        reader.skipType(tag & 7);
                        break;
                    }
                }
                return message;
            };

            /**
             * Decodes a LivenessResponse message from the specified reader or buffer, length delimited.
             * @function decodeDelimited
             * @memberof network.v1.LivenessResponse
             * @static
             * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
             * @returns {network.v1.LivenessResponse} LivenessResponse
             * @throws {Error} If the payload is not a reader or valid buffer
             * @throws {$protobuf.util.ProtocolError} If required fields are missing
             */
            LivenessResponse.decodeDelimited = function decodeDelimited(reader) {
                if (!(reader instanceof $Reader))
                    reader = new $Reader(reader);
                return this.decode(reader, reader.uint32());
            };

            /**
             * Verifies a LivenessResponse message.
             * @function verify
             * @memberof network.v1.LivenessResponse
             * @static
             * @param {Object.<string,*>} message Plain object to verify
             * @returns {string|null} `null` if valid, otherwise the reason why it is not
             */
            LivenessResponse.verify = function verify(message) {
                if (typeof message !== "object" || message === null)
                    return "object expected";
                if (message.nonce != null && message.hasOwnProperty("nonce"))
                    if (!(message.nonce && typeof message.nonce.length === "number" || $util.isString(message.nonce)))
                        return "nonce: buffer expected";
                if (message.signature != null && message.hasOwnProperty("signature"))
                    if (!(message.signature && typeof message.signature.length === "number" || $util.isString(message.signature)))
                        return "signature: buffer expected";
                if (message.result != null && message.hasOwnProperty("result"))
                    switch (message.result) {
                    default:
                        return "result: enum value expected";
                    case 0:
                    case 1:
                    case 2:
                    case 3:
                    case 4:
                    case 5:
                    case 6:
                    case 7:
                    case 8:
                    case 9:
                    case 10:
                    case 11:
                    case 12:
                    case 13:
                    case 14:
                    case 15:
                    case 16:
                    case 17:
                    case 18:
                    case 19:
                    case 20:
                    case 21:
                    case 22:
                    case 23:
                    case 24:
                    case 25:
                    case 26:
                    case 27:
                    case 28:
                    case 29:
                    case 30:
                    case 31:
                    case 32:
                    case 33:
                    case 34:
                    case 35:
                    case 36:
                    case 37:
                    case 38:
                    case 39:
                    case 40:
                    case 41:
                    case 42:
                    case 43:
                    case 44:
                    case 45:
                    case 46:
                    case 47:
                    case 48:
                    case 49:
                    case 50:
                    case 51:
                    case 52:
                    case 53:
                    case 54:
                    case 55:
                    case 56:
                    case 57:
                    case 58:
                    case 59:
                    case 60:
                    case 61:
                        break;
                    }
                return null;
            };

            /**
             * Creates a LivenessResponse message from a plain object. Also converts values to their respective internal types.
             * @function fromObject
             * @memberof network.v1.LivenessResponse
             * @static
             * @param {Object.<string,*>} object Plain object
             * @returns {network.v1.LivenessResponse} LivenessResponse
             */
            LivenessResponse.fromObject = function fromObject(object) {
                if (object instanceof $root.network.v1.LivenessResponse)
                    return object;
                var message = new $root.network.v1.LivenessResponse();
                if (object.nonce != null)
                    if (typeof object.nonce === "string")
                        $util.base64.decode(object.nonce, message.nonce = $util.newBuffer($util.base64.length(object.nonce)), 0);
                    else if (object.nonce.length >= 0)
                        message.nonce = object.nonce;
                if (object.signature != null)
                    if (typeof object.signature === "string")
                        $util.base64.decode(object.signature, message.signature = $util.newBuffer($util.base64.length(object.signature)), 0);
                    else if (object.signature.length >= 0)
                        message.signature = object.signature;
                switch (object.result) {
                default:
                    if (typeof object.result === "number") {
                        message.result = object.result;
                        break;
                    }
                    break;
                case "RESULT_CODE_UNSPECIFIED":
                case 0:
                    message.result = 0;
                    break;
                case "RESULT_CODE_OK":
                case 1:
                    message.result = 1;
                    break;
                case "RESULT_CODE_INVALID_PAYLOAD":
                case 2:
                    message.result = 2;
                    break;
                case "RESULT_CODE_RATE_LIMITED":
                case 3:
                    message.result = 3;
                    break;
                case "RESULT_CODE_SIGNATURE_INVALID":
                case 4:
                    message.result = 4;
                    break;
                case "RESULT_CODE_UNEXPECTED_ERROR":
                case 5:
                    message.result = 5;
                    break;
                case "RESULT_CODE_TIMEOUT":
                case 6:
                    message.result = 6;
                    break;
                case "RESULT_CODE_NODE_HAS_NO_WRITE_ACCESS":
                case 7:
                    message.result = 7;
                    break;
                case "RESULT_CODE_TX_ACCEPTED_PROOF_UNAVAILABLE":
                case 8:
                    message.result = 8;
                    break;
                case "RESULT_CODE_NODE_OVERLOADED":
                case 9:
                    message.result = 9;
                    break;
                case "RESULT_CODE_TX_ALREADY_PENDING":
                case 10:
                    message.result = 10;
                    break;
                case "RESULT_CODE_OPERATION_TYPE_UNKNOWN":
                case 11:
                    message.result = 11;
                    break;
                case "RESULT_CODE_SCHEMA_VALIDATION_FAILED":
                case 12:
                    message.result = 12;
                    break;
                case "RESULT_CODE_REQUESTER_ADDRESS_INVALID":
                case 13:
                    message.result = 13;
                    break;
                case "RESULT_CODE_REQUESTER_PUBLIC_KEY_INVALID":
                case 14:
                    message.result = 14;
                    break;
                case "RESULT_CODE_TX_HASH_MISMATCH":
                case 15:
                    message.result = 15;
                    break;
                case "RESULT_CODE_TX_SIGNATURE_INVALID":
                case 16:
                    message.result = 16;
                    break;
                case "RESULT_CODE_TX_EXPIRED":
                case 17:
                    message.result = 17;
                    break;
                case "RESULT_CODE_TX_ALREADY_EXISTS":
                case 18:
                    message.result = 18;
                    break;
                case "RESULT_CODE_OPERATION_ALREADY_COMPLETED":
                case 19:
                    message.result = 19;
                    break;
                case "RESULT_CODE_REQUESTER_NOT_FOUND":
                case 20:
                    message.result = 20;
                    break;
                case "RESULT_CODE_INSUFFICIENT_FEE_BALANCE":
                case 21:
                    message.result = 21;
                    break;
                case "RESULT_CODE_EXTERNAL_BOOTSTRAP_EQUALS_MSB_BOOTSTRAP":
                case 22:
                    message.result = 22;
                    break;
                case "RESULT_CODE_SELF_VALIDATION_FORBIDDEN":
                case 23:
                    message.result = 23;
                    break;
                case "RESULT_CODE_ROLE_NODE_ENTRY_NOT_FOUND":
                case 24:
                    message.result = 24;
                    break;
                case "RESULT_CODE_ROLE_NODE_ALREADY_WRITER":
                case 25:
                    message.result = 25;
                    break;
                case "RESULT_CODE_ROLE_NODE_NOT_WHITELISTED":
                case 26:
                    message.result = 26;
                    break;
                case "RESULT_CODE_ROLE_NODE_NOT_WRITER":
                case 27:
                    message.result = 27;
                    break;
                case "RESULT_CODE_ROLE_NODE_IS_INDEXER":
                case 28:
                    message.result = 28;
                    break;
                case "RESULT_CODE_ROLE_ADMIN_ENTRY_MISSING":
                case 29:
                    message.result = 29;
                    break;
                case "RESULT_CODE_ROLE_INVALID_RECOVERY_CASE":
                case 30:
                    message.result = 30;
                    break;
                case "RESULT_CODE_ROLE_UNKNOWN_OPERATION":
                case 31:
                    message.result = 31;
                    break;
                case "RESULT_CODE_ROLE_INVALID_WRITER_KEY":
                case 32:
                    message.result = 32;
                    break;
                case "RESULT_CODE_ROLE_INSUFFICIENT_FEE_BALANCE":
                case 33:
                    message.result = 33;
                    break;
                case "RESULT_CODE_MSB_BOOTSTRAP_MISMATCH":
                case 34:
                    message.result = 34;
                    break;
                case "RESULT_CODE_EXTERNAL_BOOTSTRAP_NOT_DEPLOYED":
                case 35:
                    message.result = 35;
                    break;
                case "RESULT_CODE_EXTERNAL_BOOTSTRAP_TX_MISSING":
                case 36:
                    message.result = 36;
                    break;
                case "RESULT_CODE_EXTERNAL_BOOTSTRAP_MISMATCH":
                case 37:
                    message.result = 37;
                    break;
                case "RESULT_CODE_BOOTSTRAP_ALREADY_EXISTS":
                case 38:
                    message.result = 38;
                    break;
                case "RESULT_CODE_TRANSFER_RECIPIENT_ADDRESS_INVALID":
                case 39:
                    message.result = 39;
                    break;
                case "RESULT_CODE_TRANSFER_RECIPIENT_PUBLIC_KEY_INVALID":
                case 40:
                    message.result = 40;
                    break;
                case "RESULT_CODE_TRANSFER_AMOUNT_TOO_LARGE":
                case 41:
                    message.result = 41;
                    break;
                case "RESULT_CODE_TRANSFER_SENDER_NOT_FOUND":
                case 42:
                    message.result = 42;
                    break;
                case "RESULT_CODE_TRANSFER_INSUFFICIENT_BALANCE":
                case 43:
                    message.result = 43;
                    break;
                case "RESULT_CODE_TRANSFER_RECIPIENT_BALANCE_OVERFLOW":
                case 44:
                    message.result = 44;
                    break;
                case "RESULT_CODE_TX_HASH_INVALID_FORMAT":
                case 45:
                    message.result = 45;
                    break;
                case "RESULT_CODE_INTERNAL_ENQUEUE_VALIDATION_FAILED":
                case 46:
                    message.result = 46;
                    break;
                case "RESULT_CODE_TX_COMMITTED_RECEIPT_MISSING":
                case 47:
                    message.result = 47;
                    break;
                case "RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_INVALID":
                case 48:
                    message.result = 48;
                    break;
                case "RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_UNKNOWN":
                case 49:
                    message.result = 49;
                    break;
                case "RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_UNSUPPORTED":
                case 50:
                    message.result = 50;
                    break;
                case "RESULT_CODE_VALIDATOR_RESPONSE_SCHEMA_INVALID":
                case 51:
                    message.result = 51;
                    break;
                case "RESULT_CODE_PENDING_REQUEST_MISSING_TX_DATA":
                case 52:
                    message.result = 52;
                    break;
                case "RESULT_CODE_PROOF_PAYLOAD_MISMATCH":
                case 53:
                    message.result = 53;
                    break;
                case "RESULT_CODE_VALIDATOR_WRITER_KEY_NOT_REGISTERED":
                case 54:
                    message.result = 54;
                    break;
                case "RESULT_CODE_VALIDATOR_ADDRESS_MISMATCH":
                case 55:
                    message.result = 55;
                    break;
                case "RESULT_CODE_VALIDATOR_NODE_ENTRY_NOT_FOUND":
                case 56:
                    message.result = 56;
                    break;
                case "RESULT_CODE_VALIDATOR_NODE_NOT_WRITER":
                case 57:
                    message.result = 57;
                    break;
                case "RESULT_CODE_VALIDATOR_WRITER_KEY_MISMATCH":
                case 58:
                    message.result = 58;
                    break;
                case "RESULT_CODE_VALIDATOR_TX_OBJECT_INVALID":
                case 59:
                    message.result = 59;
                    break;
                case "RESULT_CODE_VALIDATOR_VA_MISSING":
                case 60:
                    message.result = 60;
                    break;
                case "RESULT_CODE_TX_INVALID_PAYLOAD":
                case 61:
                    message.result = 61;
                    break;
                }
                return message;
            };

            /**
             * Creates a plain object from a LivenessResponse message. Also converts values to other types if specified.
             * @function toObject
             * @memberof network.v1.LivenessResponse
             * @static
             * @param {network.v1.LivenessResponse} message LivenessResponse
             * @param {$protobuf.IConversionOptions} [options] Conversion options
             * @returns {Object.<string,*>} Plain object
             */
            LivenessResponse.toObject = function toObject(message, options) {
                if (!options)
                    options = {};
                var object = {};
                if (options.defaults) {
                    if (options.bytes === String)
                        object.nonce = "";
                    else {
                        object.nonce = [];
                        if (options.bytes !== Array)
                            object.nonce = $util.newBuffer(object.nonce);
                    }
                    if (options.bytes === String)
                        object.signature = "";
                    else {
                        object.signature = [];
                        if (options.bytes !== Array)
                            object.signature = $util.newBuffer(object.signature);
                    }
                    object.result = options.enums === String ? "RESULT_CODE_UNSPECIFIED" : 0;
                }
                if (message.nonce != null && message.hasOwnProperty("nonce"))
                    object.nonce = options.bytes === String ? $util.base64.encode(message.nonce, 0, message.nonce.length) : options.bytes === Array ? Array.prototype.slice.call(message.nonce) : message.nonce;
                if (message.signature != null && message.hasOwnProperty("signature"))
                    object.signature = options.bytes === String ? $util.base64.encode(message.signature, 0, message.signature.length) : options.bytes === Array ? Array.prototype.slice.call(message.signature) : message.signature;
                if (message.result != null && message.hasOwnProperty("result"))
                    object.result = options.enums === String ? $root.network.v1.ResultCode[message.result] === undefined ? message.result : $root.network.v1.ResultCode[message.result] : message.result;
                return object;
            };

            /**
             * Converts this LivenessResponse to JSON.
             * @function toJSON
             * @memberof network.v1.LivenessResponse
             * @instance
             * @returns {Object.<string,*>} JSON object
             */
            LivenessResponse.prototype.toJSON = function toJSON() {
                return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
            };

            /**
             * Gets the default type url for LivenessResponse
             * @function getTypeUrl
             * @memberof network.v1.LivenessResponse
             * @static
             * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
             * @returns {string} The default type url
             */
            LivenessResponse.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
                if (typeUrlPrefix === undefined) {
                    typeUrlPrefix = "type.googleapis.com";
                }
                return typeUrlPrefix + "/network.v1.LivenessResponse";
            };

            return LivenessResponse;
        })();

        /**
         * ResultCode enum.
         * @name network.v1.ResultCode
         * @enum {number}
         * @property {number} RESULT_CODE_UNSPECIFIED=0 RESULT_CODE_UNSPECIFIED value
         * @property {number} RESULT_CODE_OK=1 RESULT_CODE_OK value
         * @property {number} RESULT_CODE_INVALID_PAYLOAD=2 RESULT_CODE_INVALID_PAYLOAD value
         * @property {number} RESULT_CODE_RATE_LIMITED=3 RESULT_CODE_RATE_LIMITED value
         * @property {number} RESULT_CODE_SIGNATURE_INVALID=4 RESULT_CODE_SIGNATURE_INVALID value
         * @property {number} RESULT_CODE_UNEXPECTED_ERROR=5 RESULT_CODE_UNEXPECTED_ERROR value
         * @property {number} RESULT_CODE_TIMEOUT=6 RESULT_CODE_TIMEOUT value
         * @property {number} RESULT_CODE_NODE_HAS_NO_WRITE_ACCESS=7 RESULT_CODE_NODE_HAS_NO_WRITE_ACCESS value
         * @property {number} RESULT_CODE_TX_ACCEPTED_PROOF_UNAVAILABLE=8 RESULT_CODE_TX_ACCEPTED_PROOF_UNAVAILABLE value
         * @property {number} RESULT_CODE_NODE_OVERLOADED=9 RESULT_CODE_NODE_OVERLOADED value
         * @property {number} RESULT_CODE_TX_ALREADY_PENDING=10 RESULT_CODE_TX_ALREADY_PENDING value
         * @property {number} RESULT_CODE_OPERATION_TYPE_UNKNOWN=11 RESULT_CODE_OPERATION_TYPE_UNKNOWN value
         * @property {number} RESULT_CODE_SCHEMA_VALIDATION_FAILED=12 RESULT_CODE_SCHEMA_VALIDATION_FAILED value
         * @property {number} RESULT_CODE_REQUESTER_ADDRESS_INVALID=13 RESULT_CODE_REQUESTER_ADDRESS_INVALID value
         * @property {number} RESULT_CODE_REQUESTER_PUBLIC_KEY_INVALID=14 RESULT_CODE_REQUESTER_PUBLIC_KEY_INVALID value
         * @property {number} RESULT_CODE_TX_HASH_MISMATCH=15 RESULT_CODE_TX_HASH_MISMATCH value
         * @property {number} RESULT_CODE_TX_SIGNATURE_INVALID=16 RESULT_CODE_TX_SIGNATURE_INVALID value
         * @property {number} RESULT_CODE_TX_EXPIRED=17 RESULT_CODE_TX_EXPIRED value
         * @property {number} RESULT_CODE_TX_ALREADY_EXISTS=18 RESULT_CODE_TX_ALREADY_EXISTS value
         * @property {number} RESULT_CODE_OPERATION_ALREADY_COMPLETED=19 RESULT_CODE_OPERATION_ALREADY_COMPLETED value
         * @property {number} RESULT_CODE_REQUESTER_NOT_FOUND=20 RESULT_CODE_REQUESTER_NOT_FOUND value
         * @property {number} RESULT_CODE_INSUFFICIENT_FEE_BALANCE=21 RESULT_CODE_INSUFFICIENT_FEE_BALANCE value
         * @property {number} RESULT_CODE_EXTERNAL_BOOTSTRAP_EQUALS_MSB_BOOTSTRAP=22 RESULT_CODE_EXTERNAL_BOOTSTRAP_EQUALS_MSB_BOOTSTRAP value
         * @property {number} RESULT_CODE_SELF_VALIDATION_FORBIDDEN=23 RESULT_CODE_SELF_VALIDATION_FORBIDDEN value
         * @property {number} RESULT_CODE_ROLE_NODE_ENTRY_NOT_FOUND=24 RESULT_CODE_ROLE_NODE_ENTRY_NOT_FOUND value
         * @property {number} RESULT_CODE_ROLE_NODE_ALREADY_WRITER=25 RESULT_CODE_ROLE_NODE_ALREADY_WRITER value
         * @property {number} RESULT_CODE_ROLE_NODE_NOT_WHITELISTED=26 RESULT_CODE_ROLE_NODE_NOT_WHITELISTED value
         * @property {number} RESULT_CODE_ROLE_NODE_NOT_WRITER=27 RESULT_CODE_ROLE_NODE_NOT_WRITER value
         * @property {number} RESULT_CODE_ROLE_NODE_IS_INDEXER=28 RESULT_CODE_ROLE_NODE_IS_INDEXER value
         * @property {number} RESULT_CODE_ROLE_ADMIN_ENTRY_MISSING=29 RESULT_CODE_ROLE_ADMIN_ENTRY_MISSING value
         * @property {number} RESULT_CODE_ROLE_INVALID_RECOVERY_CASE=30 RESULT_CODE_ROLE_INVALID_RECOVERY_CASE value
         * @property {number} RESULT_CODE_ROLE_UNKNOWN_OPERATION=31 RESULT_CODE_ROLE_UNKNOWN_OPERATION value
         * @property {number} RESULT_CODE_ROLE_INVALID_WRITER_KEY=32 RESULT_CODE_ROLE_INVALID_WRITER_KEY value
         * @property {number} RESULT_CODE_ROLE_INSUFFICIENT_FEE_BALANCE=33 RESULT_CODE_ROLE_INSUFFICIENT_FEE_BALANCE value
         * @property {number} RESULT_CODE_MSB_BOOTSTRAP_MISMATCH=34 RESULT_CODE_MSB_BOOTSTRAP_MISMATCH value
         * @property {number} RESULT_CODE_EXTERNAL_BOOTSTRAP_NOT_DEPLOYED=35 RESULT_CODE_EXTERNAL_BOOTSTRAP_NOT_DEPLOYED value
         * @property {number} RESULT_CODE_EXTERNAL_BOOTSTRAP_TX_MISSING=36 RESULT_CODE_EXTERNAL_BOOTSTRAP_TX_MISSING value
         * @property {number} RESULT_CODE_EXTERNAL_BOOTSTRAP_MISMATCH=37 RESULT_CODE_EXTERNAL_BOOTSTRAP_MISMATCH value
         * @property {number} RESULT_CODE_BOOTSTRAP_ALREADY_EXISTS=38 RESULT_CODE_BOOTSTRAP_ALREADY_EXISTS value
         * @property {number} RESULT_CODE_TRANSFER_RECIPIENT_ADDRESS_INVALID=39 RESULT_CODE_TRANSFER_RECIPIENT_ADDRESS_INVALID value
         * @property {number} RESULT_CODE_TRANSFER_RECIPIENT_PUBLIC_KEY_INVALID=40 RESULT_CODE_TRANSFER_RECIPIENT_PUBLIC_KEY_INVALID value
         * @property {number} RESULT_CODE_TRANSFER_AMOUNT_TOO_LARGE=41 RESULT_CODE_TRANSFER_AMOUNT_TOO_LARGE value
         * @property {number} RESULT_CODE_TRANSFER_SENDER_NOT_FOUND=42 RESULT_CODE_TRANSFER_SENDER_NOT_FOUND value
         * @property {number} RESULT_CODE_TRANSFER_INSUFFICIENT_BALANCE=43 RESULT_CODE_TRANSFER_INSUFFICIENT_BALANCE value
         * @property {number} RESULT_CODE_TRANSFER_RECIPIENT_BALANCE_OVERFLOW=44 RESULT_CODE_TRANSFER_RECIPIENT_BALANCE_OVERFLOW value
         * @property {number} RESULT_CODE_TX_HASH_INVALID_FORMAT=45 RESULT_CODE_TX_HASH_INVALID_FORMAT value
         * @property {number} RESULT_CODE_INTERNAL_ENQUEUE_VALIDATION_FAILED=46 RESULT_CODE_INTERNAL_ENQUEUE_VALIDATION_FAILED value
         * @property {number} RESULT_CODE_TX_COMMITTED_RECEIPT_MISSING=47 RESULT_CODE_TX_COMMITTED_RECEIPT_MISSING value
         * @property {number} RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_INVALID=48 RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_INVALID value
         * @property {number} RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_UNKNOWN=49 RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_UNKNOWN value
         * @property {number} RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_UNSUPPORTED=50 RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_UNSUPPORTED value
         * @property {number} RESULT_CODE_VALIDATOR_RESPONSE_SCHEMA_INVALID=51 RESULT_CODE_VALIDATOR_RESPONSE_SCHEMA_INVALID value
         * @property {number} RESULT_CODE_PENDING_REQUEST_MISSING_TX_DATA=52 RESULT_CODE_PENDING_REQUEST_MISSING_TX_DATA value
         * @property {number} RESULT_CODE_PROOF_PAYLOAD_MISMATCH=53 RESULT_CODE_PROOF_PAYLOAD_MISMATCH value
         * @property {number} RESULT_CODE_VALIDATOR_WRITER_KEY_NOT_REGISTERED=54 RESULT_CODE_VALIDATOR_WRITER_KEY_NOT_REGISTERED value
         * @property {number} RESULT_CODE_VALIDATOR_ADDRESS_MISMATCH=55 RESULT_CODE_VALIDATOR_ADDRESS_MISMATCH value
         * @property {number} RESULT_CODE_VALIDATOR_NODE_ENTRY_NOT_FOUND=56 RESULT_CODE_VALIDATOR_NODE_ENTRY_NOT_FOUND value
         * @property {number} RESULT_CODE_VALIDATOR_NODE_NOT_WRITER=57 RESULT_CODE_VALIDATOR_NODE_NOT_WRITER value
         * @property {number} RESULT_CODE_VALIDATOR_WRITER_KEY_MISMATCH=58 RESULT_CODE_VALIDATOR_WRITER_KEY_MISMATCH value
         * @property {number} RESULT_CODE_VALIDATOR_TX_OBJECT_INVALID=59 RESULT_CODE_VALIDATOR_TX_OBJECT_INVALID value
         * @property {number} RESULT_CODE_VALIDATOR_VA_MISSING=60 RESULT_CODE_VALIDATOR_VA_MISSING value
         * @property {number} RESULT_CODE_TX_INVALID_PAYLOAD=61 RESULT_CODE_TX_INVALID_PAYLOAD value
         */
        v1.ResultCode = (function() {
            var valuesById = {}, values = Object.create(valuesById);
            values[valuesById[0] = "RESULT_CODE_UNSPECIFIED"] = 0;
            values[valuesById[1] = "RESULT_CODE_OK"] = 1;
            values[valuesById[2] = "RESULT_CODE_INVALID_PAYLOAD"] = 2;
            values[valuesById[3] = "RESULT_CODE_RATE_LIMITED"] = 3;
            values[valuesById[4] = "RESULT_CODE_SIGNATURE_INVALID"] = 4;
            values[valuesById[5] = "RESULT_CODE_UNEXPECTED_ERROR"] = 5;
            values[valuesById[6] = "RESULT_CODE_TIMEOUT"] = 6;
            values[valuesById[7] = "RESULT_CODE_NODE_HAS_NO_WRITE_ACCESS"] = 7;
            values[valuesById[8] = "RESULT_CODE_TX_ACCEPTED_PROOF_UNAVAILABLE"] = 8;
            values[valuesById[9] = "RESULT_CODE_NODE_OVERLOADED"] = 9;
            values[valuesById[10] = "RESULT_CODE_TX_ALREADY_PENDING"] = 10;
            values[valuesById[11] = "RESULT_CODE_OPERATION_TYPE_UNKNOWN"] = 11;
            values[valuesById[12] = "RESULT_CODE_SCHEMA_VALIDATION_FAILED"] = 12;
            values[valuesById[13] = "RESULT_CODE_REQUESTER_ADDRESS_INVALID"] = 13;
            values[valuesById[14] = "RESULT_CODE_REQUESTER_PUBLIC_KEY_INVALID"] = 14;
            values[valuesById[15] = "RESULT_CODE_TX_HASH_MISMATCH"] = 15;
            values[valuesById[16] = "RESULT_CODE_TX_SIGNATURE_INVALID"] = 16;
            values[valuesById[17] = "RESULT_CODE_TX_EXPIRED"] = 17;
            values[valuesById[18] = "RESULT_CODE_TX_ALREADY_EXISTS"] = 18;
            values[valuesById[19] = "RESULT_CODE_OPERATION_ALREADY_COMPLETED"] = 19;
            values[valuesById[20] = "RESULT_CODE_REQUESTER_NOT_FOUND"] = 20;
            values[valuesById[21] = "RESULT_CODE_INSUFFICIENT_FEE_BALANCE"] = 21;
            values[valuesById[22] = "RESULT_CODE_EXTERNAL_BOOTSTRAP_EQUALS_MSB_BOOTSTRAP"] = 22;
            values[valuesById[23] = "RESULT_CODE_SELF_VALIDATION_FORBIDDEN"] = 23;
            values[valuesById[24] = "RESULT_CODE_ROLE_NODE_ENTRY_NOT_FOUND"] = 24;
            values[valuesById[25] = "RESULT_CODE_ROLE_NODE_ALREADY_WRITER"] = 25;
            values[valuesById[26] = "RESULT_CODE_ROLE_NODE_NOT_WHITELISTED"] = 26;
            values[valuesById[27] = "RESULT_CODE_ROLE_NODE_NOT_WRITER"] = 27;
            values[valuesById[28] = "RESULT_CODE_ROLE_NODE_IS_INDEXER"] = 28;
            values[valuesById[29] = "RESULT_CODE_ROLE_ADMIN_ENTRY_MISSING"] = 29;
            values[valuesById[30] = "RESULT_CODE_ROLE_INVALID_RECOVERY_CASE"] = 30;
            values[valuesById[31] = "RESULT_CODE_ROLE_UNKNOWN_OPERATION"] = 31;
            values[valuesById[32] = "RESULT_CODE_ROLE_INVALID_WRITER_KEY"] = 32;
            values[valuesById[33] = "RESULT_CODE_ROLE_INSUFFICIENT_FEE_BALANCE"] = 33;
            values[valuesById[34] = "RESULT_CODE_MSB_BOOTSTRAP_MISMATCH"] = 34;
            values[valuesById[35] = "RESULT_CODE_EXTERNAL_BOOTSTRAP_NOT_DEPLOYED"] = 35;
            values[valuesById[36] = "RESULT_CODE_EXTERNAL_BOOTSTRAP_TX_MISSING"] = 36;
            values[valuesById[37] = "RESULT_CODE_EXTERNAL_BOOTSTRAP_MISMATCH"] = 37;
            values[valuesById[38] = "RESULT_CODE_BOOTSTRAP_ALREADY_EXISTS"] = 38;
            values[valuesById[39] = "RESULT_CODE_TRANSFER_RECIPIENT_ADDRESS_INVALID"] = 39;
            values[valuesById[40] = "RESULT_CODE_TRANSFER_RECIPIENT_PUBLIC_KEY_INVALID"] = 40;
            values[valuesById[41] = "RESULT_CODE_TRANSFER_AMOUNT_TOO_LARGE"] = 41;
            values[valuesById[42] = "RESULT_CODE_TRANSFER_SENDER_NOT_FOUND"] = 42;
            values[valuesById[43] = "RESULT_CODE_TRANSFER_INSUFFICIENT_BALANCE"] = 43;
            values[valuesById[44] = "RESULT_CODE_TRANSFER_RECIPIENT_BALANCE_OVERFLOW"] = 44;
            values[valuesById[45] = "RESULT_CODE_TX_HASH_INVALID_FORMAT"] = 45;
            values[valuesById[46] = "RESULT_CODE_INTERNAL_ENQUEUE_VALIDATION_FAILED"] = 46;
            values[valuesById[47] = "RESULT_CODE_TX_COMMITTED_RECEIPT_MISSING"] = 47;
            values[valuesById[48] = "RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_INVALID"] = 48;
            values[valuesById[49] = "RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_UNKNOWN"] = 49;
            values[valuesById[50] = "RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_UNSUPPORTED"] = 50;
            values[valuesById[51] = "RESULT_CODE_VALIDATOR_RESPONSE_SCHEMA_INVALID"] = 51;
            values[valuesById[52] = "RESULT_CODE_PENDING_REQUEST_MISSING_TX_DATA"] = 52;
            values[valuesById[53] = "RESULT_CODE_PROOF_PAYLOAD_MISMATCH"] = 53;
            values[valuesById[54] = "RESULT_CODE_VALIDATOR_WRITER_KEY_NOT_REGISTERED"] = 54;
            values[valuesById[55] = "RESULT_CODE_VALIDATOR_ADDRESS_MISMATCH"] = 55;
            values[valuesById[56] = "RESULT_CODE_VALIDATOR_NODE_ENTRY_NOT_FOUND"] = 56;
            values[valuesById[57] = "RESULT_CODE_VALIDATOR_NODE_NOT_WRITER"] = 57;
            values[valuesById[58] = "RESULT_CODE_VALIDATOR_WRITER_KEY_MISMATCH"] = 58;
            values[valuesById[59] = "RESULT_CODE_VALIDATOR_TX_OBJECT_INVALID"] = 59;
            values[valuesById[60] = "RESULT_CODE_VALIDATOR_VA_MISSING"] = 60;
            values[valuesById[61] = "RESULT_CODE_TX_INVALID_PAYLOAD"] = 61;
            return values;
        })();

        v1.BroadcastTransactionRequest = (function() {

            /**
             * Properties of a BroadcastTransactionRequest.
             * @memberof network.v1
             * @interface IBroadcastTransactionRequest
             * @property {Uint8Array|null} [data] BroadcastTransactionRequest data
             * @property {Uint8Array|null} [nonce] BroadcastTransactionRequest nonce
             * @property {Uint8Array|null} [signature] BroadcastTransactionRequest signature
             */

            /**
             * Constructs a new BroadcastTransactionRequest.
             * @memberof network.v1
             * @classdesc Represents a BroadcastTransactionRequest.
             * @implements IBroadcastTransactionRequest
             * @constructor
             * @param {network.v1.IBroadcastTransactionRequest=} [properties] Properties to set
             */
            function BroadcastTransactionRequest(properties) {
                if (properties)
                    for (var keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                        if (properties[keys[i]] != null)
                            this[keys[i]] = properties[keys[i]];
            }

            /**
             * BroadcastTransactionRequest data.
             * @member {Uint8Array} data
             * @memberof network.v1.BroadcastTransactionRequest
             * @instance
             */
            BroadcastTransactionRequest.prototype.data = $util.newBuffer([]);

            /**
             * BroadcastTransactionRequest nonce.
             * @member {Uint8Array} nonce
             * @memberof network.v1.BroadcastTransactionRequest
             * @instance
             */
            BroadcastTransactionRequest.prototype.nonce = $util.newBuffer([]);

            /**
             * BroadcastTransactionRequest signature.
             * @member {Uint8Array} signature
             * @memberof network.v1.BroadcastTransactionRequest
             * @instance
             */
            BroadcastTransactionRequest.prototype.signature = $util.newBuffer([]);

            /**
             * Creates a new BroadcastTransactionRequest instance using the specified properties.
             * @function create
             * @memberof network.v1.BroadcastTransactionRequest
             * @static
             * @param {network.v1.IBroadcastTransactionRequest=} [properties] Properties to set
             * @returns {network.v1.BroadcastTransactionRequest} BroadcastTransactionRequest instance
             */
            BroadcastTransactionRequest.create = function create(properties) {
                return new BroadcastTransactionRequest(properties);
            };

            /**
             * Encodes the specified BroadcastTransactionRequest message. Does not implicitly {@link network.v1.BroadcastTransactionRequest.verify|verify} messages.
             * @function encode
             * @memberof network.v1.BroadcastTransactionRequest
             * @static
             * @param {network.v1.IBroadcastTransactionRequest} message BroadcastTransactionRequest message or plain object to encode
             * @param {$protobuf.Writer} [writer] Writer to encode to
             * @returns {$protobuf.Writer} Writer
             */
            BroadcastTransactionRequest.encode = function encode(message, writer) {
                if (!writer)
                    writer = $Writer.create();
                if (message.data != null && Object.hasOwnProperty.call(message, "data"))
                    writer.uint32(/* id 1, wireType 2 =*/10).bytes(message.data);
                if (message.nonce != null && Object.hasOwnProperty.call(message, "nonce"))
                    writer.uint32(/* id 2, wireType 2 =*/18).bytes(message.nonce);
                if (message.signature != null && Object.hasOwnProperty.call(message, "signature"))
                    writer.uint32(/* id 3, wireType 2 =*/26).bytes(message.signature);
                return writer;
            };

            /**
             * Encodes the specified BroadcastTransactionRequest message, length delimited. Does not implicitly {@link network.v1.BroadcastTransactionRequest.verify|verify} messages.
             * @function encodeDelimited
             * @memberof network.v1.BroadcastTransactionRequest
             * @static
             * @param {network.v1.IBroadcastTransactionRequest} message BroadcastTransactionRequest message or plain object to encode
             * @param {$protobuf.Writer} [writer] Writer to encode to
             * @returns {$protobuf.Writer} Writer
             */
            BroadcastTransactionRequest.encodeDelimited = function encodeDelimited(message, writer) {
                return this.encode(message, writer).ldelim();
            };

            /**
             * Decodes a BroadcastTransactionRequest message from the specified reader or buffer.
             * @function decode
             * @memberof network.v1.BroadcastTransactionRequest
             * @static
             * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
             * @param {number} [length] Message length if known beforehand
             * @returns {network.v1.BroadcastTransactionRequest} BroadcastTransactionRequest
             * @throws {Error} If the payload is not a reader or valid buffer
             * @throws {$protobuf.util.ProtocolError} If required fields are missing
             */
            BroadcastTransactionRequest.decode = function decode(reader, length, error) {
                if (!(reader instanceof $Reader))
                    reader = $Reader.create(reader);
                var end = length === undefined ? reader.len : reader.pos + length, message = new $root.network.v1.BroadcastTransactionRequest();
                while (reader.pos < end) {
                    var tag = reader.uint32();
                    if (tag === error)
                        break;
                    switch (tag >>> 3) {
                    case 1: {
                            message.data = reader.bytes();
                            break;
                        }
                    case 2: {
                            message.nonce = reader.bytes();
                            break;
                        }
                    case 3: {
                            message.signature = reader.bytes();
                            break;
                        }
                    default:
                        reader.skipType(tag & 7);
                        break;
                    }
                }
                return message;
            };

            /**
             * Decodes a BroadcastTransactionRequest message from the specified reader or buffer, length delimited.
             * @function decodeDelimited
             * @memberof network.v1.BroadcastTransactionRequest
             * @static
             * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
             * @returns {network.v1.BroadcastTransactionRequest} BroadcastTransactionRequest
             * @throws {Error} If the payload is not a reader or valid buffer
             * @throws {$protobuf.util.ProtocolError} If required fields are missing
             */
            BroadcastTransactionRequest.decodeDelimited = function decodeDelimited(reader) {
                if (!(reader instanceof $Reader))
                    reader = new $Reader(reader);
                return this.decode(reader, reader.uint32());
            };

            /**
             * Verifies a BroadcastTransactionRequest message.
             * @function verify
             * @memberof network.v1.BroadcastTransactionRequest
             * @static
             * @param {Object.<string,*>} message Plain object to verify
             * @returns {string|null} `null` if valid, otherwise the reason why it is not
             */
            BroadcastTransactionRequest.verify = function verify(message) {
                if (typeof message !== "object" || message === null)
                    return "object expected";
                if (message.data != null && message.hasOwnProperty("data"))
                    if (!(message.data && typeof message.data.length === "number" || $util.isString(message.data)))
                        return "data: buffer expected";
                if (message.nonce != null && message.hasOwnProperty("nonce"))
                    if (!(message.nonce && typeof message.nonce.length === "number" || $util.isString(message.nonce)))
                        return "nonce: buffer expected";
                if (message.signature != null && message.hasOwnProperty("signature"))
                    if (!(message.signature && typeof message.signature.length === "number" || $util.isString(message.signature)))
                        return "signature: buffer expected";
                return null;
            };

            /**
             * Creates a BroadcastTransactionRequest message from a plain object. Also converts values to their respective internal types.
             * @function fromObject
             * @memberof network.v1.BroadcastTransactionRequest
             * @static
             * @param {Object.<string,*>} object Plain object
             * @returns {network.v1.BroadcastTransactionRequest} BroadcastTransactionRequest
             */
            BroadcastTransactionRequest.fromObject = function fromObject(object) {
                if (object instanceof $root.network.v1.BroadcastTransactionRequest)
                    return object;
                var message = new $root.network.v1.BroadcastTransactionRequest();
                if (object.data != null)
                    if (typeof object.data === "string")
                        $util.base64.decode(object.data, message.data = $util.newBuffer($util.base64.length(object.data)), 0);
                    else if (object.data.length >= 0)
                        message.data = object.data;
                if (object.nonce != null)
                    if (typeof object.nonce === "string")
                        $util.base64.decode(object.nonce, message.nonce = $util.newBuffer($util.base64.length(object.nonce)), 0);
                    else if (object.nonce.length >= 0)
                        message.nonce = object.nonce;
                if (object.signature != null)
                    if (typeof object.signature === "string")
                        $util.base64.decode(object.signature, message.signature = $util.newBuffer($util.base64.length(object.signature)), 0);
                    else if (object.signature.length >= 0)
                        message.signature = object.signature;
                return message;
            };

            /**
             * Creates a plain object from a BroadcastTransactionRequest message. Also converts values to other types if specified.
             * @function toObject
             * @memberof network.v1.BroadcastTransactionRequest
             * @static
             * @param {network.v1.BroadcastTransactionRequest} message BroadcastTransactionRequest
             * @param {$protobuf.IConversionOptions} [options] Conversion options
             * @returns {Object.<string,*>} Plain object
             */
            BroadcastTransactionRequest.toObject = function toObject(message, options) {
                if (!options)
                    options = {};
                var object = {};
                if (options.defaults) {
                    if (options.bytes === String)
                        object.data = "";
                    else {
                        object.data = [];
                        if (options.bytes !== Array)
                            object.data = $util.newBuffer(object.data);
                    }
                    if (options.bytes === String)
                        object.nonce = "";
                    else {
                        object.nonce = [];
                        if (options.bytes !== Array)
                            object.nonce = $util.newBuffer(object.nonce);
                    }
                    if (options.bytes === String)
                        object.signature = "";
                    else {
                        object.signature = [];
                        if (options.bytes !== Array)
                            object.signature = $util.newBuffer(object.signature);
                    }
                }
                if (message.data != null && message.hasOwnProperty("data"))
                    object.data = options.bytes === String ? $util.base64.encode(message.data, 0, message.data.length) : options.bytes === Array ? Array.prototype.slice.call(message.data) : message.data;
                if (message.nonce != null && message.hasOwnProperty("nonce"))
                    object.nonce = options.bytes === String ? $util.base64.encode(message.nonce, 0, message.nonce.length) : options.bytes === Array ? Array.prototype.slice.call(message.nonce) : message.nonce;
                if (message.signature != null && message.hasOwnProperty("signature"))
                    object.signature = options.bytes === String ? $util.base64.encode(message.signature, 0, message.signature.length) : options.bytes === Array ? Array.prototype.slice.call(message.signature) : message.signature;
                return object;
            };

            /**
             * Converts this BroadcastTransactionRequest to JSON.
             * @function toJSON
             * @memberof network.v1.BroadcastTransactionRequest
             * @instance
             * @returns {Object.<string,*>} JSON object
             */
            BroadcastTransactionRequest.prototype.toJSON = function toJSON() {
                return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
            };

            /**
             * Gets the default type url for BroadcastTransactionRequest
             * @function getTypeUrl
             * @memberof network.v1.BroadcastTransactionRequest
             * @static
             * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
             * @returns {string} The default type url
             */
            BroadcastTransactionRequest.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
                if (typeUrlPrefix === undefined) {
                    typeUrlPrefix = "type.googleapis.com";
                }
                return typeUrlPrefix + "/network.v1.BroadcastTransactionRequest";
            };

            return BroadcastTransactionRequest;
        })();

        v1.BroadcastTransactionResponse = (function() {

            /**
             * Properties of a BroadcastTransactionResponse.
             * @memberof network.v1
             * @interface IBroadcastTransactionResponse
             * @property {Uint8Array|null} [nonce] BroadcastTransactionResponse nonce
             * @property {Uint8Array|null} [signature] BroadcastTransactionResponse signature
             * @property {Uint8Array|null} [proof] BroadcastTransactionResponse proof
             * @property {number|Long|null} [timestamp] BroadcastTransactionResponse timestamp
             * @property {network.v1.ResultCode|null} [result] BroadcastTransactionResponse result
             */

            /**
             * Constructs a new BroadcastTransactionResponse.
             * @memberof network.v1
             * @classdesc Represents a BroadcastTransactionResponse.
             * @implements IBroadcastTransactionResponse
             * @constructor
             * @param {network.v1.IBroadcastTransactionResponse=} [properties] Properties to set
             */
            function BroadcastTransactionResponse(properties) {
                if (properties)
                    for (var keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                        if (properties[keys[i]] != null)
                            this[keys[i]] = properties[keys[i]];
            }

            /**
             * BroadcastTransactionResponse nonce.
             * @member {Uint8Array} nonce
             * @memberof network.v1.BroadcastTransactionResponse
             * @instance
             */
            BroadcastTransactionResponse.prototype.nonce = $util.newBuffer([]);

            /**
             * BroadcastTransactionResponse signature.
             * @member {Uint8Array} signature
             * @memberof network.v1.BroadcastTransactionResponse
             * @instance
             */
            BroadcastTransactionResponse.prototype.signature = $util.newBuffer([]);

            /**
             * BroadcastTransactionResponse proof.
             * @member {Uint8Array} proof
             * @memberof network.v1.BroadcastTransactionResponse
             * @instance
             */
            BroadcastTransactionResponse.prototype.proof = $util.newBuffer([]);

            /**
             * BroadcastTransactionResponse timestamp.
             * @member {number|Long} timestamp
             * @memberof network.v1.BroadcastTransactionResponse
             * @instance
             */
            BroadcastTransactionResponse.prototype.timestamp = $util.Long ? $util.Long.fromBits(0,0,true) : 0;

            /**
             * BroadcastTransactionResponse result.
             * @member {network.v1.ResultCode} result
             * @memberof network.v1.BroadcastTransactionResponse
             * @instance
             */
            BroadcastTransactionResponse.prototype.result = 0;

            /**
             * Creates a new BroadcastTransactionResponse instance using the specified properties.
             * @function create
             * @memberof network.v1.BroadcastTransactionResponse
             * @static
             * @param {network.v1.IBroadcastTransactionResponse=} [properties] Properties to set
             * @returns {network.v1.BroadcastTransactionResponse} BroadcastTransactionResponse instance
             */
            BroadcastTransactionResponse.create = function create(properties) {
                return new BroadcastTransactionResponse(properties);
            };

            /**
             * Encodes the specified BroadcastTransactionResponse message. Does not implicitly {@link network.v1.BroadcastTransactionResponse.verify|verify} messages.
             * @function encode
             * @memberof network.v1.BroadcastTransactionResponse
             * @static
             * @param {network.v1.IBroadcastTransactionResponse} message BroadcastTransactionResponse message or plain object to encode
             * @param {$protobuf.Writer} [writer] Writer to encode to
             * @returns {$protobuf.Writer} Writer
             */
            BroadcastTransactionResponse.encode = function encode(message, writer) {
                if (!writer)
                    writer = $Writer.create();
                if (message.nonce != null && Object.hasOwnProperty.call(message, "nonce"))
                    writer.uint32(/* id 1, wireType 2 =*/10).bytes(message.nonce);
                if (message.signature != null && Object.hasOwnProperty.call(message, "signature"))
                    writer.uint32(/* id 2, wireType 2 =*/18).bytes(message.signature);
                if (message.proof != null && Object.hasOwnProperty.call(message, "proof"))
                    writer.uint32(/* id 3, wireType 2 =*/26).bytes(message.proof);
                if (message.timestamp != null && Object.hasOwnProperty.call(message, "timestamp"))
                    writer.uint32(/* id 4, wireType 0 =*/32).uint64(message.timestamp);
                if (message.result != null && Object.hasOwnProperty.call(message, "result"))
                    writer.uint32(/* id 5, wireType 0 =*/40).int32(message.result);
                return writer;
            };

            /**
             * Encodes the specified BroadcastTransactionResponse message, length delimited. Does not implicitly {@link network.v1.BroadcastTransactionResponse.verify|verify} messages.
             * @function encodeDelimited
             * @memberof network.v1.BroadcastTransactionResponse
             * @static
             * @param {network.v1.IBroadcastTransactionResponse} message BroadcastTransactionResponse message or plain object to encode
             * @param {$protobuf.Writer} [writer] Writer to encode to
             * @returns {$protobuf.Writer} Writer
             */
            BroadcastTransactionResponse.encodeDelimited = function encodeDelimited(message, writer) {
                return this.encode(message, writer).ldelim();
            };

            /**
             * Decodes a BroadcastTransactionResponse message from the specified reader or buffer.
             * @function decode
             * @memberof network.v1.BroadcastTransactionResponse
             * @static
             * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
             * @param {number} [length] Message length if known beforehand
             * @returns {network.v1.BroadcastTransactionResponse} BroadcastTransactionResponse
             * @throws {Error} If the payload is not a reader or valid buffer
             * @throws {$protobuf.util.ProtocolError} If required fields are missing
             */
            BroadcastTransactionResponse.decode = function decode(reader, length, error) {
                if (!(reader instanceof $Reader))
                    reader = $Reader.create(reader);
                var end = length === undefined ? reader.len : reader.pos + length, message = new $root.network.v1.BroadcastTransactionResponse();
                while (reader.pos < end) {
                    var tag = reader.uint32();
                    if (tag === error)
                        break;
                    switch (tag >>> 3) {
                    case 1: {
                            message.nonce = reader.bytes();
                            break;
                        }
                    case 2: {
                            message.signature = reader.bytes();
                            break;
                        }
                    case 3: {
                            message.proof = reader.bytes();
                            break;
                        }
                    case 4: {
                            message.timestamp = reader.uint64();
                            break;
                        }
                    case 5: {
                            message.result = reader.int32();
                            break;
                        }
                    default:
                        reader.skipType(tag & 7);
                        break;
                    }
                }
                return message;
            };

            /**
             * Decodes a BroadcastTransactionResponse message from the specified reader or buffer, length delimited.
             * @function decodeDelimited
             * @memberof network.v1.BroadcastTransactionResponse
             * @static
             * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
             * @returns {network.v1.BroadcastTransactionResponse} BroadcastTransactionResponse
             * @throws {Error} If the payload is not a reader or valid buffer
             * @throws {$protobuf.util.ProtocolError} If required fields are missing
             */
            BroadcastTransactionResponse.decodeDelimited = function decodeDelimited(reader) {
                if (!(reader instanceof $Reader))
                    reader = new $Reader(reader);
                return this.decode(reader, reader.uint32());
            };

            /**
             * Verifies a BroadcastTransactionResponse message.
             * @function verify
             * @memberof network.v1.BroadcastTransactionResponse
             * @static
             * @param {Object.<string,*>} message Plain object to verify
             * @returns {string|null} `null` if valid, otherwise the reason why it is not
             */
            BroadcastTransactionResponse.verify = function verify(message) {
                if (typeof message !== "object" || message === null)
                    return "object expected";
                if (message.nonce != null && message.hasOwnProperty("nonce"))
                    if (!(message.nonce && typeof message.nonce.length === "number" || $util.isString(message.nonce)))
                        return "nonce: buffer expected";
                if (message.signature != null && message.hasOwnProperty("signature"))
                    if (!(message.signature && typeof message.signature.length === "number" || $util.isString(message.signature)))
                        return "signature: buffer expected";
                if (message.proof != null && message.hasOwnProperty("proof"))
                    if (!(message.proof && typeof message.proof.length === "number" || $util.isString(message.proof)))
                        return "proof: buffer expected";
                if (message.timestamp != null && message.hasOwnProperty("timestamp"))
                    if (!$util.isInteger(message.timestamp) && !(message.timestamp && $util.isInteger(message.timestamp.low) && $util.isInteger(message.timestamp.high)))
                        return "timestamp: integer|Long expected";
                if (message.result != null && message.hasOwnProperty("result"))
                    switch (message.result) {
                    default:
                        return "result: enum value expected";
                    case 0:
                    case 1:
                    case 2:
                    case 3:
                    case 4:
                    case 5:
                    case 6:
                    case 7:
                    case 8:
                    case 9:
                    case 10:
                    case 11:
                    case 12:
                    case 13:
                    case 14:
                    case 15:
                    case 16:
                    case 17:
                    case 18:
                    case 19:
                    case 20:
                    case 21:
                    case 22:
                    case 23:
                    case 24:
                    case 25:
                    case 26:
                    case 27:
                    case 28:
                    case 29:
                    case 30:
                    case 31:
                    case 32:
                    case 33:
                    case 34:
                    case 35:
                    case 36:
                    case 37:
                    case 38:
                    case 39:
                    case 40:
                    case 41:
                    case 42:
                    case 43:
                    case 44:
                    case 45:
                    case 46:
                    case 47:
                    case 48:
                    case 49:
                    case 50:
                    case 51:
                    case 52:
                    case 53:
                    case 54:
                    case 55:
                    case 56:
                    case 57:
                    case 58:
                    case 59:
                    case 60:
                    case 61:
                        break;
                    }
                return null;
            };

            /**
             * Creates a BroadcastTransactionResponse message from a plain object. Also converts values to their respective internal types.
             * @function fromObject
             * @memberof network.v1.BroadcastTransactionResponse
             * @static
             * @param {Object.<string,*>} object Plain object
             * @returns {network.v1.BroadcastTransactionResponse} BroadcastTransactionResponse
             */
            BroadcastTransactionResponse.fromObject = function fromObject(object) {
                if (object instanceof $root.network.v1.BroadcastTransactionResponse)
                    return object;
                var message = new $root.network.v1.BroadcastTransactionResponse();
                if (object.nonce != null)
                    if (typeof object.nonce === "string")
                        $util.base64.decode(object.nonce, message.nonce = $util.newBuffer($util.base64.length(object.nonce)), 0);
                    else if (object.nonce.length >= 0)
                        message.nonce = object.nonce;
                if (object.signature != null)
                    if (typeof object.signature === "string")
                        $util.base64.decode(object.signature, message.signature = $util.newBuffer($util.base64.length(object.signature)), 0);
                    else if (object.signature.length >= 0)
                        message.signature = object.signature;
                if (object.proof != null)
                    if (typeof object.proof === "string")
                        $util.base64.decode(object.proof, message.proof = $util.newBuffer($util.base64.length(object.proof)), 0);
                    else if (object.proof.length >= 0)
                        message.proof = object.proof;
                if (object.timestamp != null)
                    if ($util.Long)
                        (message.timestamp = $util.Long.fromValue(object.timestamp)).unsigned = true;
                    else if (typeof object.timestamp === "string")
                        message.timestamp = parseInt(object.timestamp, 10);
                    else if (typeof object.timestamp === "number")
                        message.timestamp = object.timestamp;
                    else if (typeof object.timestamp === "object")
                        message.timestamp = new $util.LongBits(object.timestamp.low >>> 0, object.timestamp.high >>> 0).toNumber(true);
                switch (object.result) {
                default:
                    if (typeof object.result === "number") {
                        message.result = object.result;
                        break;
                    }
                    break;
                case "RESULT_CODE_UNSPECIFIED":
                case 0:
                    message.result = 0;
                    break;
                case "RESULT_CODE_OK":
                case 1:
                    message.result = 1;
                    break;
                case "RESULT_CODE_INVALID_PAYLOAD":
                case 2:
                    message.result = 2;
                    break;
                case "RESULT_CODE_RATE_LIMITED":
                case 3:
                    message.result = 3;
                    break;
                case "RESULT_CODE_SIGNATURE_INVALID":
                case 4:
                    message.result = 4;
                    break;
                case "RESULT_CODE_UNEXPECTED_ERROR":
                case 5:
                    message.result = 5;
                    break;
                case "RESULT_CODE_TIMEOUT":
                case 6:
                    message.result = 6;
                    break;
                case "RESULT_CODE_NODE_HAS_NO_WRITE_ACCESS":
                case 7:
                    message.result = 7;
                    break;
                case "RESULT_CODE_TX_ACCEPTED_PROOF_UNAVAILABLE":
                case 8:
                    message.result = 8;
                    break;
                case "RESULT_CODE_NODE_OVERLOADED":
                case 9:
                    message.result = 9;
                    break;
                case "RESULT_CODE_TX_ALREADY_PENDING":
                case 10:
                    message.result = 10;
                    break;
                case "RESULT_CODE_OPERATION_TYPE_UNKNOWN":
                case 11:
                    message.result = 11;
                    break;
                case "RESULT_CODE_SCHEMA_VALIDATION_FAILED":
                case 12:
                    message.result = 12;
                    break;
                case "RESULT_CODE_REQUESTER_ADDRESS_INVALID":
                case 13:
                    message.result = 13;
                    break;
                case "RESULT_CODE_REQUESTER_PUBLIC_KEY_INVALID":
                case 14:
                    message.result = 14;
                    break;
                case "RESULT_CODE_TX_HASH_MISMATCH":
                case 15:
                    message.result = 15;
                    break;
                case "RESULT_CODE_TX_SIGNATURE_INVALID":
                case 16:
                    message.result = 16;
                    break;
                case "RESULT_CODE_TX_EXPIRED":
                case 17:
                    message.result = 17;
                    break;
                case "RESULT_CODE_TX_ALREADY_EXISTS":
                case 18:
                    message.result = 18;
                    break;
                case "RESULT_CODE_OPERATION_ALREADY_COMPLETED":
                case 19:
                    message.result = 19;
                    break;
                case "RESULT_CODE_REQUESTER_NOT_FOUND":
                case 20:
                    message.result = 20;
                    break;
                case "RESULT_CODE_INSUFFICIENT_FEE_BALANCE":
                case 21:
                    message.result = 21;
                    break;
                case "RESULT_CODE_EXTERNAL_BOOTSTRAP_EQUALS_MSB_BOOTSTRAP":
                case 22:
                    message.result = 22;
                    break;
                case "RESULT_CODE_SELF_VALIDATION_FORBIDDEN":
                case 23:
                    message.result = 23;
                    break;
                case "RESULT_CODE_ROLE_NODE_ENTRY_NOT_FOUND":
                case 24:
                    message.result = 24;
                    break;
                case "RESULT_CODE_ROLE_NODE_ALREADY_WRITER":
                case 25:
                    message.result = 25;
                    break;
                case "RESULT_CODE_ROLE_NODE_NOT_WHITELISTED":
                case 26:
                    message.result = 26;
                    break;
                case "RESULT_CODE_ROLE_NODE_NOT_WRITER":
                case 27:
                    message.result = 27;
                    break;
                case "RESULT_CODE_ROLE_NODE_IS_INDEXER":
                case 28:
                    message.result = 28;
                    break;
                case "RESULT_CODE_ROLE_ADMIN_ENTRY_MISSING":
                case 29:
                    message.result = 29;
                    break;
                case "RESULT_CODE_ROLE_INVALID_RECOVERY_CASE":
                case 30:
                    message.result = 30;
                    break;
                case "RESULT_CODE_ROLE_UNKNOWN_OPERATION":
                case 31:
                    message.result = 31;
                    break;
                case "RESULT_CODE_ROLE_INVALID_WRITER_KEY":
                case 32:
                    message.result = 32;
                    break;
                case "RESULT_CODE_ROLE_INSUFFICIENT_FEE_BALANCE":
                case 33:
                    message.result = 33;
                    break;
                case "RESULT_CODE_MSB_BOOTSTRAP_MISMATCH":
                case 34:
                    message.result = 34;
                    break;
                case "RESULT_CODE_EXTERNAL_BOOTSTRAP_NOT_DEPLOYED":
                case 35:
                    message.result = 35;
                    break;
                case "RESULT_CODE_EXTERNAL_BOOTSTRAP_TX_MISSING":
                case 36:
                    message.result = 36;
                    break;
                case "RESULT_CODE_EXTERNAL_BOOTSTRAP_MISMATCH":
                case 37:
                    message.result = 37;
                    break;
                case "RESULT_CODE_BOOTSTRAP_ALREADY_EXISTS":
                case 38:
                    message.result = 38;
                    break;
                case "RESULT_CODE_TRANSFER_RECIPIENT_ADDRESS_INVALID":
                case 39:
                    message.result = 39;
                    break;
                case "RESULT_CODE_TRANSFER_RECIPIENT_PUBLIC_KEY_INVALID":
                case 40:
                    message.result = 40;
                    break;
                case "RESULT_CODE_TRANSFER_AMOUNT_TOO_LARGE":
                case 41:
                    message.result = 41;
                    break;
                case "RESULT_CODE_TRANSFER_SENDER_NOT_FOUND":
                case 42:
                    message.result = 42;
                    break;
                case "RESULT_CODE_TRANSFER_INSUFFICIENT_BALANCE":
                case 43:
                    message.result = 43;
                    break;
                case "RESULT_CODE_TRANSFER_RECIPIENT_BALANCE_OVERFLOW":
                case 44:
                    message.result = 44;
                    break;
                case "RESULT_CODE_TX_HASH_INVALID_FORMAT":
                case 45:
                    message.result = 45;
                    break;
                case "RESULT_CODE_INTERNAL_ENQUEUE_VALIDATION_FAILED":
                case 46:
                    message.result = 46;
                    break;
                case "RESULT_CODE_TX_COMMITTED_RECEIPT_MISSING":
                case 47:
                    message.result = 47;
                    break;
                case "RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_INVALID":
                case 48:
                    message.result = 48;
                    break;
                case "RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_UNKNOWN":
                case 49:
                    message.result = 49;
                    break;
                case "RESULT_CODE_VALIDATOR_RESPONSE_TX_TYPE_UNSUPPORTED":
                case 50:
                    message.result = 50;
                    break;
                case "RESULT_CODE_VALIDATOR_RESPONSE_SCHEMA_INVALID":
                case 51:
                    message.result = 51;
                    break;
                case "RESULT_CODE_PENDING_REQUEST_MISSING_TX_DATA":
                case 52:
                    message.result = 52;
                    break;
                case "RESULT_CODE_PROOF_PAYLOAD_MISMATCH":
                case 53:
                    message.result = 53;
                    break;
                case "RESULT_CODE_VALIDATOR_WRITER_KEY_NOT_REGISTERED":
                case 54:
                    message.result = 54;
                    break;
                case "RESULT_CODE_VALIDATOR_ADDRESS_MISMATCH":
                case 55:
                    message.result = 55;
                    break;
                case "RESULT_CODE_VALIDATOR_NODE_ENTRY_NOT_FOUND":
                case 56:
                    message.result = 56;
                    break;
                case "RESULT_CODE_VALIDATOR_NODE_NOT_WRITER":
                case 57:
                    message.result = 57;
                    break;
                case "RESULT_CODE_VALIDATOR_WRITER_KEY_MISMATCH":
                case 58:
                    message.result = 58;
                    break;
                case "RESULT_CODE_VALIDATOR_TX_OBJECT_INVALID":
                case 59:
                    message.result = 59;
                    break;
                case "RESULT_CODE_VALIDATOR_VA_MISSING":
                case 60:
                    message.result = 60;
                    break;
                case "RESULT_CODE_TX_INVALID_PAYLOAD":
                case 61:
                    message.result = 61;
                    break;
                }
                return message;
            };

            /**
             * Creates a plain object from a BroadcastTransactionResponse message. Also converts values to other types if specified.
             * @function toObject
             * @memberof network.v1.BroadcastTransactionResponse
             * @static
             * @param {network.v1.BroadcastTransactionResponse} message BroadcastTransactionResponse
             * @param {$protobuf.IConversionOptions} [options] Conversion options
             * @returns {Object.<string,*>} Plain object
             */
            BroadcastTransactionResponse.toObject = function toObject(message, options) {
                if (!options)
                    options = {};
                var object = {};
                if (options.defaults) {
                    if (options.bytes === String)
                        object.nonce = "";
                    else {
                        object.nonce = [];
                        if (options.bytes !== Array)
                            object.nonce = $util.newBuffer(object.nonce);
                    }
                    if (options.bytes === String)
                        object.signature = "";
                    else {
                        object.signature = [];
                        if (options.bytes !== Array)
                            object.signature = $util.newBuffer(object.signature);
                    }
                    if (options.bytes === String)
                        object.proof = "";
                    else {
                        object.proof = [];
                        if (options.bytes !== Array)
                            object.proof = $util.newBuffer(object.proof);
                    }
                    if ($util.Long) {
                        var long = new $util.Long(0, 0, true);
                        object.timestamp = options.longs === String ? long.toString() : options.longs === Number ? long.toNumber() : long;
                    } else
                        object.timestamp = options.longs === String ? "0" : 0;
                    object.result = options.enums === String ? "RESULT_CODE_UNSPECIFIED" : 0;
                }
                if (message.nonce != null && message.hasOwnProperty("nonce"))
                    object.nonce = options.bytes === String ? $util.base64.encode(message.nonce, 0, message.nonce.length) : options.bytes === Array ? Array.prototype.slice.call(message.nonce) : message.nonce;
                if (message.signature != null && message.hasOwnProperty("signature"))
                    object.signature = options.bytes === String ? $util.base64.encode(message.signature, 0, message.signature.length) : options.bytes === Array ? Array.prototype.slice.call(message.signature) : message.signature;
                if (message.proof != null && message.hasOwnProperty("proof"))
                    object.proof = options.bytes === String ? $util.base64.encode(message.proof, 0, message.proof.length) : options.bytes === Array ? Array.prototype.slice.call(message.proof) : message.proof;
                if (message.timestamp != null && message.hasOwnProperty("timestamp"))
                    if (typeof message.timestamp === "number")
                        object.timestamp = options.longs === String ? String(message.timestamp) : message.timestamp;
                    else
                        object.timestamp = options.longs === String ? $util.Long.prototype.toString.call(message.timestamp) : options.longs === Number ? new $util.LongBits(message.timestamp.low >>> 0, message.timestamp.high >>> 0).toNumber(true) : message.timestamp;
                if (message.result != null && message.hasOwnProperty("result"))
                    object.result = options.enums === String ? $root.network.v1.ResultCode[message.result] === undefined ? message.result : $root.network.v1.ResultCode[message.result] : message.result;
                return object;
            };

            /**
             * Converts this BroadcastTransactionResponse to JSON.
             * @function toJSON
             * @memberof network.v1.BroadcastTransactionResponse
             * @instance
             * @returns {Object.<string,*>} JSON object
             */
            BroadcastTransactionResponse.prototype.toJSON = function toJSON() {
                return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
            };

            /**
             * Gets the default type url for BroadcastTransactionResponse
             * @function getTypeUrl
             * @memberof network.v1.BroadcastTransactionResponse
             * @static
             * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
             * @returns {string} The default type url
             */
            BroadcastTransactionResponse.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
                if (typeUrlPrefix === undefined) {
                    typeUrlPrefix = "type.googleapis.com";
                }
                return typeUrlPrefix + "/network.v1.BroadcastTransactionResponse";
            };

            return BroadcastTransactionResponse;
        })();

        return v1;
    })();

    return network;
})();

module.exports = $root;
