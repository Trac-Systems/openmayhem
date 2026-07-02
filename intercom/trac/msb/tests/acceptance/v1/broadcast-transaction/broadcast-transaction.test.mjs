import request from "supertest"
import b4a from "b4a"
import { $TNK } from "../../../../src/core/state/utils/balance.js"
import { buildRpcSelfTransferPayload, waitForConnection } from "../../../helpers/transactionPayloads.mjs"

const toBase64 = (value) => b4a.toString(b4a.from(JSON.stringify(value)), "base64")

export const registerBroadcastTransactionTests = (context) => {
    describe("POST /v1/broadcast-transaction", () => {
        it("broadcasts transaction and returns lengths", async () => {
            const { payload } = await buildRpcSelfTransferPayload(
                context,
                context.rpcMsb.state,
                1n
            );
            await waitForConnection(context.rpcMsb)
            const res = await request(context.server)
                .post("/v1/broadcast-transaction")
                .set("Accept", "application/json")
                .send(JSON.stringify({ payload }))

            expect(res.statusCode).toBe(200)
            expect(res.body).toMatchObject({
                result: {
                    message: "Transaction broadcasted successfully.",
                    signedLength: expect.any(Number),
                    unsignedLength: expect.any(Number),
                    tx: expect.any(String)
                }
            })
        })

        it("returns 400 when payload is missing", async () => {
            await waitForConnection(context.rpcMsb)
            const res = await request(context.server)
                .post("/v1/broadcast-transaction")
                .set("Accept", "application/json")
                .send(JSON.stringify({}))

            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Payload is missing." })
        })

        it("returns 400 when payload is not base64", async () => {
            await waitForConnection(context.rpcMsb)
            const res = await request(context.server)
                .post("/v1/broadcast-transaction")
                .set("Accept", "application/json")
                .send(JSON.stringify({ payload: "not-base64" }))

            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Payload must be a valid base64 string." })
        })

        // TODO: enable once handler returns 400 for client-side decode errors
        it.skip("returns 400 when decoded payload is not valid JSON", async () => {
            const invalidJsonBase64 = b4a.toString(b4a.from("{{invalid"), "base64")

            await waitForConnection(context.rpcMsb)
            const res = await request(context.server)
                .post("/v1/broadcast-transaction")
                .set("Accept", "application/json")
                .send(JSON.stringify({ payload: invalidJsonBase64 }))

            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Decoded payload is not valid JSON." })
        })

        // TODO: enable once handler returns 400 for client-side validation errors
        it.skip("returns 400 for invalid transaction structure", async () => {
            const invalidStructure = {
                type: 1,
                address: context.wallet.address,
            }

            await waitForConnection(context.rpcMsb)
            const res = await request(context.server)
                .post("/v1/broadcast-transaction")
                .set("Accept", "application/json")
                .send(JSON.stringify({ payload: toBase64(invalidStructure) }))

            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Invalid payload structure." })
        })

        // TODO: AFTER REFACTORIZATION IMPROVE THESE IMPLEMENTATIONS ENDPOINT TO COVER THESE TESTS.
        it.skip("returns 413 when payload exceeds size limit", async () => {
            const largeString = "a".repeat(3_000_000)
            const payload = toBase64({ type: 1, address: context.wallet.address, txo: { large: largeString } })

            await waitForConnection(context.rpcMsb)
            const res = await request(context.server)
                .post("/v1/broadcast-transaction")
                .set("Accept", "application/json")
                .send(JSON.stringify({ payload }))

            expect(res.statusCode).toBe(413)
        })

        it.skip("returns 429 on repeated broadcast failures", async () => {
            // TODO: Would require forcing msb to throw 'Failed to broadcast transaction after multiple attempts.'
            const txData = await tracCrypto.transaction.preBuild(
                context.wallet.address,
                context.wallet.address,
                b4a.toString($TNK(1n), 'hex'),
                b4a.toString(await context.rpcMsb.state.getIndexerSequenceState(), 'hex')
            )

            const payload = tracCrypto.transaction.build(txData, b4a.from(context.wallet.secretKey, 'hex'))
            await waitForConnection(context.rpcMsb)
            const res = await request(context.server)
                .post("/v1/broadcast-transaction")
                .set("Accept", "application/json")
                .send(JSON.stringify({ payload }))

            expect(res.statusCode).toBe(429)
            expect(res.body).toEqual({ error: "Failed to broadcast transaction after multiple attempts." })
        })
    })
}
