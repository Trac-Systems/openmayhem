export const toArgMap = (argv) => {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const raw = argv[i];
    if (!raw.startsWith("--")) continue;
    const eq = raw.indexOf("=");
    if (eq !== -1) {
      const k = raw.slice(2, eq);
      const v = raw.slice(eq + 1);
      out[k] = v;
      continue;
    }
    const k = raw.slice(2);
    const next = argv[i + 1];
    if (next !== undefined && !String(next).startsWith("--")) {
      out[k] = next;
      i++;
    } else {
      out[k] = true;
    }
  }
  return out;
};

export const ensureTrailingSlash = (value) => (value.endsWith("/") ? value : `${value}/`);

export const getPearRuntime = () => {
  const pearApp = typeof Pear !== "undefined" ? (Pear.app ?? Pear.config) : undefined;
  const runtimeArgs = typeof process !== "undefined" ? process.argv.slice(2) : [];
  const argv = pearApp?.args ?? runtimeArgs;
  const env = typeof process !== "undefined" && process.env ? process.env : {};

  const storeLabel = argv[0] && !String(argv[0]).startsWith("--") ? String(argv[0]) : null;
  const flags = toArgMap(argv.slice(storeLabel ? 1 : 0));

  return { argv, env, storeLabel, flags };
};

