import request from "supertest"

export const registerUnconfirmedLengthTests = (context) => {
    describe("GET /v1/unconfirmed-length", () => {
        it("returns unconfirmed length", async () => {
            const res = await request(context.server).get("/v1/unconfirmed-length")
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual({ unconfirmed_length: expect.any(Number) })
        })
    })
}
