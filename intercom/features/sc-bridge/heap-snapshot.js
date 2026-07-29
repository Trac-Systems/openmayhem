const moduleDefault = (module) => module.default ?? module;

const safeHeapSnapshotLabel = (value) =>
  String(value ?? '')
    .trim()
    .replace(/[^a-zA-Z0-9_-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 80);

const resolveRuntimeDeps = async (deps = {}) => ({
  fs: deps.fs ?? moduleDefault(await import('bare-fs')),
  path: deps.path ?? moduleDefault(await import('bare-path')),
  inspector: deps.inspector ?? moduleDefault(await import('bare-inspector')),
});

const runtimePid = () => globalThis.Bare?.pid ?? globalThis.process?.pid ?? 'unknown';

const writeBareHeapSnapshot = async (directory, label = '', deps = {}) => {
  if (typeof directory !== 'string' || !directory.trim()) {
    throw new Error('Heap snapshots are disabled.');
  }
  const { fs, path, inspector } = await resolveRuntimeDeps(deps);
  const { Session, HeapSnapshot } = inspector;
  if (typeof Session !== 'function' || typeof HeapSnapshot !== 'function') {
    throw new Error('Bare heap snapshot support is unavailable.');
  }

  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  const now = new Date().toISOString().replace(/[:.]/g, '');
  const suffix = safeHeapSnapshotLabel(label);
  const filename =
    `mayhem-peer-${runtimePid()}-${now}${suffix ? `-${suffix}` : ''}.heapsnapshot`;
  const filePath = path.join(directory, filename);
  const session = new Session();
  session.connect();
  try {
    const snapshot = new HeapSnapshot(session);
    await new Promise((resolve, reject) => {
      const out = fs.createWriteStream(filePath, { mode: 0o600 });
      const fail = (error) => reject(error);
      out.on('error', fail);
      snapshot.on('error', fail);
      out.on('finish', resolve);
      snapshot.pipe(out);
    });
  } finally {
    session.destroy();
  }
  try {
    fs.chmodSync(filePath, 0o600);
  } catch (_e) {}
  const stat = fs.statSync(filePath);
  return {
    path: filePath,
    bytes: Number(stat.size) || 0,
  };
};

export {
  safeHeapSnapshotLabel,
  writeBareHeapSnapshot,
};
