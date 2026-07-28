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
  if (['settlement-transfer-prepare', 'settlement-transfer-execute'].includes(normalizedCommand)) {
    const { runPreparedSettlementTransferHelper } =
      await import('./msb-settlement-transfer-helper.js');
    return runPreparedSettlementTransferHelper(normalizedCommand, args);
  }
  if (normalizedCommand !== 'transfer') {
    throw new Error(
      'Supported MSB helper commands: balance, transfer, settlement-transfer, settlement-transfer-prepare, settlement-transfer-execute.'
    );
  }
  const { runTransferHelper } = await import('./msb-settlement-transfer-helper.js');
  return runTransferHelper([normalizedCommand, ...args]);
}
