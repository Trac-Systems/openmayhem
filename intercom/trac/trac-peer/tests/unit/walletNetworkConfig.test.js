import test from "brittle";
import b4a from "b4a";
import PeerWallet from "trac-wallet";
import { TRAC_NETWORK_MSB_TESTNET1_PREFIX } from "trac-wallet/constants.js";
import { TRAC_NETWORK_TESTNET_ID } from "trac-crypto-api/constants.js";

import Wallet from "../../src/wallet.js";
import { createConfig, ENV } from "../../src/index.js";

const MNEMONIC = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const TESTNET_DERIVATION_PATH = `m/${TRAC_NETWORK_TESTNET_ID}'/0'/0'/0'`;

test("wallet: constructor options are forwarded to trac-wallet", async (t) => {
  const wallet = new Wallet({
    networkPrefix: TRAC_NETWORK_MSB_TESTNET1_PREFIX,
    derivationPath: TESTNET_DERIVATION_PATH,
    mnemonic: MNEMONIC,
  });
  await wallet.ready;

  const expected = new PeerWallet({
    networkPrefix: TRAC_NETWORK_MSB_TESTNET1_PREFIX,
    mnemonic: MNEMONIC,
    derivationPath: TESTNET_DERIVATION_PATH,
  });
  await expected.ready;

  t.is(wallet.address, expected.address);
  t.is(wallet.publicKey, b4a.toString(expected.publicKey, "hex"));
  t.is(wallet.secretKey, b4a.toString(expected.secretKey, "hex"));
  t.is(wallet.derivationPath, TESTNET_DERIVATION_PATH);
});

test("config: TESTNET1 preset is exported and can be instantiated", async (t) => {
  t.is(ENV.TESTNET1, "testnet1");

  const config = createConfig(ENV.TESTNET1, {});

  t.is(config.storeName, "testnet");
  t.ok(b4a.isBuffer(config.channel));
  t.is(config.bootstrap, null);
});
