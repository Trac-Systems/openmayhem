export async function runRootMsbTransferHelper(command, args = []) {
  const normalizedCommand = String(command || '').trim();
  if (!normalizedCommand) {
    throw new Error('MSB transfer helper command is required.');
  }
  const { runTransferHelper } = await import('trac-msb/src/transferHelper.js');
  return runTransferHelper([normalizedCommand, ...args]);
}
