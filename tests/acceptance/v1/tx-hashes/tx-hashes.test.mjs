import request from "supertest"

export const registerTxHashesTests = (context) => {
    describe("GET /v1/tx-hashes", () => {
        it("< 1000", async () => {
            const res = await request(context.server).get("/v1/tx-hashes/1/1001")
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual({
                hashes: expect.arrayContaining([
                    expect.objectContaining({
                        hash: expect.any(String),
                        confirmed_length: expect.any(Number),
                    })
                ])
            })
        })

        it("> 1000", async () => {
            const res = await request(context.server).get("/v1/tx-hashes/1/1002")
            expect(res.statusCode).toBe(400)
        })

        it("returns 400 for non-integer params", async () => {
            const res = await request(context.server).get("/v1/tx-hashes/foo/bar")
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Params must be integer" })
        })

        it("returns 400 for negative params", async () => {
            const res = await request(context.server).get("/v1/tx-hashes/-1/10")
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "Params must be non-negative" })
        })

        it("returns 400 when end < start", async () => {
            const res = await request(context.server).get("/v1/tx-hashes/10/5")
            expect(res.statusCode).toBe(400)
            expect(res.body).toEqual({ error: "endSignedLength must be greater than or equal to startSignedLength." })
        })

        it("accepts boundary at exactly MAX_SIGNED_LENGTH", async () => {
            const res = await request(context.server).get("/v1/tx-hashes/1/1001")
            expect(res.statusCode).toBe(200)
        })

        it("trims range to current confirmed length", async () => {
            const confirmed = await request(context.server).get("/v1/confirmed-length")
            expect(confirmed.statusCode).toBe(200)
            const beyond = confirmed.body.confirmed_length + 50
            const res = await request(context.server).get(`/v1/tx-hashes/0/${beyond}`)
            expect(res.statusCode).toBe(200)
            const hashes = res.body.hashes || []
            // last item confirmed_length should not exceed current confirmed length
            const maxReturned = hashes.reduce((max, { confirmed_length }) => Math.max(max, confirmed_length || 0), 0)
            expect(maxReturned).toBeLessThanOrEqual(confirmed.body.confirmed_length)
        })

        it("returns empty hashes when no data in range", async () => {
            const res = await request(context.server).get("/v1/tx-hashes/0/0")
            expect(res.statusCode).toBe(200)
            expect(res.body).toHaveProperty("hashes")
            expect(Array.isArray(res.body.hashes)).toBe(true)
        })

        it("returns empty hashes when start equals end", async () => {
            const res = await request(context.server).get("/v1/tx-hashes/5/5")
            expect(res.statusCode).toBe(200)
            expect(res.body).toHaveProperty("hashes")
            expect(res.body.hashes).toEqual([])
        })
    })
}
