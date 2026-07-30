import request from "supertest"
import { randomAddress } from "../../../unit/state/stateTestUtils.js"

export const registerBalanceTests = (context) => {
    let expectedBalance
    describe("GET /v1/balance", () => {
        it("returns balance for confirmed view by default", async () => {
            const res = await request(context.server).get(`/v1/balance/${context.wallet.address}`)
            expect(res.statusCode).toBe(200)
            expect(res.body.address).toBe(context.wallet.address)
            expect(typeof res.body.balance).toBe("string")
            expectedBalance = res.body.balance
        })

        it("returns balance for unconfirmed view", async () => {
            const res = await request(context.server).get(`/v1/balance/${context.wallet.address}?confirmed=false`)
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual({ address: context.wallet.address, balance: expectedBalance })
        })

        it("returns balance when confirmed flag is explicitly true", async () => {
            const res = await request(context.server).get(`/v1/balance/${context.wallet.address}?confirmed=true`)
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual({ address: context.wallet.address, balance: expectedBalance })
        })

        it("falls back to unconfirmed view on invalid confirmed flag", async () => {
            const res = await request(context.server).get(`/v1/balance/${context.wallet.address}?confirmed=test`)
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual({ address: context.wallet.address, balance: expectedBalance })
        })

        it("returns 400 when address is missing", async () => {
            const res = await request(context.server).get("/v1/balance")
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Wallet address is required" })
        })

        it("returns zero balance for an unknown address", async () => {
            const address = randomAddress(context.rpcMsb.config.addressPrefix)
            const res = await request(context.server).get(`/v1/balance/${address}`)
            expect(res.statusCode).toBe(200)
            expect(res.body.address).toBe(address)
            expect(BigInt(res.body.balance)).toBe(0n)
        })

        it("returns 400 for an invalid address format", async () => {
            const invalidAddress = "not-a-valid-address"
            const res = await request(context.server).get(`/v1/balance/${invalidAddress}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid account address format" })
        })
    })
}
