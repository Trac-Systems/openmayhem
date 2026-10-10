// Local-only real signing/Hyperbee fixture. No DHT, sockets or funded wallet.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import Corestore from 'corestore';
import Hyperbee from 'hyperbee';
import PeerWallet from 'trac-wallet';
import { safeEncodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { normalizeTransferOperation } from 'trac-msb/src/utils/normalizers.js';
import { bigIntTo16ByteBuffer } from 'trac-msb/src/utils/amountSerialization.js';
import { createMayhemMsbConfig, MAYHEM_NETWORK_ENV } from '../../src/network-config.js';
import { createAdmissionMsbReader } from '../../features/mayhem/proxy-admission-msb.js';

export async function tnkRefundRuntime({ wallet,network,root }) {
  const directory=root??fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'refund-tnk-ledger-')));
  fs.chmodSync(directory,0o700);
  const store=new Corestore(directory),view=new Hyperbee(store.get({name:'signed'}),{keyEncoding:'utf-8',valueEncoding:'binary',extension:false});
  await view.ready();await view.put('initial',Buffer.from('metadata'));
  const config=createMayhemMsbConfig(MAYHEM_NETWORK_ENV.TESTNET1,{bootstrap:network.msb_bootstrap});
  const state={broadcasts:[],dropBefore:false,dropAfter:false,accept:true,pad:3,reads:0,scans:0};
  const originalCheckout=view.checkout.bind(view);
  const base={core:view.core,checkout(length){const snapshot=originalCheckout(length);return {
    core:snapshot.core,async get(hash){state.reads++;return snapshot.get(hash);},
    createHistoryStream(){state.scans++;throw Error('refund must not scan history');},close:()=>snapshot.close(),
  };}};
  const msb={config,wallet,network:{validatorConnectionManager:{connectionCount:()=>2}},state:{base:{view:base},isIndexer:()=>true,
    getSignedLength:()=>view.core.signedLength,getNodeEntry:async()=>({balance:bigIntTo16ByteBuffer(10n**24n)}),
    getFee:()=>bigIntTo16ByteBuffer(10n),getIndexerSequenceState:async()=>Buffer.alloc(32,3).toString('hex')},
    async broadcastPartialTransaction(payload){
      state.broadcasts.push(structuredClone(payload));
      if(state.dropBefore){state.dropBefore=false;throw Error('simulated loss before acceptance');}
      if(!state.accept)return false;
      const parsed=normalizeTransferOperation(payload,config);
      if(!await view.get(payload.tro.tx))await view.put(payload.tro.tx,safeEncodeApplyOperation(parsed));
      for(let i=0;i<state.pad;i++)await view.put(`padding/${view.core.length}`,Buffer.from('metadata'));
      if(state.dropAfter){state.dropAfter=false;throw Error('simulated loss after acceptance');}return true;
    }};
  const read=createAdmissionMsbReader(msb);
  return {msb,state,view,root:directory,frontier:()=>read(network),async close(){await view.close();await store.close();fs.rmSync(directory,{recursive:true,force:true});}};
}
export async function testWallet(){const wallet=new PeerWallet({networkPrefix:'testtrac'});await wallet.ready;await wallet.generateKeyPair();return wallet;}
