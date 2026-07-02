import { test } from 'brittle';

export default class OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		mutatePayload,
		applyInvalidPayload,
		assertStateUnchanged,
		expectedLogs = []
	}) {
		Object.entries({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload,
			applyInvalidPayload,
			assertStateUnchanged
		}).forEach(([key, value]) => {
			if (!value) {
				throw new Error(`Operation validation scenario missing required ${key}`);
			}
		});

		if (typeof title !== 'string') {
			throw new Error('Operation validation scenario requires a title string.');
		}

		this.title = title;
		this.setupScenario = setupScenario;
		this.buildValidPayload = buildValidPayload;
		this.mutatePayload = mutatePayload;
		this.applyInvalidPayload = applyInvalidPayload;
		this.assertStateUnchanged = assertStateUnchanged;
		this.expectedLogs = this.#normalizeExpectedLogs(expectedLogs);
	}

	async execute(t) {
		const shouldCaptureLogs = this.expectedLogs.length > 0;
		const capturedLogs = [];
		const originalConsoleError = console.error;
		if (shouldCaptureLogs) {
			console.error = (...args) => {
				capturedLogs.push(args);
				originalConsoleError(...args);
			};
		}

		const context = await this.setupScenario(t);
		const validPayload = await this.buildValidPayload(context, t);
		t.ok(validPayload?.length > 0, 'valid payload encodes');

		const invalidPayload = await this.mutatePayload(t, validPayload, context);
		t.ok(invalidPayload.length > 0, 'invalid payload still encodes');

		try {
			await this.applyInvalidPayload(context, invalidPayload, t, validPayload);
			await this.assertStateUnchanged(t, context, validPayload, invalidPayload);
		} finally {
			if (shouldCaptureLogs) {
				console.error = originalConsoleError;
				this.#assertExpectedLogs(t, capturedLogs);
			}
		}
	}

	performScenario() {
		test(this.title, t => this.execute(t));
	}

	#assertExpectedLogs(t, capturedLogs) {
		for (const matcher of this.expectedLogs) {
			const found = matcher.values.some(expected =>
				capturedLogs.some(args =>
					args.some(arg => this.#toComparableString(arg).includes(expected))
				)
			);
			t.ok(found, `expected apply log ${matcher.label} was emitted`);
		}
	}

	#normalizeExpectedLogs(expectedLogs) {
		if (!Array.isArray(expectedLogs) || expectedLogs.length === 0) return [];
		return expectedLogs.map(entry => {
			if (typeof entry === 'string') {
				return { values: [entry], label: `"${entry}"` };
			}
			if (Array.isArray(entry) && entry.length > 0) {
				return {
					values: entry.map(value => (typeof value === 'string' ? value : String(value))),
					label: entry.map(value => `"${value}"`).join(' or ')
				};
			}
			if (entry && Array.isArray(entry.anyOf) && entry.anyOf.length > 0) {
				return {
					values: entry.anyOf.map(value => (typeof value === 'string' ? value : String(value))),
					label: entry.anyOf.map(value => `"${value}"`).join(' or ')
				};
			}
			throw new Error('expectedLogs entries must be strings, arrays of strings, or { anyOf: string[] }');
		});
	}

	#toComparableString(arg) {
		if (typeof arg === 'string') return arg;
		if (arg instanceof Error) return arg.message || arg.toString();
		try {
			return JSON.stringify(arg);
		} catch {
			return String(arg);
		}
	}
}
