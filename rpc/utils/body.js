export async function readJsonBody(req, { maxBytes }) {
  let body = "";
  let bytesRead = 0;
  let done = false;

  return await new Promise((resolve, reject) => {
    req.on("data", (chunk) => {
      if (done) return;
      bytesRead += chunk.length;
      if (bytesRead > maxBytes) {
        done = true;
        const err = new Error("Request body too large.");
        err.code = "BODY_TOO_LARGE";
        // Keep the socket alive long enough to let the server respond with 413.
        // Drain remaining incoming data without buffering it.
        try {
          req.removeAllListeners("data");
          req.removeAllListeners("end");
          req.on("data", () => {});
          req.on("end", () => {});
          req.resume?.();
        } catch (_e) {}
        reject(err);
        return;
      }
      body += chunk.toString();
    });

    req.on("end", () => {
      if (done) return;
      if (!body) return resolve(null);
      try {
        resolve(JSON.parse(body));
      } catch (e) {
        const err = new Error("Invalid JSON payload.");
        err.code = "BAD_JSON";
        reject(err);
      }
    });

    req.on("error", (err) => {
      if (done) return;
      const e = new Error("Request stream error.");
      e.code = "STREAM_ERROR";
      e.cause = err;
      reject(e);
    });
  });
}
