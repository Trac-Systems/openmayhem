// Drives the real operator commands against a loopback SITE fixture only.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
import {createPrivateKey} from 'node:crypto';
import {main,RefundReviewApi,prepareRecovery} from '../../scripts/proxy-admission-refund-review.mjs';
const chunks=[];let size=0;for await(const c of process.stdin){size+=c.length;if(size>32768)throw Error('fixture too large');chunks.push(c);}
const input=JSON.parse(Buffer.concat(chunks).toString('utf8')),origin=new URL(input.origin);
assert.equal(origin.hostname,'127.0.0.1');assert.equal(origin.protocol,'http:');assert.equal(origin.pathname,'/');
const root=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'refund-review-fixture-')));fs.chmodSync(root,0o700);
const file=(name,value)=>{const p=path.join(root,name);fs.writeFileSync(p,value,{mode:0o600});return p;};
try {
 const key=createPrivateKey(input.key),password='public-test-encrypted-review-key-only';
 const config=file('config.json',JSON.stringify({api_origin:origin.origin,api_credential_file:file('credential',input.credential),policy_file:file('policy.json',JSON.stringify(input.policy)),
  review_key_file:file('review.pem',key.export({format:'pem',type:'pkcs8',cipher:'aes-256-cbc',passphrase:password})),review_password_file:file('password',password),
  approval_ms:59000,allow_loopback_http:true}));
 const evidence=file('evidence.txt','Isolated fixture operator review; original refund authority and pending state checked.'),output=path.join(root,'approval.json');
 const view=await main(['inspect',config,input.refund_id]);assert.equal(view.state,'review');
 const args={key,policy:input.policy,evidenceHash:'ab'.repeat(32),approvalMs:59000};
 for(const patch of [{state:'delivered'},{authorization_digest:'00'.repeat(32)},{next_revision:2},{preparation_digest:'00'.repeat(32)}])
  await assert.rejects(prepareRecovery({...view,...patch},args));
 const api=new RefundReviewApi({origin:origin.origin,credential:input.credential,allowLoopbackHttp:true});
 await assert.rejects(api.post('dispatch',{}));await assert.rejects(api.post('reserve',{}));
 const prepared=await main(['prepare',config,input.refund_id,evidence,output]);
 assert.equal(prepared.state,'prepared_not_submitted');assert.equal(fs.statSync(output).mode&0o777,0o600);
 assert.equal((await main(['inspect',config,input.refund_id])).state,'review');
 await assert.rejects(main(['prepare',config,input.refund_id,evidence,output]));
 const retained=fs.readFileSync(output,'utf8');
 const done=await main(['submit',config,output]),replay=await main(['submit',config,output]);
 assert.equal(done.replayed,false);assert.equal(replay.replayed,true);assert.equal(fs.readFileSync(output,'utf8'),retained);
 console.log(JSON.stringify({done,replay,review_unchanged_until_submit:true,approval_file_protected:true,execution_actions_rejected:true}));
}finally{fs.rmSync(root,{recursive:true,force:true});}
