import fs from "fs/promises";
import path from "path";

export async function mkdtempPortable(prefix) {
  if (typeof fs.mkdtemp === "function") return await fs.mkdtemp(prefix);
  const base = String(prefix ?? "");
  if (!base) throw new Error("mkdtempPortable: missing prefix");

  for (let i = 0; i < 50; i++) {
    const suffix = `${Date.now().toString(16)}-${Math.random().toString(16).slice(2)}`;
    const dir = `${base}${suffix}`;
    try {
      await fs.mkdir(dir, { recursive: false });
      return dir;
    } catch (e) {
      if (e && (e.code === "EEXIST" || e.code === "EACCES")) continue;
      throw e;
    }
  }
  throw new Error("mkdtempPortable: failed to create temp dir");
}

export async function rmrfPortable(target) {
  if (!target) return;
  if (typeof fs.rm === "function") {
    try {
      await fs.rm(target, { recursive: true, force: true });
    } catch (_e) {}
    return;
  }

  let stat = null;
  try {
    stat = await fs.stat(target);
  } catch (_e) {
    return;
  }

  if (stat.isDirectory()) {
    let entries = [];
    try {
      entries = await fs.readdir(target);
    } catch (_e) {
      entries = [];
    }
    for (const name of entries) {
      await rmrfPortable(path.join(target, name));
    }
    try {
      await fs.rmdir(target);
    } catch (_e) {}
  } else {
    try {
      await fs.unlink(target);
    } catch (_e) {}
  }
}

