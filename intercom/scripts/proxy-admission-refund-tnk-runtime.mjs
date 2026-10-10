// Explicit platform custody/transport setup. Never discover a wallet, generate a
// replacement key, reset a store, or borrow a retail/epoch payout worker.
import fs from 'node:fs';
import path from 'node:path';
import {createPrivateKey,createPublicKey,randomBytes} from 'node:crypto';
import {performance} from 'node:perf_hooks';
import Wallet from 'trac-wallet';
import tracCryptoApi from 'trac-crypto-api';
import {MainSettlementBus} from 'trac-msb/src/index.js';
import {openMsbWithDirectPeers,parseCanonicalMsbDirectPeers} from '../src/msb-direct-peers.js';
import {createLocalConfig} from './msb-local-common.mjs';
import {readCustodyFile} from './proxy-admission-custody.mjs';
import {fixedOrigin,boundedJson,validatePolicy} from './proxy-admission-worker.mjs';
import {need,shape,hex,uint,validateNetwork} from './proxy-admission-wire.mjs';
import {TnkAdmissionRefund} from './proxy-admission-refund-tnk.mjs';

export async function readRefundTnkWallet({format,key_file,password_file,receiver,network_name}) {
  need(['trac_wallet','encrypted_pkcs8'].includes(format)&&['mainnet','testnet1'].includes(network_name),'explicit encrypted TNK custody format required');
  const password=readCustodyFile(password_file,{max:4096}),raw=readCustodyFile(key_file,{max:16384});let plaintext,secret,wallet;
  try {
    let publicKey;
    if(format==='trac_wallet') {
      const c=JSON.parse(raw.toString('utf8'));shape(c,['salt','nonce','ciphertext']);
      need(typeof c.salt==='string'&&/^[0-9a-f]{32}$/.test(c.salt)&&typeof c.nonce==='string'&&/^[0-9a-f]{48}$/.test(c.nonce)
        &&typeof c.ciphertext==='string'&&/^(?:[0-9a-f]{2}){16,}$/.test(c.ciphertext),'invalid encrypted TNK keystore');
      plaintext=tracCryptoApi.data.decrypt(Object.fromEntries(Object.entries(c).map(([k,v])=>[k,Buffer.from(v,'hex')])),password);
      const k=JSON.parse(plaintext.toString('utf8'));need(hex(k.publicKey)&&typeof k.secretKey==='string'&&/^[0-9a-f]{128}$/.test(k.secretKey),'invalid TNK custody key');
      publicKey=Buffer.from(k.publicKey,'hex');secret=Buffer.from(k.secretKey,'hex');
    } else {
      need(raw.toString('ascii',0,40).startsWith('-----BEGIN ENCRYPTED PRIVATE KEY-----'),'encrypted PKCS8 required');
      const k=createPrivateKey({key:raw,format:'pem',passphrase:password});need(k.asymmetricKeyType==='ed25519','TNK custody key must be Ed25519');
      publicKey=Buffer.from(createPublicKey(k).export({format:'jwk'}).x,'base64url');
      const seed=Buffer.from(k.export({format:'jwk'}).d,'base64url');try{secret=Buffer.concat([seed,publicKey]);}finally{seed.fill(0);}
    }
    wallet=await Wallet.fromKeyPair({publicKey,secretKey:Buffer.from(secret)},network_name==='mainnet'?'trac':'testtrac');
    need(wallet.address===receiver,'TNK configured receiver differs from custody key');
    const challenge=Buffer.from('mayhem/proxy/admission-refund-custody/v1');
    need(Wallet.verify(wallet.sign(challenge),challenge,publicKey),'TNK custody key pair differs');
    return wallet;
  } catch(error){wallet?.secretKey?.fill(0);throw error;}
  finally {password.fill(0);raw.fill(0);plaintext?.fill(0);secret?.fill(0);}
}
export function canonicalRefundMsbReader({coreOrigin,network,allowLoopbackHttp=false,fetcher=fetch}) {
  validateNetwork(network);const origin=fixedOrigin(coreOrigin,{allowLoopbackHttp});
  return async signal=>{
    signal.throwIfAborted();
    const nonce=randomBytes(32).toString('hex'),start=performance.now();
    const result=await boundedJson(`${origin}/v1/proxy/admission-policy`,{body:{request_nonce:nonce,msb_frontier:true},signal,fetcher,maxBytes:8192});
    signal.throwIfAborted();need(performance.now()-start<=15000,'canonical refund observation expired');
    // Read canonical identity only. Disabling new admissions must not disable
    // previously approved returns. No admission permit is issued here.
    return validatePolicy(result,network,nonce,null,true).msb_snapshot;
  };
}
export async function openTnkRefundRuntime(c,{policy,key,journalRoot,allowLoopbackHttp=false}) {
  shape(c,['network','network_name','core_origin','state_dir','channel','dht_bootstrap','direct_peers','format','key_file','password_file','receiver','finality','reader_timeout_seconds']);
  validateNetwork(c.network);
  need(['mainnet','testnet1'].includes(c.network_name)&&typeof c.state_dir==='string'&&path.isAbsolute(c.state_dir)&&fs.realpathSync(c.state_dir)===path.resolve(c.state_dir),
    'explicit canonical TNK store directory required');
  const stat=fs.lstatSync(c.state_dir);
  need(process.platform!=='win32'&&stat.isDirectory()&&stat.uid===process.getuid()&&(stat.mode&0o077)===0,'protected TNK store required');
  need(typeof c.channel==='string'&&c.channel.length>0&&c.channel.length<=256&&Array.isArray(c.dht_bootstrap)&&c.dht_bootstrap.length<=16
    &&c.dht_bootstrap.every(v=>typeof v==='string'&&v.length>0&&v.length<=256)&&uint(c.finality,1)&&uint(c.reader_timeout_seconds,1)&&c.reader_timeout_seconds<=10,'invalid bounded TNK transport');
  const directPeers=parseCanonicalMsbDirectPeers(c.direct_peers);
  const canonicalFrontier=canonicalRefundMsbReader({coreOrigin:c.core_origin,network:c.network,allowLoopbackHttp});
  const config=createLocalConfig({network:c.network_name,stateDir:c.state_dir,storeName:'proxy-admission-refunds',channel:c.channel,
    bootstrap:c.network.msb_bootstrap,dhtBootstrap:c.dht_bootstrap,enableWallet:true});
  need(String(config.networkId)===c.network.network_id,'TNK named network identity differs');
  const wallet=await readRefundTnkWallet(c),msb=new MainSettlementBus(config,wallet);
  const close=async()=>{try{await msb.close();}finally{wallet.secretKey.fill(0);}};
  try {
    const adapter=new TnkAdmissionRefund({msb,network:c.network,networkName:c.network_name,canonicalFrontier,policy,key,journalRoot,
      finality:c.finality,timeoutSeconds:c.reader_timeout_seconds});
    await openMsbWithDirectPeers(msb,{directPeers,timeoutSeconds:c.reader_timeout_seconds});
    return {adapter,close};
  } catch(error){await close();throw error;}
}
