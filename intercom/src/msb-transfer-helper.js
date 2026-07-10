export async function runRootMsbTransferHelper(command, args = []) {
  const normalizedCommand = String(command || '').trim();
  if (!normalizedCommand) {
    throw new Error('MSB transfer helper command is required.');
  }
  if (normalizedCommand === 'balance') {
    const { runRootMsbBalanceHelper } = await import('./msb-balance-helper.js');
    return runRootMsbBalanceHelper(args);
  }
  const { runTransferHelper } = await import('trac-msb/src/transferHelper.js');
  return runTransferHelper([normalizedCommand, ...args]);
}
