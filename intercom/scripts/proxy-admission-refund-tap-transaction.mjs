// Admission-return transaction codec uses the existing pinned ethers package.
// No provider, environment access, approval, pool deposit or broadcast lives here.
import { ethers } from 'ethers';
const token=new ethers.Interface(['function transfer(address,uint256) returns (bool)','function balanceOf(address) view returns (uint256)']);
export const tapTransferData=(to,amount)=>token.encodeFunctionData('transfer',[to,BigInt(amount)]);
export const tapBalanceData=from=>token.encodeFunctionData('balanceOf',[from]);
export const tapRefundSigner=secret=>new ethers.Wallet(new ethers.SigningKey(secret));
export function inspectTapRefundTransaction(raw,expected) {
  if(typeof raw!=='string'||!/^0x(?:[0-9a-f]{2}){1,2048}$/.test(raw))throw Error('Invalid bounded TAP refund transaction');
  const tx=ethers.Transaction.from(raw);
  if(!tx.isSigned()||tx.type!==2||tx.chainId!==BigInt(expected.chainId)||tx.to?.toLowerCase()!==expected.token
    ||tx.from?.toLowerCase()!==expected.from||tx.value!==0n||tx.data!==tapTransferData(expected.to,expected.amount)
    ||!Number.isSafeInteger(tx.nonce)||tx.nonce<0||tx.accessList?.length!==0||tx.authorizationList!=null||tx.blobs!=null
    ||tx.maxPriorityFeePerGas===null||tx.maxFeePerGas===null||tx.maxPriorityFeePerGas>tx.maxFeePerGas||tx.maxFeePerGas<=0n
    ||tx.gasLimit<=0n||tx.gasLimit>BigInt(expected.maxGas)||tx.maxFeePerGas>BigInt(expected.maxFeePerGas)
    ||tx.gasLimit*tx.maxFeePerGas>BigInt(expected.maxFee)||tx.serialized!==raw)throw Error('TAP refund transaction bindings differ');
  return {hash:tx.hash,nonce:tx.nonce,from:tx.from.toLowerCase(),to:tx.to.toLowerCase(),data:tx.data,
    gas:tx.gasLimit,maxFee:tx.maxFeePerGas,priorityFee:tx.maxPriorityFeePerGas};
}
