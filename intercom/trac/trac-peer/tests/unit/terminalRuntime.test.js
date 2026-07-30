import test from "brittle";

import { createTerminalReadline } from "../../src/terminal/index.js";

test("terminal runtime: readline uses process streams", (t) => {
  const runtimeProcess = { stdin: {}, stdout: {} };
  const expected = {};
  let options = null;

  const readlineModule = {
    createInterface(value) {
      options = value;
      return expected;
    },
  };

  const actual = createTerminalReadline({
    readlineModule,
    runtimeProcess,
  });

  t.is(actual, expected);
  t.is(options.input, runtimeProcess.stdin);
  t.is(options.output, runtimeProcess.stdout);
});
