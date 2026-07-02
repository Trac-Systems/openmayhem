import request from "supertest"
import { buildRpcSelfTransferPayload, waitForConnection } from "../../../helpers/transactionPayloads.mjs"

export const registerTxDetailsTests = (context) => {
    describe("GET /v1/tx/details", () => {
        it("returns 200 for broadcasted hash (confirmed and unconfirmed)", async () => {
            const { payload, txHashHex } = await buildRpcSelfTransferPayload(
                context,
                context.rpcMsb.state,
                1n
            );

            await waitForConnection(context.rpcMsb)
            const broadcastRes = await request(context.server)
                .post("/v1/broadcast-transaction")
                .set("Accept", "application/json")
                .send(JSON.stringify({ payload }))
            expect(broadcastRes.statusCode).toBe(200)

            const resConfirmed = await request(context.server)
                .get(`/v1/tx/details/${txHashHex}?confirmed=true`)
            expect(resConfirmed.statusCode).toBe(200)

            expect(resConfirmed.body).toMatchObject({
                txDetails: expect.any(Object),
                confirmed_length: expect.any(Number),
                fee: expect.any(String)
            })

            const resUnconfirmed = await request(context.server)
                .get(`/v1/tx/details/${txHashHex}?confirmed=false`)
            expect(resUnconfirmed.statusCode).toBe(200)

            expect(resUnconfirmed.body).toMatchObject({
                txDetails: expect.any(Object),
                confirmed_length: expect.any(Number),
                fee: expect.any(String)
            })
        })

        it("handles null confirmed_length for unconfirmed transaction", async () => {
            const { payload, txHashHex } = await buildRpcSelfTransferPayload(
                context,
                context.rpcMsb.state,
                1n
            );

            const originalGetConfirmedLength = context.rpcMsb.state.getTransactionConfirmedLength
            context.rpcMsb.state.getTransactionConfirmedLength = async () => null

            try {

                await waitForConnection(context.rpcMsb)
                const broadcastRes = await request(context.server)
                    .post("/v1/broadcast-transaction")
                    .set("Accept", "application/json")
                    .send(JSON.stringify({ payload }))
                expect(broadcastRes.statusCode).toBe(200)

                const res = await request(context.server)
                    .get(`/v1/tx/details/${txHashHex}?confirmed=false`)
                expect(res.statusCode).toBe(200)

                expect(res.body).toMatchObject({
                    txDetails: expect.any(Object),
                    confirmed_length: 0,
                    fee: '0'
                })
            } finally {
                context.rpcMsb.state.getTransactionConfirmedLength = originalGetConfirmedLength
            }
        })

        it("returns 404 for non-existent transaction hash", async () => {
            const nonExistentHash = "0b4d1c1dac48af13212f616601d7399457476a0b644850875b7f4b79df6ff89c"
            const res = await request(context.server)
                .get(`/v1/tx/details/${nonExistentHash}`)

            expect(res.statusCode).toBe(404)
            expect(res.body).toEqual({
                error: `No payload found for tx hash: ${nonExistentHash}`
            })
        })

        it("returns 400 for invalid hash format (too short)", async () => {
            const invalidHash = '0'.repeat(63)
            const res = await request(context.server)
                .get(`/v1/tx/details/${invalidHash}`)

            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({
                error: "Invalid transaction hash format"
            })
        })

        it("returns 400 for invalid hash format (non-hex)", async () => {
            const invalidHash = 'Z'.repeat(64)
            const res = await request(context.server)
                .get(`/v1/tx/details/${invalidHash}`)

            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({
                error: "Invalid transaction hash format"
            })
        })

        it("returns 400 for invalid confirmed parameter", async () => {
            const hash = "0b4d1c1dac48af13212f616601d7399457476a0b644850875b7f4b79df6ff89c"

            const res = await request(context.server)
                .get(`/v1/tx/details/${hash}?confirmed=invalid`)

            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({
                error: 'Parameter "confirmed" must be exactly "true" or "false"'
            })
        })

        it("returns 400 for invalid confirmed parameter case (UPPERCASE)", async () => {
            const hash = "0b4d1c1dac48af13212f616601d7399457476a0b644850875b7f4b79df6ff89c"
            const res = await request(context.server).get(`/v1/tx/details/${hash}?confirmed=TRUE`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({
                error: 'Parameter "confirmed" must be exactly "true" or "false"'
            })
        })

        it("returns 400 when no hash provided", async () => {
            const res = await request(context.server)
                .get('/v1/tx/details')

            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({
                error: "Transaction hash is required"
            })
        })

        it("returns 400 for hash with invalid characters", async () => {
            const invalidHash = '0b4d1c1dac48$af13212f6166017399457476a0b644850875b7f4b79df6ff89c'
            const res = await request(context.server)
                .get(`/v1/tx/details/${invalidHash}`)

            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({
                error: "Invalid transaction hash format"
            })
        })

        it("returns 400 for hash with special characters", async () => {
            const invalidHash = '!@#$%^&*'.repeat(8)
            const res = await request(context.server)
                .get(`/v1/tx/details/${invalidHash}`)

            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({
                error: "Invalid transaction hash format"
            })
        })

        it("returns 400 for hash with spaces", async () => {
            const invalidHash = '0b4d1c1dac48af13212f616601d7399457476a0b644850875b7 4b79df6ff89c'
            const res = await request(context.server)
                .get(`/v1/tx/details/${invalidHash}`)

            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({
                error: "Invalid transaction hash format"
            })
        })

        it("returns 400 for hash with 0x prefix", async () => {
            const hash = "0x" + "0".repeat(62)
            const res = await request(context.server).get(`/v1/tx/details/${hash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it("returns 400 for odd-length hex", async () => {
            const hash = "a".repeat(63)
            const res = await request(context.server).get(`/v1/tx/details/${hash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        // TODOadjust implementation to cover tests below
        it.skip("accepts uppercase hex", async () => {
            const hash = "A".repeat(64)
            const res = await request(context.server).get(`/v1/tx/details/${hash}?confirmed=false`)
            expect([200]).toContain(res.statusCode)
            expect(res.statusCode).toBe(200)
        })

        it.skip("returns 400 for trailing space hash", async () => {
            const hash = `${"a".repeat(64)} `
            const res = await request(context.server).get(`/v1/tx/details/${hash}`)
            expect(res.statusCode).toBe(400)
        })
    })
}
