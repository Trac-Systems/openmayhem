export function createState(overrides = {}) {
    const {
        txEntries = new Map(),
        signedEntries = new Map(),
        unsignedEntries = new Map(),
        registeredWriterKeys = new Map(),
        registeredBootstrapEntries = new Map(),
        registeredBootstrapEntriesUnsigned = new Map(),
        txValidity = null,
        adminEntry = null,
        verifyProofOfPublication = async () => ({ ok: true }),
        ...stateOverrides
    } = overrides;

    return {
        getIndexerSequenceState: async () => txValidity,
        get: async key => txEntries.get(key) ?? null,
        getNodeEntry: async address => signedEntries.get(address) ?? null,
        getNodeEntryUnsigned: async address => unsignedEntries.get(address) ?? null,
        getRegisteredWriterKey: async key => registeredWriterKeys.get(key) ?? null,
        getAdminEntry: async () => adminEntry,
        getRegisteredBootstrapEntry: async key => registeredBootstrapEntries.get(key) ?? null,
        getRegisteredBootstrapEntryUnsigned: async key => registeredBootstrapEntriesUnsigned.get(key) ?? null,
        verifyProofOfPublication,
        ...stateOverrides
    };
}
