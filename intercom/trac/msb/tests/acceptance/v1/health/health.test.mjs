import request from "supertest"

export const registerHealthTests = (context) => {
    describe("GET /v1/health", () => {
        it("should return 200 and ok:true when healthy", async () => {
            const res = await request(context.server).get("/v1/health")
            expect(res.statusCode).toBe(200)
            expect(res.body).toEqual({ ok: true })
        })
    })
}
