#!/usr/bin/env node

import { chromium } from 'playwright';
import AxeBuilder from '@axe-core/playwright';

const PRODUCT_ROUTES = [
  '/mayhem/dashboard',
  '/mayhem/dashboard/playground',
  '/mayhem/dashboard/models',
  '/mayhem/dashboard/activity',
  '/mayhem/dashboard/wallet',
  '/mayhem/dashboard/connect',
  '/mayhem/dashboard/earn',
  '/mayhem/dashboard/earn/jobs',
  '/mayhem/dashboard/earn/machines',
  '/mayhem/dashboard/earn/opportunities',
  '/mayhem/dashboard/earn/earnings',
  '/mayhem/dashboard/earn/reliability',
  '/mayhem/dashboard/network',
  '/mayhem/dashboard/network/models',
  '/mayhem/dashboard/network/providers',
  '/mayhem/dashboard/network/markets',
  '/mayhem/dashboard/network/activity',
  '/mayhem/dashboard/network/evidence',
  '/mayhem/dashboard/help',
  '/mayhem/dashboard/settings',
];

const VIEWPORTS = [
  { name: 'phone-320', width: 320, height: 568 },
  { name: 'phone-390', width: 390, height: 844 },
  { name: 'desktop', width: 1440, height: 900 },
  { name: 'short-landscape', width: 844, height: 390 },
  { name: 'tablet', width: 768, height: 1024 },
  { name: 'ultrawide', width: 2560, height: 1440 },
];

function option(name, fallback) {
  const index = process.argv.indexOf(name);
  return index >= 0 && process.argv[index + 1] ? process.argv[index + 1] : fallback;
}

const baseUrl = option('--base-url', 'http://127.0.0.1:11436').replace(/\/$/, '');
const failures = [];
let assertions = 0;

function check(condition, scope, message, detail = '') {
  assertions += 1;
  if (condition) return;
  failures.push(`${scope}: ${message}${detail ? ` (${detail})` : ''}`);
}

function equal(actual, expected, scope, message) {
  check(
    Object.is(actual, expected),
    scope,
    message,
    `expected ${JSON.stringify(expected)}, received ${JSON.stringify(actual)}`,
  );
}

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ reducedMotion: 'no-preference' });
await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: baseUrl });
const page = await context.newPage();
const consoleErrors = [];
let expectedConsoleFailure = '';
page.on('console', (message) => {
  if (message.type() !== 'error') return;
  const value = message.text();
  if (expectedConsoleFailure && value.includes(expectedConsoleFailure)) {
    expectedConsoleFailure = '';
    return;
  }
  consoleErrors.push(value);
});
page.on('pageerror', (error) => consoleErrors.push(error.message));

async function open(path, scenario = 'showcase') {
  const separator = path.includes('?') ? '&' : '?';
  const response = await page.goto(
    `${baseUrl}${path}${separator}scenario=${encodeURIComponent(scenario)}`,
    { waitUntil: 'domcontentloaded' },
  );
  check(response?.ok() === true, `${scenario}${path}`, 'returns a successful document');
  await page.waitForFunction(() => document.documentElement.classList.contains('js-ready'));
}

async function waitForDashboardReady(targetPage) {
  await targetPage.waitForFunction(() => document.documentElement.classList.contains('js-ready'));
}

function axeDetail(violations) {
  return violations.map((violation) => {
    const targets = violation.nodes
      .slice(0, 3)
      .map((node) => node.target.join(' '))
      .join(', ');
    return `${violation.id} (${violation.impact || 'impact unknown'}): ${targets}`;
  }).join(' | ');
}

try {
  for (const viewport of VIEWPORTS) {
    await page.setViewportSize({ width: viewport.width, height: viewport.height });
    for (const path of PRODUCT_ROUTES) {
      const scope = `${viewport.name}${path}`;
      await open(path);
      const audit = await page.evaluate(() => {
        const visible = (element) => {
          const style = getComputedStyle(element);
          const rect = element.getBoundingClientRect();
          return style.display !== 'none'
            && style.visibility !== 'hidden'
            && rect.width > 0
            && rect.height > 0;
        };
        const accessibleName = (element) => {
          const labelledBy = element.getAttribute('aria-labelledby');
          const labelledText = labelledBy
            ? labelledBy.split(/\s+/).map((id) => document.getElementById(id)?.textContent || '').join(' ')
            : '';
          const labels = 'labels' in element && element.labels
            ? [...element.labels].map((label) => label.textContent || '').join(' ')
            : '';
          return [
            element.getAttribute('aria-label'),
            labelledText,
            labels,
            element.getAttribute('title'),
            element.textContent,
            element.getAttribute('placeholder'),
          ].some((value) => typeof value === 'string' && value.trim().length > 0);
        };
        const ids = [...document.querySelectorAll('[id]')].map((element) => element.id);
        const interactive = [...document.querySelectorAll(
          'a[href],button,input:not([type="hidden"]),select,textarea,summary,[tabindex="0"]',
        )].filter((element) => visible(element) && !element.closest('[data-workbench-chrome]'));
        const targetSelectors = [
          'button',
          '.primary-button',
          '.soft-button',
          '.quiet-button',
          '.icon-button',
          '.subnav a',
          '.panel-footer a',
          '.playground-meta a',
          'summary',
        ];
        const undersized = [...document.querySelectorAll(targetSelectors.join(','))]
          .filter((element) => visible(element) && !element.closest('[data-workbench-chrome]'))
          .filter((element) => element.getBoundingClientRect().height < 43.5)
          .map((element) => `${element.tagName}:${(element.textContent || element.getAttribute('aria-label') || '').trim()}`);
        const misalignedCodeCopies = [...document.querySelectorAll('.code-block .copy-corner')]
          .filter((element) => visible(element) && !element.closest('[data-workbench-chrome]'))
          .filter((element) => {
            const button = element.getBoundingClientRect();
            const block = element.closest('.code-block').getBoundingClientRect();
            const clipped = button.top < block.top || button.bottom > block.bottom;
            const singleLineOffCenter = block.height <= 64
              && Math.abs((button.top + button.bottom - block.top - block.bottom) / 2) > 1.5;
            return clipped || singleLineOffCenter;
          })
          .map((element) => (element.textContent || element.getAttribute('aria-label') || '').trim());
        const mobileNav = document.querySelector('.mobile-bottom-nav');
        const topStatus = document.querySelector('.topbar-status');
        return {
          overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
          mains: document.querySelectorAll('main').length,
          h1s: document.querySelectorAll('h1').length,
          duplicateIds: ids.length - new Set(ids).size,
          unnamed: interactive.filter((element) => !accessibleName(element)).length,
          undersized,
          misalignedCodeCopies,
          mobileNavVisible: mobileNav ? visible(mobileNav) : false,
          topStatusVisible: topStatus ? visible(topStatus) : false,
          openDialogs: [...document.querySelectorAll('dialog')].filter((dialog) => dialog.open).length,
        };
      });
      equal(audit.overflow, 0, scope, 'keeps horizontal overflow inside bounded regions');
      equal(audit.mains, 1, scope, 'renders one main landmark');
      equal(audit.h1s, 1, scope, 'renders one page heading');
      equal(audit.duplicateIds, 0, scope, 'uses unique element IDs');
      equal(audit.unnamed, 0, scope, 'names every visible interactive control');
      check(
        audit.undersized.length === 0,
        scope,
        'keeps visible application targets at least 44px high',
        audit.undersized.join(' | '),
      );
      check(
        audit.misalignedCodeCopies.length === 0,
        scope,
        'keeps command copy buttons centered and inside their code blocks',
        audit.misalignedCodeCopies.join(' | '),
      );
      equal(audit.openDialogs, 0, scope, 'does not open a dialog on navigation');
      if (viewport.width <= 780) {
        check(audit.mobileNavVisible, scope, 'shows compact mobile navigation');
        check(audit.topStatusVisible, scope, 'keeps the critical page status visible on mobile');
      } else {
        check(!audit.mobileNavVisible, scope, 'hides mobile navigation on desktop');
      }
    }
  }

  const professionalHeadings = [
    ['/mayhem/dashboard', 'Overview'],
    ['/mayhem/dashboard/models', 'Model catalog'],
    ['/mayhem/dashboard/earn', 'Provider overview'],
    ['/mayhem/dashboard/earn/opportunities', 'Model opportunities'],
    ['/mayhem/dashboard/network', 'Network health'],
    ['/mayhem/dashboard/network/activity', 'Route status'],
  ];
  for (const [path, heading] of professionalHeadings) {
    await open(path);
    equal(await page.locator('h1').innerText(), heading, 'page naming', `uses the descriptive heading for ${path}`);
  }
  for (const scenario of ['auth-required', 'empty', 'loading', 'failure', 'offline', 'update-required']) {
    await open('/mayhem/dashboard', scenario);
    equal(await page.locator('h1').innerText(), 'Overview', 'page naming', `keeps the Home heading stable in ${scenario}`);
    await open('/mayhem/dashboard/earn', scenario);
    equal(await page.locator('h1').innerText(), 'Provider overview', 'page naming', `keeps the Provider heading stable in ${scenario}`);
  }

  await open('/mayhem/dashboard/settings');
  const settingsNavIcon = page.locator('.app-nav a[href="/mayhem/dashboard/settings"] .nav-icon svg');
  equal(await settingsNavIcon.count(), 1, 'navigation icons', 'renders one Settings navigation icon');
  equal(await settingsNavIcon.locator('circle').count(), 1, 'navigation icons', 'uses a centered hub in the Settings gear');
  check(
    (await settingsNavIcon.locator('path').getAttribute('d'))?.startsWith('M12.22 2h-.44'),
    'navigation icons',
    'uses a complete gear outline for Settings',
  );

  await open('/mayhem/dashboard/help');
  equal(await page.locator('h1').innerText(), 'Help', 'Help experience', 'uses a stable page title');
  equal(await page.locator('.page-head-actions').locator('a,button').count(), 0, 'Help experience', 'does not duplicate a task action in the page header');
  equal(await page.getByRole('link', { name: 'Open Playground' }).count(), 1, 'Help experience', 'offers the Playground action only in its relevant path');
  equal(await page.locator('.help-problem').count(), 5, 'Help experience', 'covers the five launch-critical recovery paths');
  equal(await page.locator('.help-meaning-table tbody tr').count(), 4, 'Help experience', 'maps every dashboard data source to its meaning and limitation');
  check(Number.parseFloat(await page.locator('.help-terms .check-copy span').first().evaluate((element) => getComputedStyle(element).fontSize)) >= 13, 'Help experience', 'keeps explanatory terms readable');
  const accessTokenProblem = page.locator('.help-problem').filter({ hasText: 'My API key is rejected' });
  await accessTokenProblem.locator('summary').click();
  equal(await accessTokenProblem.getByRole('link', { name: 'Review access tokens' }).getAttribute('href'), '/mayhem/dashboard/connect#access-tokens', 'Help experience', 'routes rejected credentials to the relevant recovery section');
  check(await page.getByText('Advanced verification', { exact: true }).isVisible(), 'Help experience', 'keeps attestation guidance under an advanced disclosure');

  await page.setViewportSize({ width: 320, height: 568 });
  await open('/mayhem/dashboard');
  const menu = page.locator('.mobile-menu-button');
  await menu.click();
  check(await page.locator('body').evaluate((body) => body.classList.contains('nav-open')), 'mobile drawer', 'opens from the menu button');
  check(await page.locator('#app-navigation').evaluate((drawer) => drawer.contains(document.activeElement)), 'mobile drawer', 'moves focus into the drawer');
  await page.keyboard.press('Escape');
  check(!await page.locator('body').evaluate((body) => body.classList.contains('nav-open')), 'mobile drawer', 'closes with Escape');
  check(await menu.evaluate((button) => button === document.activeElement), 'mobile drawer', 'returns focus to the trigger');

  await open('/mayhem/dashboard/playground');
  equal(
    await page.locator('.mobile-bottom-nav a[href="/mayhem/dashboard/playground"]').getAttribute('aria-current'),
    'page',
    'mobile navigation',
    'marks Playground as the current golden-path destination',
  );

  await open('/mayhem/dashboard/models?page=2&q=gemma&sort=0&direction=ascending', 'scale');
  const filter = page.locator('[data-table-filter]');
  await filter.press('Escape');
  check(!new URL(page.url()).searchParams.has('q'), 'table filter', 'Escape removes the persisted query');
  check(await page.locator('.pagination a').evaluateAll((links) => links.every((link) => !new URL(link.href).searchParams.has('q'))), 'table filter', 'Escape resynchronizes pagination links');

  await page.setViewportSize({ width: 1440, height: 900 });
  await open('/mayhem/dashboard/models');
  const catalogAudit = await page.locator('#models-table').evaluate((table) => {
    const wrap = table.closest('.data-table-wrap');
    const rows = [...table.querySelectorAll('tbody tr')];
    const logos = [...table.querySelectorAll('.catalog-model-logo img')];
    return {
      rowCount: rows.length,
      unloadedLogos: logos.filter((logo) => !logo.complete || logo.naturalWidth === 0).map((logo) => logo.getAttribute('src')),
      rowsWithoutCapabilities: rows.filter((row) => !row.querySelector('.catalog-capability')).length,
      rowsWithoutStructuredPrices: rows.filter((row) => !row.querySelector('.catalog-price-line')).length,
      internalOverflow: wrap ? Math.ceil(wrap.scrollWidth - wrap.clientWidth) : Number.POSITIVE_INFINITY,
    };
  });
  check(catalogAudit.rowCount > 0, 'model catalog', 'renders catalog rows');
  equal(catalogAudit.unloadedLogos.length, 0, 'model catalog', 'loads every visible lab logo');
  equal(catalogAudit.rowsWithoutCapabilities, 0, 'model catalog', 'groups capabilities into scannable labels');
  equal(catalogAudit.rowsWithoutStructuredPrices, 0, 'model catalog', 'separates price amounts from billing units');
  check(catalogAudit.internalOverflow <= 1, 'model catalog', 'fits the comparison list without desktop horizontal scrolling', `${catalogAudit.internalOverflow}px overflow`);
  const capabilityDisclosure = page.locator('[data-catalog-capabilities-toggle]').first();
  check(await capabilityDisclosure.count() === 1, 'model catalog capabilities', 'offers an explicit control for additional capabilities');
  if (await capabilityDisclosure.count()) {
    await capabilityDisclosure.click();
    equal(await capabilityDisclosure.getAttribute('aria-expanded'), 'true', 'model catalog capabilities', 'expands additional capabilities');
    check(await page.locator('#models-table tbody tr').first().locator('.catalog-capability-extra:not([hidden])').count() > 0, 'model catalog capabilities', 'shows every additional capability inline');
    equal((await capabilityDisclosure.innerText()).trim(), 'Show less', 'model catalog capabilities', 'offers a clear collapse action');
    await capabilityDisclosure.click();
    equal(await capabilityDisclosure.getAttribute('aria-expanded'), 'false', 'model catalog capabilities', 'collapses additional capabilities');
  }
  const modelDetailTrigger = page.locator('[data-model-detail-open]').first();
  await modelDetailTrigger.click();
  const modelDetailDialog = page.locator('#model-detail-dialog');
  check(await modelDetailDialog.evaluate((element) => element.open), 'model details', 'opens from the model identity');
  check(await modelDetailDialog.locator('.model-detail-capability').count() > 4, 'model details', 'shows the complete capability set');
  check(await modelDetailDialog.locator('.model-detail-price .catalog-price-line').count() > 0, 'model details', 'shows structured catalog pricing');
  check(await modelDetailDialog.getByRole('link', { name: 'Use in Playground' }).count() === 1, 'model details', 'offers the primary Playground action');
  check(await modelDetailDialog.getByRole('link', { name: 'Verify evidence' }).count() === 1, 'model details', 'keeps evidence as a separate secondary action');
  await modelDetailDialog.getByRole('button', { name: 'Close model details' }).click();
  check(await modelDetailTrigger.evaluate((trigger) => trigger === document.activeElement), 'model details', 'returns focus to the model identity');

  await page.setViewportSize({ width: 390, height: 844 });
  await open('/mayhem/dashboard/models', 'scale');
  const evidenceTrigger = page.locator('[data-evidence-url]').first();
  await evidenceTrigger.click();
  const dialog = page.locator('#dashboard-evidence-dialog');
  check(await dialog.evaluate((element) => element.open), 'evidence dialog', 'opens on demand');
  await page.waitForFunction(() => {
    const facts = document.querySelector('[data-evidence-facts]');
    return facts && (facts.textContent || '').trim().length > 0;
  });
  check(await dialog.locator('[data-evidence-facts]').innerText().then((text) => text.trim().length > 0), 'evidence dialog', 'renders structured facts');
  check(await dialog.locator('[data-evidence-fact-count]').innerText().then((text) => /\d+ facts?/.test(text)), 'evidence dialog', 'summarizes the number of human-readable facts');
  check(await dialog.locator('[data-evidence-interpretation]').innerText().then((text) => text.trim().length > 0), 'evidence dialog', 'separates interpretation guidance from the evidence identifier');
  const evidenceLayout = await dialog.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    const actions = [...element.querySelectorAll('.verify-action-button')].map((button) => button.getBoundingClientRect().height);
    return {
      insideViewport: rect.left >= 0 && rect.right <= innerWidth && rect.top >= 0 && rect.bottom <= innerHeight,
      undersizedActions: actions.filter((height) => height < 43.5).length,
    };
  });
  check(evidenceLayout.insideViewport, 'evidence dialog', 'fits inside the mobile viewport');
  equal(evidenceLayout.undersizedActions, 0, 'evidence dialog', 'keeps evidence actions at least 44px high');
  const disclosure = dialog.locator('[data-evidence-raw-toggle]');
  if (await disclosure.isVisible()) {
    await disclosure.click();
    equal(await disclosure.getAttribute('aria-expanded'), 'true', 'evidence dialog', 'expands raw JSON on request');
    check(await dialog.locator('[data-evidence-raw]').isVisible(), 'evidence dialog', 'shows the raw payload after disclosure');
    await disclosure.click();
    equal(await disclosure.getAttribute('aria-expanded'), 'false', 'evidence dialog', 'collapses raw JSON again');
  }
  await dialog.getByRole('button', { name: 'Close evidence' }).click();
  check(await evidenceTrigger.evaluate((trigger) => trigger === document.activeElement), 'evidence dialog', 'returns focus to its trigger');
  await page.getByRole('button', { name: 'Hide amounts' }).click();
  await evidenceTrigger.click();
  await page.waitForFunction(() => {
    const raw = document.querySelector('[data-evidence-raw]');
    return raw && (raw.textContent || '').includes('[amount hidden]');
  });
  const rawToggle = dialog.locator('[data-evidence-raw-toggle]');
  if (await rawToggle.isVisible()) {
    await rawToggle.click();
  }
  const privateEvidence = await dialog.locator('[data-evidence-raw]').innerText();
  check(privateEvidence.includes('[amount hidden]'), 'evidence financial privacy', 'redacts monetary values in raw evidence');
  check(!/"(?:per_unit_au|per_req_au|min_session_au|active_demand_au)"\s*:\s*"\d/.test(privateEvidence), 'evidence financial privacy', 'does not leave atomic monetary values readable');
  await dialog.getByRole('button', { name: 'Close evidence' }).click();
  await page.getByRole('button', { name: 'Show amounts' }).click();

  await open('/mayhem/dashboard/playground');
  const playgroundModel = page.locator('[data-playground-model]');
  const playgroundPrompt = page.locator('[data-playground-prompt]');
  const playgroundPrice = page.locator('[data-playground-max-price]');
  const playgroundOutput = page.locator('[data-playground-max-tokens]');
  const playgroundSend = page.locator('[data-playground-send]');
  const playgroundModelTrigger = page.locator('[data-playground-model-trigger]');
  const rateOption = playgroundModel.locator('option[data-price-mode="rate"]').first();
  const fixedOption = playgroundModel.locator('option[data-price-mode="fixed"]').first();
  const choosePlaygroundModel = async (value) => {
    if (await playgroundModelTrigger.getAttribute('aria-expanded') !== 'true') {
      await playgroundModelTrigger.click();
    }
    const optionId = await page.locator('[data-playground-model-option]').evaluateAll(
      (options, modelId) => options.find((option) => option.dataset.playgroundModelOption === modelId)?.id || '',
      value,
    );
    check(Boolean(optionId), 'Playground model picker', 'renders the requested model in the visual listbox', value);
    if (optionId) await page.locator(`#${optionId}`).click();
    equal(await playgroundModel.inputValue(), value, 'Playground model picker', 'keeps the routed model control synchronized');
  };
  check(await playgroundModelTrigger.locator('.model-lab-mark').count() === 1, 'Playground model picker', 'shows the selected model lab mark');
  check(
    (await playgroundModelTrigger.locator('img').getAttribute('src'))?.endsWith('/qwen.svg'),
    'Playground model picker',
    'uses the local Qwen image asset for the Qwen-family fixture',
  );
  await playgroundModelTrigger.focus();
  await page.keyboard.press('ArrowDown');
  equal(await playgroundModelTrigger.getAttribute('aria-expanded'), 'true', 'Playground model picker', 'opens from the keyboard');
  check(await page.locator('[data-playground-model-option]:focus').count() === 1, 'Playground model picker', 'moves focus to the selected listbox option');
  await page.keyboard.press('Escape');
  equal(await playgroundModelTrigger.getAttribute('aria-expanded'), 'false', 'Playground model picker', 'closes with Escape');
  check(await playgroundModelTrigger.evaluate((trigger) => trigger === document.activeElement), 'Playground model picker', 'returns focus to its trigger');

  await page.getByRole('tab', { name: 'Image', exact: true }).click();
  const imageOption = playgroundModel.locator('option:checked');
  const imageSizes = JSON.parse((await imageOption.getAttribute('data-image-sizes')) || '{}');
  const selectedRatio = await page.locator('[data-playground-aspect-ratio][aria-pressed="true"]').getAttribute('data-playground-aspect-ratio');
  const expectedImageSize = imageSizes[selectedRatio];
  check(Boolean(expectedImageSize), 'Playground image dimensions', 'selects dimensions published by the signed model contract');
  equal(expectedImageSize, '1024x1024', 'Playground image dimensions', 'uses the landing Playground proven square preset');
  check(expectedImageSize !== '512x512', 'Playground image dimensions', 'does not reuse the rejected 512 by 512 hardcoded size');
  equal(
    await page.locator('[data-playground-image-size]').innerText(),
    expectedImageSize.replace('x', '\u00d7'),
    'Playground image dimensions',
    'shows the exact dimensions that will be requested',
  );
  await page.locator('[data-playground-image-prompt]').fill('A signed-dimension compatibility check.');
  const imageRequestPromise = page.waitForRequest((request) => request.url().endsWith('/v1/images/generations') && request.method() === 'POST');
  await page.locator('[data-playground-generate-image]').click();
  const imageRequest = await imageRequestPromise;
  const imageRequestBody = imageRequest.postDataJSON();
  equal(imageRequestBody.size, expectedImageSize, 'Playground image dimensions', 'submits the selected model-compatible size');
  await page.locator('.pg-generated-image').waitFor();
  await page.getByRole('tab', { name: 'Text', exact: true }).click();

  await page.locator('.pg-advanced > summary').click();
  check(await rateOption.count() > 0, 'Playground price controls', 'offers a rate-priced fixture model');
  check(await fixedOption.count() > 0, 'Playground price controls', 'offers a fixed-only fixture model');
  await choosePlaygroundModel(await rateOption.getAttribute('value'));
  equal(await page.locator('[data-playground-price-unit]').innerText(), '$ / 1M-unit basket', 'Playground price controls', 'names the composite rate basis');
  await playgroundPrice.fill('0.50');
  await playgroundPrompt.fill('Verify the rate-price request control.');
  const rateRequestPromise = page.waitForRequest((request) => request.url().endsWith('/v1/chat/completions') && request.method() === 'POST');
  await playgroundSend.click();
  const rateRequest = await rateRequestPromise;
  equal(rateRequest.headers()['x-mayhem-max-price-au'], '500000000000000', 'Playground price controls', 'converts $0.50 per 1M-unit basket to the gateway rate basis');
  // Wait on the result COUNT: a `.last()` wait would match the previous
  // send's result while this one is still streaming, racing the composer
  // clear that runs when the in-flight response completes.
  await page.waitForFunction(() => document.querySelectorAll('.pg-message.is-assistant .message-result[data-finish-reason="stop"]').length >= 1);
  check(await page.locator('.pg-message.is-assistant').last().getByText('Actual charge', { exact: false }).count() > 0, 'Playground completion', 'shows receipt-backed actual charge after a completed request');

  await choosePlaygroundModel(await fixedOption.getAttribute('value'));
  equal(await playgroundPrice.inputValue(), '', 'Playground price controls', 'clears a ceiling when the selected model changes price basis');
  equal(await page.locator('[data-playground-price-label]').innerText(), 'Fixed route charge ceiling', 'Playground price controls', 'labels fixed-only routing separately');
  equal(await page.locator('[data-playground-price-unit]').innerText(), 'USD', 'Playground price controls', 'uses USD for a fixed route ceiling');
  await playgroundPrice.fill('0.50');
  await page.getByRole('button', { name: 'Hide amounts' }).click();
  equal(await playgroundPrice.getAttribute('type'), 'password', 'Playground financial privacy', 'masks the price input while amounts are hidden');
  check((await page.locator('[data-playground-request-summary]').innerText()).includes('price ceiling hidden'), 'Playground financial privacy', 'removes the entered amount from the request summary');
  check(!(await page.locator('[data-playground-request-summary]').innerText()).includes('$0.50'), 'Playground financial privacy', 'does not leak the entered ceiling in nearby text');
  await page.getByRole('button', { name: 'Show amounts' }).click();
  await playgroundPrompt.fill('Verify the fixed-charge request control.');
  const fixedRequestPromise = page.waitForRequest((request) => request.url().endsWith('/v1/chat/completions') && request.method() === 'POST');
  await playgroundSend.click();
  const fixedRequest = await fixedRequestPromise;
  equal(fixedRequest.headers()['x-mayhem-max-price-au'], '500000000000000000', 'Playground price controls', 'converts a $0.50 fixed charge to atomic units');
  await page.waitForFunction(() => document.querySelectorAll('.pg-message.is-assistant .message-result[data-finish-reason="stop"]').length >= 2);

  await playgroundOutput.fill('64');
  await playgroundPrompt.fill('Exercise the deterministic output-limit fixture.');
  await playgroundSend.click();
  const lengthResult = page.locator('.pg-message.is-assistant .message-result[data-finish-reason="length"]').last();
  await lengthResult.waitFor();
  check((await lengthResult.innerText()).includes('Output limit reached'), 'Playground output limit', 'explains a length-limited response without calling it complete');
  const continueButton = page.locator('[data-playground-continue]').last();
  check(await continueButton.isVisible(), 'Playground output limit', 'offers an explicit continuation action');
  await continueButton.click();
  equal(await playgroundOutput.inputValue(), '128', 'Playground output limit', 'prepares a larger bounded output limit');
  equal(await playgroundPrompt.inputValue(), 'Continue from where you stopped.', 'Playground output limit', 'prepares but does not auto-send the continuation');

  await page.locator('[data-playground-system]').fill('Temporary saved instruction');
  await playgroundPrice.fill('0.25');
  await page.locator('[data-playground-min-att-tier]').selectOption('2');
  await page.locator('[data-playground-reset-draft]').click();
  equal(await playgroundPrompt.inputValue(), '', 'Playground draft reset', 'clears the tab-scoped message draft');
  equal(await page.locator('[data-playground-system]').inputValue(), '', 'Playground draft reset', 'clears saved system instructions');
  equal(await playgroundOutput.inputValue(), '512', 'Playground draft reset', 'restores the output limit default');
  equal(await playgroundPrice.inputValue(), '', 'Playground draft reset', 'clears the saved price ceiling');
  equal(await page.evaluate(() => sessionStorage.getItem('mayhem.dashboard.playgroundDraft')), null, 'Playground draft reset', 'removes the tab-scoped saved record');

  await page.route('**/v1/chat/completions', async (route) => {
    const body = `data: ${JSON.stringify({
      id: 'partial-fixture',
      model: 'partial-fixture',
      choices: [{ index: 0, delta: { content: 'Preserved partial output.' }, finish_reason: null }],
    })}\n\n`;
    await route.fulfill({ status: 200, contentType: 'text/event-stream; charset=utf-8', body });
  });
  await playgroundPrompt.fill('Preserve an incomplete stream.');
  await playgroundSend.click();
  const partialMarker = page.locator('[data-playground-partial-output]').last();
  await partialMarker.waitFor();
  check((await page.locator('.pg-message.is-assistant').last().innerText()).includes('Preserved partial output.'), 'Playground transport recovery', 'preserves partial streamed content');
  check(await page.locator('.pg-message.is-assistant').last().locator('[data-playground-retry]').isVisible(), 'Playground transport recovery', 'offers a retry after an incomplete stream');
  await page.unroute('**/v1/chat/completions');

  await page.route('**/v1/chat/completions', async (route) => {
    await route.fulfill({
      status: 402,
      contentType: 'application/json',
      body: JSON.stringify({ error: { message: 'Fixture balance is insufficient.' } }),
    });
  });
  await playgroundPrompt.fill('Exercise a funding recovery path.');
  const failedMessagesBeforeFunding = await page.locator('.pg-message.is-assistant.is-failed').count();
  expectedConsoleFailure = '402 (Payment Required)';
  await playgroundSend.click();
  await page.waitForFunction(
    (previousCount) => document.querySelectorAll('.pg-message.is-assistant.is-failed').length > previousCount,
    failedMessagesBeforeFunding,
  );
  const fundingFailure = page.locator('.pg-message.is-assistant.is-failed').last();
  const fundingFailureText = await fundingFailure.innerText();
  check(fundingFailureText.includes('available balance was not sufficient'), 'Playground request recovery', 'translates a funding failure into user impact', fundingFailureText);
  check(await fundingFailure.getByRole('link', { name: 'Review wallet' }).isVisible(), 'Playground request recovery', 'offers the relevant next destination', fundingFailureText);
  equal(await playgroundPrompt.inputValue(), 'Exercise a funding recovery path.', 'Playground request recovery', 'preserves the failed message in the composer');
  await page.unroute('**/v1/chat/completions');

  await open('/mayhem/dashboard/wallet');
  equal(await page.locator('#add-funds').getAttribute('open'), null, 'billing funding', 'keeps optional funding compact when credit is already available');
  check((await page.locator('#add-funds > summary').innerText()).includes('Card, TAP, or TNK'), 'billing funding', 'makes every supported payment method discoverable while collapsed');
  await page.locator('#add-funds > summary').click();
  equal(await page.locator('[data-wallet-funding-method]').count(), 3, 'billing funding', 'offers Stripe, TAP, and TNK without hiding non-active rails');
  const fundingLogos = await page.locator('.wallet-method-icon').evaluateAll((icons) => ({
    count: icons.filter((icon) => icon.querySelector('img, svg')).length,
    unloaded: icons.flatMap((icon) => [...icon.querySelectorAll('img')]).filter((logo) => !logo.complete || logo.naturalWidth === 0).map((logo) => logo.getAttribute('src')),
  }));
  equal(fundingLogos.count, 3, 'billing funding', 'uses a clear icon for every funding method');
  equal(fundingLogos.unloaded.length, 0, 'billing funding', 'loads every branded funding method logo');
  check(await page.locator('[data-wallet-funding-method][value="fiat"]').isChecked(), 'billing funding', 'selects the gateway spending rail by default');
  check((await page.locator('[data-wallet-method="fiat"]').innerText()).includes('Recommended'), 'billing funding', 'recommends card and Stripe for most users');
  await page.locator('[data-wallet-funding-method][value="tap"]').check();
  const tapOnboarding = page.locator('[data-wallet-funding-panel="tap"] .wallet-onboarding-hint a');
  check(await tapOnboarding.isVisible(), 'billing funding onboarding', 'shows TAP acquisition help only in the selected workflow');
  equal(await tapOnboarding.getAttribute('href'), 'https://app.uniswap.org/explore/tokens/ethereum/0x5e7F6e008C6d9D7AD4c7EB75Bd4ce62864cc7454', 'billing funding onboarding', 'links directly to the specified TAP token on Uniswap');
  equal(await tapOnboarding.getAttribute('target'), '_blank', 'billing funding onboarding', 'keeps the Billing workflow open while visiting Uniswap');
  check((await page.locator('[data-wallet-funding-panel="tap"] .wallet-onboarding-hint small').innerText()).includes('small amount of ETH'), 'billing funding onboarding', 'explains that TAP approval and Ethereum gas require ETH in the same wallet');
  await page.locator('[data-wallet-funding-method][value="tnk"]').check();
  const tnkOnboarding = page.locator('[data-wallet-funding-panel="tnk"] .wallet-onboarding-hint a');
  check(await tnkOnboarding.isVisible(), 'billing funding onboarding', 'shows TNK wallet help only in the selected workflow');
  equal(await tnkOnboarding.getAttribute('href'), 'https://www.tracsystems.io/tap-wallet', 'billing funding onboarding', 'links TNK users to Trac Systems TAP Wallet');
  await page.locator('[data-wallet-funding-method][value="tap"]').check();
  await page.locator('#wallet-tap-amount').fill('12.5');
  equal(await page.locator('#wallet-funding-command-tap').innerText(), 'mayhem pay tap --amount-tap 12.5', 'billing funding', 'builds the selected method command from the chosen amount');
  equal(await page.locator('#wallet-deposit-status-command').innerText(), 'mayhem deposit status --rail tap', 'billing funding', 'keeps confirmation on the selected deposit rail');
  check((await page.locator('[data-wallet-funding-panel="tap"] .wallet-rail-warning').innerText()).includes('currently spends'), 'billing funding', 'warns before funding a balance the gateway does not currently spend');
  equal(await page.locator('[data-wallet-funding-panel="tap"] .wallet-switch-disclosure').getAttribute('open'), null, 'billing funding', 'keeps optional rail-switch commands collapsed');
  equal(await page.locator('[data-wallet-funding-panel].is-active').count(), 1, 'billing funding', 'shows one focused funding workflow at a time');
  await page.locator('[data-wallet-funding-panel="tap"] [data-wallet-copy-command]').click();
  equal(await page.evaluate(() => navigator.clipboard.readText()), 'mayhem pay tap --amount-tap 12.5', 'billing funding', 'copies the complete method-specific command');
  await page.locator('#wallet-tap-amount').fill('0');
  check(await page.locator('[data-wallet-funding-panel="tap"] [data-wallet-copy-command]').isDisabled(), 'billing funding', 'blocks an invalid zero-value command');
  await page.locator('#wallet-tap-amount').fill('12.5');
  equal(await page.locator('[data-wallet-funding-panel="tap"] [data-wallet-copy-command]').isEnabled(), true, 'billing funding', 'reenables the command after correction');
  equal(await page.locator('.wallet-confirmation-row').count(), 1, 'billing funding', 'keeps deposit confirmation in one compact final step');
  await page.getByRole('button', { name: 'Hide amounts' }).click();
  check(await page.locator('html').evaluate((html) => html.classList.contains('amounts-hidden')), 'financial privacy', 'enables amount hiding');
  check(await page.locator('.money-value').evaluateAll((values) => values.every((value) => value.textContent === '\u2022\u2022\u2022\u2022')), 'financial privacy', 'masks every visible monetary value');

  await open('/mayhem/dashboard/settings');
  check(await page.locator('#wallet-security').isVisible(), 'wallet security', 'keeps host-only recovery guidance available from Settings');
  await page.locator('[data-preference="motion"]').click();
  check(await page.locator('html').evaluate((html) => html.classList.contains('motion-reduced')), 'reduced motion', 'enables the saved reduced-motion mode');
  equal(await page.locator('.app-sidebar').evaluate((element) => getComputedStyle(element).transitionDuration), '0s', 'reduced motion', 'removes navigation transitions');

  await open('/mayhem/dashboard/connect');
  await page.locator('[data-connection-test]').click();
  await page.waitForFunction(() => document.querySelector('#connection-result')?.textContent?.includes('Workbench dashboard session is reachable'));
  check((await page.locator('#connection-result').innerText()).includes('Production API credentials and inference are intentionally not exercised'), 'connection workflow', 'labels the dashboard-session check without implying an API or inference test');
  check(await page.getByRole('heading', { name: 'Ready to connect' }).count() === 1, 'connection workflow', 'summarizes readiness once');
  check((await page.locator('.connect-helper').first().innerText()).includes('only for applications running on this computer'), 'connection workflow', 'explains the localhost boundary');
  equal(await page.locator('#access-tokens').getAttribute('open'), null, 'connection workflow', 'keeps optional token administration secondary');
  check((await page.locator('#access-tokens > summary').innerText()).includes('Access tokens'), 'connection workflow', 'labels the read-only token section without implying browser editing');
  const copyBaseUrl = page.getByRole('button', { name: 'Copy Mayhem API address' });
  await copyBaseUrl.click();
  const copiedFeedback = await page.waitForFunction(() => document.querySelector('[data-copy-target="#gateway-base-url"] [data-copy-label]')?.textContent === 'Copied');
  equal(await copiedFeedback.jsonValue(), true, 'connection workflow', 'confirms the copied integration value');
  equal(await page.evaluate(() => navigator.clipboard.readText()), await page.locator('#gateway-base-url').innerText(), 'connection workflow', 'copies the exact local base URL');
  await page.locator('#access-tokens > summary').click();
  check(await page.getByRole('heading', { name: 'Need a new token?' }).isVisible(), 'connection workflow', 'explains how to create a token');
  check((await page.locator('.token-secret-note').innerText()).includes('shown only once'), 'connection workflow', 'warns that the generated secret cannot be recovered');
  const copyTokenCommand = page.getByRole('button', { name: 'Copy access token creation command' });
  await copyTokenCommand.click();
  equal(await page.evaluate(() => navigator.clipboard.readText()), await page.locator('#token-create-command').innerText(), 'connection workflow', 'copies the complete constrained token command');
  const privateExportPromise = page.waitForEvent('download');
  await page.locator('[data-export-table="#access-tokens-table"]').click();
  const privateExport = await privateExportPromise;
  const privateExportStream = await privateExport.createReadStream();
  let privateExportCsv = '';
  for await (const chunk of privateExportStream) privateExportCsv += chunk.toString('utf8');
  check(privateExportCsv.includes('"Hidden"'), 'financial privacy', 'redacts money cells in CSV while amounts are hidden');
  check(!privateExportCsv.includes('$9.73') && !privateExportCsv.includes('$50.00'), 'financial privacy', 'does not leak raw token budgets through CSV export');

  await open('/mayhem/dashboard/connect', 'auth-required');
  check(await page.getByRole('heading', { name: 'API key needed' }).count() === 1, 'connection workflow', 'makes required authentication explicit');
  equal(await page.locator('#access-tokens').getAttribute('open'), '', 'connection workflow', 'opens token administration when a credential is required');

  const tableSurfaces = [
    ['/mayhem/dashboard/connect', '#access-tokens-table', 50],
    ['/mayhem/dashboard/earn', '#earn-routes-table', 60],
    ['/mayhem/dashboard/earn/machines', '#machine-routes-table', 60],
    ['/mayhem/dashboard/earn/reliability', '#reliability-routes-table', 60],
    ['/mayhem/dashboard/network/evidence', '#evidence-probes-table', 30],
  ];
  for (const [path, tableSelector, rowLimit] of tableSurfaces) {
    await open(path, 'scale');
    if (path === '/mayhem/dashboard/connect' && await page.locator('#access-tokens').getAttribute('open') === null) {
      await page.locator('#access-tokens > summary').click();
    }
    const scope = `shown-page tools ${path}`;
    const table = page.locator(tableSelector);
    const filter = page.locator(`[data-table-filter="${tableSelector}"]`);
    check(await table.count() === 1, scope, 'renders the bounded table');
    check(await filter.isVisible(), scope, 'offers shown-page filtering');
    check(await table.locator('[data-sort-column]').count() > 0, scope, 'offers shown-page sorting');
    check(await filter.locator('xpath=ancestor::*[contains(@class,"panel-actions")]').locator('[data-export-table]').isVisible(), scope, 'offers an explicitly shown-page export');
    check(await table.locator('tbody tr').count() <= rowLimit, scope, 'keeps the rendered page within its documented server-side limit', `rendered ${await table.locator('tbody tr').count()} rows; limit ${rowLimit}`);
  }
  await open('/mayhem/dashboard/network/evidence', 'scale');
  const downloadPromise = page.waitForEvent('download');
  await page.locator('[data-export-table="#evidence-probes-table"]').click();
  const shownPageDownload = await downloadPromise;
  check(shownPageDownload.suggestedFilename().endsWith('-shown-page.csv'), 'shown-page export', 'names the export as a bounded page');

  const sessionContext = await browser.newContext();
  const sessionPage = await sessionContext.newPage();
  await sessionPage.clock.install({ time: Date.now() });
  await sessionPage.goto(`${baseUrl}/mayhem/dashboard/playground?scenario=showcase`, { waitUntil: 'domcontentloaded' });
  await waitForDashboardReady(sessionPage);
  const sessionSeconds = Number.parseInt(await sessionPage.locator('[data-session-seconds]').getAttribute('data-session-seconds') || '', 10);
  check(Number.isFinite(sessionSeconds) && sessionSeconds > 60, 'session renewal', 'starts with a renewable browser session');
  await sessionPage.locator('[data-playground-prompt]').fill('Keep this draft while extending the session.');
  await sessionPage.clock.fastForward(Math.max(1, sessionSeconds - 59) * 1000);
  const sessionWarning = sessionPage.locator('[data-session-warning]');
  await sessionWarning.waitFor();
  check(await sessionWarning.getByRole('button', { name: 'Extend session' }).isVisible(), 'session renewal', 'warns before the authentication cliff with an in-place action');
  await sessionWarning.getByRole('button', { name: 'Extend session' }).click();
  await sessionWarning.waitFor({ state: 'detached' });
  equal(await sessionPage.locator('[data-playground-prompt]').inputValue(), 'Keep this draft while extending the session.', 'session renewal', 'preserves the active draft during renewal');
  check((await sessionPage.locator('[data-session-seconds]').innerText()).includes('active'), 'session renewal', 'returns the visible session state to active');
  await sessionContext.close();

  const freshnessContext = await browser.newContext();
  const freshnessPage = await freshnessContext.newPage();
  await freshnessPage.clock.install({ time: Date.now() });
  await freshnessPage.goto(`${baseUrl}/mayhem/dashboard?scenario=showcase`, { waitUntil: 'domcontentloaded' });
  await waitForDashboardReady(freshnessPage);
  const freshnessMarker = freshnessPage.locator('[data-page-status-freshness]');
  check(await freshnessMarker.count() === 1, 'volatile freshness', 'marks a source-backed page status with an expiry');
  const expiresAt = Number.parseInt(await freshnessMarker.getAttribute('data-expires-at-ms') || '', 10);
  const browserNow = await freshnessPage.evaluate(() => Date.now());
  check(Number.isFinite(expiresAt) && expiresAt > browserNow, 'volatile freshness', 'starts inside the authoritative heartbeat window');
  await freshnessPage.clock.fastForward(Math.max(1, expiresAt - browserNow + 1100));
  await freshnessPage.locator('[data-page-status-text][data-volatile-expired="true"]').waitFor();
  equal(await freshnessPage.locator('[data-page-status-text]').innerText(), 'Refresh to reconfirm', 'volatile freshness', 'degrades a long-open page instead of leaving a frozen green claim');
  check(await freshnessPage.locator('[data-volatile-expired="true"]').count() > 1, 'volatile freshness', 'degrades volatile values as well as the page status');
  await freshnessContext.close();

  const expiringStateChecks = [
    {
      scope: 'provider route freshness',
      path: '/mayhem/dashboard/network/providers?scenario=showcase',
      selector: '#provider-table td:nth-child(4) [data-volatile-value][data-volatile-expired="true"]',
      expected: 'Unavailable',
      summary: 'Live capacity evidence expired',
    },
    {
      scope: 'wallet freshness',
      path: '/mayhem/dashboard/wallet?scenario=showcase&fresh_evidence=true',
      selector: '.metric-status .status-badge[data-volatile-expired="true"]',
      expected: 'Refresh to reconfirm',
      summary: 'Ledger evidence expired',
    },
    {
      scope: 'earnings freshness',
      path: '/mayhem/dashboard/earn/earnings?scenario=showcase&fresh_evidence=true',
      selector: '.status-badge[data-volatile-expired="true"]',
      expected: 'Refresh to reconfirm',
      summary: 'Earnings ledger evidence expired',
    },
    {
      scope: 'provider preparation freshness',
      path: '/mayhem/dashboard/earn/machines?scenario=loading',
      selector: '[data-volatile-status][data-volatile-expired="true"]',
      expected: 'Preparation snapshot expired',
      summary: 'Provider preparation evidence expired',
    },
  ];
  for (const stateCheck of expiringStateChecks) {
    const expiringContext = await browser.newContext();
    const expiringPage = await expiringContext.newPage();
    await expiringPage.clock.install({ time: Date.now() });
    await expiringPage.goto(`${baseUrl}${stateCheck.path}`, { waitUntil: 'domcontentloaded' });
    await waitForDashboardReady(expiringPage);
    const marker = expiringPage.locator('[data-page-status-freshness]');
    equal(await marker.count(), 1, stateCheck.scope, 'marks the authoritative source window');
    const markerExpiry = Number.parseInt(await marker.getAttribute('data-expires-at-ms') || '', 10);
    const stateNow = await expiringPage.evaluate(() => Date.now());
    check(Number.isFinite(markerExpiry) && markerExpiry > stateNow, stateCheck.scope, 'starts with current evidence');
    const advanceBy = Math.max(1, markerExpiry - stateNow + 1100);
    try {
      await expiringPage.clock.fastForward(advanceBy);
    } catch (error) {
      throw new Error(`${stateCheck.scope}: could not advance ${advanceBy}ms from ${stateNow} to ${markerExpiry}`, { cause: error });
    }
    await expiringPage.locator('[data-page-status-text][data-volatile-expired="true"]').waitFor();
    const dependentState = expiringPage.locator(stateCheck.selector).first();
    await dependentState.waitFor();
    check((await dependentState.innerText()).includes(stateCheck.expected), stateCheck.scope, 'degrades the dependent value or state');
    check((await expiringPage.locator('.page-summary').innerText()).includes(stateCheck.summary), stateCheck.scope, 'explains why the long-open view needs refresh');
    await expiringContext.close();
  }

  const axeRoutes = [
    '/mayhem/dashboard',
    '/mayhem/dashboard/playground',
    '/mayhem/dashboard/models',
    '/mayhem/dashboard/activity',
    '/mayhem/dashboard/connect',
    '/mayhem/dashboard/earn/machines',
    '/mayhem/dashboard/network/evidence',
    '/mayhem/dashboard/help',
    '/mayhem/dashboard/settings',
  ];
  const axeContext = await browser.newContext({ reducedMotion: 'no-preference' });
  const axePage = await axeContext.newPage();
  for (const viewport of [VIEWPORTS[1], VIEWPORTS[2]]) {
    await axePage.setViewportSize({ width: viewport.width, height: viewport.height });
    for (const path of axeRoutes) {
      const scope = `accessibility ${viewport.name}${path}`;
      const response = await axePage.goto(`${baseUrl}${path}?scenario=showcase`, { waitUntil: 'domcontentloaded' });
      check(response?.ok() === true, scope, 'returns a successful document');
      await waitForDashboardReady(axePage);
      const analysis = await new AxeBuilder({ page: axePage }).analyze();
      check(analysis.violations.length === 0, scope, 'passes automated accessibility analysis', axeDetail(analysis.violations));
    }
  }
  await axeContext.close();

  const noScriptContext = await browser.newContext({ javaScriptEnabled: false, viewport: { width: 390, height: 844 } });
  const noScriptPage = await noScriptContext.newPage();
  for (const path of ['/mayhem/dashboard', '/mayhem/dashboard/playground', '/mayhem/dashboard/models', '/mayhem/dashboard/earn', '/mayhem/dashboard/network/evidence', '/mayhem/dashboard/help']) {
    const scope = `no-JavaScript ${path}`;
    const response = await noScriptPage.goto(`${baseUrl}${path}?scenario=showcase`, { waitUntil: 'domcontentloaded' });
    check(response?.ok() === true, scope, 'returns a successful fallback document');
    equal(await noScriptPage.locator('main').count(), 1, scope, 'keeps one main landmark');
    equal(await noScriptPage.locator('h1').count(), 1, scope, 'keeps one page heading');
    check(await noScriptPage.locator('#app-navigation').isVisible(), scope, 'keeps complete navigation visible without scripting');
    check(await noScriptPage.locator('#app-navigation a[href]').count() >= 9, scope, 'keeps every primary task destination reachable');
    equal(await noScriptPage.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth), 0, scope, 'does not introduce document overflow');
  }
  await noScriptContext.close();

  check(consoleErrors.length === 0, 'browser console', 'emits no console or page errors', consoleErrors.join(' | '));
} finally {
  await context.close();
  await browser.close();
}

if (failures.length > 0) {
  console.error(`[browser-smoke] FAIL: ${failures.length} of ${assertions} assertions failed`);
  failures.forEach((failure) => console.error(`  - ${failure}`));
  process.exitCode = 1;
} else {
  console.log(`[browser-smoke] PASS: ${assertions} assertions across ${PRODUCT_ROUTES.length} routes and ${VIEWPORTS.length} viewport classes`);
}
