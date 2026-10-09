'use strict';
(() => {
  const $ = id => document.getElementById(id);
  let view, csrf, plan, runPlan, busy = false;
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
  function render(v) {
    view = v; plan = null; runPlan = null; $('publish').disabled = true; $('start-run').disabled = true;
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
      if (action === 'publication_plan' || action === 'recover_publication') button.disabled = !v.capabilities.publication;
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
        if (operation !== 'status' && !confirm(operation === 'create' ? 'Create or recover the original admission invoice? No funds will be sent.' : 'Request the original FIAT checkout? Opening checkout does not prove payment.')) return;
        a = {action:'enrollment',expected_revision:revision,operation,rail:operation === 'create' ? $('invoice-rail').value : null};
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
