import fs from 'fs';

const MAX_SC_BRIDGE_TOKEN_FILE_BYTES = 4096;

const nonEmptyFlag = (value) => {
  if (value === undefined || value === null || value === true) return '';
  return String(value).trim();
};

export const resolveScBridgeToken = (flags = {}, env = {}) => {
  const explicitToken = nonEmptyFlag(flags['sc-bridge-token']);
  const tokenFile = nonEmptyFlag(flags['sc-bridge-token-file']);
  if (explicitToken && tokenFile) {
    throw new Error(
      'SC-Bridge accepts either --sc-bridge-token or --sc-bridge-token-file, not both.'
    );
  }
  if (tokenFile) {
    const stat = fs.statSync(tokenFile);
    if (!stat.isFile()) {
      throw new Error('SC-Bridge token file is not a regular file.');
    }
    if (stat.size < 1 || stat.size > MAX_SC_BRIDGE_TOKEN_FILE_BYTES) {
      throw new Error(
        `SC-Bridge token file must contain 1-${MAX_SC_BRIDGE_TOKEN_FILE_BYTES} bytes.`
      );
    }
    const token = fs.readFileSync(tokenFile, 'utf8').trim();
    if (!token) {
      throw new Error('SC-Bridge token file is empty.');
    }
    return token;
  }
  return explicitToken || nonEmptyFlag(env.SC_BRIDGE_TOKEN);
};
