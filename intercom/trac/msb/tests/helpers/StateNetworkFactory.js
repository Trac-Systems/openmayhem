/**
 * Lightweight factory for spinning up Autobase+State test networks.
 *
 * Each node gets:
 *  - a Corestore/Autobase pair created via test/testHelpers/autobaseTestHelpers
 *  - a PeerWallet (bootstrap uses the deterministic test mnemonic)
 *  - a State instance wired to the same bootstrap key so we can plug its apply handler directly
 *
 * Tests can grab `network.adminBootstrap` (the first node) plus the rest from `network.peers`,
 * call `await network.sync()` to replicate, and `await network.teardown()` in their cleanup hook.
 */

import State from '../../src/core/state/State.js';
import {
	create,
	createStores,
	replicateAndSync,
	defaultOpenHyperbeeView,
	createWallet,
	seedIndexerList
} from './autobaseTestHelpers.js';
import {
	AUTOBASE_VALUE_ENCODING,
	ACK_INTERVAL
} from '../../src/utils/constants.js';
import { testKeyPair1 } from '../fixtures/apply.fixtures.js';
import { createConfig, ENV } from '../../src/config/env.js';

export class StateNetworkFactory {
	
	static async create(options = {}) {
		const factory = new StateNetworkFactory(options);
		await factory.#initialize();
		return factory;
	}

	#adminPeer = null;
	#peers = [];
	#bases = [];
	#harness = createHarness();
	#options;

	constructor({
		nodes = 2,
		valueEncoding = AUTOBASE_VALUE_ENCODING,
		stateOptions = {},
		autobaseOptions = {},
		open = defaultOpenHyperbeeView,
		seedIndexers = true
	} = {}) {
		if (nodes < 1) throw new Error('StateNetworkFactory requires at least one node');
		this.#options = {
			nodes,
			valueEncoding,
			stateOptions,
			autobaseOptions,
			open,
			seedIndexers
		};
	}

	get adminBootstrap() {
		return this.#adminPeer;
	}

	get peers() {
		return this.#peers;
	}

	// Backwards compatibility
	get bootstrap() {
		return this.#adminPeer;
	}

	get nodes() {
		return this.#peers;
	}

	async sync() {
		await replicateAndSync(this.#bases);
	}

	async teardown() {
		await this.#harness.cleanup();
	}

	async #initialize() {
		const { nodes, valueEncoding, stateOptions, autobaseOptions, open, seedIndexers } = this.#options;

		const stateStores = await createStores(nodes, this.#harness);
		const { bases } = await create(nodes, this.#harness, {
			apply: async () => {},
			open,
			valueEncoding,
			ackInterval: ACK_INTERVAL,
			bigBatches: false,
			optimistic: false,
			...autobaseOptions
		});
		this.#bases = bases;

		await Promise.all(bases.map(base => base.ready()));

		const bootstrapKey = bases[0].local.key;
		const descriptors = [];

		for (let i = 0; i < nodes; i++) {
			const mnemonic = i === 0 ? testKeyPair1.mnemonic : null;
			const wallet = await createWallet(mnemonic);
			const stateConfig = createConfig(ENV.DEVELOPMENT, { ...stateOptions, bootstrap: bootstrapKey })
			const state = new State(stateStores[i].session(), wallet, stateConfig);

			bases[i]._handlers.apply = state.applyHandler;
			descriptors.push({
				index: i,
				name: bases[i].name,
				base: bases[i],
				state,
				wallet
			});
		}

		if (seedIndexers) {
			for (const descriptor of descriptors) {
				seedIndexerList(descriptor.base, [bootstrapKey]);
			}
		}

		this.#adminPeer = descriptors[0];
		this.#peers = descriptors;
	}
}

export async function setupStateNetwork(options = {}) {
	return StateNetworkFactory.create(options);
}

function createHarness() {
	const teardowns = [];
	return {
		teardown(fn, { order = 0 } = {}) {
			teardowns.push({ fn, order });
		},
		async cleanup() {
			const sorted = teardowns.sort((a, b) => b.order - a.order);
			for (const { fn } of sorted) {
				try {
					await fn();
				} catch (err) {
					console.error('teardown failed', err);
				}
			}
		}
	};
}
