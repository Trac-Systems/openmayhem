import request from "supertest"
import { buildRpcSelfTransferPayload, waitForConnection } from "../../../helpers/transactionPayloads.mjs"
import { sleep } from "../../../../src/utils/helpers.js"

const TX_ENDPOINT_TIMEOUT_MS = 4000
const TX_ENDPOINT_RETRY_INTERVAL_MS = 100

const waitForStatusCode = async (requestFactory, expectedStatusCode, timeoutMs = TX_ENDPOINT_TIMEOUT_MS) => {
    const startedAt = Date.now()
    let response = null

    while ((Date.now() - startedAt) < timeoutMs) {
        response = await requestFactory()
        if (response.statusCode === expectedStatusCode) {
            return response
        }
        await sleep(TX_ENDPOINT_RETRY_INTERVAL_MS)
    }

    return response
}

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

            const res = await waitForStatusCode(
                () => request(context.server).get(`/v1/tx/${txHashHex}`),
                200
            )
            expect(res.statusCode).toBe(200)
            expect(res.body).toMatchObject({ txDetails: expect.any(Object) })
        })

        it("returns 404 for non-existent transaction hash", async () => {
            const nonExistentHash = "0b4d1c1dac48af13212f616601d7399457476a0b644850875b7f4b79df6ff89c"
            const res = await request(context.server).get(`/v1/tx/${nonExistentHash}`)
            expect(res.statusCode).toBe(404)
            expect(res.body).toEqual({
                error: `Transaction ${nonExistentHash} not found.`
            })
        })

        it("returns 400 for invalid hash format (too short)", async () => {
            const invalidHash = '0'.repeat(63)
            const res = await request(context.server).get(`/v1/tx/${invalidHash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it("returns 400 for invalid hash format (non-hex)", async () => {
            const invalidHash = 'Z'.repeat(64)
            const res = await request(context.server).get(`/v1/tx/${invalidHash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it("returns 400 when no hash provided", async () => {
            const res = await request(context.server).get('/v1/tx')
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Transaction hash is required" })
        })

        it("returns 400 for hash with invalid characters", async () => {
            const invalidHash = '0b4d1c1dac48$af13212f6166017399457476a0b644850875b7f4b79df6ff89c'
            const res = await request(context.server).get(`/v1/tx/${invalidHash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it("returns 400 for hash with special characters", async () => {
            const invalidHash = '!@#$%^&*'.repeat(8)
            const res = await request(context.server).get(`/v1/tx/${invalidHash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it("returns 400 for hash with spaces", async () => {
            const invalidHash = '0b4d1c1dac48af13212f616601d7399457476a0b644850875b7 4b79df6ff89c'
            const res = await request(context.server).get(`/v1/tx/${invalidHash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it("returns 400 for hash with 0x prefix", async () => {
            const hash = "0x" + "0".repeat(62)
            const res = await request(context.server).get(`/v1/tx/${hash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })

        it("returns 400 for odd-length hex", async () => {
            const hash = "a".repeat(63)
            const res = await request(context.server).get(`/v1/tx/${hash}`)
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid transaction hash format" })
        })


        it("accepts uppercase hex", async () => {
            const { payload, txHashHex } = await buildRpcSelfTransferPayload(context, context.rpcMsb.state, 1n);
            
            // Send the transaction
            await request(context.server)
                .post("/v1/broadcast-transaction")
                .send(JSON.stringify({ payload }));

            // Waits for the node indexer to process
            await new Promise(resolve => setTimeout(resolve, 500)); 

            const uppercaseHash = txHashHex.toUpperCase();
            const res = await request(context.server).get(`/v1/tx/${uppercaseHash}`);
            
            expect(res.statusCode).toBe(200);
        });

        it("returns 400 for trailing space hash", async () => {
            const hash = "a".repeat(64) + "%20" // Forcing space
            const res = await request(context.server).get(`/v1/tx/${hash}`)
            expect(res.statusCode).toBe(400)
        })
    })
}
