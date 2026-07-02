// This runner is intentionally small (mirrors MSB's brittle runner style).
import test from "brittle";

if (typeof globalThis.fetch !== "function") {
  const mod = await import("fetch");
  globalThis.fetch = mod?.default ?? mod?.fetch ?? mod;
}
test.pause();
await import("./rpc.test.js");
test.resume();
