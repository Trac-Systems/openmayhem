'use strict';
(() => {
  const $ = id => document.getElementById(id);
  let view, csrf, plan, runPlan, busy = false, declarationPage, rateChoices = [];
  const declarationChoices = new Map();
  const text = (id, value) => { $(id).textContent = value; };
  const json = value => JSON.stringify(value, null, 2);
  const field = (parent, label, value, change, numeric = false) => {
    const l = document.createElement('label'); l.textContent = label + ' ';
    const i = document.createElement('input'); i.value = value; i.type = numeric ? 'number' : 'text';
    if (numeric) { i.min = '1'; i.step = '1'; }
    i.addEventListener('change', () => {
      try { change(numeric ? integer(i.value) : i.value); i.removeAttribute('aria-invalid'); }
      catch (error) { i.setAttribute('aria-invalid','true'); text('message',error.message); }
    });
    l.append(i); parent.append(l, document.createElement('br')); return i;
  };
  const integer = value => { const n = Number(value); if (!Number.isSafeInteger(n) || n < 1) throw Error('Enter a positive whole number.'); return n; };
  const auToUsd = raw => {
    const n = BigInt(raw), scale = 1000000000000000000n;
    const fraction = (n % scale).toString().padStart(18, '0').replace(/0+$/, '');
    return (n / scale).toString() + (fraction ? '.' + fraction : '');
  };
  const usdToAu = raw => {
    if (raw.length > 59 || !/^(0|[1-9][0-9]*)(\.[0-9]{1,18})?$/.test(raw)) throw Error('Enter an exact nonnegative USD amount, at most 18 decimal places.');
    const [whole, fraction = ''] = raw.split('.');
    const value = BigInt(whole) * 1000000000000000000n + BigInt(fraction.padEnd(18, '0'));
    if (value > 340282366920938463463374607431768211455n) throw Error('Amount exceeds the supported bound.');
    return value.toString();
  };
  function renderRates(v) {
    rateChoices = structuredClone(v.rate_choices || []);
    $('rate-fields').replaceChildren();
    rateChoices.forEach((choice, index) => {
      const offer = v.selection.offers[index], group = document.createElement('fieldset'), legend = document.createElement('legend');
      legend.textContent = `Offer ${index + 1}: ${offer.ctx_bracket}${offer.outcome_class ? ' / ' + offer.outcome_class : ''}`; group.append(legend);
      const rails = document.createElement('p'); rails.textContent = 'Existing rails: ' + offer.accepted_rails.map(r=>r.toUpperCase()).join(', '); group.append(rails);
      field(group, 'USD per request', auToUsd(choice.per_request_au), value => choice.per_request_au = usdToAu(value));
      field(group, 'USD minimum session', auToUsd(choice.min_session_au), value => choice.min_session_au = usdToAu(value));
      for (const rate of choice.rates) field(group, `USD per ${rate.granularity} ${rate.unit}`, auToUsd(rate.per_unit_au), value => rate.per_unit_au = usdToAu(value));
      $('rate-fields').append(group);
    });
    text('rate-state', v.rates ? v.rates.state : 'Publish the initial configuration before reviewing a rate update.');
    text('rate-review', json(v.rates || {}));
    $('rate-summary').replaceChildren();
    if (v.rates) for (const [index, operation] of v.rates.plan.publication.operations.entries()) {
      const offer = operation.action.offer, previous = v.rates.plan.previous_offers[index], line = document.createElement('p');
      line.textContent = `${offer.ctx_bracket}: request USD ${auToUsd(previous.per_request_au)} → ${auToUsd(offer.per_request_au)}; minimum USD ${auToUsd(previous.min_session_au)} → ${auToUsd(offer.min_session_au)}. ` + offer.rates.map(rate => `${rate.unit}: USD ${auToUsd(rate.per_unit_au)} per ${rate.granularity}`).join('; ');
      $('rate-summary').append(line);
    }
    $('publish-rates').disabled = !v.rates || !['needs_confirmation','ready_to_resume_publication','recover_original_publication'].includes(v.rates.state);
  }
  function render(v) {
    renderRates(v);
    view = v; plan = null; runPlan = null; $('publish').disabled = true; $('start-run').disabled = true;
    text('declaration-state', v.declaration ? `${v.declaration.state}. Declared, not verified. A running controller configured for these updates reads signed changes automatically; inference and prices are unchanged.` : 'No signed declarations. Missing claims remain unknown.');
    text('declaration-review', json(v.pending_declaration || v.declaration || {}));
    $('declaration-summary').replaceChildren();
    const reviewed = v.pending_declaration || v.declaration;
    if (reviewed) {
      const expiry = document.createElement('p'); expiry.textContent = `Valid until ${new Date(reviewed.plan.body.expires_at_ms).toISOString()}. No automatic renewal.`; $('declaration-summary').append(expiry);
      const list = document.createElement('ul');
      for (const claim of reviewed.plan.body.claims) { const item=document.createElement('li'); item.textContent=`${claim.field_id}: ${claim.status === 'supported' ? JSON.stringify(claim.value.value) : claim.status}`; list.append(item); }
      $('declaration-summary').append(list);
    }
    $('confirm-declaration').disabled = !v.pending_declaration || v.pending_declaration.state !== 'needs_confirmation';
    text('steps', v.steps.map(s => s.step + ': ' + (s.state || s.structural) + (s.probe ? ' / probe ' + s.probe : '')).join(' → '));
    text('identity', `Connection ${v.connection.id}, revision ${v.connection.revision}; ${v.endpoint}. Draft ${v.review?.draft_id || 'not saved'}, revision ${v.review?.revision || '—'}.`);
    $('model').value = v.selection.upstream_model; $('models').replaceChildren();
    for (const name of v.inventory?.model_ids || []) { const o = document.createElement('option'); o.value = name; $('models').append(o); }
    const market = v.selection.market;
    $('market-fields').hidden = market.action !== 'create_market';
    if (market.action === 'create_market') { $('public-model').value = market.model.model_id; $('slug').value = market.slug; }
    $('context').value = v.selection.membership.served_context; $('concurrency').value = v.selection.membership.max_concurrency;
    $('rails').replaceChildren();
    for (const rail of ['fiat','tap','tnk']) {
      const l = document.createElement('label'), i = document.createElement('input'); i.type = 'checkbox'; i.value = rail;
      i.checked = v.selection.membership.accepted_rails.includes(rail); l.append(i, document.createTextNode(rail.toUpperCase() + ' ')); $('rails').append(l);
    }
    $('prices').replaceChildren();
    field($('prices'), 'Membership revision', v.selection.membership.revision, n => v.selection.membership.revision = n, true);
    v.selection.offers.forEach((offer, index) => {
      const group = document.createElement('fieldset'), legend = document.createElement('legend'); legend.textContent = `Offer ${index + 1}: ${offer.ctx_bracket}`; group.append(legend);
      field(group, 'Offer revision', offer.revision, n => offer.revision = n, true);
      field(group, 'Per request AU', offer.per_request_au, n => offer.per_request_au = n);
      field(group, 'Minimum session AU', offer.min_session_au, n => offer.min_session_au = n);
      for (const rate of offer.rates) {
        field(group, `${rate.unit} AU per ${rate.granularity} units`, rate.per_unit_au, n => rate.per_unit_au = n);
      }
      $('prices').append(group);
    });
    text('probe', json(v.probe_plan || {state:'No protected probe plan configured'}));
    text('review', json(v.review));
    text('run',json(v.run || {state:v.capabilities.run ? 'Review Run after the exact publication is confirmed.' : 'A protected runtime template and host lifecycle are required.'}));
    for (const button of document.querySelectorAll('button[data-action]')) {
      const action = button.dataset.action;
      if (['run_plan','recover_run'].includes(action)) button.disabled = !v.capabilities.run;
      if (action === 'probe') button.disabled = !v.capabilities.probe;
      if (action === 'admission_check') button.disabled = !v.capabilities.canonical_admission;
      if (action.startsWith('invoice_')) button.disabled = !v.capabilities.enrollment;
      if (action === 'invoice_refresh') {
        const i = v.enrollment?.invoice;
        button.disabled ||= !v.enrollment?.for_current_revision || !i?.invoice_commitment || !i.quote_expired || i.payment_status !== 'awaiting_payment' || i.received_amount_base_units !== '0' || Boolean(i.review_code);
      }
      if (action === 'rate_plan') button.disabled = !v.capabilities.rates || v.review?.publication_status !== 'canonical_operations_confirmed';
      if (action === 'publication_plan' || action === 'recover_publication') button.disabled = !v.capabilities.publication;
      if (['declaration_fields','declaration_plan'].includes(action)) button.disabled = !v.capabilities.declarations;
    }
    const retained = v.enrollment, invoice = retained?.invoice;
    $('invoice').replaceChildren();
    if (!invoice) { text('invoice', retained ? `${retained.state}. No invoice payment instructions available.` : 'No reconciled invoice. Read admission first; create/recover only when needed.'); }
    else {
      const p = document.createElement('p'); p.textContent = `Invoice ${invoice.invoice_id}: ${invoice.payment_status}; ${invoice.rail.toUpperCase()}. Fee USD ${invoice.fee_usd}. Required ${invoice.amount_base_units}, verified ${invoice.received_amount_base_units}, short ${invoice.missing_amount_base_units}, excess ${invoice.excess_amount_base_units} (rail base units; FIAT minor units). Quote ${invoice.quote_expired ? 'expired' : 'expires at ' + new Date(invoice.quote_expires_at_ms).toISOString()}. ${retained.for_current_revision ? '' : 'Saved projection belongs to an older draft revision; reconcile status.'}`;
      $('invoice').append(p);
      const details = document.createElement('dl');
      const labels = {network:'Transfer network',chain_id:'Chain ID',token_contract:'Token contract',destination:'Exact invoice receiver',currency:'Currency'};
      for (const [key, label] of Object.entries(labels)) {
        if (invoice.collection[key] == null) continue;
        const term=document.createElement('dt'), value=document.createElement('dd');
        term.textContent=label; value.textContent=String(invoice.collection[key]); value.style.overflowWrap='anywhere'; details.append(term,value);
      }
      $('invoice').append(details);
      const freshness=document.createElement('p'); freshness.textContent='Last authenticated status snapshot, not live payment confirmation. Reconcile status to refresh payment and permit facts.'; $('invoice').append(freshness);
      const status = document.createElement('p'); status.textContent = `Review: ${invoice.review_code || 'none'}. Permit: ${invoice.permit ? 'retained; canonical validation still required at publication' : 'not available'}.`; $('invoice').append(status);
    }
    if (!invoice && retained?.review_code) {
      const review = document.createElement('p'); review.textContent = `Payment requires review: ${retained.review_code}. Admission history is retained; reconcile status for updates.`; $('invoice').append(review);
    }
  }
  async function request(action) {
    $('checkout').replaceChildren();
    const response = await fetch('/mayhem/dashboard/provider/setup/' + (action ? 'action' : 'state'), {
      method: action ? 'POST' : 'GET', credentials:'same-origin', cache:'no-store',
      headers: action ? {'Content-Type':'application/json','x-mayhem-setup-csrf':csrf} : {},
      body: action ? JSON.stringify(action) : undefined
    });
    const data = await response.json(); if (!response.ok) throw Error(data.error || 'Setup request unavailable. Inspect the original retained state.');
    if (data.csrf) csrf = data.csrf; render(data.view); return data.action_result;
  }
  async function perform(name) {
    if (name === 'refresh') return request();
    if (name === 'rate_plan') {
      if (document.querySelector('#rate-fields [aria-invalid="true"]')) throw Error('Correct invalid prices before reviewing rates.');
      return request({action:name,expected_revision:view.review?.revision,choices:rateChoices});
    }
    if (name === 'publish_rates') {
      if (!view.rates || !confirm('Publish exactly these reviewed prices in the same market? Existing accepted jobs retain their original terms. No second admission fee, model restart or new probe.')) return;
      return request({action:name,expected_revision:view.review.revision,plan_digest:view.rates.plan.plan_digest});
    }
    if (name.startsWith('declaration_') || name === 'confirm_declaration' || name === 'withdraw_declaration_plan') return declarations(name);
    const revision = view.review?.revision;
    let a = {action:name,expected_revision:revision};
    if (name === 'recover_run') a = {action:name};
    else if (name === 'connect') a = {action:name};
    else if (name === 'discover') a = {action:name,expected_inventory_revision:view.inventory?.revision || 0};
    else if (name === 'select') {
      if (document.querySelector('[aria-invalid="true"]')) throw Error('Correct the invalid numeric fields before saving.');
      const choice = structuredClone(view.selection); choice.upstream_model = $('model').value;
      if (choice.market.action === 'create_market') { choice.market.model.model_id = $('public-model').value; choice.market.slug = $('slug').value; }
      choice.membership.served_context = integer($('context').value); choice.membership.max_concurrency = integer($('concurrency').value);
      const rails = [...$('rails').querySelectorAll('input:checked')].map(i=>i.value); choice.membership.accepted_rails = rails;
      // Offer rails may only narrow their original accepted rails through this simple form.
      for (const offer of choice.offers) offer.accepted_rails = offer.accepted_rails.filter(r=>rails.includes(r));
      a = {action:'select',expected_revision:revision ?? null,choice};
    } else {
      if (!revision) throw Error('Save your selection first.');
      if (name === 'probe') {
        if (!view.probe_plan) throw Error('No protected probe plan configured.');
        if (!confirm('Run exactly one configured upstream probe? Review its budget and scope above. It may consume your upstream allowance.')) return;
        a.probe_plan_digest = view.probe_plan.digest;
      } else if (name.startsWith('invoice_')) {
        const operation = name.slice(8);
        if (operation !== 'status' && !confirm(operation === 'create' ? 'Create or recover the original admission invoice? No funds will be sent.' : operation === 'refresh' ? 'Reconcile this expired quote and request a new one only if it is still unpaid? If a transfer is pending, wait for status instead. No funds will be sent.' : 'Request the original FIAT checkout? Opening checkout does not prove payment.')) return;
        a = {action:'enrollment',expected_revision:revision,operation,rail:operation === 'create' ? $('invoice-rail').value : null,quote:operation === 'refresh' ? {invoice_id:view.enrollment?.invoice?.invoice_id,invoice_commitment:view.enrollment?.invoice?.invoice_commitment} : null};
      } else if (name === 'start_run') {
        if (!runPlan || !confirm('Install exactly this retained controller? Configured recovery probes may consume the remaining cumulative allowance. Native/model-server configuration is unchanged.')) return;
        a.plan_digest=runPlan.plan_digest;
      } else if (name === 'publication_plan') a.offers_only = false;
      else if (name === 'publish') {
        if (!plan || !confirm('Sign and submit exactly the reviewed publication? No model server will start.')) return;
        a = {action:'publish',expected_revision:revision,offers_only:false,plan_digest:plan.plan_digest};
      }
    }
    const result = await request(a);
    if (name === 'run_plan') { runPlan=result; text('run',json(result)); $('start-run').disabled=false; }
    if (name === 'publication_plan') { plan = result; text('publication',json(result)); $('publish').disabled = false; }
    else if (['publish','recover_publication','admission_check'].includes(name)) text('publication',json(result));
    if (name === 'invoice_checkout') {
      $('checkout').replaceChildren();
      if (result.checkout_url) { const url = new URL(result.checkout_url); if (url.protocol !== 'https:') throw Error('Checkout requires HTTPS.');
        const link=document.createElement('a'); link.href=url.href; link.target='_blank'; link.rel='noopener noreferrer'; link.textContent='Open original invoice checkout'; $('checkout').append(link); }
    }
  }
  async function declarations(name) {
    const revision = view.review?.revision;
    if (!revision) throw Error('Save your selection first.');
    if (name === 'declaration_fields' || name === 'declaration_next') {
      if (name === 'declaration_next' && document.querySelector('#declaration-fields [aria-invalid="true"]')) throw Error('Correct the invalid field before changing pages.');
      if (name === 'declaration_next' && !declarationPage?.next_cursor) return;
      const next = name === 'declaration_next';
      const page = await request({action:'declaration_fields', release_id:next ? declarationPage.release_id : null, cursor:next ? declarationPage.next_cursor : null});
      if (!next) {
        declarationChoices.clear();
        for (const c of view.declaration?.plan.body.claims || []) declarationChoices.set(c.field_id, {field_id:c.field_id,schema_revision:c.schema_revision,status:c.status,value:c.value});
      }
      declarationPage = page; $('declaration-next').disabled = !page.next_cursor;
      $('declaration-fields').replaceChildren();
      for (const doc of page.data) {
        const d = doc.definition, saved = declarationChoices.get(d.field_id), box = document.createElement('fieldset');
        const title = document.createElement('legend'); title.textContent = d.labels.en || Object.values(d.labels)[0] || d.field_id; box.append(title);
        const help = document.createElement('p'); help.textContent = d.help.en || Object.values(d.help)[0] || d.field_id; box.append(help);
        const status = document.createElement('select'); status.setAttribute('aria-label', `${title.textContent}: declaration`);
        for (const [value,label] of [['omit','Do not declare'],['supported','Declare a value'],['unknown','Explicitly unknown'],['unsupported','Unsupported']]) { const option=document.createElement('option'); option.value=value; option.textContent=label; status.append(option); }
        status.value = saved?.status || 'omit'; box.append(status);
        const schema = d.value_schema, value = ['boolean','enum','set'].includes(schema.type) ? document.createElement('select') : document.createElement('input');
        value.setAttribute('aria-label', `${title.textContent}: value`);
        if (value.tagName === 'SELECT') {
          value.multiple = schema.type === 'set';
          const values = schema.type === 'boolean' ? ['false','true'] : schema.values;
          for (const item of values) { const o=document.createElement('option'); o.value=item; o.textContent=item; value.append(o); }
          if (schema.type === 'set') for (const o of value.options) o.selected = saved?.value?.value?.includes(o.value) || false;
          else if (saved?.value) value.value = String(saved.value.value);
        } else { value.value = saved?.value?.value ?? ''; value.maxLength = schema.max_length || 512; }
        value.disabled = status.value !== 'supported'; box.append(value);
        const update = () => {
          value.disabled = status.value !== 'supported';
          if (status.value === 'omit') { declarationChoices.delete(d.field_id); return; }
          if (!declarationChoices.has(d.field_id) && declarationChoices.size >= 32) throw Error('A declaration can contain up to 32 selected fields.');
          let typed = null;
          if (status.value === 'supported') {
            let v = value.value;
            if (schema.type === 'boolean') v = v === 'true';
            if (schema.type === 'set') v = [...value.selectedOptions].map(o=>o.value).sort();
            if (schema.type === 'integer') { v = Number(v); if (!value.value.trim() || !Number.isSafeInteger(v)) throw Error('Enter an exact whole number.'); }
            typed = {type:schema.type,value:v};
          }
          declarationChoices.set(d.field_id,{field_id:d.field_id,schema_revision:d.schema_revision,status:status.value,value:typed});
        };
        for (const control of [status,value]) control.addEventListener('change',()=>{ try { update(); box.removeAttribute('aria-invalid'); } catch(e) { box.setAttribute('aria-invalid','true'); text('message',e.message); } });
        $('declaration-fields').append(box);
      }
      return;
    }
    if (name === 'withdraw_declaration_plan') {
      const expires = Date.parse($('declaration-expiry').value);
      if (!view.declaration || !Number.isSafeInteger(expires) || !/Z$/.test($('declaration-expiry').value) || expires <= Date.now()) throw Error('Choose a future UTC expiry for the withdrawal record.');
      return request({action:name,expected_revision:revision,expected_declaration_revision:view.declaration.latest_revision || view.declaration.plan.body.revision,expires_at_ms:expires});
    }
    if (name === 'declaration_plan') {
      if (!declarationPage || document.querySelector('#declaration-fields [aria-invalid="true"]')) throw Error('Choose published fields and correct invalid values first.');
      const expires = Date.parse($('declaration-expiry').value);
      if (!Number.isSafeInteger(expires) || !/Z$/.test($('declaration-expiry').value) || expires <= Date.now()) throw Error('Enter a future UTC expiry ending in Z.');
      return request({action:name,expected_revision:revision,expected_declaration_revision:view.declaration?.latest_revision || view.declaration?.plan.body.revision || 0,
        release_id:declarationPage.release_id,release_hash:declarationPage.release_hash,choices:[...declarationChoices.values()].sort((a,b)=>a.field_id<b.field_id?-1:a.field_id>b.field_id?1:0),expires_at_ms:expires});
    }
    const pending = view.pending_declaration;
    if (!pending || !confirm('Sign exactly the reviewed provider promises until their stated expiry? This does not attest compliance. A configured running controller will use the signed changes without restarting inference.')) return;
    return request({action:'confirm_declaration',expected_revision:revision,plan_digest:pending.plan.plan_digest});
  }
  document.addEventListener('input', () => {
    if (runPlan) {runPlan=null; $('start-run').disabled=true;}
    if (plan) { plan = null; $('publish').disabled = true; text('message','Save changed selections, then review their exact publication.'); }
  });
  document.addEventListener('click', async event => {
    const button = event.target.closest('button[data-action]'); if (!button || busy) return;
    busy = true; text('message','Working on the explicit action…');
    try { await perform(button.dataset.action); text('message','Retained state refreshed.'); } catch(error) { text('message',error.message); }
    finally { busy = false; }
  });
  request().catch(error=>text('message',error.message));
})();
