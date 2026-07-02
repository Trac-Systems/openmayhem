import { Buffer } from "buffer";

const isConstructable = (Ctor) => {
  if (typeof Ctor !== "function") return false;
  try {
    // eslint-disable-next-line no-new
    new Ctor();
    return true;
  } catch (_e) {
    return false;
  }
};

const ensureGlobals = () => {
  // Pear/Bare runtime can provide non-constructable TextEncoder/TextDecoder.
  if (!isConstructable(globalThis.TextEncoder)) {
    globalThis.TextEncoder = class TextEncoder {
      encode(input = "") {
        return Buffer.from(String(input), "utf8");
      }
    };
  }

  if (!isConstructable(globalThis.TextDecoder)) {
    globalThis.TextDecoder = class TextDecoder {
      decode(input) {
        return Buffer.from(input ?? []).toString("utf8");
      }
    };
  }
};

export const ensureTextCodecs = async () => {
  // 1) Ensure globals exist and are constructable.
  ensureGlobals();

  // 2) trac-crypto-api overwrites globals from `util.TextEncoder/TextDecoder` in bare runtime.
  // Patch util's exports too so that overwrite remains constructable.
  try {
    const utilMod = await import("util");
    const utilObj = utilMod?.default ?? utilMod;
    if (utilObj && typeof utilObj === "object") {
      utilObj.TextEncoder = globalThis.TextEncoder;
      utilObj.TextDecoder = globalThis.TextDecoder;
    }
  } catch (_e) {
    // Ignore if util is unavailable in this runtime.
  }
};
