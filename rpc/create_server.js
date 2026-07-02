import http from "http";
import { applyCors } from "./cors.js";
import { routes } from "./routes/index.js";
import { DEFAULT_MAX_BODY_BYTES } from "./constants.js";

export const createServer = (peer, { maxBodyBytes = DEFAULT_MAX_BODY_BYTES, allowOrigin = "*" } = {}) => {
  const server = http.createServer({}, async (req, res) => {
    const respond = (code, payload) => {
      if (res.headersSent) return;
      res.writeHead(code, { "Content-Type": "application/json" });
      res.end(JSON.stringify(payload ?? {}));
    };

    req.on("error", (err) => {
      console.error("RPC request stream error:", err);
      respond(500, { error: "Request stream error." });
    });

    if (applyCors(req, res, { allowOrigin })) return;

    const requestPath = (req.url || "/").split("?")[0];
    const sortedRoutes = [...routes].sort((a, b) => b.path.length - a.path.length);

    for (const route of sortedRoutes) {
      if (req.method !== route.method) continue;
      if (requestPath !== route.path) continue;
      try {
        await route.handler({ req, res, respond, peer, maxBodyBytes });
      } catch (error) {
        const code =
          error?.code === "BODY_TOO_LARGE"
            ? 413
            : error?.code === "BAD_JSON"
              ? 400
              : typeof error?.message === "string" &&
                  /^(Missing|Invalid|Empty|Peer subnet is not writable)/.test(error.message)
                ? 400
                : 500;
        if (code === 500) console.error("RPC handler error:", error);
        respond(code, { error: code === 500 ? "An internal error occurred." : error?.message || "Bad Request" });
      }
      return;
    }

    respond(404, { error: "Not Found" });
  });

  return server;
};
