import request from "supertest"

export const registerTxPayloadsBulkTests = (context) => {
    describe("POST /v1/tx-payloads-bulk", () => {
        it("returns payloads for requested hashes", async () => {
            const result = await context.rpcMsb.state.confirmedTransactionsBetween(0, 40) // Arbitrary range that should contain valid entries
            const hashes = result.map(({ hash }) => hash)

            const payload = { hashes }

            const res = await request(context.server)
                .post("/v1/tx-payloads-bulk")
                .set("Accept", "application/json")
                .send(JSON.stringify(payload))

            expect(res.statusCode).toBe(200)
            expect(res.body).toMatchObject({
                results: expect.arrayOf(
                    expect.objectContaining({
                        hash: expect.any(String),
                    })
                ),
                missing: []
            })
        })
    })
}
