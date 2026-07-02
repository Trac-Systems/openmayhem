const HOST_HEADER_PATTERN = /^[\[\]A-Za-z0-9:.-]+(:\d{1,5})?$/;

const isValidHostHeader = (hostHeader) => {
    if (!hostHeader) return false;
    if (hostHeader.length > 255) return false;
    const trimmed = hostHeader.trim();

    if (/[\/\\\s]/.test(trimmed)) return false;

    return HOST_HEADER_PATTERN.test(trimmed);
};

const socketAddressToHost = (address, port) => {
    if (!address) return null;
    const needsBrackets = address.includes(':') && !address.startsWith('[');
    const host = needsBrackets ? `[${address}]` : address;
    return port ? `${host}:${port}` : host;
};

const detectProtocol = (req) => {
    const forwardedProto = req?.headers?.['x-forwarded-proto'];
    if (forwardedProto === 'https') return 'https';
    if (forwardedProto === 'http') return 'http';

    if (req?.socket?.encrypted) return 'https';

    return 'http';
};

export const buildRequestUrl = (req) => {
    const safeHost = isValidHostHeader(req?.headers?.host)
        ? req.headers.host.trim()
        : socketAddressToHost(req?.socket?.localAddress, req?.socket?.localPort);

    const protocol = detectProtocol(req);
    const base = safeHost ? `${protocol}://${safeHost}` : `${protocol}://localhost`;
    return new URL(req?.url || '/', base);
};
