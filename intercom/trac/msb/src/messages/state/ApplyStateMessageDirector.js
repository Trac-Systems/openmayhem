import { OperationType } from '../../utils/constants.js';

/**
 * Director that orchestrates ApplyStateMessageBuilder for partial and complete messages.
 */
class ApplyStateMessageDirector {
    #builder;

    /**
     * @param {ApplyStateMessageBuilder} builderInstance
     */
    constructor(builderInstance) {
        this.#builder = builderInstance;
    }

    /**
     * Build a partial add writer payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} writingKey
     * @param {string|Buffer} txValidity
     * @param {'json'|'buffer'} output
     * @returns {Promise<object>}
     */
    async buildPartialAddWriterMessage(invokerAddress, writingKey, txValidity, output) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('partial')
            .setOutput(output)
            .setOperationType(OperationType.ADD_WRITER)
            .setAddress(invokerAddress)
            .setTxValidity(txValidity)
            .setWriterKey(writingKey)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a partial remove writer payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} writerKey
     * @param {string|Buffer} txValidity
     * @param {'json'|'buffer'} output
     * @returns {Promise<object>}
     */
    async buildPartialRemoveWriterMessage(invokerAddress, writerKey, txValidity, output) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('partial')
            .setOutput(output)
            .setOperationType(OperationType.REMOVE_WRITER)
            .setAddress(invokerAddress)
            .setTxValidity(txValidity)
            .setWriterKey(writerKey)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a partial admin recovery payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} writingKey
     * @param {string|Buffer} txValidity
     * @param {'json'|'buffer'} output
     * @returns {Promise<object>}
     */
    async buildPartialAdminRecoveryMessage(invokerAddress, writingKey, txValidity, output) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('partial')
            .setOutput(output)
            .setOperationType(OperationType.ADMIN_RECOVERY)
            .setAddress(invokerAddress)
            .setTxValidity(txValidity)
            .setWriterKey(writingKey)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a partial transaction payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} incomingWritingKey
     * @param {string|Buffer} txValidity
     * @param {string|Buffer} contentHash
     * @param {string|Buffer} externalBootstrap
     * @param {string|Buffer} msbBootstrap
     * @param {'json'|'buffer'} output
     * @returns {Promise<object>}
     */
    async buildPartialTransactionOperationMessage(
        invokerAddress,
        incomingWritingKey,
        txValidity,
        contentHash,
        externalBootstrap,
        msbBootstrap,
        output
    ) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('partial')
            .setOutput(output)
            .setOperationType(OperationType.TX)
            .setAddress(invokerAddress)
            .setTxValidity(txValidity)
            .setWriterKey(incomingWritingKey)
            .setContentHash(contentHash)
            .setExternalBootstrap(externalBootstrap)
            .setMsbBootstrap(msbBootstrap)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a partial bootstrap deployment payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} bootstrap
     * @param {string|Buffer} channel
     * @param {string|Buffer} txValidity
     * @param {'json'|'buffer'} output
     * @returns {Promise<object>}
     */
    async buildPartialBootstrapDeploymentMessage(invokerAddress, bootstrap, channel, txValidity, output) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('partial')
            .setOutput(output)
            .setOperationType(OperationType.BOOTSTRAP_DEPLOYMENT)
            .setAddress(invokerAddress)
            .setTxValidity(txValidity)
            .setExternalBootstrap(bootstrap)
            .setChannel(channel)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a partial transfer payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} recipientAddress
     * @param {string|Buffer} amount
     * @param {string|Buffer} txValidity
     * @param {'json'|'buffer'} output
     * @returns {Promise<object>}
     */
    async buildPartialTransferOperationMessage(invokerAddress, recipientAddress, amount, txValidity, output) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('partial')
            .setOutput(output)
            .setOperationType(OperationType.TRANSFER)
            .setAddress(invokerAddress)
            .setTxValidity(txValidity)
            .setIncomingAddress(recipientAddress)
            .setAmount(amount)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete add admin payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} writingKey
     * @param {string|Buffer} txValidity
     * @returns {Promise<object>}
     */
    async buildCompleteAddAdminMessage(invokerAddress, writingKey, txValidity) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.ADD_ADMIN)
            .setAddress(invokerAddress)
            .setWriterKey(writingKey)
            .setTxValidity(txValidity)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete disable initialization payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} writingKey
     * @param {string|Buffer} txValidity
     * @returns {Promise<object>}
     */
    async buildCompleteDisableInitializationMessage(invokerAddress, writingKey, txValidity) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.DISABLE_INITIALIZATION)
            .setAddress(invokerAddress)
            .setWriterKey(writingKey)
            .setTxValidity(txValidity)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete balance initialization payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} recipientAddress
     * @param {string|Buffer} amount
     * @param {string|Buffer} txValidity
     * @returns {Promise<object>}
     */
    async buildCompleteBalanceInitializationMessage(invokerAddress, recipientAddress, amount, txValidity) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.BALANCE_INITIALIZATION)
            .setAddress(invokerAddress)
            .setIncomingAddress(recipientAddress)
            .setAmount(amount)
            .setTxValidity(txValidity)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete append whitelist payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} incomingAddress
     * @param {string|Buffer} txValidity
     * @returns {Promise<object>}
     */
    async buildCompleteAppendWhitelistMessage(invokerAddress, incomingAddress, txValidity) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.APPEND_WHITELIST)
            .setAddress(invokerAddress)
            .setTxValidity(txValidity)
            .setIncomingAddress(incomingAddress)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete add writer payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} txHash
     * @param {string|Buffer} txValidity
     * @param {string|Buffer} incomingWritingKey
     * @param {string|Buffer} incomingNonce
     * @param {string|Buffer} incomingSignature
     * @returns {Promise<object>}
     */
    async buildCompleteAddWriterMessage(
        invokerAddress,
        txHash,
        txValidity,
        incomingWritingKey,
        incomingNonce,
        incomingSignature
    ) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.ADD_WRITER)
            .setAddress(invokerAddress)
            .setTxHash(txHash)
            .setTxValidity(txValidity)
            .setIncomingWriterKey(incomingWritingKey)
            .setIncomingNonce(incomingNonce)
            .setIncomingSignature(incomingSignature)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete remove writer payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} txHash
     * @param {string|Buffer} txValidity
     * @param {string|Buffer} incomingWritingKey
     * @param {string|Buffer} incomingNonce
     * @param {string|Buffer} incomingSignature
     * @returns {Promise<object>}
     */
    async buildCompleteRemoveWriterMessage(
        invokerAddress,
        txHash,
        txValidity,
        incomingWritingKey,
        incomingNonce,
        incomingSignature
    ) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.REMOVE_WRITER)
            .setAddress(invokerAddress)
            .setTxHash(txHash)
            .setTxValidity(txValidity)
            .setIncomingWriterKey(incomingWritingKey)
            .setIncomingNonce(incomingNonce)
            .setIncomingSignature(incomingSignature)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete admin recovery payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} txHash
     * @param {string|Buffer} txValidity
     * @param {string|Buffer} incomingWritingKey
     * @param {string|Buffer} incomingNonce
     * @param {string|Buffer} incomingSignature
     * @returns {Promise<object>}
     */
    async buildCompleteAdminRecoveryMessage(
        invokerAddress,
        txHash,
        txValidity,
        incomingWritingKey,
        incomingNonce,
        incomingSignature
    ) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.ADMIN_RECOVERY)
            .setAddress(invokerAddress)
            .setTxHash(txHash)
            .setTxValidity(txValidity)
            .setIncomingWriterKey(incomingWritingKey)
            .setIncomingNonce(incomingNonce)
            .setIncomingSignature(incomingSignature)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete add indexer payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} incomingAddress
     * @param {string|Buffer} txValidity
     * @returns {Promise<object>}
     */
    async buildCompleteAddIndexerMessage(invokerAddress, incomingAddress, txValidity) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.ADD_INDEXER)
            .setAddress(invokerAddress)
            .setTxValidity(txValidity)
            .setIncomingAddress(incomingAddress)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete remove indexer payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} incomingAddress
     * @param {string|Buffer} txValidity
     * @returns {Promise<object>}
     */
    async buildCompleteRemoveIndexerMessage(invokerAddress, incomingAddress, txValidity) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.REMOVE_INDEXER)
            .setAddress(invokerAddress)
            .setTxValidity(txValidity)
            .setIncomingAddress(incomingAddress)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete ban validator payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} incomingAddress
     * @param {string|Buffer} txValidity
     * @returns {Promise<object>}
     */
    async buildCompleteBanWriterMessage(invokerAddress, incomingAddress, txValidity) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.BAN_VALIDATOR)
            .setAddress(invokerAddress)
            .setTxValidity(txValidity)
            .setIncomingAddress(incomingAddress)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete transaction payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} txHash
     * @param {string|Buffer} txValidity
     * @param {string|Buffer} incomingWriterKey
     * @param {string|Buffer} incomingNonce
     * @param {string|Buffer} contentHash
     * @param {string|Buffer} incomingSignature
     * @param {string|Buffer} externalBootstrap
     * @param {string|Buffer} msbBootstrap
     * @returns {Promise<object>}
     */
    async buildCompleteTransactionOperationMessage(
        invokerAddress,
        txHash,
        txValidity,
        incomingWriterKey,
        incomingNonce,
        contentHash,
        incomingSignature,
        externalBootstrap,
        msbBootstrap
    ) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.TX)
            .setAddress(invokerAddress)
            .setTxHash(txHash)
            .setTxValidity(txValidity)
            .setIncomingWriterKey(incomingWriterKey)
            .setIncomingNonce(incomingNonce)
            .setContentHash(contentHash)
            .setIncomingSignature(incomingSignature)
            .setExternalBootstrap(externalBootstrap)
            .setMsbBootstrap(msbBootstrap)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete bootstrap deployment payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} transactionHash
     * @param {string|Buffer} txValidity
     * @param {string|Buffer} externalBootstrap
     * @param {string|Buffer} channel
     * @param {string|Buffer} incomingNonce
     * @param {string|Buffer} incomingSignature
     * @returns {Promise<object>}
     */
    async buildCompleteBootstrapDeploymentMessage(
        invokerAddress,
        transactionHash,
        txValidity,
        externalBootstrap,
        channel,
        incomingNonce,
        incomingSignature
    ) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.BOOTSTRAP_DEPLOYMENT)
            .setAddress(invokerAddress)
            .setTxHash(transactionHash)
            .setTxValidity(txValidity)
            .setExternalBootstrap(externalBootstrap)
            .setChannel(channel)
            .setIncomingNonce(incomingNonce)
            .setIncomingSignature(incomingSignature)
            .build();
        return this.#builder.getPayload();
    }

    /**
     * Build a complete transfer payload.
     * @param {string|Buffer} invokerAddress
     * @param {string|Buffer} transactionHash
     * @param {string|Buffer} txValidity
     * @param {string|Buffer} incomingNonce
     * @param {string|Buffer} recipientAddress
     * @param {string|Buffer} amount
     * @param {string|Buffer} incomingSignature
     * @returns {Promise<object>}
     */
    async buildCompleteTransferOperationMessage(
        invokerAddress,
        transactionHash,
        txValidity,
        incomingNonce,
        recipientAddress,
        amount,
        incomingSignature
    ) {
        if (!this.#builder) throw new Error('Builder has not been set.');
        await this.#builder
            .setPhase('complete')
            .setOutput('buffer')
            .setOperationType(OperationType.TRANSFER)
            .setAddress(invokerAddress)
            .setTxHash(transactionHash)
            .setTxValidity(txValidity)
            .setIncomingNonce(incomingNonce)
            .setIncomingAddress(recipientAddress)
            .setAmount(amount)
            .setIncomingSignature(incomingSignature)
            .build();
        return this.#builder.getPayload();
    }
}

export default ApplyStateMessageDirector;
