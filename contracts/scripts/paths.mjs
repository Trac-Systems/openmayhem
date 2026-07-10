import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const REPO = join(dirname(fileURLToPath(import.meta.url)), '..', '..');

export const ADDRESSES_FILE = join(REPO, '.mayhem-local', 'contracts', 'eth-addresses.json');
