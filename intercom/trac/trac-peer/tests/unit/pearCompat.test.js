import test from "brittle";

import { parsePearMajor, selectPearRunnerMode } from "../../src/pearCompat.js";

test("pear compat: Pear v2 JSON selects the legacy runner", (t) => {
  const output = JSON.stringify({
    key: "example",
    fork: 0,
    length: 123,
    semver: "2.6.5",
  });

  t.is(parsePearMajor(output), 2);
  t.is(selectPearRunnerMode(output), "legacy");
});

test("pear compat: Pear v3 JSON selects the module runner", (t) => {
  const output = JSON.stringify({
    key: "example",
    fork: 0,
    length: 456,
    semver: "3.0.0",
  });

  t.is(parsePearMajor(output), 3);
  t.is(selectPearRunnerMode(output), "module");
});

test("pear compat: legacy SemVer output selects the legacy runner", (t) => {
  const output = [
    "v0.9609.example / v1.18.0",
    "Key=example",
    "Fork=0",
    "Length=9609",
    "SemVer=1.18.0",
  ].join("\n");

  t.is(parsePearMajor(output), 1);
  t.is(selectPearRunnerMode(output), "legacy");
});

test("pear compat: unknown output safely selects the module runner", (t) => {
  t.is(parsePearMajor("Pear development checkout"), null);
  t.is(selectPearRunnerMode("Pear development checkout"), "module");
  t.is(selectPearRunnerMode(""), "module");
});
