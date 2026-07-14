export async function runRootMsbTransferHelper(command, args = []) {
  const normalizedCommand = String(command || '').trim();
  if (!normalizedCommand) {
    throw new Error('MSB transfer helper command is required.');
  }
  if (normalizedCommand === 'balance') {
    const { runRootMsbBalanceHelper } = await import('./msb-balance-helper.js');
    return runRootMsbBalanceHelper(args);
  }
  if (normalizedCommand === 'settlement-transfer') {
    const { runSettlementTransferHelper } = await import('./msb-settlement-transfer-helper.js');
    return runSettlementTransferHelper(args);
  }
  const { runTransferHelper } = await import('trac-msb/src/transferHelper.js');
  return runTransferHelper([normalizedCommand, ...args]);
}
