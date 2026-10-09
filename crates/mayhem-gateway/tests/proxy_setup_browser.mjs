// Explicit local coordinator for the ignored Rust fixture. No intercepted APIs.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import path from 'node:path';
const run = promisify(execFile);
const [readyPath, evidenceDirectory] = process.argv.slice(2);
assert(readyPath && evidenceDirectory && process.env.MAYHEM_BINARY && process.env.MAYHEM_PLAYWRIGHT_MODULE);
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.MAYHEM_PLAYWRIGHT_MODULE);
let fixture;
for (let n=0;n<600;n++) {
  try { fixture=JSON.parse(await readFile(readyPath,'utf8')); break; } catch { await new Promise(r=>setTimeout(r,200)); }
}
assert(fixture,'fixture did not become ready');
const origin = new URL(fixture.origin);
assert.equal(origin.hostname,'127.0.0.1'); assert.equal(origin.protocol,'http:');
const browser = await chromium.launch({headless:true});
const failures=[];
const context=await browser.newContext({viewport:{width:1440,height:1000}});
await context.route('**/*', async route => {
  const u=new URL(route.request().url());
  if (u.port===origin.port && ['127.0.0.1','localhost'].includes(u.hostname)) await route.continue();
  else await route.abort();
});
const page=await context.newPage(); page.on('pageerror',e=>failures.push(e.message));
try {
  // Same existing bootstrap session, redirected from localhost before consumption.
  const bootstrap=new URL(fixture.url).searchParams.get('token');
  await page.goto(`http://localhost:${origin.port}/mayhem/dashboard/provider?token=${bootstrap}`);
  await page.getByRole('link',{name:'Open proxy setup wizard'}).click();
  await page.locator('#identity').filter({hasText:'not saved'}).waitFor();
  assert.equal(new URL(page.url()).origin,fixture.origin);
  await page.locator('[data-action="connect"]').click();
  await page.locator('#message').filter({hasText:'Retained state refreshed'}).waitFor();
  await page.locator('#model').fill('manual-private-model');
  await page.locator('[data-action="select"]').click();
  await page.locator('#identity').filter({hasText:'revision 1.'}).waitFor();
  await page.locator('[data-action="check"]').click();
  await page.locator('#identity').filter({hasText:'revision 2.'}).waitFor();
  const cli = async (...args) => {
    const result=await run(process.env.MAYHEM_BINARY,['provider','proxy','setup','wizard','--config',fixture.config,...args],{maxBuffer:1024*1024,timeout:20000});
    return JSON.parse(result.stdout.trim());
  };
  const guided = await new Promise((resolve, reject) => {
    const child=execFile(process.env.MAYHEM_BINARY,['provider','proxy','setup','wizard','--config',fixture.config],{maxBuffer:1024*1024,timeout:20000},(error,stdout)=>error?reject(error):resolve(stdout));
    child.stdin.end('x\n');
  });
  assert.match(guided,/Select\/price/); assert.match(guided,/No automatic payment/);
  const retained=await cli('--inspect'); assert.equal(retained.review.revision,2); assert.equal(retained.selection.upstream_model,'manual-private-model');
  retained.selection.upstream_model='cli-selected-model';
  const action=path.join(evidenceDirectory,'checkpoint93-synthetic-action.json');
  await writeFile(action,JSON.stringify({action:'select',expected_revision:2,choice:retained.selection}),{mode:0o600});
  const changed=await cli('--action-file',action); assert.equal(changed.view.review.revision,3);
  await page.locator('[data-action="refresh"]').click();
  await page.locator('#identity').filter({hasText:'revision 3.'}).waitFor();assert.equal(await page.locator('#model').inputValue(),'cli-selected-model');
  await page.locator('[data-action="check"]').click();await page.locator('#identity').filter({hasText:'revision 4.'}).waitFor();
  assert.match(await page.locator('#review').textContent(),/structurally_valid/);
  assert.match(await page.getByRole('heading',{name:'Run',exact:true}).locator('..').textContent(),/configured and started separately/);
  assert.equal(await page.locator('[data-action="invoice_create"]').isDisabled(),true);
  await page.screenshot({path:path.join(evidenceDirectory,'checkpoint93-wizard-desktop.png'),fullPage:true});
  await page.setViewportSize({width:390,height:844});await page.reload();await page.locator('#identity').filter({hasText:'revision 4.'}).waitFor();
  const dimensions=await page.evaluate(()=>({viewport:innerWidth,body:document.documentElement.scrollWidth}));
  assert(dimensions.body<=dimensions.viewport+1,`mobile overflow: ${JSON.stringify(dimensions)}`);
  await page.screenshot({path:path.join(evidenceDirectory,'checkpoint93-wizard-mobile.png'),fullPage:true});
  assert.deepEqual(failures,[]);
  const evidence={schema_version:1,kind:'actual_local_provider_wizard_acceptance',intercepted_api:false,real_upstream:false,real_payment:false,desktop:{width:1440,height:1000},mobile:{width:390,height:844},checks:['existing bootstrap localhost-to-configured-origin redirect','browser configuration/select/check','actual guided CLI menu resumes and exits','actual CLI reads browser revision','actual CLI typed action changes same retained draft','browser refresh/check sees exact CLI revision','private credentials not loaded','unconfigured financial actions disabled','run explicitly unavailable','no mobile horizontal overflow','no browser runtime errors'],final_revision:4};
  await writeFile(path.join(evidenceDirectory,'checkpoint93-wizard-browser-cli.json'),JSON.stringify(evidence,null,2)+'\n',{mode:0o600});
  console.log(JSON.stringify(evidence));
} finally {
  await browser.close();
  await writeFile(readyPath.replace(/\.[^.]+$/,'.done'),'done\n',{mode:0o600});
}
