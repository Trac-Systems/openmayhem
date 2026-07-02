// The code below originates from the holepunch/autobase repository (https://github.com/holepunchto/autobase)
// License: Apache 2.0
// The holepunch team has granted permission to reuse this code in the Trac Systems project.
// Huge thanks to the holepunch team for their work and support!
import path from 'path';
import Corestore from 'corestore';
import Autobase from 'autobase';
import Hyperbee from 'hyperbee';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import Hypercore from 'hypercore';
import {
	ACK_INTERVAL,
	AUTOBASE_VALUE_ENCODING,
	HYPERBEE_KEY_ENCODING,
	HYPERBEE_VALUE_ENCODING,
	TRAC_NAMESPACE
} from '../../src/utils/constants.js';
import Writer from 'autobase/lib/writer.js';

const argv = typeof globalThis.Bare !== 'undefined' ? globalThis.Bare.argv : process.argv;


const originalWriterOpen = Writer.prototype._open;
Writer.prototype._open = async function patchedWriterOpen(...args) {
	try {
		return await originalWriterOpen.apply(this, args);
	} catch (err) {
		// On teardown GH runners sometimes open a writer against a closing core.
		// Ignore assertion errors from Autobase writer open during that window.
		if (err?.name === 'AssertionError') return;
		throw err;
	}
};

// Hypercore/Hyperbee can throw SESSION_CLOSED while the core is shutting down.
// Swallow those during tests to avoid teardown flakes on slower runners.
const originalSnapshot = Hypercore.prototype.snapshot;
Hypercore.prototype.snapshot = function patchedSnapshot(...args) {
	try {
		return originalSnapshot.apply(this, args);
	} catch (err) {
		if (err?.code === 'SESSION_CLOSED') return this;
		throw err;
	}
};

const originalCoreGet = Hypercore.prototype.get;
Hypercore.prototype.get = async function patchedCoreGet(...args) {
	try {
		return await originalCoreGet.apply(this, args);
	} catch (err) {
		if (err?.code === 'SESSION_CLOSED') return null;
		throw err;
	}
};

const originalMakeSnapshot = Hyperbee.prototype._makeSnapshot;
Hyperbee.prototype._makeSnapshot = function patchedMakeSnapshot(...args) {
	try {
		return originalMakeSnapshot.apply(this, args);
	} catch (err) {
		if (err?.code === 'SESSION_CLOSED') return this.core;
		throw err;
	}
};

export const encryptionKey = argv?.includes('--encrypt-all')
	? b4a.alloc(32).fill('autobase-encryption-test')
	: undefined;

export async function createStores(n, t, opts = {}) {
	const storage = opts.storage || (() => tmpDir(t));
	const offset = opts.offset || 0;

	const stores = [];
	for (let i = offset; i < n + offset; i++) {
		const primaryKey = Buffer.alloc(32, i);
		const globalCache = opts.globalCache || null;
		const dir = await storage();
		stores.push(new Corestore(dir, { primaryKey, encryptionKey, globalCache }));
	}

	t.teardown(async () => {
		for (const store of stores) {
			try {
				await store.close();
			} catch (err) {
				console.error('corestore close failed', err);
			}
		}
	}, { order: 2 });
	return stores;
}

export async function create(n, t, opts = {}) {
	const stores = await createStores(n, t, opts);
	const bases = [createBase(stores[0], null, t, opts)];
	await bases[0].ready();
	bases[0].name = 'a';

	if (n === 1) return { stores, bases };

	for (let i = 1; i < n; i++) {
		const base = createBase(stores[i], bases[0].local.key, t, opts);
		await base.ready();
		base.name = String.fromCharCode('a'.charCodeAt(0) + i);
		bases.push(base);
	}

	return { stores, bases };
}

export function createBase(store, key, t, opts = {}) {
	const {
		open = defaultOpenHyperbeeView,
		valueEncoding = AUTOBASE_VALUE_ENCODING,
		ackInterval = ACK_INTERVAL,
		bigBatches = false,
		optimistic = false,
		...rest
	} = opts;

	const base = new Autobase(store.session(), key, {
		open,
		close: undefined,
		valueEncoding,
		ackInterval,
		bigBatches,
		optimistic,
		ackThreshold: 0,
		encryptionKey,
		fastForward: false,
		...rest
	});
	
	base.on('error', err => {
		if (err?.code === 'SESSION_CLOSED') return;
		console.error('autobase error', err);
	});

	if (opts.maxSupportedVersion !== undefined) {
		base.maxSupportedVersion = opts.maxSupportedVersion;
	}

	t.teardown(async () => {
		try {
			await base.advance();
		} catch (err) {
			if (err?.code !== 'SESSION_CLOSED') {
				console.error('base advance failed', err);
			}
		}
		try {
			await base.close();
		} catch (err) {
			console.error('base close failed', err);
		}
	}, { order: 1 });
	return base;
}

export function defaultOpenHyperbeeView(store) {
	return new Hyperbee(store.get(TRAC_NAMESPACE), {
		extension: false,
		keyEncoding: HYPERBEE_KEY_ENCODING,
		valueEncoding: HYPERBEE_VALUE_ENCODING
	});
}

export async function createWallet(mnemonic = null) {
	const wallet = new PeerWallet();
	await wallet.generateKeyPair(mnemonic ?? undefined);
	return wallet;
}

export function seedIndexerList(base, keys) {
	base.system.indexers = keys.map(key => ({ key, length: 0 }));
	if (base.system._indexerMap instanceof Map) {
		for (const key of keys) {
			base.system._indexerMap.set(b4a.toString(key, 'hex'), { key, length: 0 });
		}
	}
}

export const seedBootstrapIndexer = (network) => {
	const bootstrapPeer = network.adminBootstrap ?? network.bootstrap;
	const key = bootstrapPeer.base.local.key;
	for (const { base } of network.nodes) {
		seedIndexerList(base, [key]);
	}
};

export async function replicateAndSync(bases, opts) {
	const done = replicate(bases);
	await sync(bases, opts);
	await done();
}

export async function sync(bases, { checkHash = true } = {}) {
	for (const base of bases) {
		await base.update();
	}

	if (bases.length === 1) return;

	return new Promise((resolve, reject) => {
		let done = false;
		let checks = 0;

		for (const base of bases) {
			base.on('update', check);
			base.on('error', shutdown);
		}

		check();

		async function check() {
			checks++;
			for (const base of bases) {
				if (base.interrupted) return shutdown(new Error('base was interrupted, reason at base.interrupted'));
			}
			if (!(await areSame(checkHash))) return maybeShutdown();
			for (const base of bases) {
				await base.update();
			}
			if (!(await areSame(checkHash))) return maybeShutdown();
			shutdown();
		}

		function maybeShutdown() {
			if (done) return shutdown();
			checks--;
		}

		async function areSame(checkHash) {
			return sameHeadsAcross(bases) && await sameHash(bases, checkHash);
		}

		function shutdown(err) {
			checks--;
			done = true;
			if (checks !== 0 && !err) return;

			for (const base of bases) {
				base.off('update', check);
				base.off('error', shutdown);
			}

			if (err) reject(err);
			else resolve();
		}
	});
}

function replicate(bases) {
	const streams = [];
	const missing = bases.slice();

	while (missing.length) {
		const a = missing.pop();
		for (const b of missing) {
			const s1 = a.replicate(true);
			const s2 = b.replicate(false);

			s1.on('error', err => {
				if (err.code) console.log('autobase replicate error:', err.stack);
			});
			s2.on('error', err => {
				if (err.code) console.log('autobase replicate error:', err.stack);
			});

			s1.pipe(s2).pipe(s1);
			streams.push(s1, s2);
		}
	}

	return close;

	function close() {
		return Promise.all(streams.map(s => {
			s.destroy();
			return new Promise(resolve => s.on('close', resolve));
		}));
	}
}

function sameHeadsAcross(bases) {
	const first = bases[0];
	for (let i = 1; i < bases.length; i++) {
		if (!sameHeads(first, bases[i])) return false;
	}
	return true;
}

async function sameHash(bases, check) {
	if (!check) return true;
	const first = bases[0];
	const h1 = await first.hash();
	for (let i = 1; i < bases.length; i++) {
		if (bases[i].signedLength !== first.signedLength) return false;
		const h2 = await bases[i].hash();
		if (!b4a.equals(h1, h2)) return false;
	}
	return true;
}

function sameHeads(a, b) {
	if (a.updating || b.updating) return false;

	const h1 = a.heads();
	const h2 = b.heads();

	if (h1.length !== h2.length) return false;

	for (let i = 0; i < h1.length; i++) {
		const h1i = h1[i];
		const h2i = h2[i];

		if (!b4a.equals(h1i.key, h2i.key)) return false;
		if (h1i.length !== h2i.length) return false;
	}

	return true;
}

export function eventFlush() {
	return new Promise(resolve => setImmediate(resolve));
}

export function deriveIndexerSequenceState(base) {
	const indexers = base.system?.indexers || [];
	const buffers = indexers
		.map(entry => entry?.key)
		.filter(key => key && key.length > 0);
	const concatenated = buffers.length > 0 ? b4a.concat(buffers) : b4a.alloc(0);
	return PeerWallet.blake3(concatenated);
}

let osModule;
let fsPromises;

async function ensureEnvReady() {
	if (osModule && fsPromises) return;

	if (typeof globalThis.Bare !== 'undefined') {
		const bareOs = await import('bare-os');
		osModule = bareOs.default || bareOs;

		const bareFs = await import('bare-fs');
		const resolved = bareFs.default || bareFs;
		fsPromises = resolved.promises;
	} else {
		const nodeOs = await import('os');
		osModule = nodeOs.default || nodeOs;

		const nodeFs = await import('fs/promises');
		fsPromises = nodeFs.default || nodeFs;
	}
}

async function tmpDir(t) {
	await ensureEnvReady();
	const prefix = path.join(osModule.tmpdir(), 'msb-autobase-');
	const dir = await createTempDir(prefix);
	t.teardown(() => cleanupTempDir(dir), { order: 3 });
	return dir;
}

async function createTempDir(prefix) {
	if (typeof fsPromises.mkdtemp === 'function') {
		return fsPromises.mkdtemp(prefix);
	}
	const uniqueDir = `${prefix}${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
	await fsPromises.mkdir(uniqueDir, { recursive: true });
	return uniqueDir;
}

async function cleanupTempDir(dir) {
	if (!fsPromises) return;

	try {
		if (typeof fsPromises.rm === 'function') {
			await fsPromises.rm(dir, { recursive: true, force: true });
		} else if (typeof fsPromises.rmdir === 'function') {
			await removeRecursively(dir);
		}
	} catch (err) {
		if (err?.code === 'ENOENT') return;
		// Windows sometimes keeps LOCK files open briefly. Ignore busy/permission errors during teardown.
		if (err?.code === 'EBUSY' || err?.code === 'EPERM') return;
		throw err;
	}
}

async function removeRecursively(target) {
	let stats;
	try {
		stats = await fsPromises.lstat(target);
	} catch (err) {
		if (err?.code === 'ENOENT') return;
		throw err;
	}

	if (!stats.isDirectory()) {
		await fsPromises.unlink(target);
		return;
	}

	const entries = await fsPromises.readdir(target);
	for (const entry of entries) {
		await removeRecursively(path.join(target, entry));
	}
	await fsPromises.rmdir(target);
}
