const URL_PATTERN = /https?:\/\/[^\s"'<>]+/gi;
const STRIPE_KEY_PATTERN = /\b(?:sk|pk|rk|mk)_(?:live|test)?_[A-Za-z0-9]+\b/g;
const SECRET_ASSIGNMENT_PATTERN = /\b(api[_-]?key|access[_-]?token|bearer|password|secret|private[_-]?key)(\s*[=:]\s*)[^\s,;]+/gi;

function redactEmbeddedUrl(raw) {
  const trailingMatch = raw.match(/[),.;]+$/);
  const trailing = trailingMatch?.[0] ?? '';
  const candidate = trailing ? raw.slice(0, -trailing.length) : raw;
  try {
    const url = new URL(candidate);
    if (url.username) url.username = '***';
    if (url.password) url.password = '***';
    url.search = '';
    url.hash = '';
    if ((url.pathname || '').length > 1) url.pathname = '/...';
    return `${url.toString()}${trailing}`;
  } catch (_error) {
    return `https://<redacted>${trailing}`;
  }
}

export function redactSensitiveText(value, maxLength = 2_000) {
  return String(value ?? '')
    .replace(URL_PATTERN, redactEmbeddedUrl)
    .replace(STRIPE_KEY_PATTERN, '<redacted-key>')
    .replace(SECRET_ASSIGNMENT_PATTERN, (_match, label, separator) => `${label}${separator}<redacted>`)
    .slice(0, maxLength);
}

export function safeErrorMessage(error, maxLength = 2_000) {
  const message = error?.shortMessage || error?.reason || error?.message || error || 'unknown error';
  return redactSensitiveText(message, maxLength);
}
