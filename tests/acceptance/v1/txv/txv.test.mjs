import request from "supertest"

export const registerTxvTests = (context) => {
    describe("GET /v1/txv", () => {
        it("returns txv hash", async () => {
            const res = await request(context.server).get("/v1/txv")
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual({ txv: expect.stringMatching(/^[a-z0-9]{64}$/) })
        })
    })
}
