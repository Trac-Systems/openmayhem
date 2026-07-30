import b4a from 'b4a';
import { address } from 'trac-crypto-api';
import { config } from './config.js';

export const asAddress = pubKeyHex =>
    address.encode(config.addressPrefix, b4a.from(pubKeyHex, 'hex'));
