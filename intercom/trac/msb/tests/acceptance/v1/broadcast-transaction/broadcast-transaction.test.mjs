import request from "supertest"
import b4a from "b4a"
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

        it("returns 400 when decoded payload is not valid JSON", async () => {
            const invalidJsonBase64 = b4a.toString(b4a.from("{{invalid"), "base64")

            await waitForConnection(context.rpcMsb)
            const res = await request(context.server)
                .post("/v1/broadcast-transaction")
                .set("Accept", "application/json")
                .send(JSON.stringify({ payload: invalidJsonBase64 }))

            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Decoded payload is not valid JSON." })
        })

        it("returns 400 for invalid transaction structure", async () => {
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

        it("returns 413 when payload exceeds size limit", async () => {
            const largeString = "a".repeat(2_100_000); 
            const payload = toBase64({ type: 1, address: context.wallet.address, txo: { large: largeString } });

            await waitForConnection(context.rpcMsb);
            const res = await request(context.server)
                .post("/v1/broadcast-transaction")
                .set("Accept", "application/json")
                .send(JSON.stringify({ payload }));

            expect(res.statusCode).toBe(413);
        });


        it("returns 429 on repeated broadcast failures", async () => {
            const { payload } = await buildRpcSelfTransferPayload(context, context.rpcMsb.state, 1n);
            const originalMethod = context.rpcMsb.broadcastPartialTransaction;
            context.rpcMsb.broadcastPartialTransaction = async () => false; 

            try {
                await waitForConnection(context.rpcMsb);
                const res = await request(context.server)
                    .post("/v1/broadcast-transaction")
                    .set("Accept", "application/json")
                    .send(JSON.stringify({ payload }));

                expect(res.statusCode).toBe(429);
                expect(res.body).toEqual({ error: "Failed to broadcast transaction after multiple attempts." });
            } finally {
                context.rpcMsb.broadcastPartialTransaction = originalMethod;
            }
        });
    })
}
