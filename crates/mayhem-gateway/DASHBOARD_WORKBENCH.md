# Dashboard workbench

The dashboard workbench runs the real user, provider, and network dashboard
renderers against local fixture data. It does not start the peer, bridge,
provider runtime, payment rails, or the rest of `mayhem up`.

From the repository root, start the watched development workflow:

```powershell
node scripts/dashboard-workbench.mjs
```

Then open <http://127.0.0.1:11436/>. Choose a dashboard and a scenario from the
workbench bar. The selected scenario is remembered in a local cookie. Saving a
Rust dashboard source, gateway asset, or catalog file triggers a rebuild and the
open browser page reloads when the new server is ready.

The eight supplied scenarios are `showcase`, `auth-required`, `empty`,
`loading`, `failure`, `offline`, `update-required`, and `scale`. They are
deterministic presentation fixtures, not authoritative ledger or provider
state.

## Architecture and routes

The workbench uses the same dashboard implementation as the gateway:

- `dashboard_pages.rs` owns the 20 product routes, view construction, source
  labels, and freshness semantics.
- `dashboard_ui.rs` owns the shared shell, responsive CSS, progressive-
  enhancement JavaScript, focus/dialog behavior, local preferences, exports,
  session handling, and long-open-page degradation.
- `openai.rs` owns production routing, assets, evidence/request endpoints,
  authentication, and dashboard sessions.
- `dashboard_workbench.rs` owns only the feature-gated local fixture router and
  its eight scenarios.

The route set includes Home, Playground, Models, Activity, Billing, Integrations;
Earn Overview, Jobs, Setup, Model fit, Earnings, and Reliability; Network
Overview, Models, Providers, Markets, Activity, and Evidence; plus Help and
Settings. Help is available at `/mayhem/dashboard/help`.

Workflow-capable catalog entries appear through the same model and market
surfaces. Production `/v1/models` and dashboard filters can select
`endpoint_family=mayhem_comfy_workflows` plus workflow media, runtime id,
outcome class, inventory root, and liveness. A workflow route is live only when
the provider heartbeat carries a saved admission for the signed workflow class;
installed parts alone are not serving capacity.

No inference worker runs in the workbench. Playground exercises the complete
browser/request/receipt UI flow against deterministic workbench responses.

For a one-shot run without file watching:

```powershell
cargo run -p mayhem-gateway --features dashboard-workbench --bin mayhem-dashboard-workbench
```

Use a different loopback port with either command:

```powershell
node scripts/dashboard-workbench.mjs --bind 127.0.0.1:21436
```

The binary rejects non-loopback bind addresses. Its feature-gated entry point is
excluded from normal gateway builds, and generated fixture/runtime files remain
under the ignored `target/dashboard-workbench/` directory.

## Responsive layout contract

Every product route uses the same app shell with a bounded, centered content
column: prose-led routes cap at the standard content tier and table-dense
routes (catalog, activity, and the earn/network analysis pages) use a wider
tier, while the topbar and footer rails stay full width. Paragraph measure is
capped for readability, and the shell changes at four content-driven
thresholds: launch-path cards stack their actions below 1360px, wide layouts
collapse secondary columns around 1120px, navigation becomes an accessible
off-canvas drawer below 780px when JavaScript is available, and dense panels
become single-column below 520px. Without JavaScript, the complete navigation
remains in normal document flow.

Use the workbench scenarios to check at least these viewport classes during UI
work: 320x568 and 390x844 phones, 844x390 short landscape, 768x1024 tablet,
1366x768 laptop, 1920x1080 desktop, and 2560x1440 or wider displays. Also check
the browser at 400% zoom.

The layout contract is:

- the document itself never scrolls horizontally;
- navigation, skip links, dialogs, and the workbench scenario bar remain fully
  keyboard-reachable;
- dense tables retain every column inside their own horizontal scroller;
- charts, filters, long identifiers, cards, and empty/error states stay inside
  their containing section; and
- the mobile bottom navigation never replaces access to the full navigation.

The fixture suite also covers no-JavaScript fallbacks, reduced motion, hidden
amounts, empty/loading/failure/offline/update/scale states, and on-demand
evidence. Run it against a live workbench with:

```powershell
node scripts/dashboard-workbench-smoke.mjs
```

For the real-browser regression pass, install its pinned local dependency and
Chromium once, keep the workbench running, and execute:

```powershell
Push-Location crates/mayhem-gateway
npm ci
npx playwright install chromium
npm run test:dashboard-browser
Pop-Location
```

That pass exercises every product route at 320px and 390px phone widths, short
landscape, tablet, desktop, and 2560px ultrawide widths. It also exercises
Playground workflows against deterministic responses for rate and fixed
pricing, output limits,
partial-stream and funding recovery, draft reset, amount masking, drawer and
dialog focus return, shown-page table tools, session extension, long-open-tab
freshness degradation, reduced motion, representative automated accessibility
analysis, and no-JavaScript navigation. CI runs both smoke layers against the
same isolated workbench binary.

## Freshness and privacy contracts

Operational heartbeat claims use the gateway's runtime heartbeat TTL; the
production default is 60 seconds and workbench fixtures deliberately use a
longer deterministic window. Payment snapshots expire after 30 seconds,
earnings snapshots after 60 seconds, and provider-preparation progress after 5
minutes. Long-open pages degrade dependent text and controls to `Refresh to
reconfirm` or `Unavailable`; they never fabricate updated data.

Hide Amounts is a local browser preference. It masks visible and accessible
money text, Playground pricing data, evidence JSON/downloads, and shown-page
CSV money cells. CSV exports write `Hidden` instead of raw amounts. It does not
change gateway state or previously saved files.

The Connect check verifies the current workbench dashboard session only. It
does not validate production API credentials or prove that an inference worker
is reachable.
