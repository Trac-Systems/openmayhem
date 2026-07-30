import BaseProtocol from "../src/artifacts/protocol.js";
import { bufferToBigInt, bigIntToDecimalString } from "trac-msb/src/utils/amountSerialization.js";

class TuxemonProtocol extends BaseProtocol {
  mapTxCommand(command) {
    if (typeof command !== "string" || command.trim() === "") return null;
    const raw = command.trim();
    if (raw.startsWith("{")) {
      try {
        const parsed = JSON.parse(raw);
        if (parsed && typeof parsed.type === "string" && parsed.value !== undefined) {
          return { type: parsed.type, value: parsed.value };
        }
      } catch (_e) {}
    }
    if (raw === "ping" || raw.startsWith("ping ")) {
      const msg = raw === "ping" ? "pong" : raw.slice(5);
      return { type: "ping", value: { msg } };
    }
    if (raw.startsWith("set ")) {
      const parts = raw.split(" ").filter(Boolean);
      if (parts.length >= 3) {
        const key = parts[1];
        const value = parts.slice(2).join(" ");
        return { type: "set", value: { key, value } };
      }
    }
    return { type: "ping", value: { msg: raw } };
  }

  async customCommand(input) {
    if (typeof input !== "string") return;

    if (input.startsWith("/get")) {
      const m = input.match(/(?:^|\s)--key(?:=|\s+)(\"[^\"]+\"|'[^']+'|\S+)/);
      const raw = m ? m[1].trim() : null;
      if (!raw) {
        console.log('Usage: /get --key "<hyperbee-key>" [--confirmed true|false] [--unconfirmed 1]');
        return;
      }
      const key = raw.replace(/^\"(.*)\"$/, "$1").replace(/^'(.*)'$/, "$1");
      const confirmedMatch = input.match(/(?:^|\s)--confirmed(?:=|\s+)(\S+)/);
      const unconfirmedMatch = input.match(/(?:^|\s)--unconfirmed(?:=|\s+)?(\S+)?/);
      const confirmed = unconfirmedMatch ? false : confirmedMatch ? confirmedMatch[1] === "true" || confirmedMatch[1] === "1" : true;
      const v = confirmed ? await this.getSigned(key) : await this.get(key);
      console.log(v);
      return;
    }

    if (input.startsWith("/msb")) {
      const txv = await this.peer.msbClient.getTxvHex();
      const peerMsbAddress = this.peer.msbClient.pubKeyHexToAddress(this.peer.wallet.publicKey);
      const entry = await this.peer.msbClient.getNodeEntryUnsigned(peerMsbAddress);
      const balance = entry?.balance ? bigIntToDecimalString(bufferToBigInt(entry.balance)) : 0;
      const feeBuf = this.peer.msbClient.getFee();
      const fee = feeBuf ? bigIntToDecimalString(bufferToBigInt(feeBuf)) : 0;
      const validators = this.peer.msbClient.getConnectedValidatorsCount();
      console.log({
        networkId: this.peer.msbClient.networkId,
        msbBootstrap: this.peer.msbClient.bootstrapHex,
        txv,
        msbSignedLength: this.peer.msbClient.getSignedLength(),
        msbUnsignedLength: this.peer.msbClient.getUnsignedLength(),
        connectedValidators: validators,
        peerMsbAddress,
        peerMsbBalance: balance,
        msbFee: fee,
      });
      return;
    }
  }

  async printOptions() {
    console.log("");
    console.log("- Dev commands:");
    console.log('- /msb | prints MSB txv + lengths (local MSB node view).');
    console.log('- /get --key "<key>" [--confirmed true|false] | reads subnet state key (confirmed defaults to true).');
    console.log("");
    console.log("- Dev TX examples:");
    console.log('- /tx --command "ping hello"');
    console.log('- /tx --command "set foo bar"');
    console.log('- /tx --command "{\\"type\\":\\"ping\\",\\"value\\":{\\"msg\\":\\"hi\\"}}"');
  }
}

export default TuxemonProtocol;
