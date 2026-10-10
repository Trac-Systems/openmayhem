// Actual dashboard assets; controlled HTTP state/actions. API authorization and
// renewal are exercised separately by the connected SITE/Rust acceptance suite.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
const output = process.argv[2];
assert(output && process.env.MAYHEM_PLAYWRIGHT_MODULE);
const { chromium } = createRequire(import.meta.url)(process.env.MAYHEM_PLAYWRIGHT_MODULE);
const html = await readFile(new URL('../src/openai/proxy_setup.html', import.meta.url), 'utf8');
const script = await readFile(new URL('../src/openai/proxy_setup.js', import.meta.url), 'utf8');
const origin = 'http://127.0.0.1:45871', id = '11'.repeat(32), commitment = '22'.repeat(32);
const view = { endpoint:'openai_chat_completions', connection:{id:'fixture',revision:1},steps:[],
  selection:{upstream_model:'fixture',market:{action:'join_market'},membership:{revision:1,served_context:4096,max_concurrency:1,accepted_rails:['fiat']},offers:[]},
  review:{revision:2,draft_id:id},capabilities:{enrollment:true},enrollment:null };
const original = { invoice_id:id,invoice_commitment:commitment,payment_status:'awaiting_payment',rail:'fiat',fee_usd:'10.00',
  amount_base_units:'1000',received_amount_base_units:'0',missing_amount_base_units:'1000',excess_amount_base_units:'0',
  quote_expired:true,quote_expires_at_ms:Date.now()-60000,review_code:null,collection:{currency:'usd'},permit:null };
const actions=[],errors=[]; let returnError=false;
const browser=await chromium.launch({headless:true}),page=await browser.newPage({viewport:{width:1280,height:900}});
page.on('pageerror',e=>errors.push(e.message));page.on('dialog',d=>d.accept());
await page.route('**/*',async route=>{
  const request=route.request(),url=new URL(request.url());if(url.origin!==origin)return route.abort();
  if(url.pathname==='/mayhem/dashboard/assets/proxy-setup.js')return route.fulfill({contentType:'text/javascript',body:script});
  if(url.pathname==='/mayhem/dashboard/provider/setup/state')return route.fulfill({json:{csrf:'fixture-csrf',view}});
  if(url.pathname==='/mayhem/dashboard/provider/setup/action'){
    assert.equal(request.headers()['x-mayhem-setup-csrf'],'fixture-csrf');
    const a=request.postDataJSON();actions.push(a);
    if(a.action==='admission_returns') {
      assert.equal(a.expected_revision,2);assert([null,'last_return'].includes(a.after));
      if(returnError)return route.fulfill({status:503,json:{error:'Return status temporarily unavailable'}});
      const entries=a.after ? [] : [
        {rail:'fiat',amount_base_units:'125',state:'returned',reason:'returned',destination:{kind:'original_payment_method',currency:'eur'},completed_at_ms:Date.now(),action_required:'none'},
        {rail:'tnk',amount_base_units:'10000000',state:'queued',reason:'funding_required',destination:{kind:'crypto_address',address:'testtrac1'+'a'.repeat(60)},next_attempt_at_ms:Date.now()+30000,action_required:'operator'},
        {rail:'tap',amount_base_units:'10000000000000000000',state:'review',reason:'approval_expired',destination:{kind:'crypto_address',address:'0x'+'b'.repeat(40)},action_required:'operator'},
        {rail:'tap',amount_base_units:'1000000',state:'confirming',reason:'confirming',destination:{kind:'crypto_address',address:'0x'+'c'.repeat(40)},action_required:'none'}
      ].map((e,n)=>({...e,refund_id:'return_'+n+'a'.repeat(100),invoice_id:'invoice_'+'b'.repeat(100)}));
      return route.fulfill({json:{view,action_result:{state:'returns',authorizes_publication:false,return_page:{entries,next_cursor:a.after?null:'last_return',observed_at_ms:Date.now()}}}});
    }
    assert.deepEqual(a,{action:'enrollment',expected_revision:2,operation:'refresh',rail:null,quote:{invoice_id:id,invoice_commitment:commitment}});
    view.enrollment.invoice={...view.enrollment.invoice,invoice_id:'33'.repeat(32),invoice_commitment:'44'.repeat(32),quote_expired:false};
    return route.fulfill({json:{view,action_result:{state:'awaiting_payment',invoice:view.enrollment.invoice,authorizes_publication:false}}});
  }
  if(url.pathname==='/')return route.fulfill({contentType:'text/html',body:`<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><style>body{margin:0;font-family:system-ui;background:#111217;color:#eee}a{color:#aabaff}.panel{border:1px solid #363840;border-radius:10px;margin:16px 0;padding:16px}input,select,button{font:inherit;box-sizing:border-box}button{padding:8px;white-space:normal}input,select{padding:6px}fieldset{border:1px solid #454751}pre{font-size:12px}</style>${html}`});
  return route.abort();
});
const ready=async()=>{await page.goto(origin);await page.waitForFunction(()=>document.querySelector('#identity').textContent.includes('fixture'));};
try {
  await mkdir(output,{recursive:true});
  const button=page.locator('[data-action="invoice_refresh"]');
  await ready();assert(await button.isDisabled());
  for(const mutation of [{quote_expired:false},{invoice_commitment:null},{payment_status:'short_payment',received_amount_base_units:'1'},
    {payment_status:'confirming'},{review_code:'prior_quote_payment_review'}]){
    view.enrollment={for_current_revision:true,invoice:{...original,...mutation}};await ready();assert(await button.isDisabled());
  }
  view.enrollment={for_current_revision:false,invoice:{...original}};await ready();assert(await button.isDisabled());
  for(const rail of ['fiat','tnk','tap']){
    view.enrollment={for_current_revision:true,invoice:{...original,rail}};await ready();assert(!(await button.isDisabled()));
    await button.click();await page.waitForFunction(()=>document.querySelector('#message').textContent==='Retained state refreshed.');
    assert(await button.isDisabled());
  }
  view.enrollment={for_current_revision:true,invoice:{...original}};await ready();await button.scrollIntoViewIfNeeded();
  await page.screenshot({path:path.join(output,'desktop.png')});
  await page.setViewportSize({width:390,height:844});await button.scrollIntoViewIfNeeded();
  assert(!(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1)));
  await page.screenshot({path:path.join(output,'mobile.png')});
  assert.equal(actions.length,3);
  await page.setViewportSize({width:1280,height:900});
  await page.getByText('Admission fee returns',{exact:true}).click();
  await page.getByRole('button',{name:'Check returns',exact:true}).click();
  await page.locator('#returns').filter({hasText:'Platform funding required'}).waitFor();
  assert.match(await page.locator('#returns').textContent(),/Operator approval must be renewed/);
  assert.match(await page.locator('#returns').textContent(),/Return confirmed/);
  assert.match(await page.locator('#returns').textContent(),/Checking the original transfer/);
  assert.match(await page.locator('#returns').textContent(),/Bank posting may take longer/);
  assert(!(await page.getByRole('button',{name:'More returns',exact:true}).isDisabled()));
  await page.locator('#returns').scrollIntoViewIfNeeded();await page.screenshot({path:path.join(output,'returns-desktop.png')});
  await page.setViewportSize({width:390,height:844});await page.locator('#returns').scrollIntoViewIfNeeded();
  assert(!(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1)));
  await page.screenshot({path:path.join(output,'returns-mobile.png')});
  const saved=await page.locator('#returns').textContent();returnError=true;
  await page.getByRole('button',{name:'Check returns',exact:true}).click();await page.locator('#message').filter({hasText:'temporarily unavailable'}).waitFor();
  assert.equal(await page.locator('#returns').textContent(),saved);returnError=false;
  await page.getByRole('button',{name:'More returns',exact:true}).click();await page.locator('#returns').filter({hasText:'No recorded admission returns'}).waitFor();
  assert(await page.getByRole('button',{name:'More returns',exact:true}).isDisabled());
  assert.equal(actions.filter(a=>a.action==='admission_returns').length,3);assert.deepEqual(errors,[]);
  const result={scope:'actual dashboard HTML/JS with controlled HTTP state/actions; no real payment, wallet signature or ledger write',
    rails:['fiat','tnk','tap'],exact_quote_binding:true,unsafe_renewal_disabled:true,reload_readonly:true,returns_all_rails:true,returns_paged:true,returns_error_retains_view:true,mobile_no_overflow:true,errors};
  await writeFile(path.join(output,'result.json'),JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify(result));
} finally {await browser.close();}
