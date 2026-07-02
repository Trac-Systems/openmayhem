import request from "supertest"
import { bufferToBigInt, licenseBufferToBigInt } from "../../../../src/utils/amountSerialization.js"
import { ZERO_WK } from "../../../../rpc/constants.js"
import { ADMIN_INITIAL_STAKED_BALANCE } from "../../../../src/utils/constants.js"
import { BALANCE_TO_STAKE } from "../../../../src/core/state/utils/balance.js"
import { randomAddress } from "../../../unit/state/stateTestUtils.js"

export const registerAccountTests = (context) => {
    const formatNodeEntryResponse = (address, entry) => {
        const licenseValue = licenseBufferToBigInt(entry.license)
        return {
            address,
            writingKey: entry.wk.toString('hex'),
            isWhitelisted: entry.isWhitelisted,
            isValidator: entry.isWriter,
            isIndexer: entry.isIndexer,
            license: licenseValue === 0n ? null : licenseValue.toString(),
            balance: bufferToBigInt(entry.balance).toString(),
            stakedBalance: bufferToBigInt(entry.stakedBalance).toString(),
        }
    }

    describe("GET /v1/account", () => {
        it("returns admin account details", async () => {
            const adminEntry = await context.rpcMsb.state.getNodeEntry(context.adminWallet.address)
            expect(adminEntry).toBeTruthy()

            const res = await request(context.server).get(`/v1/account/${context.adminWallet.address}`)
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual(formatNodeEntryResponse(context.adminWallet.address, adminEntry))
            expect(res.body).toMatchObject({
                isWhitelisted: true,
                isValidator: true,
                isIndexer: true,
                stakedBalance: bufferToBigInt(ADMIN_INITIAL_STAKED_BALANCE).toString(),
                license: '1',
            })
        })

        it("returns validator account details", async () => {
            const writerEntry = await context.rpcMsb.state.getNodeEntry(context.wallet.address)
            expect(writerEntry).toBeTruthy()

            const res = await request(context.server).get(`/v1/account/${context.wallet.address}`)
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual(formatNodeEntryResponse(context.wallet.address, writerEntry))
            expect(res.body).toMatchObject({
                isWhitelisted: true,
                isValidator: true,
                stakedBalance: bufferToBigInt(BALANCE_TO_STAKE.value).toString(),
            })
            expect(res.body.license).not.toBeNull()
            expect(res.body.license).not.toBe('1')
        })

        it("returns validator account details when confirmed=true", async () => {
            const confirmedEntry = await context.rpcMsb.state.getNodeEntry(context.wallet.address)
            expect(confirmedEntry).toBeTruthy()

            const res = await request(context.server).get(`/v1/account/${context.wallet.address}?confirmed=true`)
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual(formatNodeEntryResponse(context.wallet.address, confirmedEntry))
        })

        it("returns validator account details (unconfirmed view)", async () => {
            const unsignedEntry = await context.rpcMsb.state.getNodeEntryUnsigned(context.wallet.address)
            expect(unsignedEntry).toBeTruthy()

            const res = await request(context.server).get(`/v1/account/${context.wallet.address}?confirmed=false`)
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual(formatNodeEntryResponse(context.wallet.address, unsignedEntry))
        })

        it("returns default state for non-existent node", async () => {
            const address = randomAddress(context.rpcMsb.config.addressPrefix)

            const res = await request(context.server).get(`/v1/account/${address}`)
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual({
                address: address,
                writingKey: ZERO_WK.toString('hex'),
                isWhitelisted: false,
                isValidator: false,
                isIndexer: false,
                license: null,
                balance: '0',
                stakedBalance: '0',
            })
        })

        it("returns 400 when address is missing", async () => {
            const res = await request(context.server).get("/v1/account")
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Account address is required" })
        })

        it("returns 400 for invalid address format", async () => {
            const res = await request(context.server).get("/v1/account/not-a-valid-address")
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid account address format" })
        })

        it("returns 400 for invalid confirmed parameter", async () => {
            const res = await request(context.server).get(`/v1/account/${context.wallet.address}?confirmed=test`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: 'Parameter "confirmed" must be exactly "true" or "false"' })
        })

        it("returns 500 on internal error", async () => {
            const originalGetNodeEntry = context.rpcMsb.state.getNodeEntry

            context.rpcMsb.state.getNodeEntry = async () => { throw new Error("test") }

            try {
                const res = await request(context.server).get(`/v1/account/${context.wallet.address}`)
                expect(res.statusCode).toBe(500)
                expect(res.body).toEqual({ error: 'An error occurred processing the request.' })
            } finally {
                context.rpcMsb.state.getNodeEntry = originalGetNodeEntry
            }
        })
    })
}
