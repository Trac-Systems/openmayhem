// Ephemeral local EVM + actual custody/RPC/worker + SITE HTTP acceptance. Public
// addresses only on stdout; test keys stay on stdin or protected temporary files.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
import {generateKeyPairSync,createPrivateKey,randomBytes} from 'node:crypto';
import Ganache from 'ganache';
import solc from 'solc';
import {ethers} from 'ethers';
import {tapRefundSigner} from '../../../intercom/scripts/proxy-admission-refund-tap-transaction.mjs';
import {openTapRefundRuntime} from '../../../intercom/scripts/proxy-admission-refund-tap-runtime.mjs';
import {AdmissionRefundWorker,RefundApi,main} from '../../../intercom/scripts/proxy-admission-refund-worker.mjs';
const root=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'refund-tap-evm-')));fs.chmodSync(root,0o700);
let server,provider;
const save=(name,value)=>{const file=path.join(root,name);fs.writeFileSync(file,value,{mode:0o600});return file;};
try {
 const key=generateKeyPairSync('ec',{namedCurve:'secp256k1'}).privateKey,raw=Buffer.from(key.export({format:'jwk'}).d,'base64url'),signer=tapRefundSigner(raw);raw.fill(0);
 const receiver=signer.address.toLowerCase();
 server=Ganache.server({logging:{quiet:true},chain:{chainId:31337,hardfork:'shanghai'},wallet:{accounts:[{secretKey:signer.privateKey,balance:ethers.toBeHex(10n**20n)}]}});
 await server.listen(0,'127.0.0.1');const rpcUrl=`http://127.0.0.1:${server.address().port}`;
 provider=new ethers.JsonRpcProvider(rpcUrl,31337,{staticNetwork:true});
 const source='pragma solidity ^0.8.20; contract ReturnToken { mapping(address => uint256) public balanceOf; event Transfer(address indexed from,address indexed to,uint256 value); constructor(){balanceOf[msg.sender]=100;} function transfer(address to,uint256 n) external returns(bool){require(balanceOf[msg.sender]>=n);balanceOf[msg.sender]-=n;balanceOf[to]+=n;emit Transfer(msg.sender,to,n);return true;} }';
 const compiled=JSON.parse(solc.compile(JSON.stringify({language:'Solidity',sources:{'T.sol':{content:source}},settings:{evmVersion:'shanghai',outputSelection:{'*':{'*':['abi','evm.bytecode.object']}}}})));
 assert(!compiled.errors?.some(e=>e.severity==='error'));const artifact=compiled.contracts['T.sol'].ReturnToken;
 const contract=await new ethers.ContractFactory(artifact.abi,artifact.evm.bytecode.object,signer.connect(provider)).deploy();await contract.waitForDeployment();
 const token=(await contract.getAddress()).toLowerCase();
 console.log(JSON.stringify({receiver,token,chain_id:31337}));
 const chunks=[];let size=0;for await(const c of process.stdin){size+=c.length;if(size>32768)throw Error('fixture input too large');chunks.push(c);}
 const input=JSON.parse(Buffer.concat(chunks).toString('utf8')),origin=new URL(input.origin);
 assert.equal(origin.hostname,'127.0.0.1');assert.equal(origin.protocol,'http:');assert.equal(origin.pathname,'/');assert(!origin.username&&!origin.password);
 const password=randomBytes(32),executor=createPrivateKey(input.key);
 const c={chain_id:31337,token_contract:token,receiver,
  key_file:save('wallet.pem',key.export({format:'pem',type:'pkcs8',cipher:'aes-256-cbc',passphrase:password})),password_file:save('password',password),
  rpc_urls_file:save('rpc.json',JSON.stringify([rpcUrl])),max_gas:'100000',max_fee_per_gas:'20000000000',priority_fee:'1000000000',max_fee:'2000000000000000'};
 const runtime=openTapRefundRuntime(c,{policy:input.policy,key:executor,journalRoot:root,allowLoopbackHttp:true});
 const api=new RefundApi({origin:origin.origin,credential:input.credential,allowLoopbackHttp:true}),post=api.post.bind(api);
 let work,preparation,grant,proof,done;
 api.post=async(action,body,signal)=>{const r=await post(action,body,signal);
  if(action==='pull')work=r.work;if(action==='dispatch'){preparation=body.preparation;grant=r;}if(action==='delivered'){proof=body.delivery;done=r;}return r;};
 let sends=0;const rpc=runtime.adapter.o.rpc;
 runtime.adapter.o.rpc=async(m,p,s)=>{const r=await rpc(m,p,s);if(m==='eth_sendRawTransaction')sends++;return r;};
 const outcome=await new AdmissionRefundWorker({api,adapters:{tap:runtime.adapter},timeoutMs:15000}).once();
 assert.equal(outcome.outcome,'delivered');assert.equal(sends,1);assert.equal(await contract.balanceOf(receiver),90n);
 assert.equal(await contract.balanceOf(work.authorization.body.destination.address),10n);
 const returned=await provider.send('eth_getTransactionReceipt',[proof.body.receipt.transaction_hash]);
 assert(BigInt(returned.gasUsed)*BigInt(returned.effectiveGasPrice)>0n,'custody paid gas');
 await provider.send('evm_mine',[]);
 const replayProof=await runtime.adapter.execute({...work,action:'reconcile',preparation,first_dispatch_at_ms:grant.first_dispatch_at_ms},{...grant,action:'reconcile'},AbortSignal.timeout(5000));
 assert.deepEqual(replayProof,proof);assert.equal(sends,1);
 const replay=await post('delivered',{refund_id:work.refund_id,lease_token:work.lease_token,delivery:replayProof},AbortSignal.timeout(5000));
 const config={api_origin:origin.origin,api_credential_file:save('api',input.credential),policy_file:save('policy',JSON.stringify(input.policy)),
  executor_key_file:save('executor.pem',executor.export({format:'pem',type:'pkcs8',cipher:'aes-256-cbc',passphrase:password})),executor_password_file:c.password_file,
  journal_root:root,stripe:null,tap:c,timeout_ms:15000,poll_ms:1000,mode:'once',allow_loopback_http:true};
 // Real entry point validates TAP-only configuration and safely observes idle.
 await main({PROXY_ADMISSION_REFUND_WORKER_ENABLED:'1',PROXY_ADMISSION_REFUND_WORKER_CONFIG:save('config',JSON.stringify(config))});
 console.log(JSON.stringify({broadcasts:sends,transaction_hash:proof.body.receipt.transaction_hash,log_index:proof.body.receipt.log_index,
  before_balance:'100',after_balance:String(await contract.balanceOf(receiver)),destination_balance:String(await contract.balanceOf(work.authorization.body.destination.address)),done,replay}));
} finally {provider?.destroy();await server?.close();fs.rmSync(root,{recursive:true,force:true});}
