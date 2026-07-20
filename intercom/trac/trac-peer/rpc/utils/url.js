export function buildRequestUrl(req) {
  // We only need path + search params; base is a dummy placeholder.
  return new URL(req.url || "/", "http://localhost");
}

