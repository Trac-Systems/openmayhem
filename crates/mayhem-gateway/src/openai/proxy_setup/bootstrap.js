(() => {
  'use strict';
  const el = id => document.getElementById(id);
  const base = '/mayhem/dashboard/provider/setup';
  let csrf = '', ready = false;
  const message = text => { el('create-message').textContent = text; };
  const value = id => el(id).value.trim();
  const number = id => { const n = Number(value(id)); if (!value(id) || !Number.isSafeInteger(n) || n < 0) throw new Error('Enter a whole number for every allowance and limit.'); return n; };
  const yes = id => { if (!['yes','no'].includes(value(id))) throw new Error('Choose each policy explicitly.'); return value(id) === 'yes'; };
  const option = (select, val, text) => { const o = document.createElement('option'); o.value=val; o.textContent=text; select.append(o); };
  document.querySelectorAll('.yes-no').forEach(s => { option(s,'','Choose'); option(s,'no','No'); option(s,'yes','Yes'); });
  const decisions = () => value('endpoint') === 'mayhem_decisions';
  const units = () => decisions() ? ['decision'] : ['input_token','output_token'];
  function rates() {
    el('rate-fields').replaceChildren();
    if (!value('endpoint')) return;
    units().forEach(unit => {
      [['granularity', `${unit}: billing units per price`, 'number'],['rate', `${unit}: AU for those units`, 'text']].forEach(([suffix,label,type]) => {
        const l=document.createElement('label'); l.textContent=label;
        const input=document.createElement('input'); input.id=`${unit}-${suffix}`; input.required=true; input.type=type;
        if (type==='number') { input.min='1'; input.max='9007199254740991'; } else { input.inputMode='numeric'; input.pattern='[0-9]+'; }
        l.append(input); el('rate-fields').append(l);
      });
    });
    el('outcome-label').hidden=!decisions();
    el('tokenizer').disabled=decisions();
    if (decisions()) el('tokenizer').value='none'; else if (value('tokenizer')==='none') el('tokenizer').value='';
  }
  function collect() {
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
    const prices=units().map(unit=>({unit,granularity:number(`${unit}-granularity`),per_unit_au:value(`${unit}-rate`)}));
    return {schema_version:1,base_url:value('base-url').replace(/\/?$/,'/'),network_policy,credential,
      endpoint:value('endpoint'),upstream_model:value('model'),market:{action:'create_market',slug:value('slug'),model:{family_id:value('family'),model_id:value('public-model'),revision:value('model-revision'),quantization:value('quantization')}},
      served_context:context,concurrency:number('concurrency'),accepted_rails:rails,sequence:number('sequence'),
      offers:[{revision:1,ctx_bracket:`ctx${context}`,outcome_class:decisions()?value('outcome-class'):'',rates:prices,per_request_au:value('per-request'),min_session_au:value('min-session'),accepted_rails:rails}],
      settlement_policy:policy,probe_budget:{max_attempts:number('attempts'),max_cost_microusd:number('cost'),per_attempt_cost_microusd:number('attempt-cost')},
      probe_output_limit:number('output'),probe_timeout_ms:number('timeout'),allow_recovery_probes:yes('recovery'),tokenizer_id:decisions()?'none':value('tokenizer'),closed_retention_ms:number('retention')*86400000};
  }
  el('endpoint').addEventListener('change',rates);
  el('network').addEventListener('change',()=>{el('pinned-fields').hidden=value('network')!=='pinned';el('networks').required=value('network')==='pinned';el('allow-http').required=value('network')==='pinned';});
  el('credential').addEventListener('change',()=>{el('key-label').hidden=value('credential')!=='write';el('api-key').required=value('credential')==='write';if(value('credential')!=='write')el('api-key').value='';});
  el('create-form').addEventListener('input',()=>{el('create-review').hidden=true;ready=false;});
  el('create-form').addEventListener('submit',event=>{
    event.preventDefault();
    try {
      const c=collect();
      if(!c.tokenizer_id)throw new Error('An approved tokenizer is required. Restart the local gateway with --proxy-setup-tokenizer-file pointing to protected approved data.');
      const lines=[`${c.endpoint} · ${c.upstream_model}`,`Market ${c.market.slug} / canonical family ${c.market.model.family_id}`,`Context ${c.served_context}, shared concurrency ${c.concurrency}; rails ${c.accepted_rails.join(', ')}`,
        ...c.offers[0].rates.map(r=>`${r.per_unit_au} AU per ${r.granularity} ${r.unit} units`),`${c.offers[0].per_request_au} AU per request; ${c.offers[0].min_session_au} AU minimum session`,
        `Payable outcomes: ${c.settlement_policy.payable_outcomes.join(', ')}; checkpoints ${c.settlement_policy.allow_checkpoints?'allowed':'not allowed'}; expiry ${value('hold-expiry')}`,
        `Probe allowance: ${c.probe_budget.max_attempts} attempts, ${c.probe_budget.max_cost_microusd} micro-USD total, ${c.probe_budget.per_attempt_cost_microusd} per attempt; output ${c.probe_output_limit}, deadline ${c.probe_timeout_ms} ms`,
        `Recovery probes ${c.allow_recovery_probes?'allowed':'not allowed'}; local journal ${value('retention')} days; sequence ${c.sequence}`,`Authentication: ${value('credential')==='write'?'write-only bearer key':value('credential')}; tokenizer ${c.tokenizer_id}`];
      el('review-list').replaceChildren();lines.forEach(text=>{const li=document.createElement('li');li.textContent=text;el('review-list').append(li);});
      ready=true;el('create-review').hidden=false;message('Review the exact choices, then save.');el('create-review').scrollIntoView({block:'nearest'});
    } catch(e){message(e.message);}
  });
  el('edit-button').addEventListener('click',()=>{ready=false;el('create-review').hidden=true;el('base-url').focus();});
  el('create-button').addEventListener('click',async()=>{
    if(!ready||!el('create-form').reportValidity())return;
    el('create-button').disabled=true;el('create-fields').disabled=true;
    try {
      const payload=JSON.stringify(collect());el('api-key').value='';
      const response=await fetch(`${base}/bootstrap`,{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json','x-mayhem-setup-csrf':csrf},body:payload});
      const data=await response.json();
      if(response.ok){location.replace(base);return;}
      if(data.error==='setup_original_exists'){message('An original setup is already saved. Reloading it without replacing any choices.');location.replace(base);return;}
      throw new Error(data.error==='setup_create_failed_inspect_original'?'Setup could not be confirmed. Reload to recover a saved original; otherwise check the protected host prerequisites. No bundle is replaced.':data.error||'Setup unavailable');
    }catch(e){message(`${e.message}. If the response was lost, reload before retrying. Re-enter a write-only key if no setup was saved.`);ready=false;el('create-review').hidden=true;}
    finally{el('create-button').disabled=false;el('create-fields').disabled=false;}
  });
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
