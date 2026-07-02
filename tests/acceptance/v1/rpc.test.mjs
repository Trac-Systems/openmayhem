import { createServer } from "../../../rpc/create_server.js"
import { initTemporaryDirectory } from '../../helpers/setupApplyTests.js'
import { testKeyPair1, testKeyPair2, testKeyPair3 } from '../../fixtures/apply.fixtures.js'
import { randomBytes, setupMsbAdmin, setupMsbWriter, removeTemporaryDirectory, setupMsbPeer, tryToSyncWriters, waitForNodeState } from "../../helpers/setupApplyTests.js"
import { registerAccountTests } from "./account/account.test.mjs"
import { registerBalanceTests } from "./balance/balance.test.mjs"
import { registerBroadcastTransactionTests } from "./broadcast-transaction/broadcast-transaction.test.mjs"
import { registerConfirmedLengthTests } from "./confirmed-length/confirmed-length.test.mjs"
import { registerFeeTests } from "./fee/fee.test.mjs"
import { registerTxHashesTests } from "./tx-hashes/tx-hashes.test.mjs"
import { registerTxPayloadsBulkTests } from "./tx-payloads-bulk/tx-payloads-bulk.test.mjs"
import { registerTxDetailsTests } from "./tx-details/tx-details.test.mjs"
import { registerTxTests } from "./tx/tx.test.mjs"
import { registerTxvTests } from "./txv/txv.test.mjs"
import { registerUnconfirmedLengthTests } from "./unconfirmed-length/unconfirmed-length.test.mjs"

let toClose
let tmpDirectory
const additionalPeers = []

const testContext = {
    writerMsb: null,
    rpcMsb: null,
    server: null,
    wallet: null,
    adminWallet: null,
}

const setupNetwork = async () => {
    tmpDirectory = await initTemporaryDirectory()
    const rpcOpts = {
        bootstrap: randomBytes(32).toString('hex'),
        channel: randomBytes(32).toString('hex'),
        enableRoleRequester: false,
        enableWallet: true,
        enableValidatorObserver: true,
        enableInteractiveMode: false,
        disableRateLimit: true,
        enableTxApplyLogs: false,
        storesDirectory: `${tmpDirectory}/stores/`,
        storeName: '/admin'
    }

    const admin = await setupMsbAdmin(testKeyPair1, tmpDirectory, rpcOpts)
    const writer = await setupMsbWriter(admin, 'writer', testKeyPair2, tmpDirectory, admin.options);
    additionalPeers.push(writer)
    const reader = await setupMsbPeer('peer-2', testKeyPair3, tmpDirectory, { ...admin.options, enable_wallet: false });
    additionalPeers.push(reader)
    await tryToSyncWriters(admin, writer, reader)
    await waitForNodeState(reader, writer.wallet.address, {
        wk: writer.msb.state.writingKey,
        isWhitelisted: true,
        isWriter: true,
        isIndexer: false,
    })
    return { writer, admin, reader }
}

beforeAll(async () => {
    const { admin, writer, reader } = await setupNetwork()
    const server = createServer(reader.msb, reader.config)
    toClose = admin.msb
    Object.assign(testContext, {
        writerMsb: writer.msb,
        rpcMsb: reader.msb,
        server,
        wallet: writer.msb.wallet,
        adminWallet: admin.wallet,
    })
})

afterAll(async () => {
    const peersToClose = [...new Set(additionalPeers.map(peer => peer?.msb).filter(Boolean))]
    await Promise.all([
        toClose?.close(),
        ...peersToClose.map(instance => instance.close())
    ])
    await removeTemporaryDirectory(tmpDirectory)
})

// The order here is important since the OPs change the network state. We wont boot up an instance before each because the tests are to verify rpc structure and the decision is to spare ci resources.
describe("API acceptance tests", () => {
    registerConfirmedLengthTests(testContext)
    registerUnconfirmedLengthTests(testContext)
    registerTxvTests(testContext)
    registerFeeTests(testContext)
    registerTxHashesTests(testContext)
    registerTxTests(testContext)
    registerBalanceTests(testContext)
    registerBroadcastTransactionTests(testContext)
    registerTxPayloadsBulkTests(testContext)
    registerTxDetailsTests(testContext)
    registerAccountTests(testContext)
})
