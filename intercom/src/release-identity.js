import crypto from 'crypto';
import fs from 'fs';
import path from 'path';

export const RELEASE_MANIFEST_PATH = 'contract/release.json';
export const INTERCOM_BUNDLE_ASSET_PREFIX = 'share/mayhem/intercom/';
export const CONTRACT_CODE_PATHS = Object.freeze([
  'contract/contract.js',
  'contract/protocol.js',
  'features/mayhem/index.js',
]);

const CONTRACT_CODE_DIGEST_DOMAIN = 'mayhem-intercom-contract-code-v1\0';
const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const PORTABLE_PATH_PATTERN = /^[\x20-\x7e]+$/;
const WINDOWS_UNSAFE_PATH_PATTERN = /[<>:"|?*]/;
const CONTRACT_VERSION_EXPORT_PATTERN =
  /^\s*export\s+const\s+CONTRACT_VERSION\s*=\s*([1-9][0-9]*)\s*;\s*$/gm;

const moduleFilePath = (moduleUrl) => {
  const url = new URL(moduleUrl);
  if (url.protocol !== 'file:') {
    throw new Error(
      'Intercom release identity requires a local authenticated release tree; pear:// link-only launch is unsupported.'
    );
  }
  let pathname = decodeURIComponent(url.pathname);
  if (url.hostname) pathname = `//${url.hostname}${pathname}`;
  if (path.sep === '\\' && /^\/[A-Za-z]:\//.test(pathname)) pathname = pathname.slice(1);
  return pathname;
};

export const resolveIntercomRoot = ({
  moduleUrl = import.meta.url,
  app = typeof Pear !== 'undefined' ? (Pear.app ?? Pear.config) : null,
} = {}) => {
  if (app?.key === null && typeof app?.dir === 'string') {
    return path.resolve(app.dir);
  }
  return path.resolve(path.dirname(moduleFilePath(moduleUrl)), '..');
};

export const INTERCOM_ROOT = resolveIntercomRoot();

const fail = (message) => {
  const error = new Error(`Invalid Intercom release identity: ${message}`);
  error.code = 'INTERCOM_RELEASE_IDENTITY_INVALID';
  throw error;
};

const bundleFail = (message) => {
  const error = new Error(`Invalid Intercom release bundle: ${message}`);
  error.code = 'INTERCOM_RELEASE_BUNDLE_INVALID';
  throw error;
};

const exactKeys = (value, expected, label, reject = fail) => {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    reject(`${label} must be an object.`);
  }
  const actual = Object.keys(value).sort();
  const required = [...expected].sort();
  if (actual.length !== required.length ||
      actual.some((key, index) => key !== required[index])) {
    reject(`${label} keys must be exactly: ${required.join(', ')}.`);
  }
};

const readBytes = (rootDir, relativePath, label) => {
  const filePath = path.join(rootDir, relativePath);
  try {
    return fs.readFileSync(filePath);
  } catch (error) {
    const reason = error?.code === 'ENOENT' ? 'is missing' : `cannot be read: ${error?.message ?? error}`;
    fail(`${label} ${relativePath} ${reason}.`);
  }
};

const sha256Hex = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex');

const validateRuntimeRelativePath = (relativePath, label) => {
  if (typeof relativePath !== 'string' || relativePath.length === 0) {
    bundleFail(`${label} must be a non-empty string.`);
  }
  if (!PORTABLE_PATH_PATTERN.test(relativePath) ||
      relativePath.startsWith('/') ||
      relativePath.includes('\\') ||
      WINDOWS_UNSAFE_PATH_PATTERN.test(relativePath)) {
    bundleFail(`${label} is unsafe: ${JSON.stringify(relativePath)}.`);
  }
  const segments = relativePath.split('/');
  if (segments.some((segment) =>
    segment.length === 0 ||
    segment === '.' ||
    segment === '..' ||
    segment.endsWith('.') ||
    segment.endsWith(' ')
  )) {
    bundleFail(`${label} is unsafe: ${JSON.stringify(relativePath)}.`);
  }
  return relativePath;
};

const runtimeTreeFiles = (rootDir) => {
  const resolvedRoot = path.resolve(String(rootDir));
  let rootStat;
  try {
    rootStat = fs.lstatSync(resolvedRoot);
  } catch (error) {
    bundleFail(`runtime root ${resolvedRoot} cannot be inspected: ${error?.message ?? error}.`);
  }
  if (rootStat.isSymbolicLink() || !rootStat.isDirectory()) {
    bundleFail(`runtime root ${resolvedRoot} must be a real directory.`);
  }

  const files = [];
  const visit = (directoryPath, parentSegments) => {
    let entries;
    try {
      entries = fs.readdirSync(directoryPath, { withFileTypes: true });
    } catch (error) {
      bundleFail(`runtime directory ${directoryPath} cannot be read: ${error?.message ?? error}.`);
    }
    entries.sort((left, right) => left.name < right.name ? -1 : left.name > right.name ? 1 : 0);
    for (const entry of entries) {
      const segments = [...parentSegments, entry.name];
      const relativePath = validateRuntimeRelativePath(
        segments.join('/'),
        'runtime tree path'
      );
      const filePath = path.join(directoryPath, entry.name);
      let stat;
      try {
        stat = fs.lstatSync(filePath);
      } catch (error) {
        bundleFail(`runtime tree path ${relativePath} cannot be inspected: ${error?.message ?? error}.`);
      }
      if (stat.isSymbolicLink()) {
        bundleFail(`runtime tree path ${relativePath} must not be a symbolic link.`);
      }
      if (stat.isDirectory()) {
        visit(filePath, segments);
        continue;
      }
      if (!stat.isFile()) {
        bundleFail(`runtime tree path ${relativePath} must be a regular file or directory.`);
      }
      let bytes;
      try {
        bytes = fs.readFileSync(filePath);
      } catch (error) {
        bundleFail(`runtime tree file ${relativePath} cannot be read: ${error?.message ?? error}.`);
      }
      files.push(Object.freeze({
        path: relativePath,
        sha256: sha256Hex(bytes),
      }));
    }
  };
  visit(resolvedRoot, []);
  files.sort((left, right) => left.path < right.path ? -1 : left.path > right.path ? 1 : 0);
  return Object.freeze(files);
};

const contractCodeDigest = (files) => {
  const digest = crypto.createHash('sha256');
  digest.update(CONTRACT_CODE_DIGEST_DOMAIN);
  for (const file of files) {
    digest.update(file.path);
    digest.update('\0');
    digest.update(String(file.bytes.length));
    digest.update('\0');
    digest.update(file.bytes);
    digest.update('\0');
  }
  return digest.digest('hex');
};

const contractVersionFromSource = (bytes) => {
  const source = bytes.toString('utf8');
  const matches = [...source.matchAll(CONTRACT_VERSION_EXPORT_PATTERN)];
  if (matches.length !== 1) {
    fail('contract/contract.js must contain exactly one explicit CONTRACT_VERSION export.');
  }
  const version = Number(matches[0][1]);
  if (!Number.isSafeInteger(version) || version < 1) {
    fail('contract/contract.js exports an invalid CONTRACT_VERSION.');
  }
  return version;
};

export const verifyReleaseIdentity = ({ rootDir = INTERCOM_ROOT } = {}) => {
  const resolvedRoot = path.resolve(String(rootDir));
  const manifestBytes = readBytes(resolvedRoot, RELEASE_MANIFEST_PATH, 'release manifest');
  let manifest;
  try {
    manifest = JSON.parse(manifestBytes.toString('utf8'));
  } catch (error) {
    fail(`release manifest ${RELEASE_MANIFEST_PATH} is not valid JSON: ${error?.message ?? error}.`);
  }

  exactKeys(
    manifest,
    ['schema', 'release_version', 'contract_version', 'contract_code_sha256', 'files'],
    'release manifest'
  );
  if (manifest.schema !== 1) fail('release manifest schema must be 1.');
  if (typeof manifest.release_version !== 'string' ||
      !/^[0-9]+\.[0-9]+\.[0-9]+$/.test(manifest.release_version)) {
    fail('release_version must be an explicit semantic version.');
  }
  if (!Number.isSafeInteger(manifest.contract_version) || manifest.contract_version < 1) {
    fail('contract_version must be a positive safe integer.');
  }
  if (!SHA256_PATTERN.test(manifest.contract_code_sha256)) {
    fail('contract_code_sha256 must be lowercase SHA-256 hex.');
  }
  if (!Array.isArray(manifest.files) ||
      manifest.files.length !== CONTRACT_CODE_PATHS.length) {
    fail(`files must list exactly ${CONTRACT_CODE_PATHS.length} contract code paths.`);
  }

  const verifiedFiles = manifest.files.map((file, index) => {
    exactKeys(file, ['path', 'sha256'], `files[${index}]`);
    const expectedPath = CONTRACT_CODE_PATHS[index];
    if (file.path !== expectedPath) {
      fail(`files[${index}].path must be ${expectedPath}; paths must remain sorted.`);
    }
    if (!SHA256_PATTERN.test(file.sha256)) {
      fail(`files[${index}].sha256 must be lowercase SHA-256 hex.`);
    }
    const bytes = readBytes(resolvedRoot, file.path, 'contract code file');
    const actualSha256 = sha256Hex(bytes);
    if (actualSha256 !== file.sha256) {
      fail(
        `contract code file ${file.path} SHA-256 mismatch ` +
        `(expected ${file.sha256}, actual ${actualSha256}).`
      );
    }
    return { path: file.path, sha256: actualSha256, bytes };
  });

  const actualContractCodeSha256 = contractCodeDigest(verifiedFiles);
  if (actualContractCodeSha256 !== manifest.contract_code_sha256) {
    fail(
      `contract_code_sha256 mismatch ` +
      `(expected ${manifest.contract_code_sha256}, actual ${actualContractCodeSha256}).`
    );
  }

  const contractSource = verifiedFiles.find(
    ({ path: filePath }) => filePath === 'contract/contract.js'
  );
  if (!contractSource) fail('contract/contract.js is missing from verified contract code.');
  const actualContractVersion = contractVersionFromSource(contractSource.bytes);
  if (actualContractVersion !== manifest.contract_version) {
    fail(
      `contract_version mismatch ` +
      `(manifest ${manifest.contract_version}, contract.js ${actualContractVersion}).`
    );
  }

  return Object.freeze({
    releaseVersion: manifest.release_version,
    contractVersion: manifest.contract_version,
    contractCodeSha256: actualContractCodeSha256,
    files: Object.freeze(
      verifiedFiles.map(({ path: filePath, sha256 }) => Object.freeze({
        path: filePath,
        sha256,
      }))
    ),
  });
};

const validateBundleManifestShape = (manifest) => {
  exactKeys(
    manifest,
    ['schema', 'release_version', 'contract_version', 'contract_code_sha256', 'assets'],
    'bundle manifest',
    bundleFail
  );
  if (manifest.schema !== 1) bundleFail('bundle manifest schema must be 1.');
  if (typeof manifest.release_version !== 'string' ||
      !/^[0-9]+\.[0-9]+\.[0-9]+$/.test(manifest.release_version)) {
    bundleFail('release_version must be an explicit semantic version.');
  }
  if (!Number.isSafeInteger(manifest.contract_version) || manifest.contract_version < 1) {
    bundleFail('contract_version must be a positive safe integer.');
  }
  if (!SHA256_PATTERN.test(manifest.contract_code_sha256)) {
    bundleFail('contract_code_sha256 must be lowercase SHA-256 hex.');
  }
  if (!Array.isArray(manifest.assets) || manifest.assets.length === 0) {
    bundleFail('assets must be a non-empty array.');
  }

  const seen = new Set();
  let previousPath = null;
  const assets = manifest.assets.map((asset, index) => {
    exactKeys(asset, ['path', 'sha256'], `assets[${index}]`, bundleFail);
    if (typeof asset.path !== 'string' ||
        !asset.path.startsWith(INTERCOM_BUNDLE_ASSET_PREFIX)) {
      bundleFail(
        `assets[${index}].path must start with ${INTERCOM_BUNDLE_ASSET_PREFIX}.`
      );
    }
    const relativePath = validateRuntimeRelativePath(
      asset.path.slice(INTERCOM_BUNDLE_ASSET_PREFIX.length),
      `assets[${index}].path`
    );
    if (seen.has(asset.path)) {
      bundleFail(`assets contains duplicate path: ${asset.path}.`);
    }
    if (previousPath !== null && previousPath > asset.path) {
      bundleFail('assets paths must be sorted in ascending byte order.');
    }
    if (!SHA256_PATTERN.test(asset.sha256)) {
      bundleFail(`assets[${index}].sha256 must be lowercase SHA-256 hex.`);
    }
    seen.add(asset.path);
    previousPath = asset.path;
    return Object.freeze({
      path: asset.path,
      relativePath,
      sha256: asset.sha256,
    });
  });
  return Object.freeze(assets);
};

export const createIntercomBundleManifest = ({ rootDir = INTERCOM_ROOT } = {}) => {
  const resolvedRoot = path.resolve(String(rootDir));
  const identity = verifyReleaseIdentity({ rootDir: resolvedRoot });
  const assets = runtimeTreeFiles(resolvedRoot).map((file) => Object.freeze({
    path: `${INTERCOM_BUNDLE_ASSET_PREFIX}${file.path}`,
    sha256: file.sha256,
  }));
  if (assets.length === 0) bundleFail('runtime tree must contain at least one file.');
  return Object.freeze({
    schema: 1,
    release_version: identity.releaseVersion,
    contract_version: identity.contractVersion,
    contract_code_sha256: identity.contractCodeSha256,
    assets: Object.freeze(assets),
  });
};

export const verifyIntercomBundleManifest = (
  manifest,
  { rootDir = INTERCOM_ROOT } = {}
) => {
  const assets = validateBundleManifestShape(manifest);
  const resolvedRoot = path.resolve(String(rootDir));
  const identity = verifyReleaseIdentity({ rootDir: resolvedRoot });
  if (manifest.release_version !== identity.releaseVersion) {
    bundleFail(
      `release_version mismatch (bundle ${manifest.release_version}, ` +
      `release identity ${identity.releaseVersion}).`
    );
  }
  if (manifest.contract_version !== identity.contractVersion) {
    bundleFail(
      `contract_version mismatch (bundle ${manifest.contract_version}, ` +
      `release identity ${identity.contractVersion}).`
    );
  }
  if (manifest.contract_code_sha256 !== identity.contractCodeSha256) {
    bundleFail(
      `contract_code_sha256 mismatch (bundle ${manifest.contract_code_sha256}, ` +
      `release identity ${identity.contractCodeSha256}).`
    );
  }

  const actualFiles = runtimeTreeFiles(resolvedRoot);
  const actualByPath = new Map(actualFiles.map((file) => [file.path, file]));
  const expectedPaths = new Set(assets.map((asset) => asset.relativePath));
  for (const asset of assets) {
    if (!actualByPath.has(asset.relativePath)) {
      bundleFail(`listed runtime file is missing: ${asset.relativePath}.`);
    }
  }
  for (const file of actualFiles) {
    if (!expectedPaths.has(file.path)) {
      bundleFail(`runtime tree contains unlisted extra file: ${file.path}.`);
    }
  }
  for (const asset of assets) {
    const actual = actualByPath.get(asset.relativePath);
    if (actual.sha256 !== asset.sha256) {
      bundleFail(
        `runtime file ${asset.relativePath} SHA-256 mismatch ` +
        `(expected ${asset.sha256}, actual ${actual.sha256}).`
      );
    }
  }

  return Object.freeze({
    releaseVersion: identity.releaseVersion,
    contractVersion: identity.contractVersion,
    contractCodeSha256: identity.contractCodeSha256,
    assets: Object.freeze(assets.map(({ path: assetPath, sha256 }) =>
      Object.freeze({ path: assetPath, sha256 })
    )),
  });
};

export const verifyStartupReleaseIdentity = (options) => {
  try {
    return verifyReleaseIdentity(options);
  } catch (error) {
    const startupError = new Error(
      `Intercom startup release identity verification failed: ${error?.message ?? error}`
    );
    startupError.code = 'INTERCOM_STARTUP_RELEASE_IDENTITY_INVALID';
    startupError.cause = error;
    throw startupError;
  }
};
