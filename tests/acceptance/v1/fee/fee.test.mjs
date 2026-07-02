import request from "supertest"

export const registerFeeTests = (context) => {
    describe("GET /v1/fee", () => {
        it("returns fee", async () => {
            const res = await request(context.server).get("/v1/fee")
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual({ fee: expect.stringMatching(/^-?\d+(\.\d+)?$/) })
        })
    })
}
