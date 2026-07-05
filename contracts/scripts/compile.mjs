// Deterministic solc compile of our local contracts -> returns {abi, bytecode} maps.
// No network, no external fetch (solc is the pinned npm package).
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import solc from 'solc';

const __dirname = dirname(fileURLToPath(import.meta.url));
const SRC_DIR = join(__dirname, '..', 'src');
const NODE_MODULES = join(__dirname, '..', 'node_modules');

// solc import callback - resolves external imports (e.g. `@openzeppelin/contracts/...`) from our
// LOCAL node_modules. solc normalizes OZ's internal relative imports back to package paths before
// calling this, so a single node_modules lookup covers the whole OZ dependency tree.
function findImports(path) {
  try {
    return { contents: readFileSync(join(NODE_MODULES, path), 'utf8') };
  } catch {
    return { error: 'File not found: ' + path };
  }
}

export function compileAll() {
  const sources = {};
  for (const f of readdirSync(SRC_DIR)) {
    if (f.endsWith('.sol')) {
      sources[f] = { content: readFileSync(join(SRC_DIR, f), 'utf8') };
    }
  }
  const input = {
    language: 'Solidity',
    sources,
    settings: {
      // ganache 7.9.2 predates PUSH0 (Shanghai). Pin to 'paris' so solc 0.8.30
      // doesn't emit PUSH0 -> avoids "invalid opcode" on the local node.
      evmVersion: 'paris',
      optimizer: { enabled: true, runs: 200 },
      outputSelection: { '*': { '*': ['abi', 'evm.bytecode.object'] } },
    },
  };
  const out = JSON.parse(solc.compile(JSON.stringify(input), { import: findImports }));
  const fatal = (out.errors || []).filter((e) => e.severity === 'error');
  if (fatal.length) {
    throw new Error('solc errors:\n' + fatal.map((e) => e.formattedMessage).join('\n'));
  }
  const artifacts = {};
  for (const file of Object.keys(out.contracts || {})) {
    for (const [cname, c] of Object.entries(out.contracts[file])) {
      artifacts[cname] = { abi: c.abi, bytecode: '0x' + c.evm.bytecode.object };
    }
  }
  return artifacts;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const names = Object.keys(compileAll()).sort();
  console.log(JSON.stringify({ contracts: names }, null, 2));
}
