import request from "supertest"
import { buildRpcSelfTransferPayload, waitForConnection } from "../../../helpers/transactionPayloads.mjs"

export const registerTxTests = (context) => {
    describe("GET /v1/tx/:hash", () => {
        it("returns tx details for a broadcasted transaction", async () => {
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

            const res = await request(context.server).get(`/v1/tx/${txHashHex}`)
            expect(res.statusCode).toBe(200)
            expect(res.body).toMatchObject({ txDetails: expect.any(Object) })
        })

        it("returns 404 for non-existent transaction hash", async () => {
            const nonExistentHash = "0b4d1c1dac48af13212f616601d7399457476a0b644850875b7f4b79df6ff89c"
            const res = await request(context.server).get(`/v1/tx/${nonExistentHash}`)
            expect(res.statusCode).toBe(404)
            expect(res.body).toEqual({ txDetails: null })
        })

        // TODO: adjust implementation to cover tests below
        it.skip("returns 400 for invalid hash format (too short)", async () => {
            const invalidHash = '0'.repeat(63)
            const res = await request(context.server).get(`/v1/tx/${invalidHash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it.skip("returns 400 for invalid hash format (non-hex)", async () => {
            const invalidHash = 'Z'.repeat(64)
            const res = await request(context.server).get(`/v1/tx/${invalidHash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it.skip("returns 400 when no hash provided", async () => {
            const res = await request(context.server).get('/v1/tx')
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Transaction hash is required" })
        })

        it.skip("returns 400 for hash with invalid characters", async () => {
            const invalidHash = '0b4d1c1dac48$af13212f6166017399457476a0b644850875b7f4b79df6ff89c'
            const res = await request(context.server).get(`/v1/tx/${invalidHash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it.skip("returns 400 for hash with special characters", async () => {
            const invalidHash = '!@#$%^&*'.repeat(8)
            const res = await request(context.server).get(`/v1/tx/${invalidHash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it.skip("returns 400 for hash with spaces", async () => {
            const invalidHash = '0b4d1c1dac48af13212f616601d7399457476a0b644850875b7 4b79df6ff89c'
            const res = await request(context.server).get(`/v1/tx/${invalidHash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it.skip("returns 400 for hash with 0x prefix", async () => {
            const hash = "0x" + "0".repeat(62)
            const res = await request(context.server).get(`/v1/tx/${hash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it.skip("returns 400 for odd-length hex", async () => {
            const hash = "a".repeat(63)
            const res = await request(context.server).get(`/v1/tx/${hash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it.skip("accepts uppercase hex", async () => {
            const hash = "A".repeat(64)
            const res = await request(context.server).get(`/v1/tx/${hash}`)
            expect(res.statusCode).toBe(200)
        })

        it.skip("returns 400 for trailing space hash", async () => {
            const hash = `${"a".repeat(64)} `
            const res = await request(context.server).get(`/v1/tx/${hash}`)
            expect(res.statusCode).toBe(400)
        })
    })
}
