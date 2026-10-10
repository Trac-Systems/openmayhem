(() => {
  'use strict';
  const el = id => document.getElementById(id);
  const base = '/mayhem/dashboard/provider/setup';
  let csrf = '', ready = false, reviewed = null, familyCursor = null, marketCursor = null;
  const markets = new Map();
  const message = text => { el('create-message').textContent = text; };
  const value = id => el(id).value.trim();
  const number = id => { const n = Number(value(id)); if (!value(id) || !Number.isSafeInteger(n) || n < 0) throw new Error('Enter a whole number for every allowance and limit.'); return n; };
  const yes = id => { if (!['yes','no'].includes(value(id))) throw new Error('Choose each policy explicitly.'); return value(id) === 'yes'; };
  const option = (select, val, text) => { const o = document.createElement('option'); o.value=val; o.textContent=text; select.append(o); };
  const steps = [...document.querySelectorAll('[data-setup-step]')];
  let activeStep = 0;
  function showStep(index, focus = true) {
    activeStep = index;
    steps.forEach((step, i) => { step.hidden = i !== index; });
    document.querySelectorAll('[data-step]').forEach(button => {
      if (Number(button.dataset.step) === index) button.setAttribute('aria-current','step');
      else button.removeAttribute('aria-current');
    });
    el('previous-step').hidden = index === 0;
    el('next-step').hidden = index === steps.length - 1;
    el('review-button').hidden = index !== steps.length - 1;
    el('step-status').textContent = `Step ${index + 1} of ${steps.length}`;
    if (focus) { const heading = steps[index].querySelector('h2'); heading.focus(); heading.scrollIntoView({block:'nearest'}); }
  }
  function validateThrough(last) {
    for (let i = 0; i <= last; i++) {
      const invalid = [...steps[i].querySelectorAll('input,select,textarea')].find(control => control.willValidate && !control.checkValidity());
      if (invalid) { showStep(i); invalid.focus(); invalid.reportValidity(); return false; }
    }
    return true;
  }
  function advance(index) {
    if (index <= activeStep || validateThrough(index - 1)) showStep(index);
  }
  document.querySelectorAll('[data-step]').forEach(button => button.addEventListener('click',()=>advance(Number(button.dataset.step))));
  el('previous-step').addEventListener('click',()=>showStep(Math.max(0,activeStep - 1)));
  el('next-step').addEventListener('click',()=>advance(Math.min(steps.length - 1,activeStep + 1)));
  showStep(0, false);
  document.querySelectorAll('.yes-no').forEach(s => { option(s,'','Choose'); option(s,'no','No'); option(s,'yes','Yes'); });
  const decisions = () => value('endpoint') === 'mayhem_decisions';
  const units = () => decisions() ? ['decision'] : ['input_token','output_token'];
  function rates() {
    el('rate-fields').replaceChildren();
    if (!value('endpoint')) return;
    units().forEach(unit => {
      [['granularity', `${({input_token:'Input',output_token:'Output',decision:'Decisions'})[unit]}: billing units per price`, 'number'],['rate', `${({input_token:'Input',output_token:'Output',decision:'Decisions'})[unit]}: USD for those units`, 'text']].forEach(([suffix,label,type]) => {
        const l=document.createElement('label'); l.textContent=label;
        const input=document.createElement('input'); input.id=`${unit}-${suffix}`; input.required=true; input.type=type;
        if (type==='number') { input.min='1'; input.max='9007199254740991'; input.value=decisions()?'1':'1000000'; } else { input.inputMode='decimal'; }
        l.append(input); el('rate-fields').append(l);
      });
    });
    el('outcome-label').hidden=!decisions();
    el('tokenizer').disabled=decisions();
    if (decisions()) el('tokenizer').value='none'; else if (value('tokenizer')==='none') el('tokenizer').value='';
  }
  function collect(amounts, sequence) {
    const rails=value('rails').split(',').map(s=>s.trim()).sort();
    if (!rails.length || rails.some((r,i)=>!['fiat','tap','tnk'].includes(r)||rails.indexOf(r)!==i)) throw new Error('Choose distinct fiat, tnk or tap rails.');
    const auth=value('credential');
    const credential=auth==='none'?{kind:'none'}:auth==='write'?{kind:'bearer_value',value:el('api-key').value}:{kind:'reference',id:auth.slice(4)};
    if (auth==='write' && !credential.value.trim()) throw new Error('Enter the bearer key.');
    const network_policy=value('network')==='public'?{mode:'public_https'}:{mode:'pinned',networks:value('networks').split(',').map(s=>s.trim()),allow_http:yes('allow-http')};
    const context=number('context');
    const payable_outcomes=['complete']; ['cancelled','partial','refused'].forEach(o=>{if(yes(o))payable_outcomes.push(o);});payable_outcomes.sort();
    const policy={schema_version:1,lane:'proxy',payable_outcomes,allow_checkpoints:yes('checkpoints')};
    if(value('hold-expiry')!=='none') policy.hold_expiry=value('hold-expiry');
    const prices=amounts.rates;
    const market=value('market-action')==='join'?{action:'join_market',market:markets.get(value('market-picker'))}:{action:'create_market',slug:value('slug'),model:{family_id:value('family'),model_id:value('public-model'),revision:value('model-revision'),quantization:value('quantization')}};
    if(market.action==='join_market'&&!market.market)throw new Error('Choose a compatible canonical market.');
    return {schema_version:1,base_url:value('base-url').replace(/\/?$/,'/'),network_policy,credential,
      endpoint:value('endpoint'),upstream_model:value('model'),market,
      served_context:context,concurrency:number('concurrency'),accepted_rails:rails,sequence,
      offers:[{revision:1,ctx_bracket:`ctx${context}`,outcome_class:decisions()?value('outcome-class'):'',rates:prices,per_request_au:amounts.per_request_au,min_session_au:amounts.min_session_au,accepted_rails:rails}],
      settlement_policy:policy,probe_budget:{max_attempts:number('attempts'),max_cost_microusd:amounts.max_cost_microusd,per_attempt_cost_microusd:amounts.per_attempt_cost_microusd},
      probe_output_limit:number('output'),probe_timeout_ms:number('timeout'),allow_recovery_probes:yes('recovery'),tokenizer_id:decisions()?'none':value('tokenizer'),closed_retention_ms:number('retention')*86400000};
  }
  el('endpoint').addEventListener('change',rates);
  el('network').addEventListener('change',()=>{el('pinned-fields').hidden=value('network')!=='pinned';el('networks').required=value('network')==='pinned';el('allow-http').required=value('network')==='pinned';});
  el('credential').addEventListener('change',()=>{el('key-label').hidden=value('credential')!=='write';el('api-key').required=value('credential')==='write';if(value('credential')!=='write')el('api-key').value='';});
  el('create-form').addEventListener('input',()=>{el('create-review').hidden=true;ready=false;reviewed=null;});
  el('create-form').addEventListener('submit',async event=>{
    event.preventDefault();
    if (activeStep < steps.length - 1) { advance(activeStep + 1); return; }
    if (!validateThrough(steps.length - 1)) return;
    el('create-fields').disabled=true;
    try {
      const amounts=(await guide({kind:'amounts',rates:units().map(unit=>({unit,granularity:number(`${unit}-granularity`),usd:value(`${unit}-rate`)})),per_request_usd:value('per-request'),min_session_usd:value('min-session'),probe_total_usd:value('cost'),probe_per_attempt_usd:value('attempt-cost')})).result;
      const sequence=(await guide({kind:'sequence'})).result.sequence;
      const c=collect(amounts,sequence); reviewed=c;
      if(!c.tokenizer_id)throw new Error('An approved tokenizer is required. Restart the local gateway with --proxy-setup-tokenizer-file pointing to protected approved data.');
      const lines=[`${c.endpoint} · ${c.upstream_model}`,c.market.action==='join_market'?`Join market ${c.market.market.slug} / canonical family ${c.market.market.model.family_id}`:`Create market ${c.market.slug} / canonical family ${c.market.model.family_id}`,`Context ${c.served_context}, shared concurrency ${c.concurrency}; rails ${c.accepted_rails.join(', ')}`,
        ...c.offers[0].rates.map(r=>`USD ${value(`${r.unit}-rate`)} = ${r.per_unit_au} AU per ${r.granularity} ${r.unit} units`),`${c.offers[0].per_request_au} AU per request; ${c.offers[0].min_session_au} AU minimum session`,
        `Payable outcomes: ${c.settlement_policy.payable_outcomes.join(', ')}; checkpoints ${c.settlement_policy.allow_checkpoints?'allowed':'not allowed'}; expiry ${value('hold-expiry')}`,
        `Probe allowance: ${c.probe_budget.max_attempts} attempts, ${c.probe_budget.max_cost_microusd} micro-USD total, ${c.probe_budget.per_attempt_cost_microusd} per attempt; output ${c.probe_output_limit}, deadline ${c.probe_timeout_ms} ms`,
        `Recovery probes ${c.allow_recovery_probes?'allowed':'not allowed'}; local journal ${value('retention')} days; sequence ${c.sequence}`,`Authentication: ${value('credential')==='write'?'write-only bearer key':value('credential')}; tokenizer ${c.tokenizer_id}`];
      el('review-list').replaceChildren();lines.forEach(text=>{const li=document.createElement('li');li.textContent=text;el('review-list').append(li);});
      ready=true;el('create-review').hidden=false;message('Review the exact choices, then save.');el('create-review').scrollIntoView({block:'nearest'});
    } catch(e){message(e.message);ready=false;reviewed=null;}
    finally{el('create-fields').disabled=false;}
  });
  el('edit-button').addEventListener('click',()=>{ready=false;reviewed=null;el('create-review').hidden=true;showStep(0);el('base-url').focus();});
  el('create-button').addEventListener('click',async()=>{
    if(!ready||!validateThrough(steps.length - 1))return;
    el('create-button').disabled=true;el('create-fields').disabled=true;
    try {
      const payload=JSON.stringify(reviewed);reviewed=null;el('api-key').value='';
      const response=await fetch(`${base}/bootstrap`,{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json','x-mayhem-setup-csrf':csrf},body:payload});
      const data=await response.json();
      if(response.ok){location.replace(base);return;}
      if(data.error==='setup_original_exists'){message('An original setup is already saved. Reloading it without replacing any choices.');location.replace(base);return;}
      throw new Error(data.error==='setup_market_exists_choose_join'?'This exact market already exists. Choose Join, select it and review again. No duplicate market or fee is created.':data.error==='setup_selection_changed_or_unavailable'?'The canonical selection or sequence changed, or its source is unavailable. Refresh the family/market selection and review again. No new setup was saved.':data.error==='setup_create_failed_inspect_original'?'Setup could not be confirmed. Reload to recover a saved original; otherwise check the protected host prerequisites. No bundle is replaced.':data.error||'Setup unavailable');
    }catch(e){message(`${e.message}. If the response was lost, reload before retrying. Re-enter a write-only key if no setup was saved.`);ready=false;el('create-review').hidden=true;}
    finally{el('create-button').disabled=false;el('create-fields').disabled=false;}
  });
  async function guide(body) {
    const response=await fetch(`${base}/bootstrap/guide`,{method:'POST',credentials:'same-origin',cache:'no-store',headers:{'Content-Type':'application/json','x-mayhem-setup-csrf':csrf},body:JSON.stringify(body)});
    const data=await response.json();
    if(!response.ok)throw new Error(data.error==='setup_busy'?'Setup is busy. Retry this read when the current step finishes.':data.error==='setup_invalid_exact_amount'?'Enter exact nonnegative decimal USD amounts: prices up to 18 decimals; probe allowances up to 6. No rounding is performed.':data.error==='setup_canonical_unavailable'?'Canonical discovery is unavailable or its page expired. Reload this list; no empty catalog is assumed.':data.error||'Setup read unavailable');
    return data;
  }
  const networkPolicy=()=>value('network')==='public'?{mode:'public_https'}:{mode:'pinned',networks:value('networks').split(',').map(s=>s.trim()),allow_http:yes('allow-http')};
  function credentialInput(){const auth=value('credential');if(!auth)throw new Error('Choose authentication first.');return auth==='none'?{kind:'none'}:auth==='write'?{kind:'bearer_value',value:el('api-key').value}:{kind:'reference',id:auth.slice(4)};}
  el('preview-models').addEventListener('click',async()=>{
    el('preview-models').disabled=true;
    try {
      if(!value('base-url')||!value('network'))throw new Error('Choose the API URL and network access first.');
      const credential=credentialInput();
      const data=await guide({kind:'models',base_url:value('base-url').replace(/\/?$/,'/'),network_policy:networkPolicy(),credential});
      const p=data.preview;el('model-picker').replaceChildren();option(el('model-picker'),'','Choose a discovered model');
      p.model_ids.forEach(id=>option(el('model-picker'),id,id));
      el('models-status').textContent=p.state==='listed'?`${p.model_ids.length} model IDs returned${p.truncated?' (bounded list; additional models may exist)':''}. Identity and capabilities are unverified.`:`Listing ${p.state}. You may enter the exact model ID manually; no probe was run.`;
    }catch(e){el('models-status').textContent=e.message;}
    finally{el('preview-models').disabled=false;}
  });
  el('model-picker').addEventListener('change',()=>{el('model').value=value('model-picker');if(!value('public-model'))el('public-model').value=value('model-picker');ready=false;reviewed=null;el('create-review').hidden=true;});
  function clearMarkets(){markets.clear();marketCursor=null;el('market-picker').replaceChildren();option(el('market-picker'),'','Load and choose a market');el('more-markets').hidden=true;ready=false;reviewed=null;el('create-review').hidden=true;}
  el('family').addEventListener('change',clearMarkets);el('endpoint').addEventListener('change',clearMarkets);
  el('market-action').addEventListener('change',()=>{const join=value('market-action')==='join';el('join-market').hidden=!join;el('new-market').hidden=join||!value('market-action');el('new-market').disabled=join;el('market-picker').required=join;['public-model','slug'].forEach(id=>{el(id).required=!join&&!!value('market-action');});});
  async function families(next){
    el('load-families').disabled=true;el('more-families').disabled=true;
    try{const {page}=(await guide({kind:'catalog',browse:{kind:'families',cursor:next?familyCursor:null}})).result;
      el('family').replaceChildren();option(el('family'),'','Choose a canonical family');clearMarkets();
      page.entries.forEach(e=>{const id=e.key.split('/').pop();option(el('family'),id,`${e.value.label}${e.value.enabled?'':' (disabled)'}`);el('family').lastElementChild.disabled=!e.value.enabled;});
      familyCursor=page.next_cursor;el('more-families').hidden=!familyCursor;el('families-status').textContent=familyCursor?'More families are available on the next page.':'End of this canonical family list.';
    }catch(e){el('families-status').textContent=e.message;}finally{el('load-families').disabled=false;el('more-families').disabled=false;}
  }
  async function loadMarkets(next){
    el('load-markets').disabled=true;el('more-markets').disabled=true;
    try{if(!value('family')||!value('endpoint'))throw new Error('Choose a family and endpoint first.');
      const {page,compatible_market_ids:compatible}=(await guide({kind:'catalog',browse:{kind:'markets',family_id:value('family'),endpoint:value('endpoint'),cursor:next?marketCursor:null}})).result;
      markets.clear();el('market-picker').replaceChildren();option(el('market-picker'),'','Choose a compatible market');
      page.entries.forEach(e=>{const id=e.key.split('/').pop();if(compatible.includes(id)){markets.set(id,e.value);option(el('market-picker'),id,`${e.value.model.model_id} · ${e.value.slug} · ${id.slice(0,12)}…`);}});
      marketCursor=page.next_cursor;el('more-markets').hidden=!marketCursor;el('markets-status').textContent=`${markets.size} compatible markets on this page. ${marketCursor?'Continue to see later matches.':'End of this family scope; create a market if none fits your declaration.'} Market identity is separate from current capacity.`;
    }catch(e){el('markets-status').textContent=e.message;}finally{el('load-markets').disabled=false;el('more-markets').disabled=false;}
  }
  el('load-families').addEventListener('click',()=>families(false));el('more-families').addEventListener('click',()=>families(true));
  el('load-markets').addEventListener('click',()=>loadMarkets(false));el('more-markets').addEventListener('click',()=>loadMarkets(true));
  async function loadState() {
    el('retry-state').disabled=true;el('create-fields').disabled=true;
    try {
      const r=await fetch(`${base}/state`,{credentials:'same-origin',cache:'no-store'});
      const d=await r.json();
      if(!r.ok) {
        if(d.error==='setup_busy') throw new Error('Setup is being saved or restored. Your session and entered choices are retained. Retry setup status in a moment.');
        if(r.status===401) throw new Error('Unlock the existing local dashboard session, then retry setup status.');
        throw new Error('Setup status is unavailable. Retry this read; no setup is created or replaced.');
      }
      if(d.view){location.replace(base);return;}
      if(!d.bootstrap)throw new Error('First-time setup is not configured on this host.');
      csrf=d.csrf;
      const b=d.bootstrap;el('host-summary').textContent=`Network ${b.network.network_id}. Uses your already loaded wallet. Admission ${b.admission_configured?'is configured':'requires a trusted host configuration before enrollment'}.`;
      const selectedTokenizer=value('tokenizer');
      el('tokenizer').replaceChildren();option(el('tokenizer'),'','Choose a host-approved asset');option(el('tokenizer'),'none','Not applicable — decisions only');
      b.tokenizers.forEach(t=>option(el('tokenizer'),t.id,`${t.id} · ${t.digest.slice(0,12)}…`));
      el('tokenizer').value=selectedTokenizer;
      const selectedCredential=value('credential');
      [...el('credential').options].filter(o=>o.value.startsWith('ref:')).forEach(o=>o.remove());
      b.credential_references.forEach(id=>option(el('credential'),`ref:${id}`,`Protected reference: ${id}`));
      el('credential').value=selectedCredential;
      if(!b.tokenizers.length)el('tokenizer-hint').textContent='No approved tokenizer is configured. For LLMs, the host must supply --proxy-setup-tokenizer-file. Decisions needs no tokenizer.';
      el('create-fields').disabled=false;el('retry-state').hidden=true;message('');
    }catch(e){message(e.message);el('retry-state').hidden=false;}
    finally{el('retry-state').disabled=false;}
  }
  el('retry-state').addEventListener('click',loadState);
  loadState();
})();
