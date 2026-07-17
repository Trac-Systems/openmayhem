use super::html_escape;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DashboardAppPage {
    Home,
    Playground,
    Models,
    Activity,
    Wallet,
    Connect,
    Earn,
    Network,
    Help,
    Settings,
}

impl DashboardAppPage {
    fn nav_items() -> [(Self, &'static str, &'static str); 5] {
        [
            (Self::Home, "Home", "/mayhem/dashboard"),
            (
                Self::Playground,
                "Playground",
                "/mayhem/dashboard/playground",
            ),
            (Self::Activity, "Activity", "/mayhem/dashboard/activity"),
            (Self::Earn, "Earn", "/mayhem/dashboard/earn"),
            (Self::Wallet, "Billing", "/mayhem/dashboard/wallet"),
        ]
    }

    fn advanced_nav_items() -> [(Self, &'static str, &'static str); 3] {
        [
            (Self::Models, "Model catalog", "/mayhem/dashboard/models"),
            (Self::Connect, "Integrations", "/mayhem/dashboard/connect"),
            (
                Self::Network,
                "Network explorer",
                "/mayhem/dashboard/network",
            ),
        ]
    }

    fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Playground => "Playground",
            Self::Models => "Model catalog",
            Self::Activity => "Activity",
            Self::Wallet => "Billing",
            Self::Connect => "Integrations",
            Self::Earn => "Earn",
            Self::Network => "Network",
            Self::Help => "Help",
            Self::Settings => "Settings",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Home => {
                r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3.5 10.5 12 3l8.5 7.5v9a1.5 1.5 0 0 1-1.5 1.5h-5v-6h-4v6H5a1.5 1.5 0 0 1-1.5-1.5z"/></svg>"#
            }
            Self::Playground => {
                r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 3h8v4l3 3v9a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2v-9l3-3zm0 8h8M8 15h5"/></svg>"#
            }
            Self::Models => {
                r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m12 3 8 4.5-8 4.5-8-4.5zm-8 9 8 4.5 8-4.5M4 16.5 12 21l8-4.5"/></svg>"#
            }
            Self::Activity => {
                r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 12h3l2-6 4 12 2-6h5"/></svg>"#
            }
            Self::Wallet => {
                r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 6.5h14a2 2 0 0 1 2 2v10H5a2 2 0 0 1-2-2v-11a2 2 0 0 1 2-2h12M15 12h5"/></svg>"#
            }
            Self::Connect => {
                r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 12h8M9 7H6a4 4 0 0 0 0 8h3m6-8h3a4 4 0 0 1 0 8h-3"/></svg>"#
            }
            Self::Earn => {
                r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16v12H4zM8 7V4h8v3M8 13h8M12 10v6"/></svg>"#
            }
            Self::Network => {
                r#"<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="3"/><circle cx="5" cy="6" r="2"/><circle cx="19" cy="6" r="2"/><circle cx="5" cy="18" r="2"/><circle cx="19" cy="18" r="2"/><path d="m7 7.5 3 2.5m4 0 3-2.5M7 16.5l3-2.5m4 0 3 2.5"/></svg>"#
            }
            Self::Help => {
                r#"<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M9.7 9a2.5 2.5 0 1 1 3.4 2.3c-.9.4-1.1 1-1.1 1.7v.5M12 17h.01"/></svg>"#
            }
            Self::Settings => {
                r#"<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.09a2 2 0 0 1 1 1.74v.5a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.38a2 2 0 0 0-.73-2.73l-.15-.09a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/></svg>"#
            }
        }
    }
}

pub(super) struct DashboardShell<'a> {
    pub page: DashboardAppPage,
    pub eyebrow: &'a str,
    pub heading: &'a str,
    pub summary: &'a str,
    pub status: &'a str,
    pub status_tone: &'a str,
    pub actions: &'a str,
    pub content: &'a str,
    pub footer: &'a str,
    pub expires_in_seconds: u64,
    pub wide: bool,
}

pub(super) fn dashboard_app_shell(shell: DashboardShell<'_>) -> String {
    let navigation = DashboardAppPage::nav_items()
        .into_iter()
        .map(|(page, label, href)| {
            let current = if page == shell.page {
                r#" aria-current="page""#
            } else {
                ""
            };
            format!(
                r#"<a href="{href}" aria-label="{label}"{current}><span class="nav-icon">{icon}</span><span class="nav-text">{label}</span></a>"#,
                icon = page.icon(),
            )
        })
        .collect::<String>();
    let advanced_open = matches!(
        shell.page,
        DashboardAppPage::Models | DashboardAppPage::Connect | DashboardAppPage::Network
    );
    let advanced_navigation = DashboardAppPage::advanced_nav_items()
        .into_iter()
        .map(|(page, label, href)| {
            let current = if page == shell.page {
                r#" aria-current="page""#
            } else {
                ""
            };
            format!(
                r#"<a href="{href}" aria-label="{label}"{current}><span class="nav-icon">{icon}</span><span class="nav-text">{label}</span></a>"#,
                icon = page.icon(),
            )
        })
        .collect::<String>();
    let settings_current = if shell.page == DashboardAppPage::Settings {
        r#" aria-current="page""#
    } else {
        ""
    };
    let help_current = if shell.page == DashboardAppPage::Help {
        r#" aria-current="page""#
    } else {
        ""
    };
    let status_tone = match shell.status_tone {
        "good" | "warn" | "danger" => shell.status_tone,
        _ => "",
    };
    let page_class = if shell.page == DashboardAppPage::Playground {
        " app-main--playground"
    } else {
        ""
    };
    let amount_control = if shell.content.contains("data-money")
        || shell.actions.contains("data-money")
    {
        r#"<button class="soft-button js-only" type="button" data-hide-amounts aria-label="Hide amounts" aria-pressed="false"><span class="button-icon" aria-hidden="true"><svg viewBox="0 0 20 20"><path d="M2 10s3-5 8-5 8 5 8 5-3 5-8 5-8-5-8-5Z"></path><circle cx="10" cy="10" r="2.25"></circle><path class="eye-slash" d="m4 4 12 12"></path></svg></span><span class="button-label" data-hide-label>Hide amounts</span></button>"#
    } else {
        ""
    };
    format!(
        r##"<nav class="skip-links" aria-label="Skip links"><a class="skip-link" href="#main-content">Skip to content</a></nav>
<div class="app-shell">
  <aside class="app-sidebar" id="app-navigation" aria-label="Mayhem navigation">
    <a class="app-brand" href="/mayhem/dashboard" aria-label="Mayhem Home"><span class="app-brand-mark" aria-hidden="true">M</span><span class="app-brand-text">MAY<span class="hem">HEM</span></span></a>
    <nav class="app-nav" aria-label="Primary"><span class="app-nav-label">Workspace</span>{navigation}<details class="advanced-nav"{advanced_open}><summary><span class="nav-icon" aria-hidden="true">&#8943;</span><span class="nav-text">Advanced</span></summary><div class="advanced-nav-items">{advanced_navigation}</div></details><span class="app-nav-label">System</span><a href="/mayhem/dashboard/help" aria-label="Help"{help_current}><span class="nav-icon">{help_icon}</span><span class="nav-text">Help</span></a><a href="/mayhem/dashboard/settings" aria-label="Settings"{settings_current}><span class="nav-icon">{settings_icon}</span><span class="nav-text">Settings</span></a></nav>
  </aside>
  <button class="nav-scrim js-only" type="button" data-nav-close aria-label="Close navigation"></button>
  <div class="app-frame">
    <header class="app-topbar"><div class="topbar-context"><button class="icon-button mobile-menu-button js-only" type="button" data-nav-toggle aria-label="Open navigation" aria-controls="app-navigation" aria-expanded="false"><span aria-hidden="true">&#9776;</span></button><button class="icon-button sidebar-collapse-button js-only" type="button" data-sidebar-toggle aria-label="Collapse navigation" aria-controls="app-navigation" aria-expanded="true"><span aria-hidden="true">&#8592;</span></button><strong>{page_label}</strong><span class="topbar-status"><span class="state-indicator {status_tone}" aria-hidden="true"></span><span data-page-status-text>{status}</span></span></div><div class="topbar-actions">{amount_control}</div></header>
    <main class="app-main{wide_class}{page_class}" id="main-content" tabindex="-1"><header class="page-head"><div><p class="page-eyebrow">{eyebrow}</p><h1>{heading}</h1><p class="page-summary">{summary}</p></div><div class="page-head-actions">{actions}</div></header>{content}</main>
    <footer class="app-footer"><div class="app-footer-inner{wide_class_footer}"><span>{footer}</span><span class="mono" data-session-seconds="{expires}" data-session-status>Browser session active</span></div></footer>
  </div>
</div>
<nav class="mobile-bottom-nav" aria-label="Mobile primary"><a href="/mayhem/dashboard"{mobile_home}>Home</a><a href="/mayhem/dashboard/playground"{mobile_playground}>Playground</a><a href="/mayhem/dashboard/activity"{mobile_activity}>Activity</a><a href="/mayhem/dashboard/earn"{mobile_earn}>Earn</a><a href="/mayhem/dashboard/wallet"{mobile_wallet}>Billing</a></nav>
<dialog class="verify-dialog" id="dashboard-evidence-dialog" aria-labelledby="dashboard-evidence-title"><header class="verify-head"><div class="verify-head-copy"><span class="verify-eyebrow"><span class="verify-eyebrow-mark" aria-hidden="true"></span>Gateway evidence</span><h2 id="dashboard-evidence-title" data-evidence-title>Evidence</h2><p class="verify-subject" data-evidence-summary>Loading the requested snapshot&hellip;</p><p class="verify-interpretation" data-evidence-interpretation hidden></p></div><button class="icon-button verify-close" type="button" data-dialog-close aria-label="Close evidence"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18"/></svg></button></header><div class="verify-toolbar"><p class="verify-source"><span class="verify-source-dot" aria-hidden="true"></span><span data-evidence-meta>Requested from this gateway</span></p><div class="verify-actions"><button class="quiet-button verify-action-button js-only" type="button" data-copy data-copy-target="[data-evidence-raw]" data-evidence-copy disabled><svg viewBox="0 0 24 24" aria-hidden="true"><rect x="8" y="8" width="11" height="11" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/></svg><span data-copy-label>Copy JSON</span></button><button class="quiet-button verify-action-button js-only" type="button" data-evidence-download disabled><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3v12M7 10l5 5 5-5M5 20h14"/></svg><span>Download</span></button></div></div><div class="verify-body" data-evidence-body><p class="notice verify-state" data-evidence-state role="status">Loading evidence&hellip;</p><section class="verify-level" data-evidence-facts-section hidden><div class="verify-section-head"><div><span>Human-readable view</span><h3>Evidence summary</h3></div><span class="verify-section-count" data-evidence-fact-count></span></div><div class="verify-grid" data-evidence-facts></div></section><section class="verify-level verify-raw-level" data-evidence-raw-section hidden><div class="verify-section-head"><div><span>Complete source data</span><h3>Raw gateway snapshot</h3></div></div><p class="verify-section-description">Use this when you need the exact machine-readable payload for an audit or support investigation.</p><button class="verify-raw-toggle js-only" type="button" data-evidence-raw-toggle aria-expanded="false" hidden><span class="verify-raw-icon" aria-hidden="true">&#123; &#125;</span><span class="verify-raw-toggle-copy"><strong data-evidence-raw-toggle-label>Show raw JSON</strong><small>Technical details</small></span><span class="verify-raw-size" data-evidence-raw-size></span><svg viewBox="0 0 24 24" aria-hidden="true"><path d="m7 10 5 5 5-5"/></svg></button><pre class="raw-evidence" data-evidence-raw></pre></section></div></dialog><dialog class="model-detail-dialog" id="model-detail-dialog" aria-labelledby="model-detail-title"><header class="model-detail-shell-head"><div><span>Model catalog</span><h2 id="model-detail-title">Model details</h2></div><button class="icon-button model-detail-close" type="button" data-dialog-close aria-label="Close model details"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18"/></svg></button></header><div class="model-detail-body" data-model-detail-content></div></dialog>"##,
        navigation = navigation,
        advanced_navigation = advanced_navigation,
        advanced_open = if advanced_open { " open" } else { "" },
        help_current = help_current,
        settings_current = settings_current,
        status_tone = status_tone,
        status = html_escape(shell.status),
        amount_control = amount_control,
        page_label = shell.page.label(),
        help_icon = DashboardAppPage::Help.icon(),
        settings_icon = DashboardAppPage::Settings.icon(),
        mobile_home = if shell.page == DashboardAppPage::Home {
            r#" aria-current="page""#
        } else {
            ""
        },
        mobile_playground = if shell.page == DashboardAppPage::Playground {
            r#" aria-current="page""#
        } else {
            ""
        },
        mobile_activity = if shell.page == DashboardAppPage::Activity {
            r#" aria-current="page""#
        } else {
            ""
        },
        mobile_earn = if shell.page == DashboardAppPage::Earn {
            r#" aria-current="page""#
        } else {
            ""
        },
        mobile_wallet = if shell.page == DashboardAppPage::Wallet {
            r#" aria-current="page""#
        } else {
            ""
        },
        eyebrow = html_escape(shell.eyebrow),
        heading = html_escape(shell.heading),
        summary = html_escape(shell.summary),
        actions = shell.actions,
        content = shell.content,
        footer = html_escape(shell.footer),
        expires = shell.expires_in_seconds,
        wide_class = if shell.wide { " app-main--wide" } else { "" },
        page_class = page_class,
        wide_class_footer = if shell.wide {
            " app-footer-inner--wide"
        } else {
            ""
        },
    )
}

pub(super) const DASHBOARD_APP_CSS: &str = r#"
@font-face{font-family:Exo;src:url('/mayhem/dashboard/assets/exo-latin.woff2') format('woff2');font-style:normal;font-weight:400 800;font-display:swap}
:root{
  color-scheme:dark;
  --app-bg:#0b0c0e;
  --app-panel:#121419;
  --app-panel-strong:#171a20;
  --app-panel-soft:#0f1115;
  --app-border:#292d35;
  --app-border-strong:#3a404a;
  --app-text:#f4f5f7;
  --app-text-soft:#b2b8c2;
  --app-text-muted:#7e8794;
  --app-accent:#ff6b7a;
  --app-accent-strong:#ff8793;
  --app-good:#58d6a8;
  --app-info:#6ea8ff;
  --app-warn:#f5b85c;
  --app-danger:#ff5449;
  --app-focus:#9cc1ff;
  --app-radius-xs:8px;
  --app-radius-sm:12px;
  --app-radius-md:18px;
  --app-radius-lg:26px;
  --app-shadow:0 24px 70px rgba(0,0,0,.28);
  --app-fast:140ms;
  --app-standard:220ms;
}

*{box-sizing:border-box}
html{scroll-behavior:smooth}
body{min-height:100vh;margin:0;background:radial-gradient(circle at 78% -20%,rgba(255,107,122,.1),transparent 34rem),var(--app-bg);color:var(--app-text);font:15px/1.5 Exo,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}
a{color:var(--app-info)}
.js-only{display:none!important}
html.js-ready .js-only{display:initial!important}
html.js-ready .soft-button.js-only,html.js-ready .icon-button.js-only,html.js-ready .primary-button.js-only,html.js-ready .quiet-button.js-only{display:inline-flex!important}
[hidden],html.js-ready .js-only[hidden]{display:none!important}
.sr-only{position:absolute!important;width:1px!important;height:1px!important;padding:0!important;margin:-1px!important;overflow:hidden!important;clip:rect(0,0,0,0)!important;white-space:nowrap!important;border:0!important}
html.amounts-hidden [data-money] .money-value,html.amounts-hidden .money-value[data-money]{letter-spacing:.08em}
html.amounts-hidden:not(.amounts-ready) [data-money] .money-value,html.amounts-hidden:not(.amounts-ready) .money-value[data-money]{color:transparent!important;text-shadow:none!important}
html.amounts-hidden:not(.amounts-ready) .raw-evidence{color:transparent!important}
button,input,select,textarea{font:inherit}
button{cursor:pointer}
a,button{-webkit-tap-highlight-color:transparent}
:focus-visible{outline:3px solid var(--app-focus);outline-offset:3px}
.skip-link{position:fixed;left:max(16px,env(safe-area-inset-left));top:max(12px,env(safe-area-inset-top));z-index:1000;transform:translateY(-150%);padding:10px 14px;border-radius:10px;background:var(--app-text);color:var(--app-bg);font-weight:700;text-decoration:none}
.skip-link:focus{transform:none}

.app-shell{min-height:100vh;display:grid;grid-template-columns:248px minmax(0,1fr)}
.app-sidebar{position:sticky;top:0;height:100vh;height:100dvh;padding:max(24px,env(safe-area-inset-top)) 18px max(20px,env(safe-area-inset-bottom)) max(18px,env(safe-area-inset-left));border-right:1px solid var(--app-border);background:rgba(13,14,17,.96);backdrop-filter:blur(18px);display:flex;flex-direction:column;gap:22px;overflow-y:auto;overscroll-behavior:contain;z-index:22}
.app-brand{display:flex;align-items:center;gap:11px;padding:0 8px;color:var(--app-text);text-decoration:none;font-weight:800;letter-spacing:-.02em}
.app-brand-mark{width:34px;height:34px;border-radius:11px;display:grid;place-items:center;background:linear-gradient(145deg,var(--app-accent),#b83c61);box-shadow:0 8px 24px rgba(255,107,122,.24);font-size:13px}
.app-nav{display:grid;gap:4px}
.app-nav-label{margin:11px 10px 5px;color:var(--app-text-muted);font-size:12px;letter-spacing:.1em;text-transform:uppercase}
.app-nav a{min-height:44px;display:flex;align-items:center;gap:11px;padding:10px 12px;border:1px solid transparent;border-radius:12px;color:var(--app-text-soft);text-decoration:none;font-weight:600}
.app-nav a:hover{background:var(--app-panel);color:var(--app-text)}
.app-nav a[aria-current="page"]{background:linear-gradient(110deg,rgba(255,107,122,.15),rgba(255,107,122,.04));border-color:rgba(255,107,122,.25);color:var(--app-text)}
.advanced-nav{margin-top:3px}
.advanced-nav>summary{min-height:44px;display:flex;align-items:center;gap:11px;padding:10px 12px;border:1px solid transparent;border-radius:12px;color:var(--app-text-muted);font-weight:600;cursor:pointer;list-style:none}
.advanced-nav>summary::-webkit-details-marker{display:none}
.advanced-nav>summary:hover,.advanced-nav[open]>summary{background:var(--app-panel);color:var(--app-text-soft)}
.advanced-nav-items{display:grid;gap:3px;margin:3px 0 5px 14px;padding-left:10px;border-left:1px solid var(--app-border)}
.advanced-nav-items a{min-height:40px;padding-block:7px;font-size:13px}
.nav-icon{width:20px;height:20px;display:grid;place-items:center;color:var(--app-text-muted)}
.nav-icon svg{width:19px;height:19px;fill:none;stroke:currentColor;stroke-width:1.8;stroke-linecap:round;stroke-linejoin:round}
.app-nav a[aria-current="page"] .nav-icon{color:var(--app-accent-strong)}
.state-indicator{width:9px;height:9px;border-radius:999px;background:var(--app-text-muted);box-shadow:0 0 0 4px rgba(126,135,148,.1)}
.state-indicator.good{background:var(--app-good);box-shadow:0 0 0 4px rgba(88,214,168,.1)}
.state-indicator.warn{background:var(--app-warn);box-shadow:0 0 0 4px rgba(245,184,92,.1)}
.state-indicator.danger{background:var(--app-danger);box-shadow:0 0 0 4px rgba(255,84,73,.1)}

.app-frame{min-width:0}
.app-topbar{position:sticky;top:0;z-index:15;min-height:68px;padding:max(12px,env(safe-area-inset-top)) max(clamp(18px,2.7vw,44px),env(safe-area-inset-right)) 12px max(clamp(18px,2.7vw,44px),env(safe-area-inset-left));display:flex;align-items:center;justify-content:space-between;gap:16px;border-bottom:1px solid rgba(41,45,53,.78);background:rgba(11,12,14,.84);backdrop-filter:blur(18px)}
.topbar-context{min-width:0;display:flex;align-items:center;gap:12px}
.topbar-context strong{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.topbar-status{display:inline-flex;align-items:center;gap:8px;color:var(--app-text-soft);font-size:13px}
.topbar-actions{display:flex;align-items:center;gap:8px}
.icon-button,.soft-button,.primary-button,.quiet-button{min-height:44px;border-radius:12px;display:inline-flex;align-items:center;justify-content:center;gap:8px;padding:9px 13px;text-decoration:none;font-weight:700;transition:transform var(--app-fast) ease,background var(--app-fast) ease,border-color var(--app-fast) ease}
.button-icon{width:18px;height:18px;display:grid;place-items:center}
.button-icon svg{width:18px;height:18px;fill:none;stroke:currentColor;stroke-width:1.7;stroke-linecap:round;stroke-linejoin:round}
.button-icon .eye-slash{display:none}.soft-button[aria-pressed="true"] .eye-slash{display:block}
.icon-button{width:44px;padding:0;border:1px solid var(--app-border);background:var(--app-panel);color:var(--app-text)}
.soft-button,.quiet-button{border:1px solid var(--app-border);background:var(--app-panel);color:var(--app-text)}
.quiet-button{background:transparent;color:var(--app-text-soft)}
.primary-button{border:1px solid var(--app-accent);background:var(--app-accent);color:#25090e;box-shadow:0 10px 30px rgba(255,107,122,.2)}
.icon-button:hover,.soft-button:hover,.quiet-button:hover{border-color:var(--app-border-strong);background:var(--app-panel-strong)}
.primary-button:hover{background:var(--app-accent-strong)}
.icon-button:active,.soft-button:active,.primary-button:active,.quiet-button:active{transform:scale(.97)}
.mobile-menu-button{display:none}
.sidebar-collapse-button span{display:inline-block;transition:transform var(--app-standard) cubic-bezier(.2,0,.38,.9)}
html.js-ready .icon-button.mobile-menu-button.js-only,html.js-ready .nav-scrim{display:none!important}
.mobile-bottom-nav{display:none}

.app-main{width:100%;max-width:1560px;margin-inline:auto;padding:clamp(24px,3.7vw,56px) max(clamp(18px,3.1vw,52px),env(safe-area-inset-right)) 72px max(clamp(18px,3.1vw,52px),env(safe-area-inset-left))}
.app-main--wide{max-width:1880px}
.app-main--playground{max-width:1180px;padding-top:clamp(24px,3vw,42px)}
.app-main p{max-width:72ch}
.app-main p.page-summary{max-width:720px}
.check-copy span{max-width:62ch}
.notice{max-width:78ch}
.page-head{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:24px;align-items:end;margin:0 0 clamp(24px,3vw,42px)}
.page-eyebrow{margin:0 0 7px;color:var(--app-accent-strong);font-size:12px;font-weight:800;letter-spacing:.1em;text-transform:uppercase}
.page-head h1{max-width:850px;margin:0;font-size:clamp(32px,4vw,56px);line-height:1.03;letter-spacing:-.045em}
.page-summary{max-width:720px;margin:13px 0 0;color:var(--app-text-soft);font-size:clamp(15px,1.25vw,18px)}
.page-head-actions{display:flex;gap:10px;align-items:center;justify-content:flex-end;flex-wrap:wrap}
.app-main--playground .page-head{width:min(100%,960px);margin:0 auto 18px;align-items:center}
.app-main--playground .page-head h1{font-size:clamp(30px,3.2vw,42px)}
.app-main--playground .page-summary{margin-top:8px}

.attention-card{margin-bottom:24px;padding:17px 18px;border:1px solid rgba(110,168,255,.28);border-radius:16px;background:linear-gradient(110deg,rgba(110,168,255,.12),rgba(110,168,255,.035));display:grid;grid-template-columns:auto minmax(0,1fr) auto;gap:14px;align-items:center}
.attention-card.warn{border-color:rgba(245,184,92,.35);background:linear-gradient(110deg,rgba(245,184,92,.13),rgba(245,184,92,.035))}
.attention-card.danger{border-color:rgba(255,84,73,.35);background:linear-gradient(110deg,rgba(255,84,73,.13),rgba(255,84,73,.035))}
.attention-icon{width:36px;height:36px;border-radius:11px;display:grid;place-items:center;background:rgba(110,168,255,.13);color:var(--app-info);font-weight:900}
.attention-card.warn .attention-icon{background:rgba(245,184,92,.13);color:var(--app-warn)}
.attention-card.danger .attention-icon{background:rgba(255,84,73,.13);color:var(--app-danger)}
.attention-copy strong{display:block}
.attention-copy p{margin:3px 0 0;color:var(--app-text-soft);font-size:13px}

.launch-paths{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:14px;margin-bottom:24px}
.launch-path-card{min-width:0;padding:18px;border:1px solid var(--app-border);border-radius:18px;background:linear-gradient(145deg,rgba(23,26,32,.98),rgba(15,17,21,.98));display:grid;grid-template-columns:auto minmax(0,1fr) auto;gap:14px;align-items:center}
.launch-path-card.is-ready{border-color:rgba(88,214,168,.3);background:linear-gradient(145deg,rgba(88,214,168,.08),rgba(18,20,25,.98))}
.launch-path-icon{width:42px;height:42px;border:1px solid rgba(255,107,122,.25);border-radius:13px;display:grid;place-items:center;background:rgba(255,107,122,.09);color:var(--app-accent-strong);font-size:20px}
.launch-path-copy h2{margin:8px 0 3px;font-size:18px}
.launch-path-copy p{margin:0;color:var(--app-text-muted);font-size:12px;line-height:1.5}

.metric-grid{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:14px;margin-bottom:24px}
.metric-grid--three{grid-template-columns:repeat(3,minmax(0,1fr))}
.metric{min-width:0;padding:17px;border:1px solid var(--app-border);border-radius:16px;background:linear-gradient(145deg,var(--app-panel-strong),var(--app-panel));box-shadow:0 14px 40px rgba(0,0,0,.12)}
.metric-top{display:flex;align-items:center;justify-content:space-between;gap:10px}
.metric-label{color:var(--app-text-muted);font-size:12px;font-weight:700}
.metric-state{font-size:12px;color:var(--app-text-muted)}
.metric-status{margin:12px 0 6px;display:flex;align-items:center;min-height:34px}
.metric-value{margin:10px 0 4px;font-size:clamp(22px,2.25vw,32px);font-weight:800;letter-spacing:-.035em;overflow-wrap:anywhere}
.metric-meta{margin:0;color:var(--app-text-muted);font-size:12px}

.dashboard-layout{display:grid;grid-template-columns:minmax(0,1.3fr) minmax(300px,.7fr);gap:18px;align-items:start}
.stack{display:grid;gap:18px;min-width:0}.compact-stack{gap:12px;margin-top:12px}
.panel{min-width:0;border:1px solid var(--app-border);border-radius:var(--app-radius-md);background:linear-gradient(145deg,rgba(23,26,32,.96),rgba(18,20,25,.96));box-shadow:0 18px 54px rgba(0,0,0,.12);overflow:hidden}
.panel-head{min-height:64px;padding:16px 18px;display:flex;align-items:center;justify-content:space-between;gap:14px;border-bottom:1px solid var(--app-border)}
.panel-title{min-width:0}
.panel-title h2{margin:0;font-size:17px;letter-spacing:-.015em}
.panel-title p{margin:3px 0 0;color:var(--app-text-muted);font-size:12px}
.panel-actions{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.panel-body{padding:18px}
.panel-body.flush{padding:0}
.panel-footer{padding:13px 18px;border-top:1px solid var(--app-border);display:flex;justify-content:space-between;gap:12px;align-items:center;flex-wrap:wrap;color:var(--app-text-muted);font-size:12px}
.panel-footer>a:not(.icon-button):not(.soft-button):not(.primary-button):not(.quiet-button),.playground-meta>a{min-height:44px;display:inline-flex;align-items:center;padding:8px 4px}
.pagination{display:flex;align-items:center;justify-content:flex-end;gap:8px;flex-wrap:wrap}.pagination .quiet-button{min-height:44px;padding:8px 10px;font-size:12px}.pagination-page{white-space:nowrap;color:var(--app-text-soft);font-variant-numeric:tabular-nums}.pagination-disabled{opacity:.48;cursor:not-allowed}.pagination-disabled:active{transform:none}

.usage-chart{margin:0}
.usage-chart .panel-head{min-height:58px;padding:14px 16px}
.usage-chart .panel-head>strong{font-size:26px}
.usage-chart .panel-body{padding:12px 16px 14px}
.usage-bars{width:100%;max-width:1000px;height:clamp(155px,11vw,205px);margin:0 auto;padding:0;display:grid;grid-template-columns:repeat(7,minmax(0,1fr));gap:8px;align-items:end;list-style:none}
.usage-bars li{height:100%;min-width:0;display:grid;grid-template-rows:minmax(0,1fr) auto auto;gap:5px;align-items:end;text-align:center}
.usage-bar{height:100%;min-height:24px;display:flex;align-items:flex-end;justify-content:center}
.usage-bar>span{width:min(34px,72%);min-height:4px;border-radius:7px 7px 3px 3px;background:linear-gradient(180deg,var(--app-info),rgba(110,168,255,.42));box-shadow:0 6px 20px rgba(110,168,255,.1)}
.usage-bar.level-0>span{height:4px}.usage-bar.level-1>span{height:10%}.usage-bar.level-2>span{height:20%}.usage-bar.level-3>span{height:30%}.usage-bar.level-4>span{height:40%}.usage-bar.level-5>span{height:50%}.usage-bar.level-6>span{height:60%}.usage-bar.level-7>span{height:70%}.usage-bar.level-8>span{height:80%}.usage-bar.level-9>span{height:90%}.usage-bar.level-10>span{height:100%}
.usage-bars strong{font-size:12px}
.usage-bars small{color:var(--app-text-muted);font-size:11px;white-space:nowrap}

.activity-list{display:grid}
.activity-row{min-width:0;padding:14px 18px;display:grid;grid-template-columns:auto minmax(0,1fr) auto;gap:13px;align-items:center;border-bottom:1px solid var(--app-border)}
.activity-row:last-child{border-bottom:0}
.activity-state{width:34px;height:34px;border-radius:11px;display:grid;place-items:center;background:rgba(88,214,168,.1);color:var(--app-good);font-weight:800}
.activity-state.pending{background:rgba(110,168,255,.1);color:var(--app-info)}
.activity-state.failed{background:rgba(255,84,73,.1);color:var(--app-danger)}
.activity-main{min-width:0}
.activity-main strong{display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.activity-main span{display:block;margin-top:3px;color:var(--app-text-muted);font-size:12px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.activity-value{text-align:right}
.activity-value strong{display:block}
.activity-value span{display:block;margin-top:3px;color:var(--app-text-muted);font-size:12px}

.checklist{display:grid;gap:11px;margin:0;padding:0;list-style:none}
.check-step{display:grid;grid-template-columns:auto minmax(0,1fr);gap:11px;align-items:start}
.check-mark{width:24px;height:24px;border:1px solid var(--app-border-strong);border-radius:8px;display:grid;place-items:center;color:var(--app-text-muted);font-size:12px;font-weight:900}
.check-step.done .check-mark{border-color:rgba(88,214,168,.5);background:rgba(88,214,168,.12);color:var(--app-good)}
.check-step.active .check-mark{border-color:rgba(255,107,122,.5);background:rgba(255,107,122,.12);color:var(--app-accent-strong)}
.check-step.pending .check-mark{border-color:rgba(110,168,255,.3);background:rgba(110,168,255,.07);color:var(--app-info)}
.check-copy strong{display:block;font-size:13px}
.check-copy span{display:block;margin-top:2px;color:var(--app-text-muted);font-size:12px}
.check-copy .soft-button{margin-top:9px}
.help-layout{align-items:start}
.help-paths{gap:18px}
.help-paths .check-copy span,.help-terms .check-copy span{font-size:13px;line-height:1.52;color:var(--app-text-soft)}
.help-problems{display:grid}
.help-problem{border-top:1px solid var(--app-border)}
.help-problem:first-child{border-top:0}
.help-problem>summary{min-height:68px;padding:13px 18px;display:grid;grid-template-columns:minmax(0,1fr) auto;align-items:center;gap:3px 16px;cursor:pointer;list-style:none}
.help-problem>summary::-webkit-details-marker{display:none}
.help-problem>summary::after{content:"+";grid-column:2;grid-row:1/3;color:var(--app-text-muted);font-size:20px;font-weight:400}
.help-problem[open]>summary::after{content:"−"}
.help-problem>summary:hover{background:rgba(255,255,255,.025)}
.help-problem>summary>span{color:var(--app-text);font-size:13px;font-weight:800}
.help-problem>summary>small{color:var(--app-text-muted);font-size:12px;line-height:1.4}
.help-problem>div{padding:0 54px 16px 18px;display:grid;gap:11px}
.help-problem>div p{margin:0;color:var(--app-text-soft);font-size:13px;line-height:1.55}
.help-problem>div .soft-button{justify-self:start}
.help-meaning-table{min-width:760px}
.help-meaning-table tbody th{width:130px;color:var(--app-text);font-size:13px}
.help-meaning-table tbody td{color:var(--app-text-soft);font-size:13px;line-height:1.5}
.help-terms{gap:16px}
.help-disclosure-copy h3{margin:0;color:var(--app-text);font-size:14px}
.help-disclosure-copy p{margin:7px 0 0;color:var(--app-text-soft);font-size:13px;line-height:1.58}
.help-disclosure-copy .soft-button{margin-top:14px}

progress{width:100%;height:8px;border:0;border-radius:999px;overflow:hidden;background:#242830;color:var(--app-info);accent-color:var(--app-info)}
progress::-webkit-progress-bar{background:#242830;border-radius:999px}
progress::-webkit-progress-value{border-radius:999px;background:linear-gradient(90deg,var(--app-info),var(--app-good))}
progress::-moz-progress-bar{border-radius:999px;background:linear-gradient(90deg,var(--app-info),var(--app-good))}
.table-progress{min-width:190px;display:grid;gap:6px}
.table-progress strong{font-size:12px}
.table-progress progress{max-width:220px}

.data-table-wrap{width:100%;overflow:auto;overscroll-behavior-inline:contain;scrollbar-gutter:stable}
.data-table-wrap:focus-visible{outline:3px solid var(--app-focus);outline-offset:-3px;box-shadow:inset 0 0 0 1px var(--app-focus)}
.data-table{width:100%;border-collapse:collapse;min-width:680px}
.data-table th,.data-table td{padding:13px 16px;border-bottom:1px solid var(--app-border);text-align:left;vertical-align:middle}
.data-table thead th{position:sticky;top:0;background:var(--app-panel);color:var(--app-text-muted);font-size:12px;letter-spacing:.06em;text-transform:uppercase;z-index:1}
.table-sort-button{width:100%;min-height:44px;margin:-6px -8px;padding:6px 8px;border:0;border-radius:8px;background:transparent;color:inherit;text-align:left;text-transform:inherit;letter-spacing:inherit;font-weight:inherit;display:inline-flex;align-items:center;gap:6px}
.table-sort-button:hover{background:rgba(255,255,255,.035);color:var(--app-text-soft)}
.table-sort-button::after{content:"↕";opacity:.42;font-size:10px}
th[aria-sort="ascending"] .table-sort-button::after{content:"↑";opacity:1;color:var(--app-accent-strong)}
th[aria-sort="descending"] .table-sort-button::after{content:"↓";opacity:1;color:var(--app-accent-strong)}
.data-table tbody tr:last-child>*{border-bottom:0}
.data-table tbody tr:hover{background:rgba(255,255,255,.018)}
.table-primary{font-weight:700}
.table-secondary{display:block;margin-top:2px;color:var(--app-text-muted);font-size:12px}
.model-catalog-panel{overflow:hidden}
.model-catalog-table{min-width:920px;table-layout:fixed}
.model-catalog-table .model-col{width:23%}.model-catalog-table .availability-col{width:16%}.model-catalog-table .capabilities-col{width:23%}.model-catalog-table .price-col{width:25%}.model-catalog-table .action-col{width:13%}
.model-catalog-table th,.model-catalog-table td{padding:17px 16px;vertical-align:top}
.model-catalog-table thead th{padding-top:13px;padding-bottom:13px}
.model-catalog-table tbody tr{transition:background var(--app-fast) ease,box-shadow var(--app-fast) ease}
.model-catalog-table tbody tr:hover{background:rgba(255,255,255,.024);box-shadow:inset 3px 0 0 rgba(255,107,122,.56)}
.catalog-model{min-width:0;display:flex;align-items:flex-start;gap:12px}
.catalog-model-button{width:100%;min-height:52px;margin:-5px;padding:5px;border:0;border-radius:13px;background:transparent;color:inherit;text-align:left;cursor:pointer;transition:background var(--app-fast) ease}
.catalog-model-button:hover{background:rgba(255,255,255,.035)}
.catalog-model-button:focus-visible{outline:3px solid var(--app-focus);outline-offset:1px}
.catalog-model-logo{width:42px;height:42px;flex:0 0 auto;display:block}
.catalog-model-logo .model-lab-mark{width:42px;height:42px;border-radius:12px}
.catalog-model-logo .model-lab-mark svg{width:22px;height:22px}
.catalog-model-logo .model-lab--hauhau svg{width:40px;height:40px;border-radius:11px}
.catalog-model-copy{min-width:0;display:flex;flex-direction:column;align-items:flex-start}
.catalog-model-lab{margin-bottom:3px;color:var(--app-text-muted);font-size:10px;font-weight:800;letter-spacing:.09em;text-transform:uppercase}
.catalog-model-name{max-width:100%;color:var(--app-text);font-size:14px;font-weight:800;line-height:1.3;overflow-wrap:anywhere}
.catalog-model-id{max-width:100%;margin-top:4px;color:var(--app-text-muted);font-size:10px;font-weight:500;line-height:1.35;overflow-wrap:anywhere}
.catalog-model-chevron{width:17px;height:17px;margin:auto 0 auto auto;flex:0 0 auto;fill:none;stroke:var(--app-text-muted);stroke-width:1.7;stroke-linecap:round;stroke-linejoin:round;opacity:.55;transition:transform var(--app-fast) ease,color var(--app-fast) ease,opacity var(--app-fast) ease}
.catalog-model-button:hover .catalog-model-chevron{color:var(--app-text-soft);opacity:1;transform:translateX(2px)}
.status-badge .status-dot{width:6px;height:6px;border-radius:999px;background:currentColor;box-shadow:0 0 0 3px color-mix(in srgb,currentColor 13%,transparent)}
.catalog-availability-detail{max-width:190px;margin-top:7px;line-height:1.42}
.catalog-capabilities{display:flex;align-items:center;align-content:flex-start;gap:6px;flex-wrap:wrap}
.catalog-capability,.catalog-capability-more{min-height:27px;padding:5px 8px;border:1px solid var(--app-border);border-radius:8px;background:rgba(255,255,255,.026);color:var(--app-text-soft);font-size:11px;font-weight:700;line-height:1.25;display:inline-flex;align-items:center}
.catalog-capability.is-context{border-color:rgba(110,168,255,.24);background:rgba(110,168,255,.08);color:#a9c8ff}
.catalog-capability-extra[hidden]{display:none}
.catalog-capability-more{min-height:44px;margin-block:-9px;padding-inline:8px;border-style:dashed;background:transparent;color:var(--app-text-muted);cursor:pointer}
.catalog-capability-more:hover,.catalog-capability-more[aria-expanded="true"]{border-color:var(--app-border-strong);background:rgba(255,255,255,.035);color:var(--app-text-soft)}
.catalog-price{display:grid;gap:5px}
.catalog-price-line{min-width:0;display:grid;grid-template-columns:minmax(64px,max-content) minmax(0,1fr);align-items:baseline;gap:8px;color:var(--app-text-muted);font-size:11px;line-height:1.35}
.catalog-price-line.is-primary{margin-bottom:2px;color:var(--app-text-soft);font-size:12px}
.catalog-price-amount{color:var(--app-text-soft);font:700 11px/1.3 ui-monospace,SFMono-Regular,Menlo,monospace;white-space:nowrap}
.catalog-price-line.is-primary .catalog-price-amount{color:var(--app-text);font-size:14px}
.catalog-price-unit{min-width:0;overflow-wrap:anywhere}
.catalog-price-unavailable{color:var(--app-text-muted);font-size:12px}
.catalog-price-more{margin-top:1px;color:var(--app-text-muted);font-size:10px;font-weight:700}
.catalog-actions{justify-content:flex-end;gap:6px;flex-wrap:nowrap}
.catalog-actions .quiet-button{min-width:48px;padding-inline:8px}
.catalog-actions .quiet-button:first-child{border-color:rgba(110,168,255,.28);background:rgba(110,168,255,.07);color:#b7d1ff}
.catalog-actions .quiet-button:first-child:hover{border-color:rgba(110,168,255,.5);background:rgba(110,168,255,.13);color:var(--app-text)}
.status-badge{display:inline-flex;align-items:center;gap:6px;min-height:26px;padding:4px 9px;border:1px solid var(--app-border);border-radius:999px;color:var(--app-text-soft);font-size:12px;font-weight:700;white-space:nowrap}
.status-badge.good{border-color:rgba(88,214,168,.32);background:rgba(88,214,168,.08);color:var(--app-good)}
.status-badge.info{border-color:rgba(110,168,255,.32);background:rgba(110,168,255,.08);color:var(--app-info)}
.status-badge.warn{border-color:rgba(245,184,92,.32);background:rgba(245,184,92,.08);color:var(--app-warn)}
.status-badge.danger{border-color:rgba(255,84,73,.32);background:rgba(255,84,73,.08);color:var(--app-danger)}

.search-field{min-height:44px;min-width:min(260px,100%);padding:9px 12px;border:1px solid var(--app-border);border-radius:12px;background:var(--app-panel-soft);color:var(--app-text)}

.subnav{margin:-10px 0 24px;display:flex;gap:6px;overflow-x:auto;overscroll-behavior-inline:contain;scrollbar-width:thin;scrollbar-color:var(--app-border-strong) transparent;padding:4px 0}
.subnav::-webkit-scrollbar{height:4px}
.subnav::-webkit-scrollbar-track{background:transparent}
.subnav::-webkit-scrollbar-thumb{border-radius:999px;background:var(--app-border-strong)}
.subnav a{min-height:44px;padding:10px 11px;border:1px solid transparent;border-radius:10px;color:var(--app-text-muted);font-size:12px;font-weight:700;text-decoration:none;white-space:nowrap;display:inline-flex;align-items:center}
.subnav a:hover{color:var(--app-text);background:var(--app-panel)}
.subnav a[aria-current="page"]{border-color:var(--app-border);background:var(--app-panel-strong);color:var(--app-text)}
.subnav-advanced{margin:-17px 0 24px;color:var(--app-text-muted);font-size:12px}
.subnav-advanced>summary{min-height:44px;width:max-content;padding:7px 9px;border-radius:9px;cursor:pointer;font-weight:700;display:flex;align-items:center;gap:7px;list-style:none}
.subnav-advanced>summary::-webkit-details-marker{display:none}
.subnav-advanced>summary::after{content:"\25BE";font-size:12px;color:var(--app-text-muted);transition:transform var(--app-fast) ease}
.subnav-advanced[open]>summary::after{transform:rotate(180deg)}
.subnav-advanced>summary:hover{background:var(--app-panel);color:var(--app-text-soft)}
.subnav-advanced>div{display:flex;gap:6px;padding:5px 0}
.subnav-advanced a{min-height:44px;padding:8px 10px;border:1px solid var(--app-border);border-radius:9px;color:var(--app-text-muted);text-decoration:none;font-weight:700;display:inline-flex;align-items:center}
.subnav-advanced a[aria-current="page"]{background:var(--app-panel-strong);color:var(--app-text)}
.inline-actions{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.disclosure-panel>summary{min-height:56px;padding:16px 18px;display:flex;align-items:center;justify-content:space-between;gap:12px;cursor:pointer;font-weight:700;list-style:none}
.disclosure-panel>summary::-webkit-details-marker{display:none}
.disclosure-panel>summary::after{content:"+";color:var(--app-text-muted);font-size:20px;font-weight:400}
.disclosure-panel[open]>summary{border-bottom:1px solid var(--app-border)}
.disclosure-panel[open]>summary::after{content:"−"}
.notice{padding:14px 15px;border:1px solid var(--app-border);border-radius:13px;background:var(--app-panel-soft);color:var(--app-text-soft);font-size:13px}
.notice strong{color:var(--app-text)}
.notice.good{border-color:rgba(88,214,168,.28);background:rgba(88,214,168,.06)}
.notice.warn{border-color:rgba(245,184,92,.3);background:rgba(245,184,92,.07)}
.notice.danger{border-color:rgba(255,84,73,.3);background:rgba(255,84,73,.07)}
.code-block{position:relative;min-height:62px;margin:0;padding:20px 72px 20px 15px;border:1px solid var(--app-border);border-radius:13px;background:#0b0d10;color:#cdd3db;white-space:pre-wrap;overflow-wrap:anywhere;font:12px/1.58 ui-monospace,SFMono-Regular,Menlo,monospace}
.code-block .copy-corner{position:absolute;right:8px;top:8px}
.connect-ready{margin-bottom:24px;padding:17px 18px;border:1px solid var(--app-border);border-radius:16px;background:linear-gradient(120deg,rgba(88,214,168,.07),rgba(255,255,255,.015) 58%);display:grid;grid-template-columns:auto minmax(0,1fr) auto;align-items:center;gap:14px}
.connect-ready.warn{background:linear-gradient(120deg,rgba(245,184,92,.08),rgba(255,255,255,.015) 58%)}
.connect-ready-mark{width:42px;height:42px;border:1px solid rgba(88,214,168,.34);border-radius:13px;background:rgba(88,214,168,.09);color:var(--app-good);font-size:18px;font-weight:900;display:grid;place-items:center}
.connect-ready.warn .connect-ready-mark{border-color:rgba(245,184,92,.34);background:rgba(245,184,92,.09);color:var(--app-warn)}
.connect-ready-copy{min-width:0}
.connect-ready-copy>span{display:block;color:var(--app-text-muted);font-size:10px;font-weight:800;letter-spacing:.07em;text-transform:uppercase}
.connect-ready-copy h2{margin:2px 0 0;font-size:18px}
.connect-ready-copy p{margin:3px 0 0;color:var(--app-text-muted);font-size:11px}
.connect-layout{align-items:start}
.connect-setup{overflow:hidden}
.connect-steps{padding-top:4px;padding-bottom:4px}
.connect-step{padding:16px 0;display:grid;gap:12px}
.connect-step+.connect-step{border-top:1px solid var(--app-border)}
.connect-step-heading{display:grid;grid-template-columns:auto minmax(0,1fr);align-items:start;gap:11px}
.connect-step-heading>span{width:28px;height:28px;border:1px solid rgba(110,168,255,.3);border-radius:9px;background:rgba(110,168,255,.08);color:#a9c8ff;font-size:11px;font-weight:900;display:grid;place-items:center}
.connect-step-heading h3{margin:1px 0 0;font-size:14px}
.connect-step-heading p{margin:3px 0 0;color:var(--app-text-muted);font-size:11px;line-height:1.4}
.connect-helper{margin:-2px 0 0;color:var(--app-text-muted);font-size:11px;line-height:1.5}
.connect-helper strong{color:var(--app-text-soft)}
.connect-step-actions{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.connect-step-actions .primary-button,.connect-step-actions .quiet-button{min-width:170px}
.connect-step #connection-result{margin:0}
.connect-checklist .checklist{gap:16px}
.token-management>summary{padding:17px 18px}
.token-management-summary{min-width:0;flex:1;display:grid;gap:3px}
.token-management-summary strong{color:var(--app-text);font-size:14px}
.token-management-summary small{color:var(--app-text-muted);font-size:11px;font-weight:500;line-height:1.4}
.token-management-content>.panel-head{border-top:0}
.token-create-guide{padding:18px;border-bottom:1px solid var(--app-border);background:linear-gradient(120deg,rgba(110,168,255,.055),transparent 58%);display:grid;grid-template-columns:minmax(220px,.72fr) minmax(360px,1.28fr);gap:14px 22px;align-items:center}
.token-create-copy{min-width:0}
.token-create-kicker{display:block;margin-bottom:5px;color:#8db8ff;font-size:10px;font-weight:900;letter-spacing:.08em;text-transform:uppercase}
.token-create-copy h2{margin:0;font-size:16px}
.token-create-copy p{margin:5px 0 0;color:var(--app-text-muted);font-size:11px;line-height:1.5}
.token-create-command{min-height:58px;padding-top:18px;padding-bottom:18px}
.token-create-command .copy-corner{top:50%;transform:translateY(-50%)}
.token-secret-note{grid-column:1/-1;padding:11px 12px;border:1px solid rgba(245,184,92,.24);border-radius:11px;background:rgba(245,184,92,.055);display:grid;grid-template-columns:auto minmax(0,1fr);align-items:start;gap:9px;color:var(--app-text-muted);font-size:11px;line-height:1.48}
.token-secret-note>span{width:20px;height:20px;border-radius:7px;background:rgba(245,184,92,.13);color:var(--app-warn);font-weight:900;display:grid;place-items:center}
.token-secret-note p{margin:1px 0 0}.token-secret-note strong{color:var(--app-text-soft)}.token-secret-note code{color:var(--app-text-soft)}
.field{display:grid;gap:7px;min-width:0}
.field>label,.field-label{color:var(--app-text-soft);font-size:12px;font-weight:700}
.optional-label{margin-left:6px;color:var(--app-text-muted);font-weight:500}
.field input,.field select,.field textarea{width:100%;min-height:44px;padding:10px 12px;border:1px solid var(--app-border);border-radius:12px;background:var(--app-panel-soft);color:var(--app-text)}
.field textarea{min-height:116px;resize:vertical;line-height:1.5}
.field input:focus,.field select:focus,.field textarea:focus{border-color:var(--app-focus)}
.form-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:13px}
.form-grid .span-all{grid-column:1/-1}
.preflight{padding:12px 14px;border:1px solid var(--app-border);border-radius:13px;background:var(--app-panel-soft);display:flex;gap:9px 16px;align-items:center;flex-wrap:wrap;color:var(--app-text-muted);font-size:12px}
.preflight strong{color:var(--app-text-soft)}
.playground-layout{width:min(100%,960px);margin-inline:auto}
.playground-panel{overflow:hidden}
html.js-ready .playground-interactive.js-only{display:flex!important;flex-direction:column}
html.js-ready .playground-interactive{min-height:clamp(520px,64vh,720px)}
.playground-composer{min-height:0;flex:1;display:flex;flex-direction:column}
.playground-toolbar{min-height:64px;padding:10px 16px;border-bottom:1px solid var(--app-border);display:flex;align-items:center;justify-content:space-between;gap:14px;background:var(--app-panel-soft)}
.model-picker{position:relative;z-index:4;min-width:0;display:grid;grid-template-columns:auto minmax(220px,430px);align-items:center;gap:10px}
.model-picker-label{color:var(--app-text-soft);font-size:12px;font-weight:700}
.model-picker-trigger{min-width:0;min-height:48px;padding:5px 10px 5px 6px;border:1px solid var(--app-border);border-radius:12px;background:var(--app-panel-strong);color:var(--app-text);display:flex;align-items:center;gap:9px;text-align:left;cursor:pointer}
.model-picker-trigger:hover,.model-picker-trigger[aria-expanded="true"]{border-color:var(--app-border-strong);background:rgba(255,255,255,.045)}
.model-picker-trigger:disabled{cursor:not-allowed;opacity:.58}
.model-picker-trigger-copy{min-width:0;flex:1;display:grid;gap:2px}
.model-picker-trigger-copy strong,.model-picker-trigger-copy span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.model-picker-trigger-copy strong{font-size:13px}
.model-picker-trigger-copy span{color:var(--app-text-muted);font-size:11px}
.model-picker-chevron{width:20px;flex:0 0 auto;color:var(--app-text-muted);font-size:19px;line-height:1;transform:translateY(-2px);transition:transform var(--app-standard) cubic-bezier(.2,0,.38,.9)}
.model-picker-trigger[aria-expanded="true"] .model-picker-chevron{transform:translateY(2px) rotate(180deg)}
.model-lab-mark{width:36px;height:36px;flex:0 0 auto;border:1px solid rgba(255,255,255,.1);border-radius:10px;background:#111318;color:#d9dee6;display:grid;place-items:center;box-shadow:inset 0 1px 0 rgba(255,255,255,.05);font-size:11px;font-weight:800;letter-spacing:-.03em}
.model-lab-mark svg{width:20px;height:20px;fill:currentColor}
.model-lab-image{width:100%;height:100%;border-radius:inherit;object-fit:cover}.model-lab-image--contain{width:62%;height:62%;object-fit:contain}
.model-lab--hauhau svg{width:34px;height:34px;border-radius:9px}
.model-lab--google,.model-lab--deepmind{color:#6ea8ff}.model-lab--deepseek{color:#7c98ff}.model-lab--mistral{color:#ff8d62}.model-lab--meta-ai{color:#b391ff}.model-lab--qwen{color:#8f8bff}.model-lab--minimax{color:#ff7890}.model-lab--moonshot-ai{color:#f0f2f5}.model-lab--nvidia{color:#93d329}.model-lab--z-ai{color:#c9cdd4}.model-lab--openai{color:#f0f2f5}.model-lab--huggingface{color:#ffd86a}.model-lab--hauhau{color:#f6e47b;background:linear-gradient(135deg,#7750c8,#a47225)}
.model-picker-panel{position:absolute;z-index:40;top:calc(100% + 8px);left:46px;width:min(500px,calc(100vw - 72px));padding:7px;border:1px solid var(--app-border-strong);border-radius:15px;background:rgba(18,20,25,.985);box-shadow:0 28px 72px rgba(0,0,0,.52);animation:model-picker-in var(--app-standard) cubic-bezier(.2,0,.38,.9) both}
@keyframes model-picker-in{from{opacity:0;transform:translateY(-5px) scale(.99)}to{opacity:1;transform:none}}
.model-picker-panel>header{min-height:42px;padding:3px 6px 7px;display:flex;align-items:center;justify-content:space-between;gap:12px}
.model-picker-panel>header>div{min-width:0;display:grid;gap:2px}.model-picker-panel>header strong{font-size:12px}.model-picker-panel>header span{color:var(--app-text-muted);font-size:11px}
.model-picker-panel>header button{width:36px;height:36px;border:0;border-radius:9px;background:transparent;color:var(--app-text-muted);font-size:20px;cursor:pointer}.model-picker-panel>header button:hover{background:rgba(255,255,255,.05);color:var(--app-text)}
.model-picker-list{max-height:min(360px,52vh);overflow:auto;overscroll-behavior:contain;display:grid;gap:5px;scrollbar-width:thin}
.model-picker-option{width:100%;min-width:0;min-height:68px;padding:8px;border:1px solid transparent;border-radius:11px;background:rgba(255,255,255,.012);color:var(--app-text);display:flex;align-items:center;gap:10px;text-align:left;cursor:pointer}
.model-picker-option:hover,.model-picker-option:focus-visible{border-color:var(--app-border);background:rgba(255,255,255,.04)}
.model-picker-option[aria-selected="true"]{border-color:rgba(255,107,122,.34);background:rgba(255,107,122,.075)}
.model-picker-option-copy{min-width:0;flex:1;display:grid;gap:2px}.model-picker-option-copy strong,.model-picker-option-copy span,.model-picker-option-copy small{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.model-picker-option-copy strong{font-size:13px}.model-picker-option-copy span{color:var(--app-text-soft);font-size:11px}.model-picker-option-copy small{color:var(--app-text-muted);font-size:10px}
.model-picker-check{width:23px;height:23px;flex:0 0 auto;border:1px solid var(--app-border);border-radius:999px;color:transparent;display:grid;place-items:center;font-size:11px}.model-picker-option[aria-selected="true"] .model-picker-check{border-color:rgba(255,107,122,.5);background:rgba(255,107,122,.16);color:var(--app-accent-strong)}
.playground-provider-note{flex:0 0 auto;color:var(--app-text-muted);font-size:12px;display:inline-flex;align-items:center;gap:7px}
.playground-thread{min-height:280px;flex:1;padding:18px;overflow:auto;display:grid;align-content:start;gap:13px;background:linear-gradient(180deg,rgba(9,10,13,.45),rgba(16,18,23,.5))}
.message{max-width:min(85%,720px);padding:12px 14px;border:1px solid var(--app-border);border-radius:15px;white-space:pre-wrap;overflow-wrap:anywhere}
.message.user{justify-self:end;background:rgba(110,168,255,.1);border-color:rgba(110,168,255,.24)}
.message.assistant{justify-self:start;background:var(--app-panel-strong)}
.message.failed{border-color:rgba(255,84,73,.38);background:rgba(255,84,73,.08);color:var(--app-danger)}
.message.completed{animation:message-complete 420ms cubic-bezier(0,0,.38,.9) both}
.message .message-label{display:block;margin-bottom:5px;color:var(--app-text-muted);font-size:12px;font-weight:800;text-transform:uppercase;letter-spacing:.06em}
.message .message-content{display:block}
.message-actions{display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin-top:10px;padding-top:9px;border-top:1px solid var(--app-border)}
.message-actions .quiet-button{min-height:44px;padding:7px 10px;font-size:12px}
.message-result{display:flex;align-items:center;gap:7px;flex-wrap:wrap;margin-top:10px;color:var(--app-good);font-size:12px;font-weight:700}
.message-result-mark{width:20px;height:20px;border-radius:999px;display:grid;place-items:center;background:rgba(88,214,168,.13)}
.message-details{margin-top:9px;color:var(--app-text-soft)}
.message-details>summary,.playground-composer details.field>summary{min-height:44px;padding:8px 10px;border-radius:10px;display:flex;align-items:center;cursor:pointer;list-style-position:inside}
.message-details>summary:hover,.playground-composer details.field>summary:hover{background:rgba(255,255,255,.035);color:var(--app-text)}
.message-details>.table-secondary{padding:2px 10px 8px;overflow-wrap:anywhere}
.message.failed .message-result.incomplete{color:var(--app-danger)}
.message.failed .message-result.incomplete .message-result-mark{background:rgba(255,84,73,.14)}
.message-recovery-impact{margin:8px 0 0;color:var(--app-text-soft)}
.recovery-actions{display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin-top:10px}
@keyframes message-complete{from{border-color:rgba(88,214,168,.55);background:rgba(88,214,168,.1);transform:translateY(4px)}to{border-color:var(--app-border);background:var(--app-panel-strong);transform:none}}
.playground-empty{min-height:150px;display:grid;place-items:center;text-align:center;color:var(--app-text-muted)}
.playground-thread:has(>.playground-empty:only-child){min-height:0}
.playground-empty strong{color:var(--app-text);font-size:18px}
.playground-empty p{margin:7px 0 0;font-size:13px}
.playground-input-shell{padding:14px 16px 10px;border-top:1px solid var(--app-border);display:grid;gap:8px;background:var(--app-panel-soft)}
.playground-message-field textarea{min-height:72px;max-height:180px;font-size:15px}
.playground-input-shell>.result-summary{margin:0}
.request-settings{border-top:1px solid var(--app-border)}
.request-settings>summary{min-height:48px;padding:9px 16px;border-radius:0;color:var(--app-text-soft);cursor:pointer;font-size:12px;font-weight:700;display:flex;align-items:center;justify-content:space-between;gap:12px}
.request-settings>summary:hover{background:rgba(255,255,255,.035)}
.request-settings>summary .optional-label{min-width:0;margin-left:auto;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;text-align:right;font-weight:500}
.playground-settings-body{padding:4px 16px 16px;display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:14px}
.playground-settings-body>.field:first-child,.playground-settings-body>.preflight,.playground-settings-foot{grid-column:1/-1}
.playground-settings-foot{display:flex;align-items:center;justify-content:space-between;gap:14px}
.playground-settings-foot p{margin:0}
.playground-send-row{justify-content:flex-end}
.playground-send-row [data-playground-clear]{margin-right:auto}
.playground-send-row .primary-button{min-width:112px}
.playground-meta{padding:13px 18px;border-top:1px solid var(--app-border);display:flex;align-items:center;justify-content:space-between;gap:12px;color:var(--app-text-muted);font-size:12px}

/* Landing /playground port: the pg-* surface keeps the public experience and
   gateway dashboard visually identical while the request wiring stays local. */
.pg-page{
  --pg-pit:#0d0e11;--pg-surface:#121419;--pg-raised:#1a1d23;
  --pg-line:rgba(229,231,235,.12);--pg-line-soft:rgba(229,231,235,.075);
  --pg-snow:#f4f5f7;--pg-fog:#b2b8c2;--pg-dim:#7e8794;
  --pg-accent:#c54459;--pg-accent-soft:#d67866;--pg-accent-deep:#8e2e42;
  --pg-live:#58d6a8;--pg-ease:cubic-bezier(.16,1,.3,1);
  width:min(100%,1040px);margin-inline:auto
}
.pg-page button{font:inherit}
.pg-page svg{width:1rem;height:1rem;fill:none;stroke:currentColor;stroke-width:1.7;stroke-linecap:round;stroke-linejoin:round}
.playground-interactive,.playground-interactive>form{display:block!important;min-width:0}
.pg-toolbar{position:relative;z-index:35;display:flex;align-items:center;justify-content:space-between;gap:1rem}
.pg-mode-tabs{position:relative;display:inline-flex;align-items:center;gap:.25rem;padding:.28rem;border:1px solid var(--pg-line-soft);border-radius:.75rem;background:rgba(16,16,19,.85);box-shadow:inset 0 1px 0 rgba(229,231,235,.025)}
.pg-mode-pill{position:absolute;top:.28rem;bottom:.28rem;left:.28rem;width:calc((100% - .56rem)/3);border-radius:.52rem;background:var(--pg-raised);box-shadow:inset 0 1px 0 rgba(229,231,235,.07),0 5px 14px -9px #000;transition:transform .34s var(--pg-ease)}
.pg-page[data-playground-mode="image"] .pg-mode-pill{transform:translateX(100%)}
.pg-page[data-playground-mode="speech"] .pg-mode-pill{transform:translateX(200%)}
.pg-mode-tabs button{position:relative;z-index:1;min-width:6.2rem;min-height:2.75rem;padding:0 .9rem;border:0;border-radius:.52rem;background:transparent;color:var(--pg-dim);display:inline-flex;align-items:center;justify-content:center;gap:.45rem;font-size:.76rem;font-weight:700;transition:color .18s ease,transform .16s var(--pg-ease)}
.pg-mode-tabs button:hover:not(:disabled),.pg-mode-tabs button.is-active{color:var(--pg-snow)}
.pg-mode-tabs button.is-active svg{color:var(--app-accent-strong)}
.pg-mode-tabs button:active:not(:disabled){transform:scale(.97)}
.pg-mode-tabs button:disabled{cursor:not-allowed;opacity:.42}
.pg-mode-soon{display:none;font-size:.5rem;font-weight:600}
.pg-logo-tile{display:inline-flex;flex:none;align-items:center;justify-content:center;border:1px solid rgba(229,231,235,.1);border-radius:.55rem;color:var(--pg-snow);background:rgba(13,13,15,.85);box-shadow:inset 0 1px 0 rgba(255,255,255,.05)}
.pg-logo-tile .model-lab-mark{width:100%;height:100%;border:0;border-radius:inherit;background:transparent}
.pg-logo-tile .model-lab-mark svg{width:60%;height:60%;fill:currentColor;stroke:none}
.pg-logo-tile .model-lab-mark img{width:100%;height:100%;border-radius:inherit;object-fit:cover}
.pg-logo-tile .model-lab-mark .model-lab-image--contain{width:62%;height:62%;object-fit:contain}
.pg-model{position:relative;min-width:0;width:min(22rem,44vw)}
.pg-model-trigger{width:100%;min-height:3.35rem;padding:0 .75rem 0 .52rem;border:1px solid var(--pg-line);border-radius:.75rem;background:rgba(22,22,26,.9);color:var(--pg-snow);display:flex;align-items:center;gap:.7rem;text-align:left;box-shadow:inset 0 1px 0 rgba(229,231,235,.035);transition:border-color .2s ease,background .2s ease,transform .16s var(--pg-ease)}
.pg-model-trigger:hover:not(:disabled){border-color:rgba(229,231,235,.22);background:rgba(26,27,32,.95)}
.pg-model-trigger:active:not(:disabled){transform:translateY(1px)}
.pg-model-trigger:disabled{cursor:not-allowed;opacity:.55}
.pg-model-trigger-logo{width:2.2rem;height:2.2rem}
.pg-model-trigger-copy{min-width:0;flex:1;display:flex;flex-direction:column;gap:.12rem}
.pg-model-trigger-name,.pg-model-trigger-purpose{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.pg-model-trigger-name{color:var(--pg-snow);font-size:.8rem;font-weight:700}
.pg-model-trigger-purpose{color:var(--pg-dim);font-size:.65rem}
.pg-model-trigger-chevron{flex:none;color:var(--pg-dim);transition:transform .22s var(--pg-ease)}
.pg-model-trigger[aria-expanded="true"] .pg-model-trigger-chevron{transform:rotate(180deg)}
.pg-model-backdrop{display:none}
.pg-model-panel{position:absolute;z-index:45;top:calc(100% + .45rem);right:0;width:min(25rem,calc(100vw - 2rem));padding:.45rem;border:1px solid rgba(229,231,235,.14);border-radius:.95rem;background:rgba(15,15,18,.99);box-shadow:inset 0 1px 0 rgba(229,231,235,.05),0 34px 80px -34px #000;animation:pg-panel-in .24s var(--pg-ease) both}
@keyframes pg-panel-in{from{opacity:0;transform:translateY(-5px) scale(.99)}to{opacity:1;transform:none}}
.pg-model-panel-grip{display:none}
.pg-model-panel-head{min-height:2.55rem;padding:.15rem .55rem .35rem;display:flex;align-items:center;justify-content:space-between;gap:.75rem}
.pg-model-panel-head-copy{min-width:0;display:flex;flex-direction:column}
.pg-model-panel-head-copy strong{color:var(--pg-fog);font-size:.7rem}
.pg-model-panel-head-copy span{color:var(--pg-dim);font-size:.58rem}
.pg-model-panel-head button{display:flex;width:2.1rem;height:2.1rem;border:0;border-radius:.55rem;background:transparent;color:var(--pg-dim);align-items:center;justify-content:center}
.pg-model-panel-head button:hover{background:rgba(229,231,235,.06);color:var(--pg-snow)}
.pg-model-list{max-height:min(23rem,54dvh);display:flex;flex-direction:column;gap:.3rem;overflow-y:auto;overscroll-behavior:contain;scrollbar-width:thin}
.pg-model-option{min-width:0;min-height:4.4rem;padding:.65rem;border:1px solid transparent;border-radius:.7rem;background:rgba(229,231,235,.012);color:var(--pg-snow);display:flex;align-items:center;gap:.75rem;text-align:left;transition:border-color .18s ease,background .18s ease,transform .15s var(--pg-ease)}
.pg-model-option:hover,.pg-model-option:focus-visible{border-color:rgba(229,231,235,.1);background:rgba(229,231,235,.04)}
.pg-model-option:active{transform:scale(.995)}
.pg-model-option.is-selected{border-color:rgba(214,120,102,.28);background:rgba(197,68,89,.07)}
.pg-model-option-logo{width:2.55rem;height:2.55rem;border-radius:.7rem}
.pg-model-option-copy{min-width:0;flex:1;display:flex;flex-direction:column;gap:.13rem}
.pg-model-option-name,.pg-model-option-purpose{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.pg-model-option-name{font-size:.8rem;font-weight:700}
.pg-model-option-purpose{color:var(--pg-fog);font-size:.68rem}
.pg-model-option-meta{margin-top:.15rem;color:var(--pg-dim);display:flex;flex-wrap:wrap;gap:.25rem .6rem;font-size:.58rem}
.pg-model-option-meta>span+span::before{content:"·";margin-right:.6rem;opacity:.6}
.pg-model-option-check{width:1.45rem;height:1.45rem;flex:none;border:1px solid var(--pg-line);border-radius:50%;color:transparent;display:flex;align-items:center;justify-content:center}
.pg-model-option-check svg{width:.85rem;height:.85rem}
.pg-model-option.is-selected .pg-model-option-check{border-color:var(--pg-accent-soft);background:rgba(197,68,89,.18);color:var(--pg-snow)}
.pg-meta-row{margin:1rem 0 .8rem;display:flex;align-items:center;justify-content:space-between;gap:1rem}
.pg-preview-note{min-width:0;margin:0!important;color:var(--pg-dim);display:flex;align-items:center;gap:.5rem;font-size:.7rem;line-height:1.4}
.pg-preview-note>span{width:.38rem;height:.38rem;flex:none;border-radius:50%;background:var(--app-good);box-shadow:0 0 0 4px rgba(88,214,168,.08)}
.pg-evidence-link{min-height:2.3rem;flex:none;padding:.35rem .2rem;color:var(--pg-fog);display:inline-flex;align-items:center;font-size:.68rem;text-decoration:none}
.pg-evidence-link:hover{color:var(--pg-snow)}
.pg-surface{height:clamp(32rem,calc(100dvh - 19rem),42rem);min-height:32rem;overflow:hidden;border:1px solid rgba(229,231,235,.1);border-radius:1rem;background:var(--pg-surface);box-shadow:inset 0 1px 0 rgba(229,231,235,.04),0 28px 70px -42px #000}
.pg-mode-stack,.pg-mode-panel{height:100%;min-height:0}
.pg-mode-panel{animation:pg-mode-in .28s var(--pg-ease) backwards}
@keyframes pg-mode-in{from{opacity:0;transform:translateY(6px) scale(.998)}to{opacity:1;transform:none}}
.pg-chat{height:100%;min-height:0;display:flex;flex-direction:column}
.pg-chat-thread{min-height:0;flex:1;overflow-y:auto;overscroll-behavior:contain;scrollbar-gutter:stable;scrollbar-width:thin}
.pg-chat-thread.is-empty{display:grid;place-items:center;padding:3rem 1.5rem 2rem}
.pg-chat-empty{width:min(100%,38rem);text-align:center}
.pg-chat-empty h2,.pg-section-heading h2{margin:0;color:var(--pg-snow);font-size:clamp(1.45rem,3vw,1.85rem);font-weight:650;letter-spacing:-.04em}
.pg-chat-empty>p:not(.pg-empty-model),.pg-section-heading p{margin:.55rem 0 0!important;color:var(--pg-fog);font-size:.82rem}
.pg-empty-model{width:max-content;max-width:100%;margin:0 auto 1.15rem!important;padding:.3rem .8rem .3rem .34rem;border:1px solid var(--pg-line-soft);border-radius:999px;background:rgba(229,231,235,.02);color:var(--pg-fog);display:flex;align-items:center;gap:.5rem;font-size:.7rem;font-weight:650}
.pg-empty-model-logo{width:1.65rem;height:1.65rem;border-radius:50%}
.pg-empty-model em{padding-left:.6rem;border-left:1px solid var(--pg-line);color:var(--pg-fog);font-size:.6rem;font-style:normal}
.pg-starters{margin-top:1.8rem;display:flex;flex-wrap:wrap;justify-content:center;gap:.45rem}
.pg-starters button{min-height:2.75rem;padding:0 .9rem;border:1px solid var(--pg-line);border-radius:999px;background:rgba(229,231,235,.02);color:var(--pg-fog);font-size:.72rem;font-weight:650}
.pg-starters button:hover{border-color:rgba(214,120,102,.3);background:rgba(229,231,235,.045);color:var(--pg-snow);transform:translateY(-1px)}
.pg-messages{width:min(100%,48rem);margin:0 auto;padding:1.25rem 1.5rem 2.5rem}
.pg-thread-actions{min-height:2.4rem;margin-bottom:1.5rem;border-bottom:1px solid var(--pg-line-soft);color:var(--pg-dim);display:flex;align-items:center;justify-content:space-between;gap:1rem;font-size:.68rem}
.pg-thread-actions button{min-height:2.35rem;border:0;background:transparent;color:var(--pg-dim)}
.pg-thread-actions button:hover{color:var(--pg-snow)}
.pg-thread-model{min-width:0;color:var(--pg-fog);display:flex;align-items:center;gap:.45rem;font-weight:650}
.pg-thread-model-logo{width:1.45rem;height:1.45rem;border-radius:.4rem}
.pg-message{animation:pg-line-in .3s ease-out backwards}
@keyframes pg-line-in{from{opacity:0;transform:translateY(5px)}to{opacity:1;transform:none}}
.pg-message+.pg-message{margin-top:1.6rem}
.pg-message.is-user{width:fit-content;max-width:min(88%,36rem);margin-left:auto}
.pg-message-author{margin-bottom:.45rem;color:var(--pg-dim);display:flex;align-items:center;gap:.42rem;font-size:.65rem;font-weight:650}
.pg-message.is-user .pg-message-author{justify-content:flex-end}
.pg-message-logo{width:1.2rem;height:1.2rem;border-radius:.34rem}
.pg-message-body{color:var(--pg-snow);font-size:.88rem;line-height:1.72;white-space:pre-wrap;overflow-wrap:anywhere}
.pg-message.is-user .pg-message-body{padding:.7rem .9rem;border-radius:.7rem .7rem .18rem .7rem;background:var(--pg-raised);box-shadow:inset 0 1px 0 rgba(229,231,235,.05)}
.pg-message-actions,.pg-output-actions{margin-top:.65rem;display:flex;flex-wrap:wrap;align-items:center;gap:.25rem}
.pg-text-action{min-height:2.75rem;padding:0 .48rem;border:0;border-radius:.35rem;background:transparent;color:var(--pg-dim);display:inline-flex;align-items:center;gap:.35rem;font-size:.67rem;font-weight:650;text-decoration:none}
.pg-text-action:hover:not(:disabled){background:var(--pg-pit);color:var(--pg-snow)}
.pg-message.is-failed .pg-message-body{color:var(--app-danger)}
.pg-typing{min-height:1.5rem;display:inline-flex;align-items:center;gap:.25rem}
.pg-typing span{width:.3rem;height:.3rem;border-radius:50%;background:var(--pg-dim);animation:pg-typing 1.1s ease-in-out infinite}
.pg-typing span:nth-child(2){animation-delay:120ms}.pg-typing span:nth-child(3){animation-delay:240ms}
@keyframes pg-typing{0%,65%,100%{opacity:.28;transform:none}32%{opacity:1;transform:translateY(-2px)}}
.pg-composer-wrap{position:relative;padding:.9rem 1rem .75rem;border-top:1px solid var(--pg-line-soft);background:var(--pg-pit)}
.pg-composer{width:min(100%,48rem);min-height:3.15rem;margin:0 auto;padding:.42rem .42rem .42rem .9rem;border:1px solid var(--pg-line);border-radius:.65rem;background:var(--pg-surface);display:flex;align-items:flex-end;gap:.6rem;box-shadow:inset 0 1px 0 rgba(229,231,235,.03)}
.pg-composer:focus-within{border-color:rgba(214,120,102,.55);box-shadow:0 0 0 3px rgba(197,68,89,.12)}
.pg-composer textarea{min-height:2.2rem;max-height:7rem;flex:1;padding:.48rem 0;border:0;outline:0;resize:none;background:transparent;color:var(--pg-snow);font-size:.84rem;line-height:1.45}
.pg-composer-wrap>p{width:min(100%,48rem);margin:.4rem auto 0!important;color:var(--pg-fog);font-size:.62rem}
.pg-composer-count,.pg-count{color:var(--pg-dim);font-size:.6rem;font-style:normal;font-variant-numeric:tabular-nums}
.pg-composer-count{align-self:center}
.pg-send,.pg-stop,.pg-primary-action{border-radius:.48rem;display:inline-flex;align-items:center;justify-content:center;gap:.45rem;font-size:.73rem;font-weight:700}
.pg-send{width:2.75rem;height:2.75rem;flex:none;border:0;background:var(--app-accent);color:#25090e;box-shadow:0 2px 0 #aa3e50}
.pg-send:hover:not(:disabled){transform:translateY(-1px)}
.pg-send:disabled,.pg-primary-action:disabled{cursor:not-allowed;opacity:.35;box-shadow:none}
.pg-stop{min-height:2.3rem;padding:0 .7rem;border:1px solid var(--pg-line);background:transparent;color:var(--pg-fog)}
.pg-media{height:100%;min-height:0;display:grid;grid-template-columns:minmax(18rem,23rem) minmax(0,1fr)}
.pg-settings{padding:2rem;border-right:1px solid var(--pg-line-soft);background:var(--pg-pit);display:flex;flex-direction:column;overflow:auto}
.pg-section-heading{margin-bottom:1.8rem}
.pg-field{display:block}.pg-field>span,.pg-ratio-field legend,.pg-voice-field legend{margin-bottom:.5rem;color:var(--pg-fog);display:flex;align-items:baseline;justify-content:space-between;gap:.75rem;font-size:.68rem;font-weight:650}
.pg-field textarea,.pg-field select{width:100%;padding:.75rem;border:1px solid var(--pg-line);border-radius:.5rem;outline:0;background:var(--pg-surface);color:var(--pg-snow);font-size:.8rem;line-height:1.6;box-shadow:inset 0 1px 0 rgba(229,231,235,.025)}
.pg-field textarea{resize:vertical}.pg-field textarea:focus,.pg-field select:focus{border-color:rgba(214,120,102,.55);box-shadow:0 0 0 3px rgba(197,68,89,.12)}
.pg-ratio-field,.pg-voice-field{min-width:0;margin:1rem 0 0;padding:0;border:0}
.pg-ratio-field>div{display:grid;grid-template-columns:repeat(4,1fr);gap:.35rem}
.pg-ratio-field button{min-height:2.2rem;border:1px solid var(--pg-line);border-radius:.45rem;background:transparent;color:var(--pg-dim);display:flex;align-items:center;justify-content:center;gap:.4rem;font-size:.64rem}
.pg-ratio-field button.is-active{border-color:rgba(214,120,102,.45);background:rgba(197,68,89,.12);color:var(--pg-snow)}
.pg-ratio-glyph{height:.72rem;border:1px solid currentColor;border-radius:2px}.pg-ratio-glyph.ratio-1-1{aspect-ratio:1}.pg-ratio-glyph.ratio-4-3{aspect-ratio:4/3}.pg-ratio-glyph.ratio-3-4{aspect-ratio:3/4}.pg-ratio-glyph.ratio-16-9{aspect-ratio:16/9}
.pg-voice-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:.4rem}
.pg-voice-option{min-width:0;min-height:3.1rem;padding:.5rem .6rem;border:1px solid var(--pg-line);border-radius:.55rem;background:rgba(229,231,235,.012);display:flex;align-items:center;gap:.5rem;cursor:pointer}
.pg-voice-option:hover{border-color:rgba(229,231,235,.18)}.pg-voice-option.is-selected{border-color:rgba(214,120,102,.45);background:rgba(197,68,89,.1)}
.pg-voice-copy{min-width:0;flex:1;display:flex;flex-direction:column}.pg-voice-copy strong{color:var(--pg-fog);font-size:.72rem}.pg-voice-copy em{color:var(--pg-dim);font-size:.6rem;font-style:normal}
.pg-voice-check{width:1.15rem;height:1.15rem;border:1px solid var(--pg-line);border-radius:50%;color:transparent;display:flex;align-items:center;justify-content:center}.pg-voice-check svg{width:.7rem;height:.7rem}.pg-voice-option.is-selected .pg-voice-check{color:var(--pg-snow);border-color:var(--pg-accent-soft);background:rgba(197,68,89,.2)}
.pg-primary-action{width:100%;min-height:2.75rem;margin-top:1.2rem;border:0;background:var(--app-accent);color:#25090e;box-shadow:0 3px 0 #aa3e50}
.pg-output{min-width:0;min-height:0;padding:2rem;display:grid;place-items:center;overflow:auto}
.pg-output-empty{max-width:23rem;color:var(--pg-dim);display:flex;flex-direction:column;align-items:center;text-align:center}.pg-output-empty>svg{width:3rem;height:3rem;padding:.75rem;margin-bottom:1rem;border:1px solid var(--pg-line);border-radius:.85rem}.pg-output-empty>strong{color:var(--pg-fog);font-size:.82rem}.pg-output-empty>span{margin-top:.35rem;font-size:.72rem}
.pg-image-result{width:100%;display:flex;flex-direction:column;align-items:center}.pg-image-frame{width:min(100%,32rem);min-height:18rem;border:1px dashed rgba(229,231,235,.15);border-radius:.75rem;background:radial-gradient(ellipse 65% 75% at 50% 28%,rgba(197,68,89,.06),transparent 72%),var(--pg-pit);display:flex;flex-direction:column;align-items:center;justify-content:center;overflow:hidden}.pg-image-frame.ratio-4-3{aspect-ratio:4/3}.pg-image-frame.ratio-3-4{width:min(72%,24rem);aspect-ratio:3/4}.pg-image-frame.ratio-16-9{aspect-ratio:16/9}.pg-image-frame.ratio-1-1{width:min(86%,28rem);aspect-ratio:1}
.pg-generated-image{width:100%;height:100%;object-fit:contain}.pg-image-meta{margin:.65rem 0 0!important;color:var(--pg-dim);font-size:.65rem}
.pg-image-frame.is-pending{position:relative;border-style:solid;border-color:rgba(214,120,102,.25)}
.pg-latent-grid{position:absolute;inset:0;display:grid;grid-template-columns:repeat(5,1fr);opacity:.2}.pg-latent-grid span{border:1px solid rgba(214,120,102,.16);animation:pg-latent 1.8s ease-in-out infinite}.pg-latent-grid span:nth-child(3n){animation-delay:-.6s}.pg-latent-grid span:nth-child(4n){animation-delay:-1.2s}@keyframes pg-latent{50%{background:rgba(214,120,102,.18)}}
.pg-generation-center{position:relative;z-index:1;padding:1rem;display:flex;flex-direction:column;align-items:center;text-align:center}.pg-generation-orb{width:3rem;height:3rem;margin-bottom:.8rem;border:1px solid rgba(214,120,102,.3);border-radius:50%;color:var(--app-accent-strong);display:grid;place-items:center;background:rgba(13,13,15,.78)}.pg-generation-center strong{font-size:.82rem}.pg-generation-detail,.pg-generation-kicker{margin-top:.35rem;color:var(--pg-dim);font-size:.65rem}.pg-generation-kicker{color:var(--pg-fog)}
.pg-audio-result{width:min(100%,32rem);padding:1.1rem;border:1px solid var(--pg-line);border-radius:.65rem;background:var(--pg-pit);display:grid;grid-template-columns:auto 1fr;align-items:center;gap:.9rem}.pg-play{width:2.8rem;height:2.8rem;border:0;border-radius:50%;background:var(--pg-raised);color:var(--pg-fog);display:grid;place-items:center}.pg-audio-result strong,.pg-audio-result span{display:block}.pg-audio-result strong{font-size:.8rem}.pg-audio-result span{margin-top:.2rem;color:var(--pg-dim);font-size:.66rem}.pg-audio-result audio{width:100%;grid-column:1/-1;margin-top:.4rem}.pg-audio-result .pg-output-actions{grid-column:1/-1}
.pg-waveform{height:3rem;grid-column:1/-1;display:flex;align-items:center;justify-content:center;gap:.2rem}.pg-waveform span{width:.18rem;height:var(--wave-height);border-radius:999px;background:var(--app-accent-strong);animation:pg-wave .9s ease-in-out infinite alternate;animation-delay:var(--wave-delay)}@keyframes pg-wave{to{height:20%;opacity:.45}}
.pg-network{margin-top:.85rem;border:1px solid rgba(229,231,235,.1);border-radius:.85rem;background:rgba(16,16,19,.9);overflow:hidden;box-shadow:inset 0 1px 0 rgba(229,231,235,.04),0 20px 50px -38px #000}
.pg-network-summary{width:100%;min-height:3.5rem;padding:0 1rem;border:0;background:transparent;color:var(--pg-snow);display:grid;grid-template-columns:auto auto 1fr auto auto;align-items:center;gap:.7rem;text-align:left}.pg-network-summary:hover{background:rgba(229,231,235,.02)}
.pg-network-grip{display:none}.pg-network-dot{width:.45rem;height:.45rem;border-radius:50%;background:var(--pg-dim)}.pg-network.is-busy .pg-network-dot{background:var(--app-accent-strong);animation:pg-dot 1.6s ease-in-out infinite}.pg-network.is-complete .pg-network-dot{background:var(--pg-live)}@keyframes pg-dot{50%{opacity:.6;box-shadow:0 0 0 5px transparent}}
.pg-network-labels{display:flex;flex-direction:column}.pg-network-title{font-size:.72rem;font-weight:700}.pg-network-state{color:var(--pg-dim);font-size:.61rem}.pg-network.is-complete .pg-network-state{color:var(--pg-live)}
.pg-network-model{min-width:0;color:var(--pg-fog);display:flex;align-items:center;gap:.5rem;font-size:.68rem}.pg-network-model-logo{width:1.55rem;height:1.55rem}.pg-network-model>span:last-child{max-width:16rem;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.pg-network-chevron{color:var(--pg-dim);transition:transform .2s var(--pg-ease)}.pg-network-summary[aria-expanded="true"] .pg-network-chevron{transform:rotate(180deg)}
.pg-network-body{padding:1rem;border-top:1px solid var(--pg-line-soft)}.pg-network-steps{margin:0;padding:0;display:grid;grid-template-columns:repeat(4,1fr);list-style:none}.pg-network-steps li{position:relative;min-width:0;display:flex;gap:.55rem}.pg-network-steps li:not(:last-child)::after{content:"";position:absolute;top:.55rem;left:1.1rem;right:0;height:1px;background:var(--pg-line)}.pg-step-marker{position:relative;z-index:1;width:1.1rem;height:1.1rem;flex:none;border:1px solid var(--pg-line);border-radius:50%;background:var(--pg-pit)}.pg-network-steps li.is-active .pg-step-marker{border-color:var(--app-accent-strong);box-shadow:0 0 0 4px rgba(255,107,122,.1)}.pg-network-steps li.is-done .pg-step-marker{border-color:var(--pg-live);background:var(--pg-live)}.pg-step-copy{min-width:0;display:flex;flex-direction:column}.pg-step-label{color:var(--pg-fog);font-size:.65rem;font-weight:700}.pg-step-detail{color:var(--pg-dim);font-size:.57rem}
.pg-network-facts{margin:1rem 0 0;display:grid;grid-template-columns:repeat(5,minmax(0,1fr));gap:.55rem}.pg-network-facts div{min-width:0;padding:.65rem;border:1px solid var(--pg-line-soft);border-radius:.55rem}.pg-network-facts dt{color:var(--pg-dim);font-size:.56rem}.pg-network-facts dd{margin:.2rem 0 0;overflow-wrap:anywhere;color:var(--pg-fog);font-size:.63rem}.pg-network-footnote{margin:.8rem 0 0!important;color:var(--pg-dim);font-size:.6rem}
.pg-advanced{margin-top:.85rem;border:1px solid var(--pg-line-soft);border-radius:.75rem;background:rgba(16,16,19,.55)}.pg-advanced>summary{min-height:3rem;padding:.65rem .9rem;color:var(--pg-fog);display:flex;align-items:center;justify-content:space-between;gap:1rem;cursor:pointer;font-size:.68rem;font-weight:700}.pg-advanced>summary span:last-child{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--pg-dim);font-weight:500}.pg-advanced-grid{padding:.2rem .9rem .9rem;display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:.75rem}.pg-advanced-field{min-width:0;display:flex;flex-direction:column;gap:.35rem}.pg-advanced-field>span{color:var(--pg-fog);font-size:.64rem;font-weight:650}.pg-advanced-field em,.pg-advanced-field small{color:var(--pg-dim);font-size:.58rem;font-style:normal}.pg-advanced-field input,.pg-advanced-field select,.pg-advanced-field textarea{width:100%;min-height:2.45rem;padding:.55rem .65rem;border:1px solid var(--pg-line);border-radius:.5rem;background:var(--pg-surface);color:var(--pg-snow);font-size:.7rem}.pg-advanced-field textarea{resize:vertical}.pg-advanced .preflight{padding:.65rem;border:1px solid var(--pg-line-soft);border-radius:.55rem}.pg-advanced .span-all{grid-column:1/-1}
.pg-local-note{margin:.6rem .2rem 0!important;color:var(--pg-dim);font-size:.6rem;text-align:right}
@media(max-width:780px){
  .app-main--playground{padding-top:1.25rem}
  .pg-toolbar{align-items:stretch;flex-direction:column-reverse}
  .pg-model{width:100%}.pg-mode-tabs{width:100%}.pg-mode-tabs button{min-width:0;flex:1;padding-inline:.45rem}
  .pg-model-backdrop:not([hidden]){position:fixed;inset:0;z-index:44;display:block;border:0;background:rgba(4,5,7,.68);backdrop-filter:blur(3px)}
  .pg-model-panel{position:fixed;z-index:45;left:max(.65rem,env(safe-area-inset-left));right:max(.65rem,env(safe-area-inset-right));top:auto;bottom:max(.65rem,env(safe-area-inset-bottom));width:auto;max-height:min(78dvh,38rem);border-radius:1rem;padding:.55rem}
  .pg-model-panel-grip{width:2.4rem;height:.23rem;margin:.1rem auto .45rem;border-radius:999px;background:var(--pg-line);display:block}
  .pg-model-list{max-height:calc(78dvh - 4.8rem)}
  .pg-meta-row{align-items:flex-start}.pg-evidence-link{display:none}
  .pg-surface{height:clamp(31rem,calc(100dvh - 19rem),39rem);min-height:31rem}
  .pg-media{grid-template-columns:1fr;overflow-y:auto}.pg-settings{border-right:0;border-bottom:1px solid var(--pg-line-soft)}.pg-output{min-height:22rem}
  .pg-network-steps{grid-template-columns:1fr;gap:.8rem}.pg-network-steps li:not(:last-child)::after{top:1.1rem;bottom:-.8rem;left:.55rem;right:auto;width:1px;height:auto}
  .pg-network-facts{grid-template-columns:repeat(2,minmax(0,1fr))}
}
@media(max-width:520px){
  .app-main--playground .page-head{margin-bottom:1rem}.app-main--playground .page-summary{font-size:.82rem}
  .pg-mode-tabs button{font-size:.7rem}.pg-mode-tabs button svg{display:none}.pg-mode-soon{display:none}
  .pg-preview-note{font-size:.64rem}
  .pg-chat-thread.is-empty{padding:2rem 1rem 1.5rem}.pg-starters{margin-top:1.25rem}.pg-starters button{min-height:2.75rem}
  .pg-composer-wrap{padding:.75rem}.pg-messages{padding:1rem 1rem 2rem}
  .pg-settings,.pg-output{padding:1.2rem}.pg-ratio-field>div{grid-template-columns:repeat(2,1fr)}.pg-voice-grid{grid-template-columns:1fr}
  .pg-network-model{display:none}.pg-network-summary{grid-template-columns:auto auto 1fr auto}
  .pg-network-facts,.pg-advanced-grid{grid-template-columns:1fr}.pg-advanced .span-all{grid-column:1}
}
html.motion-reduced .pg-page *{animation-duration:.01ms!important;animation-iteration-count:1!important;transition-duration:.01ms!important;scroll-behavior:auto!important}
@media(prefers-reduced-motion:reduce){.pg-page *{animation-duration:.01ms!important;animation-iteration-count:1!important;transition-duration:.01ms!important;scroll-behavior:auto!important}}


.activation-panel{margin-bottom:24px}
.activation-grid{display:grid;grid-template-columns:minmax(0,1.25fr) minmax(250px,.75fr);gap:22px;align-items:start}
.activation-next{padding:15px;border:1px solid var(--app-border);border-radius:13px;background:var(--app-panel-soft)}
.activation-next>strong{display:block}
.activation-next>p{margin:6px 0 13px;color:var(--app-text-muted);font-size:12px;line-height:1.5}
.provider-start-command{display:grid;gap:7px}
.settings-list{display:grid}
.settings-row{padding:16px 18px;border-bottom:1px solid var(--app-border);display:grid;grid-template-columns:minmax(0,1fr) auto;gap:16px;align-items:center}
.settings-row:last-child{border-bottom:0}
.settings-copy strong{display:block}
.settings-copy span{display:block;margin-top:3px;color:var(--app-text-muted);font-size:12px}
.settings-control[aria-pressed="true"],.settings-control[aria-checked="true"]{border-color:rgba(88,214,168,.4);background:rgba(88,214,168,.09);color:var(--app-good)}
.settings-control .switch-track{width:40px;height:24px;flex:0 0 auto;border-radius:999px;border:1px solid var(--app-border-strong);background:var(--app-panel-soft);position:relative;transition:background var(--app-fast) ease,border-color var(--app-fast) ease}
.settings-control .switch-track::after{content:"";position:absolute;top:3px;left:3px;width:16px;height:16px;border-radius:999px;background:var(--app-text-muted);transition:transform var(--app-standard) cubic-bezier(.2,0,.38,.9),background var(--app-fast) ease}
.settings-control[aria-checked="true"] .switch-track{border-color:rgba(88,214,168,.5);background:rgba(88,214,168,.16)}
.settings-control[aria-checked="true"] .switch-track::after{transform:translateX(16px);background:var(--app-good)}
.settings-links{display:grid;gap:8px}
.settings-link{min-height:64px;padding:12px;border:1px solid var(--app-border);border-radius:12px;background:var(--app-panel-soft);color:var(--app-text);text-decoration:none;display:grid;align-content:center}
.settings-link:hover{border-color:var(--app-border-strong);background:var(--app-panel-strong)}
.settings-link strong,.settings-link span{display:block}
.settings-link span{margin-top:3px;color:var(--app-text-muted);font-size:12px}
.fact-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px}
.fact{padding:12px;border:1px solid var(--app-border);border-radius:12px;background:var(--app-panel-soft)}
.fact span{display:block;color:var(--app-text-muted);font-size:12px}
.fact strong{display:block;margin-top:4px;overflow-wrap:anywhere;font-size:13px}
.section-gap{margin-top:18px}
.field-gap{height:12px}
.result-summary{color:var(--app-text-muted);font-size:12px}
.provider-scope{margin:-10px 0 18px}
.provider-scope strong{color:var(--app-text-soft)}
.table-action{white-space:nowrap}
.session-expired{position:fixed;left:50%;bottom:max(24px,env(safe-area-inset-bottom));z-index:120;width:min(620px,calc(100vw - 28px));transform:translateX(-50%);padding:13px 14px;border:1px solid rgba(245,184,92,.44);border-radius:15px;background:#211b12;box-shadow:var(--app-shadow);color:var(--app-text);font-size:13px;display:flex;align-items:center;justify-content:space-between;gap:14px;pointer-events:none}
.session-expired-copy{min-width:0}.session-expired-copy strong,.session-expired-copy span{display:block}.session-expired-copy span{margin-top:2px;color:var(--app-text-soft)}
.session-expired-actions{display:flex;align-items:center;gap:8px;flex:0 0 auto}.session-expired-actions button{pointer-events:auto}

.empty-block{min-height:190px;padding:28px;display:grid;place-items:center;text-align:center}
.empty-block-inner{max-width:420px}
.empty-symbol{width:46px;height:46px;margin:0 auto 13px;border:1px solid var(--app-border);border-radius:15px;display:grid;place-items:center;color:var(--app-text-muted)}
.empty-block h3{margin:0;font-size:17px}
.empty-block p{margin:7px 0 17px;color:var(--app-text-muted);font-size:13px}

.app-footer{padding:18px max(clamp(18px,3.1vw,52px),env(safe-area-inset-right)) max(18px,env(safe-area-inset-bottom)) max(clamp(18px,3.1vw,52px),env(safe-area-inset-left));border-top:1px solid var(--app-border);color:var(--app-text-muted);font-size:12px}
.app-footer-inner{max-width:1560px;margin-inline:auto;display:flex;align-items:center;justify-content:space-between;gap:12px}
.app-footer-inner--wide{max-width:1880px}

.verify-dialog{width:min(820px,calc(100vw - 28px));max-height:min(88vh,960px);padding:0;border:1px solid var(--app-border-strong);border-radius:22px;background:linear-gradient(180deg,#16191f 0,var(--app-panel) 180px);color:var(--app-text);box-shadow:0 28px 90px rgba(0,0,0,.58),0 0 0 1px rgba(255,255,255,.025);overflow:hidden}
.verify-dialog[open]{display:flex;flex-direction:column}
.verify-dialog::backdrop{background:rgba(4,5,7,.76);backdrop-filter:blur(8px) saturate(.8)}
.verify-head{position:relative;padding:24px 72px 19px 26px;display:block}
.verify-head-copy{min-width:0}
.verify-eyebrow{margin-bottom:8px;color:var(--app-good);font-size:10px;font-weight:900;letter-spacing:.11em;text-transform:uppercase;display:flex;align-items:center;gap:7px}
.verify-eyebrow-mark{width:7px;height:7px;border-radius:999px;background:var(--app-good);box-shadow:0 0 0 4px rgba(88,214,168,.1),0 0 16px rgba(88,214,168,.22)}
.verify-head h2{margin:0;font-size:clamp(21px,2.8vw,26px);line-height:1.15;letter-spacing:-.025em}
.verify-subject{margin:7px 0 0;color:var(--app-text-soft);font:11px/1.45 ui-monospace,SFMono-Regular,Menlo,monospace;overflow-wrap:anywhere}
.verify-interpretation{max-width:680px;margin:12px 0 0;padding-left:11px;border-left:2px solid rgba(110,168,255,.42);color:var(--app-text-muted);font-size:12px;line-height:1.52}
.verify-close{position:absolute;top:20px;right:20px;background:rgba(255,255,255,.025)}
.verify-close svg{width:18px;height:18px;fill:none;stroke:currentColor;stroke-width:1.8;stroke-linecap:round}
.verify-toolbar{min-height:58px;padding:7px 18px 7px 26px;border-top:1px solid var(--app-border);border-bottom:1px solid var(--app-border);background:rgba(8,10,13,.28);display:flex;align-items:center;justify-content:space-between;gap:14px}
.verify-source{min-width:0;margin:0;color:var(--app-text-muted);font-size:11px;display:flex;align-items:center;gap:8px}
.verify-source-dot{width:6px;height:6px;flex:0 0 auto;border-radius:999px;background:var(--app-good)}
.verify-actions{display:flex;align-items:center;justify-content:flex-end;gap:7px;flex-wrap:nowrap}
.verify-action-button{min-height:44px;padding:7px 10px;gap:7px}
.verify-action-button svg{width:16px;height:16px;fill:none;stroke:currentColor;stroke-width:1.65;stroke-linecap:round;stroke-linejoin:round}
.verify-body{min-height:0;padding:23px 26px 26px;overflow:auto;flex:1;scrollbar-gutter:stable}
.verify-state{margin:0}
.verify-level{padding:22px 0;border-bottom:1px solid var(--app-border)}
.verify-level:first-of-type{padding-top:0}.verify-level:last-child{border-bottom:0;padding-bottom:0}
.verify-section-head{margin-bottom:13px;display:flex;align-items:flex-end;justify-content:space-between;gap:16px}
.verify-section-head>div>span{display:block;margin-bottom:3px;color:var(--app-text-muted);font-size:10px;font-weight:800;letter-spacing:.07em;text-transform:uppercase}
.verify-section-head h3{margin:0;font-size:16px;line-height:1.3}
.verify-section-count{padding:4px 8px;border:1px solid var(--app-border);border-radius:999px;color:var(--app-text-muted);font-size:10px;font-weight:700;white-space:nowrap}
.verify-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px}
.verify-fact{min-height:116px;padding:14px;border:1px solid var(--app-border);border-radius:14px;background:linear-gradient(145deg,rgba(255,255,255,.027),rgba(255,255,255,.012));display:flex;flex-direction:column}
.verify-fact span{display:block;color:var(--app-text-muted);font-size:10px;font-weight:800;letter-spacing:.055em;text-transform:uppercase}
.verify-fact strong{display:block;margin-top:7px;color:var(--app-text);overflow-wrap:anywhere;font-size:14px;line-height:1.38}
.verify-fact small{display:block;margin-top:auto;padding-top:10px;color:var(--app-text-muted);font-size:10px;line-height:1.4}
.verify-section-description{max-width:570px;margin:-5px 0 13px;color:var(--app-text-muted);font-size:11px;line-height:1.48}
.verify-raw-toggle{width:100%;min-height:66px;padding:10px 12px;border:1px solid var(--app-border);border-radius:13px;background:var(--app-panel-soft);color:var(--app-text);display:grid;grid-template-columns:auto minmax(0,1fr) auto auto;align-items:center;gap:11px;text-align:left;cursor:pointer}
.verify-raw-toggle:hover{border-color:var(--app-border-strong);background:rgba(255,255,255,.035)}
.verify-raw-icon{width:38px;height:38px;border:1px solid rgba(110,168,255,.22);border-radius:10px;background:rgba(110,168,255,.07);color:#a9c8ff;font:700 11px/1 ui-monospace,SFMono-Regular,Menlo,monospace;display:grid;place-items:center}
.verify-raw-toggle-copy{min-width:0;display:grid;gap:2px}
.verify-raw-toggle-copy strong{font-size:12px}.verify-raw-toggle-copy small{color:var(--app-text-muted);font-size:10px}
.verify-raw-size{padding:4px 7px;border-radius:7px;background:rgba(255,255,255,.04);color:var(--app-text-muted);font:10px/1.2 ui-monospace,SFMono-Regular,Menlo,monospace}
.verify-raw-toggle>svg{width:18px;height:18px;fill:none;stroke:var(--app-text-muted);stroke-width:1.7;stroke-linecap:round;stroke-linejoin:round;transition:transform var(--app-fast) ease}
.verify-raw-toggle[aria-expanded="true"]>svg{transform:rotate(180deg)}
.raw-evidence{max-height:420px;margin:12px 0 0;padding:15px;border:1px solid var(--app-border);border-radius:13px;background:#0b0d10;color:#c8d0da;white-space:pre-wrap;overflow:auto;overflow-wrap:anywhere;font:11px/1.58 ui-monospace,SFMono-Regular,Menlo,monospace}
.model-detail-dialog{width:min(760px,calc(100vw - 28px));max-height:min(88vh,920px);padding:0;border:1px solid var(--app-border-strong);border-radius:22px;background:linear-gradient(180deg,#16191f 0,var(--app-panel) 190px);color:var(--app-text);box-shadow:0 28px 90px rgba(0,0,0,.58),0 0 0 1px rgba(255,255,255,.025);overflow:hidden}
.model-detail-dialog[open]{display:flex;flex-direction:column}
.model-detail-dialog::backdrop{background:rgba(4,5,7,.76);backdrop-filter:blur(8px) saturate(.8)}
.model-detail-shell-head{min-height:68px;padding:15px 18px 14px 22px;border-bottom:1px solid var(--app-border);display:flex;align-items:center;justify-content:space-between;gap:16px}
.model-detail-shell-head>div>span{display:block;margin-bottom:2px;color:var(--app-text-muted);font-size:10px;font-weight:800;letter-spacing:.08em;text-transform:uppercase}
.model-detail-shell-head h2{margin:0;font-size:18px}
.model-detail-close svg{width:18px;height:18px;fill:none;stroke:currentColor;stroke-width:1.8;stroke-linecap:round}
.model-detail-body{min-height:0;padding:24px;overflow:auto;flex:1;scrollbar-gutter:stable}
.model-detail-hero{display:grid;grid-template-columns:auto minmax(0,1fr) auto;align-items:center;gap:14px}
.model-detail-logo{width:58px;height:58px;display:block}
.model-detail-logo .model-lab-mark{width:58px;height:58px;border-radius:16px}
.model-detail-logo .model-lab-mark svg{width:29px;height:29px}
.model-detail-logo .model-lab--hauhau svg{width:54px;height:54px;border-radius:15px}
.model-detail-identity{min-width:0;display:grid;justify-items:start}
.model-detail-lab{margin-bottom:3px;color:var(--app-text-muted);font-size:10px;font-weight:900;letter-spacing:.09em;text-transform:uppercase}
.model-detail-identity h3{margin:0;font-size:22px;line-height:1.22;letter-spacing:-.02em}
.model-detail-identity code{max-width:100%;margin-top:5px;color:var(--app-text-muted);font-size:10px;overflow-wrap:anywhere}
.model-detail-purpose{max-width:610px;margin:18px 0 0;color:var(--app-text-soft);font-size:14px;line-height:1.55}
.model-detail-facts{margin:20px 0 0;display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:9px}
.model-detail-facts>div{min-width:0;padding:12px;border:1px solid var(--app-border);border-radius:13px;background:rgba(255,255,255,.018)}
.model-detail-facts dt{color:var(--app-text-muted);font-size:10px;font-weight:800;letter-spacing:.055em;text-transform:uppercase}
.model-detail-facts dd{margin:5px 0 0;color:var(--app-text);font-size:13px;font-weight:800;overflow-wrap:anywhere}
.model-detail-facts small{display:block;margin-top:5px;color:var(--app-text-muted);font-size:10px;line-height:1.4}
.model-detail-section{margin-top:22px;padding-top:20px;border-top:1px solid var(--app-border)}
.model-detail-section-head{margin-bottom:12px;display:flex;align-items:flex-end;justify-content:space-between;gap:12px}
.model-detail-section-head>div>span{display:block;margin-bottom:2px;color:var(--app-text-muted);font-size:10px;font-weight:800;letter-spacing:.065em;text-transform:uppercase}
.model-detail-section-head h4{margin:0;font-size:16px}
.model-detail-section-head>span{padding:4px 8px;border:1px solid var(--app-border);border-radius:999px;color:var(--app-text-muted);font-size:10px;font-weight:700}
.model-detail-capabilities{display:flex;gap:7px;flex-wrap:wrap}
.model-detail-capability{min-height:31px;padding:6px 9px;border:1px solid var(--app-border);border-radius:9px;background:rgba(255,255,255,.025);color:var(--app-text-soft);font-size:11px;font-weight:700;display:inline-flex;align-items:center}
.model-detail-capability.is-context{border-color:rgba(110,168,255,.25);background:rgba(110,168,255,.08);color:#a9c8ff}
.model-detail-price{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:8px 14px}
.model-detail-price .catalog-price-line{min-height:42px;padding:8px 10px;border:1px solid var(--app-border);border-radius:10px;background:rgba(255,255,255,.018)}
.model-detail-price .catalog-price-line.is-primary{margin:0}
.model-detail-section>p{margin:10px 0 0;color:var(--app-text-muted);font-size:10px;line-height:1.45}
.model-detail-actions{margin-top:22px;padding-top:20px;border-top:1px solid var(--app-border);display:flex;justify-content:flex-end;gap:8px;flex-wrap:wrap}
.model-detail-actions .primary-button,.model-detail-actions .quiet-button{min-width:150px}
.evidence-standalone{width:min(920px,calc(100% - 36px));margin:0 auto;padding:36px 0 72px}
.evidence-page-body{max-height:none}
.evidence-page-body .verify-level h2{margin:0 0 10px;font-size:15px}

.toast-region{position:fixed;right:max(18px,env(safe-area-inset-right));bottom:max(18px,env(safe-area-inset-bottom));z-index:100;display:grid;gap:8px;pointer-events:none}
body.session-expired-visible .toast-region{bottom:max(112px,calc(env(safe-area-inset-bottom) + 96px))}
.app-toast{padding:11px 13px;border:1px solid var(--app-border-strong);border-radius:12px;background:var(--app-panel-strong);box-shadow:var(--app-shadow);color:var(--app-text);font-size:13px;animation:toast-in var(--app-standard) cubic-bezier(.2,0,.38,.9) both}
@keyframes toast-in{from{opacity:0;transform:translateY(6px)}to{opacity:1;transform:none}}

@media(max-width:1359px){
  .launch-path-card{grid-template-columns:auto minmax(0,1fr)}
  .launch-path-card>a{grid-column:1/-1;width:100%}
}

@media(max-width:1120px){
  .app-shell{grid-template-columns:218px minmax(0,1fr)}
  .metric-grid{grid-template-columns:repeat(2,minmax(0,1fr))}
  .metric-grid--three{grid-template-columns:repeat(3,minmax(0,1fr))}
  .dashboard-layout{grid-template-columns:1fr}
}

@media(min-width:781px){
  html.sidebar-collapsed .app-shell{grid-template-columns:84px minmax(0,1fr)}
  html.sidebar-collapsed .app-sidebar{padding-inline:14px;align-items:stretch}
  html.sidebar-collapsed .app-brand{justify-content:center;padding-inline:0}
  html.sidebar-collapsed .app-brand-text,html.sidebar-collapsed .app-nav-label,html.sidebar-collapsed .app-nav .nav-text,html.sidebar-collapsed .advanced-nav-items{display:none}
  html.sidebar-collapsed .app-nav a,html.sidebar-collapsed .advanced-nav>summary{justify-content:center;padding-inline:10px}
  html.sidebar-collapsed .sidebar-collapse-button span{transform:rotate(180deg)}
}

@media(max-width:780px){
  .app-shell{display:block}
  .app-sidebar{position:relative;inset:auto;width:100%;height:auto;border-right:0;border-bottom:1px solid var(--app-border)}
  html.js-ready .app-sidebar{position:fixed;inset:0 auto 0 0;width:min(310px,88vw);height:100vh;height:100dvh;border-right:1px solid var(--app-border);border-bottom:0;transform:translateX(-105%);visibility:hidden;transition:transform var(--app-standard) cubic-bezier(.2,0,.38,.9),visibility 0s linear var(--app-standard);box-shadow:var(--app-shadow)}
  html.js-ready body.nav-open{overflow:hidden}
  html.js-ready body.nav-open .app-sidebar{transform:none;visibility:visible;transition-delay:0s}
  html.js-ready .icon-button.mobile-menu-button.js-only{display:inline-flex!important;width:44px}
  html.js-ready .icon-button.sidebar-collapse-button.js-only{display:none!important}
  html.js-ready .nav-scrim{position:fixed;inset:0;z-index:21;border:0;background:rgba(0,0,0,.58);display:none!important}
  html.js-ready body.nav-open .nav-scrim{display:block!important}
  .app-topbar{padding-inline:max(14px,env(safe-area-inset-left)) max(14px,env(safe-area-inset-right))}
  .topbar-context{flex:1;gap:8px;overflow:hidden}
  .topbar-context strong{max-width:min(26vw,130px)}
  .topbar-context .topbar-status{display:inline-flex;min-width:0;max-width:min(42vw,240px);gap:6px;font-size:12px;overflow:hidden}
  .topbar-status .state-indicator{flex:0 0 auto}
  .topbar-status [data-page-status-text]{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
  .topbar-actions{flex:0 0 auto}
  .page-head{grid-template-columns:1fr;align-items:start}
  .page-head-actions{justify-content:flex-start}
  .attention-card{grid-template-columns:auto minmax(0,1fr)}
  .attention-card>.soft-button,.attention-card>.primary-button{grid-column:1/-1;width:100%}
  .app-main{padding-inline:max(14px,env(safe-area-inset-left)) max(14px,env(safe-area-inset-right))}
  .app-main{padding-bottom:108px}
  .metric-grid{grid-template-columns:1fr 1fr}
  .metric-grid--three{grid-template-columns:repeat(3,minmax(0,1fr))}
  .launch-paths{grid-template-columns:1fr}
  .activation-grid{grid-template-columns:1fr}
  .verify-grid{grid-template-columns:1fr}
  .model-detail-facts{grid-template-columns:repeat(2,minmax(0,1fr))}
  .model-detail-facts>div:last-child{grid-column:1/-1}
  .model-detail-price{grid-template-columns:1fr}
  .app-footer{padding-inline:max(14px,env(safe-area-inset-left)) max(14px,env(safe-area-inset-right));padding-bottom:calc(max(18px,env(safe-area-inset-bottom)) + 78px)}
  .app-footer-inner{align-items:flex-start;flex-direction:column}
  html.js-ready .mobile-bottom-nav{position:fixed;left:max(10px,env(safe-area-inset-left));right:max(10px,env(safe-area-inset-right));bottom:max(10px,env(safe-area-inset-bottom));z-index:18;min-height:60px;padding:6px;border:1px solid var(--app-border);border-radius:18px;background:rgba(18,20,25,.95);box-shadow:0 16px 50px rgba(0,0,0,.38);backdrop-filter:blur(18px);display:grid;grid-template-columns:repeat(5,minmax(0,1fr));gap:3px}
  .session-expired,.toast-region{bottom:calc(max(10px,env(safe-area-inset-bottom)) + 72px)}
  body.session-expired-visible .toast-region{bottom:calc(max(10px,env(safe-area-inset-bottom)) + 206px)}
  .mobile-bottom-nav a,.mobile-bottom-nav button{min-width:0;border:0;border-radius:12px;background:transparent;color:var(--app-text-muted);display:grid;place-items:center;padding:8px 3px;text-decoration:none;font-size:12px;font-weight:700}
  .mobile-bottom-nav a[aria-current="page"],.mobile-bottom-nav button[aria-current="page"]{background:rgba(255,107,122,.11);color:var(--app-accent-strong)}
  html.js-ready .mobile-bottom-nav .js-only{display:grid!important}
  html.js-ready .playground-interactive.js-only{min-height:540px}
  .playground-toolbar{align-items:stretch;flex-direction:column;gap:8px}
  .model-picker{width:100%;grid-template-columns:auto minmax(0,1fr)}
  .model-picker-panel{left:0;width:100%}
  .playground-thread{min-height:180px}
  .playground-empty{min-height:140px}
  .playground-settings-body{grid-template-columns:1fr}
  .playground-settings-body>.field,.playground-settings-body>.preflight,.playground-settings-foot{grid-column:1}
}

@media(max-width:520px){
  .topbar-actions .soft-button .button-label{display:none}
  .metric-grid{grid-template-columns:1fr}
  .metric-grid--three{grid-template-columns:1fr}
  .metric{padding:15px}
  .page-head h1{font-size:34px}
  .panel-head{align-items:flex-start;flex-direction:column}
  .panel-actions{width:100%}
  .search-field{width:100%;min-width:0}
  .activity-row{grid-template-columns:auto minmax(0,1fr)}
  .activity-value{grid-column:2;text-align:left}
  .data-table{min-width:620px}
  .form-grid,.fact-grid{grid-template-columns:1fr}
  .playground-settings-foot{align-items:stretch;flex-direction:column}
  .playground-settings-foot .quiet-button{width:100%}
  .launch-path-card{grid-template-columns:auto minmax(0,1fr)}
  .launch-path-card>a{grid-column:1/-1;width:100%}
  .usage-bars{height:145px;gap:4px}
  .settings-row{grid-template-columns:1fr;align-items:start}
  .session-expired{align-items:stretch;flex-direction:column}
  .session-expired-actions{justify-content:flex-end}
  .verify-dialog{width:calc(100vw - 12px);max-height:calc(100dvh - 12px);border-radius:18px}
  .verify-head{padding:19px 62px 16px 18px}
  .verify-close{top:14px;right:14px}
  .verify-toolbar{padding:9px 14px 10px 18px;align-items:stretch;flex-direction:column;gap:8px}
  .verify-actions{width:100%;display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:7px}
  .verify-action-button{width:100%;justify-content:center}
  .verify-body{padding:19px 18px 22px}
  .verify-fact{min-height:0}
  .verify-raw-toggle{grid-template-columns:auto minmax(0,1fr) auto;padding:9px}
  .verify-raw-size{grid-column:2;justify-self:start}
  .verify-raw-toggle>svg{grid-column:3;grid-row:1/3}
  .connect-ready{grid-template-columns:auto minmax(0,1fr);padding:15px}
  .connect-ready>.status-badge{grid-column:2;justify-self:start}
  .connect-step-actions{display:grid;grid-template-columns:1fr}
  .connect-step-actions .primary-button,.connect-step-actions .quiet-button{width:100%;min-width:0}
  .token-management>summary{align-items:flex-start;flex-wrap:wrap}
  .token-management>summary>.status-badge{margin-right:28px}
  .token-create-guide{grid-template-columns:1fr;padding:16px}
  .token-secret-note{grid-column:auto}
  .model-detail-dialog{width:calc(100vw - 12px);max-height:calc(100dvh - 12px);border-radius:18px}
  .model-detail-shell-head{padding-inline:18px 14px}
  .model-detail-body{padding:19px 18px 22px}
  .model-detail-hero{grid-template-columns:auto minmax(0,1fr)}
  .model-detail-hero>.status-badge{grid-column:1/-1;justify-self:start}
  .model-detail-facts{grid-template-columns:1fr}
  .model-detail-facts>div:last-child{grid-column:auto}
  .model-detail-actions{display:grid;grid-template-columns:1fr}
  .model-detail-actions .primary-button,.model-detail-actions .quiet-button{width:100%;min-width:0}
}

@media(max-width:430px){
  .usage-bars li:nth-child(even) small{visibility:hidden}
}

@media(max-width:360px){
  .topbar-context strong{display:none}
  .topbar-context .topbar-status{max-width:46vw}
}

@media(forced-colors:active){
  :focus-visible{outline:3px solid Highlight}
  .app-sidebar,.app-topbar,.panel,.metric,.notice,.attention-card,.message,.mobile-bottom-nav,.verify-dialog,.model-detail-dialog{background:Canvas;color:CanvasText;box-shadow:none}
  .state-indicator,.state-indicator.good,.state-indicator.warn,.state-indicator.danger{background:CanvasText;box-shadow:none;border:1px solid Canvas}
  .status-badge,.status-badge.good,.status-badge.info,.status-badge.warn,.status-badge.danger,.check-mark{background:Canvas;color:CanvasText;border-color:CanvasText}
  .settings-control .switch-track{border-color:CanvasText}
  .settings-control .switch-track::after{background:CanvasText}
  progress{border:1px solid CanvasText}
}

@media(prefers-reduced-motion:reduce){
  html{scroll-behavior:auto}
  *,*::before,*::after{animation:none!important;transition:none!important;scroll-behavior:auto!important}
  ::view-transition-group(*),::view-transition-old(*),::view-transition-new(*){animation:none!important}
}
html.motion-reduced{scroll-behavior:auto}
html.motion-reduced *,html.motion-reduced *::before,html.motion-reduced *::after{animation:none!important;transition:none!important;scroll-behavior:auto!important}
html.motion-reduced::view-transition,html.motion-reduced::view-transition-group(*),html.motion-reduced::view-transition-old(*),html.motion-reduced::view-transition-new(*){animation:none!important}
html.compact-density .app-main{padding-top:28px}
html.compact-density .metric,html.compact-density .panel-body{padding:13px}
html.compact-density .activity-row,html.compact-density .data-table th,html.compact-density .data-table td{padding-top:9px;padding-bottom:9px}
@view-transition{navigation:auto}
"#;

pub(super) const DASHBOARD_APP_JS: &str = r##"
(() => {
  'use strict';

  const root = document.documentElement;
  try {
    const cleanUrl = new URL(window.location.href);
    if (cleanUrl.searchParams.has('token')) {
      cleanUrl.searchParams.delete('token');
      window.history.replaceState(window.history.state, '', `${cleanUrl.pathname}${cleanUrl.search}${cleanUrl.hash}`);
    }
  } catch (_) {}
  const storage = {
    get(key) { try { return window.localStorage.getItem(key); } catch (_) { return null; } },
    set(key, value) { try { window.localStorage.setItem(key, value); } catch (_) {} },
    remove(key) { try { window.localStorage.removeItem(key); } catch (_) {} }
  };
  const taskStorage = {
    get(key) { try { return window.sessionStorage.getItem(key); } catch (_) { return null; } },
    set(key, value) { try { window.sessionStorage.setItem(key, value); } catch (_) {} },
    remove(key) { try { window.sessionStorage.removeItem(key); } catch (_) {} }
  };
  const preferenceKeys = {
    amounts: 'mayhem.dashboard.hideAmounts',
    motion: 'mayhem.dashboard.reduceMotion',
    density: 'mayhem.dashboard.compactDensity',
    sidebar: 'mayhem.dashboard.sidebarCollapsed'
  };
  const playgroundDraftKey = 'mayhem.dashboard.playgroundDraft';
  const playgroundConversationKey = 'mayhem.dashboard.playgroundConversation.v1';
  const localProductEventsKey = 'mayhem.dashboard.localProductEvents.v1';

  root.classList.toggle('amounts-hidden', storage.get(preferenceKeys.amounts) === '1');
  root.classList.toggle('motion-reduced', storage.get(preferenceKeys.motion) === '1');
  root.classList.toggle('compact-density', storage.get(preferenceKeys.density) === '1');
  root.classList.toggle('sidebar-collapsed', storage.get(preferenceKeys.sidebar) === '1');
  root.classList.add('js-ready');

  const ready = () => {
    const body = document.body;
    if (!body) return;
    const dialogTriggers = new WeakMap();
    const evidenceOriginals = new WeakMap();
    const evidencePayloads = new WeakMap();
    const playgroundConversation = [];
    let drawerTrigger = null;
    let playgroundController = null;
    let refreshDashboardSession = null;
    let recordDashboardSessionActivity = null;

    const safeQuery = (selector, scope = document) => {
      if (!selector) return null;
      try { return scope.querySelector(selector); } catch (_) { return null; }
    };

    const readLocalProductEvents = () => {
      const stored = storage.get(localProductEventsKey);
      if (!stored) return [];
      try {
        const parsed = JSON.parse(stored);
        return Array.isArray(parsed) ? parsed : [];
      } catch (_) {
        return [];
      }
    };

    const updateLocalProductEventCount = () => {
      const count = readLocalProductEvents().length;
      document.querySelectorAll('[data-local-event-count]').forEach((node) => {
        node.textContent = String(count);
      });
    };

    const recordProductEvent = (name, details = {}) => {
      if (!name) return;
      const events = readLocalProductEvents();
      events.push({
        version: 1,
        name: String(name).slice(0, 80),
        at: new Date().toISOString(),
        path: window.location.pathname,
        details
      });
      storage.set(localProductEventsKey, JSON.stringify(events.slice(-200)));
      updateLocalProductEventCount();
    };

    const selectedPlaygroundPriceMode = (scope = document) => {
      const model = safeQuery('[data-playground-model]', scope);
      return model?.selectedOptions?.[0]?.dataset.priceMode === 'fixed' ? 'fixed' : 'rate';
    };

    const playgroundDraftSnapshot = () => ({
      version: 3,
      mode: safeQuery('[data-playground-mode]')?.dataset.playgroundMode || 'chat',
      model: safeQuery('[data-playground-model]')?.value || '',
      prompt: safeQuery('[data-playground-prompt]')?.value || '',
      imagePrompt: safeQuery('[data-playground-image-prompt]')?.value || '',
      speechText: safeQuery('[data-playground-speech-text]')?.value || '',
      aspectRatio: safeQuery('[data-playground-aspect-ratio][aria-pressed="true"]')?.dataset.playgroundAspectRatio || '1:1',
      voice: safeQuery('[data-playground-voice]:checked')?.value || 'af_heart',
      system: safeQuery('[data-playground-system]')?.value || '',
      maxTokens: safeQuery('[data-playground-max-tokens]')?.value || '512',
      maxPrice: safeQuery('[data-playground-max-price]')?.value || '',
      maxPriceMode: selectedPlaygroundPriceMode(),
      minAttTier: safeQuery('[data-playground-min-att-tier]')?.value || ''
    });

    const savePlaygroundDraft = () => {
      if (!safeQuery('[data-playground-form]')) return;
      taskStorage.set(playgroundDraftKey, JSON.stringify(playgroundDraftSnapshot()));
    };

    const readPlaygroundDraft = () => {
      const stored = taskStorage.get(playgroundDraftKey);
      if (!stored) return null;
      try {
        const parsed = JSON.parse(stored);
        return parsed && typeof parsed === 'object' ? parsed : { prompt: stored };
      } catch (_) {
        return { prompt: stored };
      }
    };

    const savePlaygroundConversation = () => {
      if (!safeQuery('[data-playground-form]')) return;
      const messages = playgroundConversation
        .filter((message) => message && ['user', 'assistant'].includes(message.role) && typeof message.content === 'string')
        .slice(-20)
        .map((message) => ({ role: message.role, content: message.content.slice(0, 50000) }));
      storage.set(playgroundConversationKey, JSON.stringify({ version: 1, savedAt: Date.now(), messages }));
    };

    const playgroundModelOptionForValue = (value) =>
      Array.from(document.querySelectorAll('[data-playground-model-option]'))
        .find((option) => option.dataset.playgroundModelOption === value) || null;

    const currentPlaygroundMode = () =>
      safeQuery('[data-playground-mode]')?.dataset.playgroundMode ||
      safeQuery('[data-playground-model]')?.selectedOptions?.[0]?.dataset.playgroundMode ||
      'chat';

    const playgroundOptionsForMode = (mode) =>
      Array.from(safeQuery('[data-playground-model]')?.options || [])
        .filter((option) => option.dataset.playgroundMode === mode);

    const closePlaygroundModelPicker = (refocus = false) => {
      const rootNode = safeQuery('[data-playground-model-picker]');
      const trigger = safeQuery('[data-playground-model-trigger]', rootNode || document);
      const panel = safeQuery('[data-playground-model-panel]', rootNode || document);
      if (!trigger || !panel || panel.hidden) return;
      panel.hidden = true;
      safeQuery('.pg-model-backdrop', rootNode || document)?.setAttribute('hidden', '');
      trigger.setAttribute('aria-expanded', 'false');
      if (refocus) trigger.focus();
    };

    const syncPlaygroundModelPicker = () => {
      const select = safeQuery('[data-playground-model]');
      const rootNode = safeQuery('[data-playground-model-picker]');
      const trigger = safeQuery('[data-playground-model-trigger]', rootNode || document);
      if (!select || !rootNode || !trigger) return;
      const card = playgroundModelOptionForValue(select.value);
      const selected = select.selectedOptions[0];
      const iconHost = safeQuery('[data-playground-model-trigger-icon]', trigger);
      const icon = safeQuery('.model-lab-mark', card || document);
      if (iconHost && icon) iconHost.replaceChildren(icon.cloneNode(true));
      const name = selected?.dataset.modelName || selected?.value || 'Choose model';
      const purpose = selected?.dataset.modelPurpose || 'Live provider route';
      const nameNode = safeQuery('[data-playground-model-trigger-name]', trigger);
      const metaNode = safeQuery('[data-playground-model-trigger-meta]', trigger);
      if (nameNode) nameNode.textContent = name;
      if (metaNode) metaNode.textContent = purpose;
      trigger.setAttribute('aria-label', `Choose model, ${name} selected`);
      document.querySelectorAll('[data-playground-model-option]').forEach((option) => {
        const current = option === card;
        const visible = option.dataset.playgroundMode === currentPlaygroundMode();
        option.hidden = !visible;
        option.classList.toggle('is-selected', current);
        option.setAttribute('aria-selected', String(current));
        option.tabIndex = current && visible ? 0 : -1;
      });
      const visibleCards = Array.from(document.querySelectorAll('[data-playground-model-option]')).filter((option) => !option.hidden);
      const modeLabel = currentPlaygroundMode() === 'image' ? 'Image model' : currentPlaygroundMode() === 'speech' ? 'Speech model' : 'Text model';
      const panelLabel = safeQuery('[data-playground-model-panel-label]');
      const count = safeQuery('[data-playground-model-count]');
      const listbox = safeQuery('#playground-model-list');
      if (panelLabel) panelLabel.textContent = modeLabel;
      if (count) count.textContent = String(visibleCards.length);
      if (listbox) listbox.setAttribute('aria-label', modeLabel);
      document.querySelectorAll('[data-playground-active-model-name], [data-playground-network-model]').forEach((node) => { node.textContent = name; });
      document.querySelectorAll('[data-playground-active-model-icon], [data-playground-network-icon]').forEach((host) => {
        if (icon) host.replaceChildren(icon.cloneNode(true));
      });
      const modelFact = safeQuery('[data-playground-fact="model"]');
      if (modelFact) modelFact.textContent = selected?.value || name;
    };

    const syncPlaygroundInputs = () => {
      const prompt = safeQuery('[data-playground-prompt]');
      const imagePrompt = safeQuery('[data-playground-image-prompt]');
      const speechText = safeQuery('[data-playground-speech-text]');
      const chatCount = safeQuery('[data-playground-chat-count]');
      const imageCount = safeQuery('[data-playground-image-count]');
      const speechCount = safeQuery('[data-playground-speech-count]');
      if (chatCount && prompt) {
        chatCount.textContent = `${prompt.value.length}/1600`;
        chatCount.hidden = prompt.value.length < 1360;
      }
      if (imageCount && imagePrompt) imageCount.textContent = `${imagePrompt.value.length}/1200`;
      if (speechCount && speechText) speechCount.textContent = `${speechText.value.length}/800`;
      const busy = Boolean(playgroundController);
      const imageButton = safeQuery('[data-playground-generate-image]');
      const speechButton = safeQuery('[data-playground-generate-speech]');
      if (imageButton) imageButton.disabled = !imagePrompt?.value.trim() || busy;
      if (speechButton) speechButton.disabled = !speechText?.value.trim() || busy;
    };

    const setPlaygroundMode = (mode, refocus = false) => {
      const page = safeQuery('[data-playground-mode]');
      const select = safeQuery('[data-playground-model]');
      const available = playgroundOptionsForMode(mode);
      if (!page || !select || !available.length || playgroundController) return;
      page.dataset.playgroundMode = mode;
      document.querySelectorAll('[data-playground-mode-tab]').forEach((tab) => {
        const active = tab.dataset.playgroundModeTab === mode;
        tab.classList.toggle('is-active', active);
        tab.setAttribute('aria-selected', String(active));
        tab.tabIndex = active ? 0 : -1;
        if (active && refocus) tab.focus();
      });
      document.querySelectorAll('[data-playground-mode-panel]').forEach((panel) => {
        panel.hidden = panel.dataset.playgroundModePanel !== mode;
      });
      if (select.selectedOptions[0]?.dataset.playgroundMode !== mode) {
        select.value = available[0].value;
        select.dispatchEvent(new Event('change', { bubbles: true }));
      }
      closePlaygroundModelPicker(false);
      syncPlaygroundModelPicker();
      syncPlaygroundInputs();
      savePlaygroundDraft();
      recordProductEvent('playground_mode_selected', { mode, model: select.value });
    };

    const openPlaygroundModelPicker = () => {
      const rootNode = safeQuery('[data-playground-model-picker]');
      const trigger = safeQuery('[data-playground-model-trigger]', rootNode || document);
      const panel = safeQuery('[data-playground-model-panel]', rootNode || document);
      const select = safeQuery('[data-playground-model]');
      if (!trigger || !panel || !select || trigger.disabled) return;
      panel.hidden = false;
      safeQuery('.pg-model-backdrop', rootNode || document)?.removeAttribute('hidden');
      trigger.setAttribute('aria-expanded', 'true');
      playgroundModelOptionForValue(select.value)?.focus();
    };

    const choosePlaygroundModel = (value) => {
      const select = safeQuery('[data-playground-model]');
      if (!select || !Array.from(select.options).some((option) => option.value === value)) return;
      select.value = value;
      select.dispatchEvent(new Event('change', { bubbles: true }));
      closePlaygroundModelPicker(true);
    };

    const syncPlaygroundConversationUi = () => {
      const clear = safeQuery('[data-playground-clear]');
      if (clear) clear.hidden = !safeQuery('[data-playground-thread] .pg-message');
      const thread = safeQuery('[data-playground-thread]');
      if (thread) {
        const hasMessages = Boolean(safeQuery('.pg-message', thread));
        thread.classList.toggle('is-empty', !hasMessages);
        thread.classList.toggle('has-messages', hasMessages);
      }
    };

    const restorePlaygroundConversation = () => {
      const thread = safeQuery('[data-playground-thread]');
      if (!thread) return;
      const stored = storage.get(playgroundConversationKey);
      if (!stored) return;
      let parsed;
      try { parsed = JSON.parse(stored); } catch (_) { return; }
      const messages = Array.isArray(parsed?.messages) ? parsed.messages.slice(-20) : [];
      const safeMessages = messages.filter((message) =>
        message && ['user', 'assistant'].includes(message.role) && typeof message.content === 'string'
      );
      if (!safeMessages.length) return;
      playgroundConversation.push(...safeMessages.map((message) => ({ role: message.role, content: message.content })));
      const fragment = document.createDocumentFragment();
      const messagesRoot = document.createElement('div');
      messagesRoot.className = 'pg-messages';
      const actions = document.createElement('div');
      actions.className = 'pg-thread-actions';
      actions.innerHTML = '<span class="pg-thread-model">Restored conversation</span><button type="button" data-playground-clear>Clear conversation</button>';
      messagesRoot.append(actions);
      safeMessages.forEach((entry, index) => {
        const message = document.createElement('article');
        message.className = `pg-message is-${entry.role}`;
        message.setAttribute('role', 'article');
        message.setAttribute('aria-label', entry.role === 'user' ? 'You' : 'Mayhem');
        message.dataset.conversationOffset = String(index);
        const label = document.createElement('span');
        label.className = 'pg-message-author';
        label.setAttribute('aria-hidden', 'true');
        label.textContent = entry.role === 'user' ? 'You' : 'Mayhem';
        const content = document.createElement('div');
        content.className = 'pg-message-body';
        content.textContent = entry.content;
        message.append(label, content);
        messagesRoot.append(message);
      });
      fragment.append(messagesRoot);
      thread.replaceChildren(fragment);
      syncPlaygroundConversationUi();
      const metadata = safeQuery('[data-playground-meta]');
      if (metadata) metadata.textContent = `Restored ${safeMessages.length} local conversation message${safeMessages.length === 1 ? '' : 's'}`;
    };

    const announce = (message, assertive = false, visual = true) => {
      const selector = visual ? '[data-toast-region]' : '[data-live-announcer]';
      let region = safeQuery(selector);
      if (!region) {
        region = document.createElement('div');
        region.className = visual ? 'toast-region' : 'sr-only';
        if (visual) region.dataset.toastRegion = '';
        else region.dataset.liveAnnouncer = '';
        document.body.appendChild(region);
      }
      region.setAttribute('role', assertive ? 'alert' : 'status');
      region.setAttribute('aria-live', assertive ? 'assertive' : 'polite');
      region.setAttribute('aria-atomic', 'true');
      if (!visual) {
        region.textContent = '';
        window.requestAnimationFrame(() => {
          if (region.isConnected) region.textContent = message;
        });
        return;
      }
      const toast = document.createElement('div');
      toast.className = 'app-toast';
      toast.textContent = message;
      region.replaceChildren(toast);
      window.setTimeout(() => { if (toast.isConnected) toast.remove(); }, 2200);
    };

    const announcePlaygroundAnswer = (message = 'Mayhem response complete.') => {
      let region = safeQuery('[data-playground-answer-status]');
      if (!region) {
        region = document.createElement('div');
        region.className = 'sr-only';
        region.dataset.playgroundAnswerStatus = '';
        region.setAttribute('role', 'status');
        region.setAttribute('aria-live', 'polite');
        region.setAttribute('aria-atomic', 'true');
        document.body.appendChild(region);
      }
      region.textContent = '';
      window.requestAnimationFrame(() => {
        if (region.isConnected) region.textContent = message;
      });
    };

    const fallbackCopy = (value) => {
      const previousFocus = document.activeElement;
      const area = document.createElement('textarea');
      area.value = value;
      area.setAttribute('readonly', '');
      area.style.position = 'fixed';
      area.style.inset = '-9999px auto auto -9999px';
      document.body.appendChild(area);
      let copied = false;
      try {
        area.select();
        copied = document.execCommand('copy');
      } finally {
        area.remove();
        if (previousFocus && typeof previousFocus.focus === 'function') {
          previousFocus.focus({ preventScroll: true });
        }
      }
      if (!copied) throw new Error('Copy failed');
    };

    const copyText = async (value) => {
      if (!value) throw new Error('No value to copy');
      if (navigator.clipboard && window.isSecureContext) {
        try {
          await navigator.clipboard.writeText(value);
          return;
        } catch (_) {}
      }
      fallbackCopy(value);
    };

    const moneyNodes = () => Array.from(new Set([
      ...document.querySelectorAll('[data-money] .money-value'),
      ...document.querySelectorAll('.money-value[data-money]')
    ]));

    const moneyKey = /(?:^|_)au(?:_|$)|(?:^amount$|_amount$|^balance$|_balance$|^charge$|_charge$|^claimable$|_claimable$|^cost$|_cost$|^fee$|_fee$|^held$|_held$|^owed$|_owed$|^paid_cum$|^price$|_price$|^spend$|_spend$)/i;
    const redactMoney = (value, key = '') => {
      if (moneyKey.test(key)) return '[amount hidden]';
      if (Array.isArray(value)) return value.map((item) => redactMoney(item));
      if (value && typeof value === 'object') {
        return Object.fromEntries(Object.entries(value).map(([childKey, child]) => [childKey, redactMoney(child, childKey)]));
      }
      return value;
    };

    const applyAmountPreference = () => {
      const hidden = root.classList.contains('amounts-hidden');
      moneyNodes().forEach((node) => {
        if (!node.dataset.moneyOriginal) node.dataset.moneyOriginal = node.textContent || '';
        let accessible = node.nextElementSibling?.matches?.('[data-money-hidden-label]') ? node.nextElementSibling : null;
        if (hidden) {
          node.textContent = '\u2022\u2022\u2022\u2022';
          node.setAttribute('aria-hidden', 'true');
          if (!accessible) {
            accessible = document.createElement('span');
            accessible.className = 'sr-only';
            accessible.dataset.moneyHiddenLabel = '';
            accessible.textContent = 'Amount hidden';
            node.after(accessible);
          }
        } else {
          node.textContent = node.dataset.moneyOriginal;
          node.removeAttribute('aria-hidden');
          accessible?.remove();
        }
      });
      document.querySelectorAll('.raw-evidence').forEach((node) => {
        if (!evidenceOriginals.has(node)) evidenceOriginals.set(node, node.textContent || '');
        const original = evidenceOriginals.get(node) || '';
        if (hidden) {
          try {
            node.textContent = JSON.stringify(redactMoney(JSON.parse(original)), null, 2);
          } catch (_) {
            node.textContent = '[Raw evidence hidden while amounts are hidden]';
          }
          node.setAttribute('aria-label', 'Raw evidence with monetary values hidden');
        } else {
          node.textContent = original;
          node.removeAttribute('aria-label');
        }
      });
      document.querySelectorAll('[data-money-input]').forEach((input) => {
        input.type = hidden ? 'password' : 'text';
        if (hidden) input.setAttribute('aria-label', 'Route price ceiling, amount hidden');
        else input.removeAttribute('aria-label');
      });
      document.querySelectorAll('[data-hide-amounts]').forEach((button) => {
        const actionLabel = hidden ? 'Show amounts' : 'Hide amounts';
        button.setAttribute('aria-pressed', String(hidden));
        button.setAttribute('aria-label', actionLabel);
        const label = safeQuery('[data-hide-label]', button);
        if (label) label.textContent = actionLabel;
      });
      updatePlaygroundControlSummary();
      root.classList.add('amounts-ready');
    };

    const applyPreferenceButtons = () => {
      const states = {
        amounts: root.classList.contains('amounts-hidden'),
        motion: root.classList.contains('motion-reduced'),
        density: root.classList.contains('compact-density')
      };
      document.querySelectorAll('[data-preference]').forEach((button) => {
        const value = states[button.getAttribute('data-preference')];
        if (typeof value === 'boolean') {
          if (button.getAttribute('role') === 'switch') button.setAttribute('aria-checked', String(value));
          else button.setAttribute('aria-pressed', String(value));
          const label = safeQuery('[data-preference-label]', button);
          if (label) label.textContent = value ? 'On' : 'Off';
        }
      });
      const sidebarCollapsed = root.classList.contains('sidebar-collapsed');
      document.querySelectorAll('[data-sidebar-toggle]').forEach((button) => {
        button.setAttribute('aria-expanded', String(!sidebarCollapsed));
        button.setAttribute('aria-label', sidebarCollapsed ? 'Expand navigation' : 'Collapse navigation');
      });
      applyAmountPreference();
    };

    const drawer = safeQuery('#app-navigation');
    const frame = safeQuery('.app-frame');
    const mobileBottom = safeQuery('.mobile-bottom-nav');
    const mobileQuery = window.matchMedia('(max-width: 780px)');
    const drawerFocusable = () => drawer ? Array.from(drawer.querySelectorAll('a[href],button:not([disabled])')) : [];
    const setDrawer = (open, trigger = null) => {
      if (!drawer || (open && !mobileQuery.matches)) return;
      if (open) drawerTrigger = trigger || document.activeElement;
      body.classList.toggle('nav-open', open);
      document.querySelectorAll('[data-nav-toggle]').forEach((button) => {
        button.setAttribute('aria-expanded', String(open));
        button.setAttribute('aria-label', open ? 'Close navigation' : 'Open navigation');
      });
      if (frame && 'inert' in frame) frame.inert = open;
      if (mobileBottom && 'inert' in mobileBottom) mobileBottom.inert = open;
      if (open) {
        const first = drawerFocusable()[0];
        if (first) window.requestAnimationFrame(() => first.focus());
      } else if (drawerTrigger && typeof drawerTrigger.focus === 'function') {
        const restore = drawerTrigger;
        drawerTrigger = null;
        window.requestAnimationFrame(() => restore.focus());
      }
    };

    const tableToolParameter = (queryPrefix, name) => queryPrefix ? `${queryPrefix}_${name}` : name;

    const syncPaginationParameters = () => {
      try {
        const current = new URL(window.location.href);
        const toolParameters = new Set();
        document.querySelectorAll('[data-table-filter]').forEach((input) => {
          const queryPrefix = input.dataset.tableQueryPrefix || '';
          ['q', 'sort', 'direction'].forEach((name) => toolParameters.add(tableToolParameter(queryPrefix, name)));
        });
        document.querySelectorAll('.pagination a[href]').forEach((link) => {
          const target = new URL(link.href, window.location.href);
          toolParameters.forEach((key) => {
            const value = current.searchParams.get(key);
            if (value) target.searchParams.set(key, value);
            else target.searchParams.delete(key);
          });
          link.href = `${target.pathname}${target.search}${target.hash}`;
        });
      } catch (_) {}
    };

    const updateUrlParameters = (changes) => {
      try {
        const url = new URL(window.location.href);
        Object.entries(changes).forEach(([key, value]) => {
          if (value == null || value === '') url.searchParams.delete(key);
          else url.searchParams.set(key, String(value));
        });
        window.history.replaceState(window.history.state, '', `${url.pathname}${url.search}${url.hash}`);
        syncPaginationParameters();
      } catch (_) {}
    };

    const updateFilter = (input, persist = false) => {
      const table = safeQuery(input.getAttribute('data-table-filter'));
      if (!table) return;
      const query = input.value.trim().toLocaleLowerCase();
      const rows = Array.from(table.querySelectorAll('tbody tr[data-filter-row]'));
      let visible = 0;
      rows.forEach((row) => {
        const filterText = row.dataset.filterText || row.textContent;
        const match = !query || filterText.toLocaleLowerCase().includes(query);
        row.hidden = !match;
        if (match) visible += 1;
      });
      const count = safeQuery(input.getAttribute('data-filter-count'));
      if (count) {
        count.textContent = `${visible} shown ${visible === 1 ? 'row' : 'rows'}`;
        count.setAttribute('aria-live', 'polite');
      }
      const empty = safeQuery(input.getAttribute('data-filter-empty'));
      if (empty) empty.hidden = !query || visible !== 0;
      if (persist) {
        const key = tableToolParameter(input.dataset.tableQueryPrefix || '', 'q');
        updateUrlParameters({ [key]: input.value.trim() || null });
      }
    };

    const csvCell = (value) => {
      const text = String(value || '');
      const safe = /^[=+@]/.test(text) || /^-(?!\d)/.test(text) ? `'${text}` : text;
      return `"${safe.replace(/"/g, '""')}"`;
    };

    const exportShownTable = (table) => {
      const amountsHidden = document.documentElement.classList.contains('amounts-hidden');
      const headers = Array.from(table.querySelectorAll('thead th'));
      const included = headers
        .map((header, index) => ({ header, index, label: (header.textContent || '').trim() }))
        .filter(({ label }) => !['Action', 'Evidence'].includes(label));
      const rows = Array.from(table.querySelectorAll('tbody tr[data-filter-row]'))
        .filter((row) => !row.hidden)
        .map((row) => {
          const cells = Array.from(row.children);
          return included.map(({ index }) => {
            const cell = cells[index];
            const containsMoney = cell?.matches('[data-money]') || Boolean(cell?.querySelector('[data-money], .money-value'));
            const containsExpiredEvidence = Boolean(cell?.querySelector('[data-volatile-expired="true"]'));
            const value = amountsHidden && containsMoney
              ? 'Hidden'
              : (containsExpiredEvidence
                ? (cell?.textContent || '')
                : (cell?.getAttribute('data-export-value') || cell?.textContent || ''));
            return csvCell(value.replace(/\s+/g, ' ').trim());
          }).join(',');
        });
      const csv = [included.map(({ label }) => csvCell(label)).join(','), ...rows].join('\r\n');
      const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' });
      const href = URL.createObjectURL(blob);
      const link = document.createElement('a');
      const safeName = (table.id || 'mayhem-table').replace(/[^a-z0-9_-]+/gi, '-').toLocaleLowerCase();
      link.href = href;
      link.download = `${safeName}-shown-page.csv`;
      document.body.appendChild(link);
      link.click();
      link.remove();
      window.setTimeout(() => URL.revokeObjectURL(href), 0);
      announce(`Exported ${rows.length} ${rows.length === 1 ? 'row' : 'rows'} from the shown page.`, false, false);
    };

    const sortTable = (table, columnIndex, direction, label, persist = true, queryPrefix = '') => {
      const body = safeQuery('tbody', table);
      if (!body) return;
      const rows = Array.from(body.querySelectorAll('tr[data-filter-row]'));
      const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
      rows.forEach((row, index) => {
        if (!row.dataset.originalOrder) row.dataset.originalOrder = String(index);
      });
      rows.sort((left, right) => {
        const leftCell = left.children[columnIndex];
        const rightCell = right.children[columnIndex];
        const leftValue = (leftCell?.getAttribute('data-sort-value') || leftCell?.getAttribute('data-export-value') || leftCell?.textContent || '').trim();
        const rightValue = (rightCell?.getAttribute('data-sort-value') || rightCell?.getAttribute('data-export-value') || rightCell?.textContent || '').trim();
        const numericValue = (value) => {
          const match = value.replace(/,/g, '').match(/^[-+]?\s*[$€£]?\s*(\d+(?:\.\d+)?)/);
          return match ? Number.parseFloat(match[1]) * (value.trim().startsWith('-') ? -1 : 1) : null;
        };
        const leftNumber = numericValue(leftValue);
        const rightNumber = numericValue(rightValue);
        const compared = leftNumber !== null && rightNumber !== null && leftNumber !== rightNumber
          ? leftNumber - rightNumber
          : collator.compare(leftValue, rightValue);
        if (compared !== 0) return direction === 'ascending' ? compared : -compared;
        return Number(left.dataset.originalOrder) - Number(right.dataset.originalOrder);
      });
      rows.forEach((row) => body.appendChild(row));
      table.querySelectorAll('thead th[aria-sort]').forEach((header) => header.removeAttribute('aria-sort'));
      const header = table.querySelectorAll('thead th')[columnIndex];
      if (header) {
        header.setAttribute('aria-sort', direction);
        const button = safeQuery('[data-sort-column]', header);
        if (button) {
          const nextDirection = direction === 'ascending' ? 'descending' : 'ascending';
          button.setAttribute('aria-label', `Sort shown page by ${label}, ${nextDirection}`);
        }
      }
      if (persist) {
        updateUrlParameters({
          [tableToolParameter(queryPrefix, 'sort')]: String(columnIndex),
          [tableToolParameter(queryPrefix, 'direction')]: direction
        });
        announce(`Sorted the shown page by ${label}, ${direction}.`, false, false);
      }
    };

    const enhanceTableTools = (input) => {
      const table = safeQuery(input.getAttribute('data-table-filter'));
      if (!table || table.dataset.toolsReady === 'true') return;
      table.dataset.toolsReady = 'true';
      const queryPrefix = input.dataset.tableQueryPrefix || '';
      const headers = Array.from(table.querySelectorAll('thead th'));
      headers.forEach((header, columnIndex) => {
        const label = (header.textContent || '').trim();
        if (!label || ['Action', 'Evidence'].includes(label)) return;
        const button = document.createElement('button');
        button.className = 'table-sort-button';
        button.type = 'button';
        button.dataset.sortColumn = String(columnIndex);
        button.dataset.sortTable = `#${table.id}`;
        if (queryPrefix) button.dataset.tableQueryPrefix = queryPrefix;
        button.textContent = label;
        button.setAttribute('aria-label', `Sort shown page by ${label}`);
        header.replaceChildren(button);
      });
      const actions = input.closest('.panel-actions');
      if (actions && !safeQuery('[data-export-table]', actions)) {
        const exportButton = document.createElement('button');
        exportButton.className = 'quiet-button';
        exportButton.type = 'button';
        exportButton.dataset.exportTable = `#${table.id}`;
        exportButton.textContent = 'Export shown page';
        actions.append(exportButton);
      }
      try {
        const url = new URL(window.location.href);
        const savedQuery = url.searchParams.get(tableToolParameter(queryPrefix, 'q'));
        if (savedQuery && !input.value) input.value = savedQuery;
        const savedColumn = Number.parseInt(url.searchParams.get(tableToolParameter(queryPrefix, 'sort')) || '', 10);
        const savedDirection = url.searchParams.get(tableToolParameter(queryPrefix, 'direction'));
        if (Number.isInteger(savedColumn) && savedColumn >= 0 && savedColumn < headers.length && ['ascending', 'descending'].includes(savedDirection)) {
          const label = (headers[savedColumn].textContent || '').trim();
          sortTable(table, savedColumn, savedDirection, label, false, queryPrefix);
        }
      } catch (_) {}
    };

    const setConnectionResult = (target, message, tone = '') => {
      if (!target) return;
      target.textContent = message;
      target.className = `notice ${tone}`.trim();
      target.setAttribute('role', tone === 'danger' ? 'alert' : 'status');
      target.setAttribute('aria-live', tone === 'danger' ? 'assertive' : 'polite');
      target.hidden = false;
    };

    const testConnection = async (button) => {
      const target = safeQuery(button.getAttribute('data-result-target'));
      button.disabled = true;
      button.setAttribute('aria-busy', 'true');
      setConnectionResult(target, 'Checking this dashboard session…');
      try {
        if (body.classList.contains('has-workbench')) {
          const response = await fetch('/__workbench/health', { credentials: 'same-origin', cache: 'no-store' });
          if (!response.ok) throw new Error(`Preview returned ${response.status}`);
          setConnectionResult(target, 'Workbench dashboard session is reachable. Production API credentials and inference are intentionally not exercised in preview.', 'good');
        } else {
          const response = await fetch('/mayhem/dashboard/session', { credentials: 'same-origin', cache: 'no-store' });
          if (response.status === 401) throw new Error('Dashboard session expired');
          if (!response.ok) throw new Error(`Gateway returned ${response.status}`);
          const data = await response.json();
          if (!data.ok) throw new Error('Gateway did not confirm readiness');
          if (recordDashboardSessionActivity) {
            recordDashboardSessionActivity(data.expires_in_seconds);
          }
          setConnectionResult(target, 'Dashboard session is valid. Run an inference request from Playground to test the displayed API base URL, credential, and model route.', 'good');
        }
      } catch (error) {
        setConnectionResult(target, error instanceof Error ? error.message : 'Connection check failed', 'danger');
      } finally {
        button.disabled = false;
        button.removeAttribute('aria-busy');
      }
    };

    const contentText = (payload) => {
      const choice = payload && Array.isArray(payload.choices) ? payload.choices[0] : null;
      const value = choice && (choice.delta?.content ?? choice.message?.content ?? choice.text);
      if (typeof value === 'string') return value;
      if (Array.isArray(value)) return value.map((part) => typeof part === 'string' ? part : (part?.text || '')).join('');
      if (typeof payload?.output_text === 'string') return payload.output_text;
      return '';
    };

    const formatAuUsd = (value) => {
      try {
        const amount = BigInt(String(value));
        if (amount === 0n) return '$0.00';
        const perUsd = 1000000000000000000n;
        const perCent = perUsd / 100n;
        if (amount >= perCent / 2n) {
          const cents = (amount + perCent / 2n) / perCent;
          return `$${cents / 100n}.${String(cents % 100n).padStart(2, '0')}`;
        }
        const whole = amount / perUsd;
        const fraction = String(amount % perUsd).padStart(18, '0').replace(/0+$/, '');
        return `$${whole}.${fraction || '00'}`;
      } catch (_) {
        return '';
      }
    };

    const maxPriceAuFromUsd = (input, priceMode) => {
      if (!input) return null;
      const value = input.value.trim();
      input.setCustomValidity('');
      if (!value) return null;
      const fixedOnly = priceMode === 'fixed';
      const decimalPlaces = fixedOnly ? 18 : 15;
      const validValue = fixedOnly ? /^\d+(?:\.\d{1,18})?$/ : /^\d+(?:\.\d{1,15})?$/;
      if (!validValue.test(value)) {
        input.setCustomValidity(fixedOnly
          ? 'Enter a positive fixed-charge ceiling in USD with up to 18 decimal places.'
          : 'Enter dollars per 1M-unit route basket as a positive number with up to 15 decimal places.');
        return null;
      }
      const [whole, fraction = ''] = value.split('.');
      const multiplier = fixedOnly ? 1000000000000000000n : 1000000000000000n;
      const routePriceBasisAu = BigInt(whole) * multiplier
        + BigInt(fraction.padEnd(decimalPlaces, '0'));
      if (routePriceBasisAu <= 0n) {
        input.setCustomValidity('The price ceiling must be greater than zero, or left blank.');
        return null;
      }
      if (routePriceBasisAu > 340282366920938463463374607431768211455n) {
        input.setCustomValidity('The price ceiling is too large for the gateway.');
        return null;
      }
      return routePriceBasisAu.toString();
    };

    const playgroundRequestControls = (form) => {
      const output = safeQuery('[data-playground-max-tokens]', form);
      const price = safeQuery('[data-playground-max-price]', form);
      const tier = safeQuery('[data-playground-min-att-tier]', form);
      const priceMode = selectedPlaygroundPriceMode(form);
      const outputValue = output?.value.trim() || '';
      const outputTokens = /^\d+$/.test(outputValue) ? Number.parseInt(outputValue, 10) : Number.NaN;
      if (!Number.isInteger(outputTokens) || outputTokens < 64 || outputTokens > 4096) {
        output?.setCustomValidity('Choose an output limit from 64 through 4,096 tokens.');
        output?.reportValidity();
        output?.focus();
        return null;
      }
      output.setCustomValidity('');
      const maxPriceAu = maxPriceAuFromUsd(price, priceMode);
      if (price?.validationMessage) {
        price.reportValidity();
        price.focus();
        return null;
      }
      const minAttTier = tier?.value || '';
      if (minAttTier && !['1', '2', '3', '4'].includes(minAttTier)) {
        tier?.setCustomValidity('Choose a supported trust tier.');
        tier?.reportValidity();
        tier?.focus();
        return null;
      }
      tier?.setCustomValidity('');
      return { outputTokens, maxPriceAu, minAttTier, priceMode };
    };

    const classifyPlaygroundFailure = (status, message) => {
      const normalized = String(message || '').toLocaleLowerCase();
      const namesCapacityBlocker = normalized.includes('capacity')
        || normalized.includes('no route')
        || normalized.includes('no eligible route')
        || normalized.includes('no provider route');
      if (status === 401 || status === 403) {
        return {
          impact: 'The gateway did not accept this request credential. Your message is still ready to retry.',
          actionLabel: 'Review access',
          actionHref: '/mayhem/dashboard/connect'
        };
      }
      if (status === 402 || normalized.includes('balance') || normalized.includes('fund')) {
        return {
          impact: 'The request could not start because the available balance was not sufficient. Your message is preserved.',
          actionLabel: 'Review wallet',
          actionHref: '/mayhem/dashboard/wallet'
        };
      }
      if (status === 426 || normalized.includes('update required') || normalized.includes('incompatible')) {
        return {
          impact: 'This request needs a compatible Mayhem version before it can run. Your message is preserved.',
          actionLabel: 'Review update',
          actionHref: '/mayhem/dashboard/settings'
        };
      }
      if (status === 409 || normalized.includes('co-sign') || normalized.includes('session paused')) {
        return {
          impact: 'The gateway paused this session during receipt recovery. Review the recorded activity before retrying.',
          actionLabel: 'Review recovery',
          actionHref: '/mayhem/dashboard/activity'
        };
      }
      if (status === 429 && !namesCapacityBlocker) {
        return {
          impact: 'This access token reached its request rate. Wait for the rate window, then retry with the same preserved message.',
          actionLabel: 'Review access',
          actionHref: '/mayhem/dashboard/connect'
        };
      }
      if ([404, 422, 503].includes(status) || namesCapacityBlocker) {
        return {
          impact: 'No eligible route accepted this request. You can retry or choose another currently advertised model.',
          actionLabel: 'Choose model',
          actionHref: '/mayhem/dashboard/models'
        };
      }
      return {
        impact: 'The request did not complete. Your message and settings are preserved for a safe retry.',
        actionLabel: 'Check activity',
        actionHref: '/mayhem/dashboard/activity'
      };
    };

    const addPromptAction = (message, promptValue, conversationOffset, label = 'Edit and resend') => {
      const actions = document.createElement('div');
      actions.className = 'pg-message-actions';
      const button = document.createElement('button');
      button.className = 'pg-text-action';
      button.type = 'button';
      button.dataset.playgroundReusePrompt = '';
      button.dataset.prompt = promptValue;
      button.dataset.conversationOffset = String(conversationOffset);
      button.textContent = label;
      actions.append(button);
      message.append(actions);
    };

    const addFailureRecovery = (message, status, technicalMessage, promptValue, partialText = '') => {
      const failure = classifyPlaygroundFailure(status, technicalMessage);
      const content = safeQuery('.pg-message-body', message);
      if (content && !partialText) content.textContent = failure.impact;
      if (partialText) {
        const incomplete = document.createElement('span');
        incomplete.className = 'message-result incomplete';
        incomplete.dataset.playgroundPartialOutput = '';
        incomplete.dataset.finishReason = 'transport_error';
        const mark = document.createElement('span');
        mark.className = 'message-result-mark';
        mark.setAttribute('aria-hidden', 'true');
        mark.textContent = '!';
        const label = document.createElement('span');
        label.textContent = 'Incomplete response · connection ended before a finish reason';
        incomplete.append(mark, label);
        const impact = document.createElement('p');
        impact.className = 'message-recovery-impact';
        impact.textContent = failure.impact;
        message.append(incomplete, impact);
      }
      const details = document.createElement('details');
      details.className = 'message-details';
      const summary = document.createElement('summary');
      summary.textContent = 'Technical details';
      const detail = document.createElement('span');
      detail.className = 'table-secondary';
      detail.textContent = technicalMessage || `Gateway request failed${status ? ` (${status})` : ''}.`;
      details.append(summary, detail);
      const actions = document.createElement('div');
      actions.className = 'recovery-actions';
      const retry = document.createElement('button');
      retry.className = 'soft-button';
      retry.type = 'button';
      retry.dataset.playgroundRetry = '';
      retry.dataset.prompt = promptValue;
      retry.textContent = 'Retry request';
      const next = document.createElement('a');
      next.className = 'quiet-button';
      next.href = failure.actionHref;
      next.textContent = failure.actionLabel;
      actions.append(retry, next);
      message.append(details, actions);
    };

    const playgroundHeaders = (form, controls) => {
      const headers = { 'content-type': 'application/json' };
      const token = safeQuery('[data-playground-token]', form);
      if (token?.value.trim()) headers.authorization = `Bearer ${token.value.trim()}`;
      if (controls.maxPriceAu) headers['x-mayhem-max-price-au'] = controls.maxPriceAu;
      if (controls.minAttTier) headers['x-mayhem-min-att-tier'] = controls.minAttTier;
      return headers;
    };

    const playgroundFailureMessage = async (response) => {
      let message = `Request failed (${response.status})`;
      try {
        const payload = await response.json();
        message = payload?.error?.message || payload?.message || message;
      } catch (_) {}
      return message;
    };

    const setPlaygroundNetwork = (state, details = {}) => {
      const network = safeQuery('[data-playground-network]');
      if (!network) return;
      network.classList.toggle('is-busy', ['submitting', 'matching', 'generating'].includes(state));
      network.classList.toggle('is-complete', state === 'complete');
      const labels = {
        ready: 'Ready', submitting: 'Preparing request', matching: 'Opening provider route',
        generating: 'Generating', complete: 'Complete', stopped: 'Stopped', failed: 'Needs attention'
      };
      const stateNode = safeQuery('[data-playground-network-state]', network);
      if (stateNode) stateNode.textContent = labels[state] || 'Ready';
      const stepState = {
        ready: ['idle', 'idle', 'idle', 'idle'],
        submitting: ['active', 'idle', 'idle', 'idle'],
        matching: ['done', 'active', 'idle', 'idle'],
        generating: ['done', 'done', 'active', 'idle'],
        complete: ['done', 'done', 'done', details.receipt ? 'done' : 'idle'],
        stopped: ['done', 'halted', 'halted', 'idle'],
        failed: ['done', 'halted', 'halted', 'idle']
      }[state] || ['idle', 'idle', 'idle', 'idle'];
      const stepDetails = state === 'complete'
        ? ['Accepted', 'Provider session completed', 'Response delivered', details.receipt ? 'Receipt issued' : 'Not returned']
        : state === 'generating'
          ? ['Accepted', 'Provider session opened', details.mode === 'image' ? 'Generating image' : details.mode === 'speech' ? 'Synthesizing audio' : 'Streaming response', 'Pending generation']
          : state === 'matching'
            ? ['Accepted', 'Opening an eligible route', 'Not started', 'Pending generation']
            : state === 'submitting'
              ? ['Validating request', 'Not started', 'Not started', 'Pending generation']
              : state === 'failed'
                ? ['Accepted', 'Route ended', 'Ended early', 'Not issued']
                : state === 'stopped'
                  ? ['Accepted', 'Route cancelled', 'Stopped', 'Not issued']
                  : ['Waiting for input', 'Not started', 'Not started', 'Pending generation'];
      Array.from(network.querySelectorAll('[data-playground-step]')).forEach((step, index) => {
        step.className = `is-${stepState[index]}`;
        const detail = safeQuery('.pg-step-detail', step);
        if (detail) detail.textContent = stepDetails[index];
      });
      const assignFact = (name, value) => {
        const node = safeQuery(`[data-playground-fact="${name}"]`, network);
        if (node && value) node.textContent = value;
      };
      assignFact('model', details.model);
      assignFact('provider', details.provider || (details.receipt ? 'Confirmed by receipt' : null));
      assignFact('timing', details.timing);
      assignFact('cost', details.cost);
      assignFact('request', details.requestId);
    };

    const playgroundBusy = (busy) => {
      const trigger = safeQuery('[data-playground-model-trigger]');
      const imageButton = safeQuery('[data-playground-generate-image]');
      const speechButton = safeQuery('[data-playground-generate-speech]');
      document.querySelectorAll('[data-playground-mode-tab]').forEach((tab) => { tab.disabled = busy || tab.dataset.empty === 'true'; });
      if (trigger) trigger.disabled = busy;
      if (imageButton) imageButton.disabled = busy || !safeQuery('[data-playground-image-prompt]')?.value.trim();
      if (speechButton) speechButton.disabled = busy || !safeQuery('[data-playground-speech-text]')?.value.trim();
    };

    const pendingImageMarkup = (ratio) => {
      const cells = Array.from({ length: 15 }, () => '<span></span>').join('');
      return `<div class="pg-image-result" role="status"><div class="pg-image-frame is-pending ratio-${ratio.replace(':', '-')}"><span class="pg-latent-grid" aria-hidden="true">${cells}</span><div class="pg-generation-center"><span class="pg-generation-orb" aria-hidden="true"><svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="3"></rect><circle cx="9" cy="10" r="2"></circle><path d="m5 18 5-5 3 3 2-2 4 4"></path></svg></span><span class="pg-generation-kicker">Live provider request</span><strong>Generating image</strong><span class="pg-generation-detail">Waiting for confirmed output</span><button type="button" class="pg-stop" data-playground-stop><svg viewBox="0 0 24 24"><rect x="6" y="6" width="12" height="12" rx="2"></rect></svg>Stop</button></div></div></div>`;
    };

    const runPlaygroundImage = async (form) => {
      const model = safeQuery('[data-playground-model]', form);
      const prompt = safeQuery('[data-playground-image-prompt]', form);
      const output = safeQuery('[data-playground-image-output]', form);
      const metadata = safeQuery('[data-playground-meta]');
      if (!model || !prompt || !output || model.selectedOptions[0]?.dataset.playgroundMode !== 'image') return;
      const promptValue = prompt.value.trim();
      if (!promptValue) { prompt.focus(); announce('Describe the image you want to generate.', true); return; }
      if (playgroundController) { announce('A request is already in progress.', true); return; }
      const controls = playgroundRequestControls(form);
      if (!controls) return;
      const ratio = safeQuery('[data-playground-aspect-ratio][aria-pressed="true"]')?.dataset.playgroundAspectRatio || '1:1';
      const size = { '1:1': '512x512', '4:3': '640x480', '3:4': '480x640', '16:9': '768x432' }[ratio] || '512x512';
      const controller = new AbortController();
      playgroundController = controller;
      playgroundBusy(true);
      output.innerHTML = pendingImageMarkup(ratio);
      if (metadata) metadata.textContent = 'Image request in progress';
      const startedAt = Date.now();
      setPlaygroundNetwork('submitting', { mode: 'image', model: model.value });
      recordProductEvent('playground_request_started', { mode: 'image', model: model.value });
      try {
        setPlaygroundNetwork('matching', { mode: 'image', model: model.value });
        const response = await fetch('/v1/images/generations', {
          method: 'POST', credentials: 'same-origin', headers: playgroundHeaders(form, controls),
          body: JSON.stringify({ model: model.value, prompt: promptValue, n: 1, size, response_format: 'b64_json' }),
          signal: controller.signal
        });
        if (!response.ok) throw new Error(await playgroundFailureMessage(response));
        setPlaygroundNetwork('generating', { mode: 'image', model: model.value });
        const payload = await response.json();
        const image = payload?.data?.[0];
        if (!image?.b64_json) throw new Error('The provider returned no image data.');
        const contentType = image?.mayhem?.content_type || 'image/png';
        const source = `data:${contentType};base64,${image.b64_json}`;
        const receipt = payload?.mayhem?.receipt;
        const elapsed = Math.max(.1, (Date.now() - startedAt) / 1000);
        const artifact = image?.mayhem?.artifact_id || payload?.id || 'generated-image';
        const extension = contentType.split('/')[1] || 'png';
        output.replaceChildren();
        const result = document.createElement('div');
        result.className = 'pg-image-result';
        result.innerHTML = `<div class="pg-image-frame ratio-${ratio.replace(':', '-')}"><img class="pg-generated-image" alt="" width="${size.split('x')[0]}" height="${size.split('x')[1]}"></div><p class="pg-image-meta"></p><div class="pg-output-actions"><a class="pg-text-action" download></a><button class="pg-text-action" type="button" data-copy data-copy-value="">Copy prompt</button><button class="pg-text-action" type="button" data-playground-generate-image>Retry</button><button class="pg-text-action" type="button" data-playground-clear-image>Clear</button></div>`;
        const generated = safeQuery('img', result);
        generated.src = source;
        generated.alt = `Generated image: ${promptValue.slice(0, 180)}`;
        const meta = safeQuery('.pg-image-meta', result);
        if (meta) meta.textContent = `${size.replace('x', '×')} · ${artifact} · ${elapsed.toFixed(1)}s`;
        const download = safeQuery('a[download]', result);
        download.href = source; download.download = `openmayhem-${artifact}.${extension}`; download.textContent = 'Download image';
        const copy = safeQuery('[data-copy]', result); copy.dataset.copyValue = promptValue;
        output.append(result);
        if (metadata) metadata.textContent = `Image generated with ${payload?.model || model.value}`;
        const cost = receipt?.au_owed_cum != null ? formatAuUsd(receipt.au_owed_cum) : '';
        setPlaygroundNetwork('complete', { mode: 'image', model: payload?.model || model.value, requestId: receipt?.session_id || payload?.id, timing: `${elapsed.toFixed(1)}s browser-observed`, cost: cost || 'See Activity', receipt: Boolean(receipt) });
        recordProductEvent('playground_request_completed', { mode: 'image', model: payload?.model || model.value, durationMs: Date.now() - startedAt, receipt: Boolean(receipt) });
        announcePlaygroundAnswer('Image generation complete.');
      } catch (error) {
        const aborted = error instanceof DOMException && error.name === 'AbortError';
        const message = aborted ? 'Image generation stopped. Your prompt was kept.' : (error instanceof Error ? error.message : 'Image generation failed.');
        output.innerHTML = `<div class="pg-output-empty"><strong>${aborted ? 'Generation stopped' : 'Image request failed'}</strong><span></span><button type="button" class="pg-text-action" data-playground-generate-image>Try again</button></div>`;
        safeQuery('.pg-output-empty span', output).textContent = message;
        setPlaygroundNetwork(aborted ? 'stopped' : 'failed', { mode: 'image', model: model.value, timing: `${Math.max(.1, (Date.now() - startedAt) / 1000).toFixed(1)}s` });
        announce(message, !aborted);
      } finally {
        if (playgroundController === controller) { playgroundController = null; playgroundBusy(false); syncPlaygroundInputs(); }
      }
    };

    let playgroundAudioUrl = null;
    const runPlaygroundSpeech = async (form) => {
      const model = safeQuery('[data-playground-model]', form);
      const input = safeQuery('[data-playground-speech-text]', form);
      const output = safeQuery('[data-playground-speech-output]', form);
      const metadata = safeQuery('[data-playground-meta]');
      if (!model || !input || !output || model.selectedOptions[0]?.dataset.playgroundMode !== 'speech') return;
      const inputValue = input.value.trim();
      if (!inputValue) { input.focus(); announce('Enter the text you want spoken.', true); return; }
      if (playgroundController) { announce('A request is already in progress.', true); return; }
      const controls = playgroundRequestControls(form);
      if (!controls) return;
      const voice = safeQuery('[data-playground-voice]:checked')?.value || 'af_heart';
      const controller = new AbortController();
      playgroundController = controller;
      playgroundBusy(true);
      const bars = [32,58,44,76,54,88,62,96,72,84,50,68,42,60,34].map((height,index) => `<span style="--wave-height:${height}%;--wave-delay:${index * -72}ms"></span>`).join('');
      output.innerHTML = `<div class="pg-audio-result" role="status"><span class="pg-play" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M11 5 6.6 8.5H3.8a.8.8 0 0 0-.8.8v5.4a.8.8 0 0 0 .8.8h2.8L11 19Z"></path><path d="M14.8 9.2a4.1 4.1 0 0 1 0 5.6"></path></svg></span><div><strong>Synthesizing speech</strong><span>Live provider request</span></div><div class="pg-waveform">${bars}</div><div class="pg-output-actions"><button type="button" class="pg-stop" data-playground-stop>Stop</button></div></div>`;
      if (metadata) metadata.textContent = 'Speech request in progress';
      const startedAt = Date.now();
      setPlaygroundNetwork('submitting', { mode: 'speech', model: model.value });
      recordProductEvent('playground_request_started', { mode: 'speech', model: model.value });
      try {
        setPlaygroundNetwork('matching', { mode: 'speech', model: model.value });
        const response = await fetch('/v1/audio/speech', {
          method: 'POST', credentials: 'same-origin', headers: playgroundHeaders(form, controls),
          body: JSON.stringify({ model: model.value, input: inputValue, voice, response_format: 'wav' }),
          signal: controller.signal
        });
        if (!response.ok) throw new Error(await playgroundFailureMessage(response));
        setPlaygroundNetwork('generating', { mode: 'speech', model: model.value });
        const audio = await response.blob();
        if (!audio.size) throw new Error('The provider returned no audio data.');
        if (playgroundAudioUrl) URL.revokeObjectURL(playgroundAudioUrl);
        playgroundAudioUrl = URL.createObjectURL(audio);
        let receipt = null;
        try { receipt = JSON.parse(response.headers.get('x-mayhem-receipt') || 'null'); } catch (_) {}
        const elapsed = Math.max(.1, (Date.now() - startedAt) / 1000);
        output.replaceChildren();
        const result = document.createElement('div');
        result.className = 'pg-audio-result';
        result.innerHTML = `<span class="pg-play" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="m9 7 8 5-8 5Z"></path></svg></span><div><strong>Speech ready</strong><span></span></div><audio controls preload="metadata"></audio><div class="pg-output-actions"><a class="pg-text-action" download="openmayhem-speech.wav">Download audio</a><button class="pg-text-action" type="button" data-playground-generate-speech>Retry</button><button class="pg-text-action" type="button" data-playground-clear-speech>Clear</button></div>`;
        safeQuery('.pg-audio-result>div span', result).textContent = `${voice} · ${elapsed.toFixed(1)}s`;
        safeQuery('audio', result).src = playgroundAudioUrl;
        safeQuery('a[download]', result).href = playgroundAudioUrl;
        output.append(result);
        const cost = receipt?.au_owed_cum != null ? formatAuUsd(receipt.au_owed_cum) : '';
        const backend = response.headers.get('x-mayhem-backend') || 'Provider session completed';
        setPlaygroundNetwork('complete', { mode: 'speech', model: model.value, provider: backend, requestId: receipt?.session_id || 'Completed', timing: `${elapsed.toFixed(1)}s browser-observed`, cost: cost || 'See Activity', receipt: Boolean(receipt) });
        if (metadata) metadata.textContent = `Speech generated with ${model.value}`;
        recordProductEvent('playground_request_completed', { mode: 'speech', model: model.value, durationMs: Date.now() - startedAt, receipt: Boolean(receipt) });
        announcePlaygroundAnswer('Speech generation complete.');
      } catch (error) {
        const aborted = error instanceof DOMException && error.name === 'AbortError';
        const message = aborted ? 'Speech generation stopped. Your text was kept.' : (error instanceof Error ? error.message : 'Speech generation failed.');
        output.innerHTML = `<div class="pg-output-empty"><strong>${aborted ? 'Generation stopped' : 'Speech request failed'}</strong><span></span><button type="button" class="pg-text-action" data-playground-generate-speech>Try again</button></div>`;
        safeQuery('.pg-output-empty span', output).textContent = message;
        setPlaygroundNetwork(aborted ? 'stopped' : 'failed', { mode: 'speech', model: model.value, timing: `${Math.max(.1, (Date.now() - startedAt) / 1000).toFixed(1)}s` });
        announce(message, !aborted);
      } finally {
        if (playgroundController === controller) { playgroundController = null; playgroundBusy(false); syncPlaygroundInputs(); }
      }
    };

    const runPlayground = async (form) => {
      const model = safeQuery('[data-playground-model]', form);
      const prompt = safeQuery('[data-playground-prompt]', form);
      const system = safeQuery('[data-playground-system]', form);
      const token = safeQuery('[data-playground-token]', form);
      const thread = safeQuery('[data-playground-thread]');
      const send = safeQuery('[data-playground-send]', form);
      const stop = safeQuery('[data-playground-stop]', form);
      const modelTrigger = safeQuery('[data-playground-model-trigger]', form);
      const metadata = safeQuery('[data-playground-meta]');
      if (!model || !prompt || !thread || !send || !stop) return;
      const promptValue = prompt.value.trim();
      if (!promptValue) {
        prompt.focus();
        announce('Write a message before sending.', true);
        return;
      }
      const requestControls = playgroundRequestControls(form);
      if (!requestControls) {
        announce('Review the highlighted request control before sending.', true);
        return;
      }
      savePlaygroundDraft();

      if (playgroundController) {
        announce('A request is already in progress. Stop it before sending another.', true);
        return;
      }
      const controller = new AbortController();
      playgroundController = controller;
      const empty = safeQuery('[data-playground-empty]', thread);
      if (empty) empty.remove();
      const makeMessage = (role, labelText, text) => {
        const message = document.createElement('article');
        message.className = `pg-message is-${role}`;
        message.setAttribute('role', 'article');
        message.setAttribute('aria-label', labelText);
        const label = document.createElement('span');
        label.className = 'pg-message-author';
        label.setAttribute('aria-hidden', 'true');
        label.textContent = labelText;
        if (role === 'assistant') {
          const icon = safeQuery('[data-playground-model-trigger-icon] .model-lab-mark');
          if (icon) {
            const tile = document.createElement('span');
            tile.className = 'pg-logo-tile pg-message-logo';
            tile.append(icon.cloneNode(true));
            label.prepend(tile);
          }
        }
        const content = document.createElement('div');
        content.className = 'pg-message-body';
        content.textContent = text;
        message.append(label, content);
        return { message, content };
      };
      const user = makeMessage('user', 'You', promptValue);
      const assistant = makeMessage('assistant', 'Mayhem', 'Connecting…');
      const userMessage = user.message;
      const assistantMessage = assistant.message;
      const assistantContent = assistant.content;
      const conversationOffset = playgroundConversation.length;
      userMessage.dataset.conversationOffset = String(conversationOffset);
      assistantMessage.dataset.conversationOffset = String(conversationOffset);
      addPromptAction(userMessage, promptValue, conversationOffset);
      assistantMessage.setAttribute('aria-busy', 'true');
      let messagesRoot = safeQuery('.pg-messages', thread);
      if (!messagesRoot) {
        messagesRoot = document.createElement('div');
        messagesRoot.className = 'pg-messages';
        const threadActions = document.createElement('div');
        threadActions.className = 'pg-thread-actions';
        const modelName = model.selectedOptions[0]?.dataset.modelName || model.value;
        threadActions.innerHTML = '<span class="pg-thread-model"></span><button type="button" data-playground-clear>Clear conversation</button>';
        safeQuery('.pg-thread-model', threadActions).textContent = modelName;
        messagesRoot.append(threadActions);
        thread.replaceChildren(messagesRoot);
      }
      messagesRoot.append(userMessage, assistantMessage);
      syncPlaygroundConversationUi();
      thread.scrollTop = thread.scrollHeight;
      send.disabled = true;
      send.setAttribute('aria-busy', 'true');
      if (modelTrigger) modelTrigger.disabled = true;
      stop.hidden = false;
      if (metadata) metadata.textContent = 'Request in progress';
      playgroundBusy(true);
      setPlaygroundNetwork('submitting', { mode: 'chat', model: model.value });

      const messages = [];
      if (system && system.value.trim()) messages.push({ role: 'system', content: system.value.trim() });
      messages.push(...playgroundConversation.slice(-16));
      messages.push({ role: 'user', content: promptValue });
      const headers = playgroundHeaders(form, requestControls);
      const requestedMaxTokens = requestControls.outputTokens;
      let assembled = '';
      let reportedModel = '';
      let reportedSessionId = '';
      let reportedCharge = '';
      let reportedFinishReason = '';
      let failureStatus = 0;
      const startedAt = Date.now();
      recordProductEvent('playground_request_started', { model: model.value });
      try {
        const response = await fetch('/v1/chat/completions', {
          method: 'POST',
          credentials: 'same-origin',
          headers,
          body: JSON.stringify({ model: model.value, messages, stream: true, stream_options: { include_usage: true }, max_tokens: requestedMaxTokens }),
          signal: controller.signal
        });
        if (!response.ok) {
          failureStatus = response.status;
          let message = `Request failed (${response.status})`;
          try {
            const failure = await response.json();
            message = failure?.error?.message || failure?.message || message;
          } catch (_) {}
          throw new Error(message);
        }
        if (!response.body) throw new Error('Streaming response body is unavailable');
        setPlaygroundNetwork('generating', { mode: 'chat', model: model.value });
        assistantContent.textContent = '';
        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = '';
        const consumeEvent = (event) => {
          const data = event
            .split(/\r?\n/)
            .filter((line) => line.startsWith('data:'))
            .map((line) => line.slice(5).trimStart())
            .join('\n')
            .trim();
          if (!data || data === '[DONE]') return;
          const payload = JSON.parse(data);
          if (payload?.error) throw new Error(payload.error.message || 'Streaming request failed');
          if (typeof payload?.model === 'string' && payload.model.trim()) reportedModel = payload.model.trim();
          const finishReason = payload?.choices?.[0]?.finish_reason;
          if (typeof finishReason === 'string' && finishReason.trim()) reportedFinishReason = finishReason.trim();
          const receipt = payload?.mayhem?.receipt;
          if (typeof receipt?.session_id === 'string' && receipt.session_id.trim()) reportedSessionId = receipt.session_id.trim();
          if (receipt?.au_owed_cum != null) reportedCharge = formatAuUsd(receipt.au_owed_cum);
          assembled += contentText(payload);
          assistantContent.textContent = assembled;
        };
        while (true) {
          const { value, done } = await reader.read();
          buffer += decoder.decode(value || new Uint8Array(), { stream: !done });
          const events = buffer.split(/\r?\n\r?\n/);
          buffer = events.pop() || '';
          events.forEach(consumeEvent);
          thread.scrollTop = thread.scrollHeight;
          if (done) {
            if (buffer.trim()) consumeEvent(buffer);
            break;
          }
        }
        if (!reportedFinishReason) {
          throw new Error('Stream ended before the provider reported a finish reason.');
        }
        const normalizedFinishReason = reportedFinishReason.toLocaleLowerCase();
        const outputLimitReached = ['length', 'max_tokens', 'max_output_tokens'].includes(normalizedFinishReason);
        const toolRequested = ['tool_calls', 'function_call'].includes(normalizedFinishReason);
        const stoppedNormally = normalizedFinishReason === 'stop';
        const finishLabel = outputLimitReached
          ? 'Output limit reached'
          : toolRequested
            ? 'Tool call requested'
            : normalizedFinishReason === 'content_filter'
              ? 'Response stopped by provider filter'
              : stoppedNormally
                ? 'Response complete'
                : reportedFinishReason
                  ? `Response ended (${reportedFinishReason})`
                  : 'Stream ended without a finish reason';
        const emptyResponse = toolRequested
          ? 'The model requested a tool. This Playground does not execute tools.'
          : normalizedFinishReason === 'content_filter'
            ? 'The provider stopped this response without returning text.'
            : 'The request returned no text output.';
        const assistantText = assembled || emptyResponse;
        assistantContent.textContent = assistantText;
        assistantMessage.removeAttribute('aria-busy');
        if (stoppedNormally) assistantMessage.classList.add('is-complete');
        const result = document.createElement('span');
        result.className = 'message-result';
        result.dataset.finishReason = reportedFinishReason || 'unreported';
        const mark = document.createElement('span');
        mark.className = 'message-result-mark';
        mark.setAttribute('aria-hidden', 'true');
        mark.textContent = stoppedNormally ? '✓' : outputLimitReached ? '!' : '→';
        const elapsed = Math.max(0.1, (Date.now() - startedAt) / 1000);
        const resultText = document.createElement('span');
        resultText.textContent = `${finishLabel} · ${reportedModel || model.value} · ${elapsed.toFixed(1)}s browser-observed${reportedCharge ? ' · Actual charge' : ''}`;
        result.append(mark, resultText);
        if (reportedCharge) {
          const charge = document.createElement('span');
          charge.dataset.money = '';
          const value = document.createElement('span');
          value.className = 'money-value';
          value.textContent = reportedCharge;
          charge.append(value);
          result.append(charge);
        }
        assistantMessage.append(result);
        const resultActions = document.createElement('div');
        resultActions.className = 'pg-message-actions';
        const copy = document.createElement('button');
        copy.className = 'pg-text-action';
        copy.type = 'button';
        copy.dataset.copy = '';
        copy.dataset.copyValue = assistantText;
        copy.setAttribute('aria-label', 'Copy Mayhem response');
        const copyLabel = document.createElement('span');
        copyLabel.dataset.copyLabel = '';
        copyLabel.textContent = 'Copy response';
        copy.append(copyLabel);
        resultActions.append(copy);
        if (outputLimitReached) {
          const continuation = document.createElement('button');
          const nextOutputLimit = Math.min(4096, Math.max(64, requestedMaxTokens * 2));
          continuation.className = 'pg-text-action';
          continuation.type = 'button';
          continuation.dataset.playgroundContinue = '';
          continuation.dataset.nextMaxTokens = String(nextOutputLimit);
          continuation.textContent = nextOutputLimit > requestedMaxTokens
            ? `Continue with ${nextOutputLimit.toLocaleString()}-token limit`
            : 'Continue response';
          resultActions.append(continuation);
        }
        if (reportedSessionId) {
          const receipt = document.createElement('a');
          receipt.className = 'pg-text-action';
          receipt.href = `/mayhem/dashboard/evidence?kind=receipt&id=${encodeURIComponent(reportedSessionId)}`;
          receipt.dataset.evidenceUrl = '';
          receipt.textContent = 'View receipt';
          resultActions.append(receipt);
        }
        assistantMessage.append(resultActions);
        applyAmountPreference();
        playgroundConversation.push(
          { role: 'user', content: promptValue },
          { role: 'assistant', content: assistantText }
        );
        if (playgroundConversation.length > 20) playgroundConversation.splice(0, playgroundConversation.length - 20);
        savePlaygroundConversation();
        prompt.value = '';
        savePlaygroundDraft();
        const promptHelp = safeQuery('#playground-prompt-help');
        if (promptHelp) promptHelp.textContent = 'Ask a question or describe a task.';
        if (metadata) metadata.textContent = `${finishLabel} with ${reportedModel || model.value}. Actual metering appears in Activity when the route supplies a receipt.`;
        announcePlaygroundAnswer(outputLimitReached ? 'Mayhem output limit reached.' : toolRequested ? 'Mayhem requested a tool.' : stoppedNormally ? 'Mayhem response complete.' : `Mayhem response ended: ${reportedFinishReason || 'finish reason unreported'}.`);
        if (stoppedNormally) window.setTimeout(() => assistantMessage.classList.remove('is-complete'), 520);
        recordProductEvent('playground_request_completed', {
          model: reportedModel || model.value,
          finishReason: reportedFinishReason,
          durationMs: Date.now() - startedAt,
          receipt: Boolean(reportedSessionId)
        });
        setPlaygroundNetwork('complete', {
          mode: 'chat', model: reportedModel || model.value, requestId: reportedSessionId || 'Completed',
          timing: `${elapsed.toFixed(1)}s browser-observed`, cost: reportedCharge || 'See Activity', receipt: Boolean(reportedSessionId)
        });
      } catch (error) {
        const aborted = error instanceof DOMException && error.name === 'AbortError';
        const technicalMessage = error instanceof Error ? error.message : 'Request failed';
        const hasPartialOutput = !aborted && Boolean(assembled);
        assistantContent.textContent = aborted
          ? (assembled || 'Request stopped. Your message is preserved below.')
          : hasPartialOutput
            ? assembled
            : technicalMessage;
        assistantMessage.removeAttribute('aria-busy');
        assistantMessage.classList.toggle('is-failed', !aborted);
        if (metadata) metadata.textContent = aborted ? 'Stopped by you' : 'Request needs attention';
        if (aborted) {
          addPromptAction(assistantMessage, promptValue, conversationOffset, 'Use message again');
          announce('Request stopped');
        } else {
          addFailureRecovery(assistantMessage, failureStatus, technicalMessage, promptValue, hasPartialOutput ? assembled : '');
          announce(hasPartialOutput
            ? 'Incomplete response preserved. The request can be retried.'
            : 'Request failed. Your message and settings are preserved.', true);
        }
        recordProductEvent(aborted ? 'playground_request_stopped' : 'playground_request_failed', {
          model: model.value,
          status: failureStatus || null,
          partialOutput: hasPartialOutput,
          durationMs: Date.now() - startedAt
        });
        setPlaygroundNetwork(aborted ? 'stopped' : 'failed', {
          mode: 'chat', model: model.value, timing: `${Math.max(.1, (Date.now() - startedAt) / 1000).toFixed(1)}s`
        });
      } finally {
        if (playgroundController === controller) {
          send.disabled = false;
          send.removeAttribute('aria-busy');
          if (modelTrigger) modelTrigger.disabled = false;
          stop.hidden = true;
          playgroundController = null;
          playgroundBusy(false);
          syncPlaygroundInputs();
        }
      }
    };

    const updatePlaygroundPriceMode = () => {
      const input = safeQuery('[data-playground-max-price]');
      if (!input) return 'rate';
      const priceMode = selectedPlaygroundPriceMode();
      input.dataset.priceMode = priceMode;
      input.pattern = priceMode === 'fixed' ? '[0-9]+([.][0-9]{1,18})?' : '[0-9]+([.][0-9]{1,15})?';
      input.setCustomValidity('');
      const label = safeQuery('[data-playground-price-label]');
      const unit = safeQuery('[data-playground-price-unit]');
      const help = safeQuery('[data-playground-price-help]');
      if (priceMode === 'fixed') {
        if (label) label.textContent = 'Fixed route charge ceiling';
        if (unit) unit.textContent = 'USD';
        if (help) help.textContent = 'Maximum fixed route charge in USD. The gateway compares it with the larger of the model\'s per-request and minimum-session charges. This is not a total-spend cap.';
      } else {
        if (label) label.textContent = 'Route rate ceiling';
        if (unit) unit.textContent = '$ / 1M-unit basket';
        if (help) help.textContent = 'Combined catalog rate for 1M of each priced unit (input, cached input, and output for text). This is a hard route filter, not a total-spend cap.';
      }
      return priceMode;
    };

    const updatePlaygroundControlSummary = () => {
      const summary = safeQuery('[data-playground-request-summary]');
      if (!summary) return;
      const outputTokens = safeQuery('[data-playground-max-tokens]')?.value.trim() || '512';
      const maxPrice = safeQuery('[data-playground-max-price]')?.value.trim() || '';
      const minAttTier = safeQuery('[data-playground-min-att-tier]')?.value || '';
      const priceMode = selectedPlaygroundPriceMode();
      const controls = [`${outputTokens} output tokens`];
      if (maxPrice) {
        controls.push(root.classList.contains('amounts-hidden')
          ? 'price ceiling hidden'
          : priceMode === 'fixed'
            ? `≤ $${maxPrice} fixed route charge`
            : `≤ $${maxPrice} / 1M-unit route basket`);
      }
      if (minAttTier) controls.push(`minimum T${minAttTier}`);
      if (!maxPrice && !minAttTier) controls.push('gateway price/trust defaults');
      summary.textContent = controls.join(' · ');
    };

    const updatePreflight = () => {
      const select = safeQuery('[data-playground-model]');
      const target = safeQuery('[data-playground-preflight]');
      if (!select || !target) return;
      const option = select.selectedOptions[0];
      const capacityExpired = safeQuery('[data-page-status-freshness]')?.dataset.freshnessExpired === 'true';
      const values = target.querySelectorAll('[data-preflight-value]');
      values.forEach((node) => {
        const key = node.getAttribute('data-preflight-value');
        const value = key === 'availability' && capacityExpired
          ? 'Refresh to reconfirm'
          : (option?.dataset[key] || 'Unavailable');
        node.textContent = value;
        if (node.matches('.money-value')) node.dataset.moneyOriginal = value;
      });
      syncPlaygroundModelPicker();
      updatePlaygroundPriceMode();
      updatePlaygroundControlSummary();
      applyAmountPreference();
    };

    const openEvidence = async (link, dialog) => {
      const title = safeQuery('[data-evidence-title]', dialog);
      const summary = safeQuery('[data-evidence-summary]', dialog);
      const interpretation = safeQuery('[data-evidence-interpretation]', dialog);
      const meta = safeQuery('[data-evidence-meta]', dialog);
      const state = safeQuery('[data-evidence-state]', dialog);
      const factsSection = safeQuery('[data-evidence-facts-section]', dialog);
      const facts = safeQuery('[data-evidence-facts]', dialog);
      const factCount = safeQuery('[data-evidence-fact-count]', dialog);
      const rawSection = safeQuery('[data-evidence-raw-section]', dialog);
      const raw = safeQuery('[data-evidence-raw]', dialog);
      const copyButton = safeQuery('[data-evidence-copy]', dialog);
      const downloadButton = safeQuery('[data-evidence-download]', dialog);
      if (!title || !summary || !state || !factsSection || !facts || !rawSection || !raw) return;

      title.textContent = 'Evidence';
      summary.textContent = 'Loading the requested snapshot…';
      if (interpretation) {
        interpretation.hidden = true;
        interpretation.textContent = '';
      }
      if (meta) meta.textContent = 'Requesting a fresh gateway snapshot';
      state.hidden = false;
      state.className = 'notice';
      state.setAttribute('role', 'status');
      state.textContent = 'Loading evidence…';
      factsSection.hidden = true;
      rawSection.hidden = true;
      facts.replaceChildren();
      if (factCount) factCount.textContent = '';
      raw.textContent = '';
      evidencePayloads.delete(dialog);
      if (copyButton) copyButton.disabled = true;
      if (downloadButton) downloadButton.disabled = true;
      evidenceOriginals.delete(raw);
      dialog.setAttribute('aria-busy', 'true');
      try {
        const response = await fetch(link.href, {
          credentials: 'same-origin',
          cache: 'no-store',
          headers: { accept: 'application/json' }
        });
        let payload = null;
        try { payload = await response.json(); } catch (_) {}
        if (!response.ok || !payload || typeof payload !== 'object') {
          throw new Error(response.status === 401
            ? 'This dashboard session expired. Reload the dashboard to continue.'
            : 'This evidence is no longer available in the current gateway snapshot.');
        }
        if (recordDashboardSessionActivity) recordDashboardSessionActivity();
        title.textContent = typeof payload.title === 'string' ? payload.title : 'Evidence';
        const evidenceSummary = typeof payload.summary === 'string' ? payload.summary : 'Requested dashboard evidence';
        const evidenceInterpretation = typeof payload.interpretation === 'string' ? payload.interpretation : '';
        summary.textContent = evidenceSummary;
        if (interpretation) {
          interpretation.textContent = evidenceInterpretation;
          interpretation.hidden = !evidenceInterpretation;
        }
        if (meta) meta.textContent = 'Snapshot loaded from this gateway';
        const factItems = Array.isArray(payload.facts) ? payload.facts : [];
        if (factCount) factCount.textContent = `${factItems.length} ${factItems.length === 1 ? 'fact' : 'facts'}`;
        factItems.forEach((fact) => {
          const item = document.createElement('div');
          item.className = 'verify-fact';
          const label = document.createElement('span');
          label.textContent = typeof fact?.label === 'string' ? fact.label : 'Fact';
          const value = document.createElement('strong');
          value.textContent = fact?.value == null ? 'Unavailable' : String(fact.value);
          const basis = document.createElement('small');
          basis.textContent = typeof fact?.basis === 'string' ? fact.basis : 'Source unavailable';
          item.append(label, value, basis);
          facts.append(item);
        });
        const rawText = JSON.stringify(payload.raw ?? null, null, 2);
        raw.textContent = rawText;
        evidenceOriginals.set(raw, rawText);
        evidencePayloads.set(dialog, JSON.stringify(payload, null, 2));
        state.hidden = true;
        factsSection.hidden = false;
        rawSection.hidden = false;
        // Very large snapshots stay one click away so the drawer opens light;
        // Copy and Download always act on the complete payload.
        const rawToggle = safeQuery('[data-evidence-raw-toggle]', dialog);
        const rawToggleLabel = safeQuery('[data-evidence-raw-toggle-label]', dialog);
        const rawSize = safeQuery('[data-evidence-raw-size]', dialog);
        const rawIsLarge = rawText.length > 60000;
        if (rawToggle) {
          rawToggle.hidden = !rawIsLarge;
          rawToggle.setAttribute('aria-expanded', String(!rawIsLarge));
        }
        if (rawToggleLabel) rawToggleLabel.textContent = rawIsLarge ? 'Show raw JSON' : 'Raw JSON shown';
        if (rawSize) rawSize.textContent = `${Math.max(1, Math.round(rawText.length / 1024))} KB`;
        raw.hidden = rawIsLarge;
        applyAmountPreference();
        if (copyButton) copyButton.disabled = false;
        if (downloadButton) downloadButton.disabled = false;
      } catch (error) {
        state.hidden = false;
        state.className = 'notice danger';
        state.setAttribute('role', 'alert');
        state.textContent = error instanceof Error ? error.message : 'Evidence could not be loaded.';
        summary.textContent = 'The requested snapshot could not be opened.';
        if (interpretation) interpretation.hidden = true;
        if (meta) meta.textContent = 'Snapshot unavailable';
        if (copyButton) copyButton.disabled = true;
        if (downloadButton) downloadButton.disabled = true;
      } finally {
        dialog.removeAttribute('aria-busy');
      }
    };

    document.addEventListener('click', async (event) => {
      const productEvent = event.target.closest('[data-product-event]');
      if (productEvent) {
        recordProductEvent(productEvent.dataset.productEvent, {
          destination: productEvent.getAttribute('href') || null
        });
      }
      const modelPickerTrigger = event.target.closest('[data-playground-model-trigger]');
      if (modelPickerTrigger) {
        if (modelPickerTrigger.getAttribute('aria-expanded') === 'true') closePlaygroundModelPicker(false);
        else openPlaygroundModelPicker();
        return;
      }
      const modelPickerOption = event.target.closest('[data-playground-model-option]');
      if (modelPickerOption) {
        choosePlaygroundModel(modelPickerOption.dataset.playgroundModelOption || '');
        return;
      }
      if (event.target.closest('[data-playground-model-close]')) {
        closePlaygroundModelPicker(true);
        return;
      }
      if (!event.target.closest('[data-playground-model-picker]')) closePlaygroundModelPicker(false);
      const modeTab = event.target.closest('[data-playground-mode-tab]');
      if (modeTab) {
        setPlaygroundMode(modeTab.dataset.playgroundModeTab || 'chat');
        return;
      }
      const starter = event.target.closest('[data-playground-starter]');
      if (starter) {
        const form = starter.closest('[data-playground-form]');
        const prompt = safeQuery('[data-playground-prompt]', form || document);
        if (form && prompt) {
          prompt.value = starter.dataset.playgroundStarterPrompt || '';
          syncPlaygroundInputs();
          if (typeof form.requestSubmit === 'function') form.requestSubmit();
          else runPlayground(form);
        }
        return;
      }
      const ratioButton = event.target.closest('[data-playground-aspect-ratio]');
      if (ratioButton) {
        document.querySelectorAll('[data-playground-aspect-ratio]').forEach((button) => {
          const selected = button === ratioButton;
          button.classList.toggle('is-active', selected);
          button.setAttribute('aria-pressed', String(selected));
        });
        savePlaygroundDraft();
        return;
      }
      if (event.target.closest('[data-playground-generate-image]')) {
        const form = safeQuery('[data-playground-form]');
        if (form) await runPlaygroundImage(form);
        return;
      }
      if (event.target.closest('[data-playground-generate-speech]')) {
        const form = safeQuery('[data-playground-form]');
        if (form) await runPlaygroundSpeech(form);
        return;
      }
      if (event.target.closest('[data-playground-clear-image]')) {
        const output = safeQuery('[data-playground-image-output]');
        if (output) output.innerHTML = '<div class="pg-output-empty"><svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="3"></rect><circle cx="9" cy="10" r="2"></circle><path d="m5 18 5-5 3 3 2-2 4 4"></path></svg><strong>Image output</strong><span>Your generated image will appear here.</span></div>';
        setPlaygroundNetwork('ready');
        return;
      }
      if (event.target.closest('[data-playground-clear-speech]')) {
        const output = safeQuery('[data-playground-speech-output]');
        if (playgroundAudioUrl) { URL.revokeObjectURL(playgroundAudioUrl); playgroundAudioUrl = null; }
        if (output) output.innerHTML = '<div class="pg-output-empty"><svg viewBox="0 0 24 24"><path d="M11 5 6.6 8.5H3.8a.8.8 0 0 0-.8.8v5.4a.8.8 0 0 0 .8.8h2.8L11 19Z"></path><path d="M14.8 9.2a4.1 4.1 0 0 1 0 5.6"></path></svg><strong>Audio output</strong><span>Generated speech will appear here.</span></div>';
        setPlaygroundNetwork('ready');
        return;
      }
      const networkToggle = event.target.closest('[data-playground-network-toggle]');
      if (networkToggle) {
        const body = safeQuery('[data-playground-network-body]');
        const open = networkToggle.getAttribute('aria-expanded') !== 'true';
        networkToggle.setAttribute('aria-expanded', String(open));
        if (body) body.hidden = !open;
        return;
      }
      const menuButton = event.target.closest('[data-nav-toggle]');
      if (menuButton) {
        setDrawer(!body.classList.contains('nav-open'), menuButton);
        return;
      }
      if (event.target.closest('[data-nav-close]')) {
        setDrawer(false);
        return;
      }

      if (event.target.closest('[data-sidebar-toggle]')) {
        const collapsed = !root.classList.contains('sidebar-collapsed');
        root.classList.toggle('sidebar-collapsed', collapsed);
        storage.set(preferenceKeys.sidebar, collapsed ? '1' : '0');
        applyPreferenceButtons();
        announce(collapsed ? 'Navigation collapsed' : 'Navigation expanded', false, false);
        return;
      }

      const evidenceDownload = event.target.closest('[data-evidence-download]');
      if (evidenceDownload) {
        const dialog = evidenceDownload.closest('dialog');
        const original = dialog ? evidencePayloads.get(dialog) : '';
        let value = original || '';
        if (value && root.classList.contains('amounts-hidden')) {
          try { value = JSON.stringify(redactMoney(JSON.parse(value)), null, 2); } catch (_) {}
        }
        if (!value) return;
        const blob = new Blob([value], { type: 'application/json;charset=utf-8' });
        const href = URL.createObjectURL(blob);
        const link = document.createElement('a');
        link.href = href;
        link.download = 'mayhem-evidence.json';
        document.body.appendChild(link);
        link.click();
        link.remove();
        window.setTimeout(() => URL.revokeObjectURL(href), 0);
        announce('Evidence JSON download started.', false, false);
        return;
      }

      const sortButton = event.target.closest('[data-sort-column]');
      if (sortButton) {
        const table = safeQuery(sortButton.dataset.sortTable);
        const columnIndex = Number.parseInt(sortButton.dataset.sortColumn || '', 10);
        const header = sortButton.closest('th');
        if (table && Number.isInteger(columnIndex) && header) {
          const direction = header.getAttribute('aria-sort') === 'ascending' ? 'descending' : 'ascending';
          sortTable(
            table,
            columnIndex,
            direction,
            sortButton.textContent.trim(),
            true,
            sortButton.dataset.tableQueryPrefix || ''
          );
        }
        return;
      }

      const exportButton = event.target.closest('[data-export-table]');
      if (exportButton) {
        const table = safeQuery(exportButton.dataset.exportTable);
        if (table) exportShownTable(table);
        return;
      }

      const rawToggle = event.target.closest('[data-evidence-raw-toggle]');
      if (rawToggle) {
        const dialogHost = rawToggle.closest('dialog, .evidence-standalone');
        const rawNode = dialogHost ? safeQuery('[data-evidence-raw]', dialogHost) : null;
        if (rawNode) {
          const willOpen = rawNode.hidden;
          rawNode.hidden = !willOpen;
          rawToggle.setAttribute('aria-expanded', String(willOpen));
          const label = safeQuery('[data-evidence-raw-toggle-label]', rawToggle);
          if (label) label.textContent = willOpen ? 'Hide raw JSON' : 'Show raw JSON';
        }
        return;
      }

      const capabilityToggle = event.target.closest('[data-catalog-capabilities-toggle]');
      if (capabilityToggle) {
        const capabilities = capabilityToggle.closest('.catalog-capabilities');
        const expanded = capabilityToggle.getAttribute('aria-expanded') !== 'true';
        capabilities?.querySelectorAll('.catalog-capability-extra').forEach((capability) => {
          capability.hidden = !expanded;
        });
        capabilityToggle.setAttribute('aria-expanded', String(expanded));
        const label = safeQuery('[data-catalog-capabilities-label]', capabilityToggle);
        if (label) label.textContent = expanded
          ? 'Show less'
          : capabilityToggle.dataset.collapsedLabel || 'Show more';
        announce(expanded ? 'All model capabilities shown' : 'Additional model capabilities hidden', false, false);
        return;
      }

      const copyButton = event.target.closest('[data-copy]');
      if (copyButton) {
        const target = safeQuery(copyButton.getAttribute('data-copy-target'));
        const value = copyButton.getAttribute('data-copy-value') || (target ? target.textContent.trim() : '');
        const label = safeQuery('[data-copy-label]', copyButton);
        const originalLabel = label?.textContent || 'Copy';
        const originalAria = copyButton.getAttribute('aria-label');
        try {
          await copyText(value);
          if (label) label.textContent = 'Copied';
          copyButton.setAttribute('aria-label', 'Copied');
          announce('Copied to clipboard', false, false);
        } catch (_) {
          if (label) label.textContent = 'Select value';
          announce('Copy was blocked. The value remains selectable.', true, false);
        }
        window.setTimeout(() => {
          if (!copyButton.isConnected) return;
          if (label) label.textContent = originalLabel;
          if (originalAria) copyButton.setAttribute('aria-label', originalAria);
          else copyButton.removeAttribute('aria-label');
        }, 1800);
        return;
      }

      const evidenceLink = event.target.closest('[data-evidence-url]');
      if (evidenceLink) {
        const dialog = safeQuery('#dashboard-evidence-dialog');
        const plainPrimaryClick = event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;
        if (!plainPrimaryClick || !dialog || typeof dialog.showModal !== 'function') return;
        event.preventDefault();
        dialogTriggers.set(dialog, evidenceLink);
        dialog.showModal();
        await openEvidence(evidenceLink, dialog);
        return;
      }

      const modelDetailOpen = event.target.closest('[data-model-detail-open]');
      if (modelDetailOpen) {
        const dialog = safeQuery('#model-detail-dialog');
        const content = dialog ? safeQuery('[data-model-detail-content]', dialog) : null;
        const template = modelDetailOpen.closest('th')?.querySelector('[data-model-detail-template]');
        if (!dialog || !content || !(template instanceof HTMLTemplateElement) || typeof dialog.showModal !== 'function') return;
        content.replaceChildren(template.content.cloneNode(true));
        dialogTriggers.set(dialog, modelDetailOpen);
        dialog.showModal();
        applyAmountPreference();
        return;
      }

      const closeButton = event.target.closest('[data-dialog-close]');
      if (closeButton) {
        closeButton.closest('dialog')?.close();
        return;
      }

      if (event.target.closest('[data-hide-amounts]')) {
        const hidden = !root.classList.contains('amounts-hidden');
        root.classList.toggle('amounts-hidden', hidden);
        storage.set(preferenceKeys.amounts, hidden ? '1' : '0');
        applyPreferenceButtons();
        updatePlaygroundControlSummary();
        announce(hidden ? 'Amounts hidden' : 'Amounts visible');
        return;
      }

      const preference = event.target.closest('[data-preference]');
      if (preference) {
        const name = preference.getAttribute('data-preference');
        const map = {
          motion: ['motion-reduced', preferenceKeys.motion],
          density: ['compact-density', preferenceKeys.density],
          amounts: ['amounts-hidden', preferenceKeys.amounts]
        };
        const config = map[name];
        if (config) {
          const enabled = !root.classList.contains(config[0]);
          root.classList.toggle(config[0], enabled);
          storage.set(config[1], enabled ? '1' : '0');
          applyPreferenceButtons();
          if (name === 'amounts') updatePlaygroundControlSummary();
          announce(`${name === 'motion' ? 'Reduced motion' : name === 'density' ? 'Compact density' : 'Amount hiding'} ${enabled ? 'on' : 'off'}`);
        }
        return;
      }

      if (event.target.closest('[data-clear-preferences]')) {
        Object.values(preferenceKeys).forEach((key) => storage.remove(key));
        root.classList.remove('amounts-hidden', 'motion-reduced', 'compact-density', 'sidebar-collapsed');
        applyPreferenceButtons();
        updatePlaygroundControlSummary();
        announce('Local display preferences reset');
        return;
      }

      if (event.target.closest('[data-clear-playground-history]')) {
        storage.remove(playgroundConversationKey);
        playgroundConversation.length = 0;
        announce('Playground conversation history cleared from this browser');
        return;
      }

      if (event.target.closest('[data-clear-local-events]')) {
        storage.remove(localProductEventsKey);
        updateLocalProductEventCount();
        announce('Local launch diagnostics cleared');
        return;
      }

      if (event.target.closest('[data-export-local-events]')) {
        const events = readLocalProductEvents();
        const blob = new Blob([JSON.stringify({ exportedAt: new Date().toISOString(), events }, null, 2)], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const link = document.createElement('a');
        link.href = url;
        link.download = 'mayhem-local-launch-diagnostics.json';
        link.click();
        window.setTimeout(() => URL.revokeObjectURL(url), 0);
        announce(`Exported ${events.length} local diagnostic event${events.length === 1 ? '' : 's'}`);
        return;
      }

      const connectionButton = event.target.closest('[data-connection-test]');
      if (connectionButton) {
        await testConnection(connectionButton);
        return;
      }

      if (event.target.closest('[data-playground-stop]')) {
        playgroundController?.abort();
        return;
      }

      if (event.target.closest('[data-playground-reset-draft]')) {
        if (playgroundController) {
          announce('Stop the current request before resetting its saved draft.', true);
          return;
        }
        const model = safeQuery('[data-playground-model]');
        const prompt = safeQuery('[data-playground-prompt]');
        const imagePrompt = safeQuery('[data-playground-image-prompt]');
        const speechText = safeQuery('[data-playground-speech-text]');
        const system = safeQuery('[data-playground-system]');
        const output = safeQuery('[data-playground-max-tokens]');
        const price = safeQuery('[data-playground-max-price]');
        const tier = safeQuery('[data-playground-min-att-tier]');
        if (model) {
          const defaultValue = model.dataset.defaultValue || model.options[0]?.value || '';
          if (Array.from(model.options).some((option) => option.value === defaultValue)) model.value = defaultValue;
        }
        if (prompt) prompt.value = '';
        if (imagePrompt) imagePrompt.value = '';
        if (speechText) speechText.value = '';
        if (system) system.value = '';
        if (output) output.value = '512';
        if (price) price.value = '';
        if (tier) tier.value = '';
        document.querySelectorAll('[data-playground-aspect-ratio]').forEach((button) => {
          const selected = button.dataset.playgroundAspectRatio === '1:1';
          button.classList.toggle('is-active', selected);
          button.setAttribute('aria-pressed', String(selected));
        });
        const defaultVoice = safeQuery('[data-playground-voice][value="af_heart"]');
        if (defaultVoice) defaultVoice.checked = true;
        taskStorage.remove(playgroundDraftKey);
        const promptHelp = safeQuery('#playground-prompt-help');
        if (promptHelp) promptHelp.textContent = 'Ask a question or describe a task.';
        const metadata = safeQuery('[data-playground-meta]');
        if (metadata) metadata.textContent = 'Saved draft and controls reset';
        updatePreflight();
        const defaultMode = model?.selectedOptions?.[0]?.dataset.playgroundMode || 'chat';
        if (playgroundOptionsForMode(defaultMode).length) setPlaygroundMode(defaultMode);
        syncPlaygroundInputs();
        taskStorage.remove(playgroundDraftKey);
        announce('Saved Playground draft and controls reset. Access token unchanged.', false, false);
        return;
      }

      const continuation = event.target.closest('[data-playground-continue]');
      if (continuation) {
        if (playgroundController) {
          announce('Stop the current request before preparing a continuation.', true);
          return;
        }
        const prompt = safeQuery('[data-playground-prompt]');
        const output = safeQuery('[data-playground-max-tokens]');
        const nextOutput = Number.parseInt(continuation.dataset.nextMaxTokens || '', 10);
        if (output && Number.isInteger(nextOutput) && nextOutput >= 64 && nextOutput <= 4096) {
          output.value = String(nextOutput);
          output.closest('details')?.setAttribute('open', '');
        }
        if (prompt) {
          prompt.value = 'Continue from where you stopped.';
          prompt.focus();
        }
        const promptHelp = safeQuery('#playground-prompt-help');
        if (promptHelp) promptHelp.textContent = 'Continuation prepared; review it, then send when ready.';
        const metadata = safeQuery('[data-playground-meta]');
        if (metadata) metadata.textContent = 'Continuation ready to review';
        savePlaygroundDraft();
        updatePreflight();
        announce('Continuation prepared with the updated output limit.', false, false);
        return;
      }

      const reusedPrompt = event.target.closest('[data-playground-reuse-prompt]');
      if (reusedPrompt) {
        if (playgroundController) {
          announce('Stop the current request before editing an earlier message.', true);
          return;
        }
        const prompt = safeQuery('[data-playground-prompt]');
        const value = reusedPrompt.dataset.prompt || '';
        const offset = Number.parseInt(reusedPrompt.dataset.conversationOffset || '', 10);
        if (Number.isInteger(offset) && offset >= 0) {
          playgroundConversation.splice(offset);
          savePlaygroundConversation();
          document.querySelectorAll('.pg-message[data-conversation-offset]').forEach((message) => {
            const messageOffset = Number.parseInt(message.dataset.conversationOffset || '', 10);
            if (Number.isInteger(messageOffset) && messageOffset >= offset) message.remove();
          });
          const thread = safeQuery('[data-playground-thread]');
          if (thread && !safeQuery('.pg-message', thread)) {
            const empty = document.createElement('div');
            empty.className = 'pg-chat-empty';
            empty.dataset.playgroundEmpty = '';
            empty.innerHTML = '<h2>Message ready to revise</h2><p>Edit it below, then send when ready.</p>';
            thread.append(empty);
          }
          syncPlaygroundConversationUi();
        }
        if (prompt) {
          prompt.value = value;
          savePlaygroundDraft();
          prompt.focus();
          const promptHelp = safeQuery('#playground-prompt-help');
          if (promptHelp) promptHelp.textContent = 'Earlier conversation after this point was removed.';
          const metadata = safeQuery('[data-playground-meta]');
          if (metadata) metadata.textContent = 'Editing an earlier message';
          announce('Message restored to the composer.', false, false);
        }
        return;
      }

      const retryRequest = event.target.closest('[data-playground-retry]');
      if (retryRequest) {
        if (playgroundController) {
          announce('A request is already in progress.', true);
          return;
        }
        const form = safeQuery('[data-playground-form]');
        const prompt = safeQuery('[data-playground-prompt]', form || document);
        const value = retryRequest.dataset.prompt || '';
        if (form && prompt) {
          prompt.value = value;
          savePlaygroundDraft();
          if (typeof form.requestSubmit === 'function') form.requestSubmit();
          else runPlayground(form);
        }
        return;
      }

      if (event.target.closest('[data-playground-clear]')) {
        if (playgroundController) {
          announce('Stop the current request before clearing the conversation.', true);
          return;
        }
        playgroundConversation.length = 0;
        storage.remove(playgroundConversationKey);
        const thread = safeQuery('[data-playground-thread]');
        if (thread) {
          const empty = document.createElement('div');
          empty.className = 'pg-chat-empty';
          empty.dataset.playgroundEmpty = '';
          empty.innerHTML = '<h2>How can I help?</h2><p>Conversation context was cleared. Write a message below.</p>';
          thread.replaceChildren(empty);
        }
        syncPlaygroundConversationUi();
        const answerStatus = safeQuery('[data-playground-answer-status]');
        if (answerStatus) answerStatus.textContent = '';
        const metadata = safeQuery('[data-playground-meta]');
        if (metadata) metadata.textContent = 'No request sent';
        const prompt = safeQuery('[data-playground-prompt]');
        if (prompt) prompt.value = '';
        savePlaygroundDraft();
        const promptHelp = safeQuery('#playground-prompt-help');
        if (promptHelp) promptHelp.textContent = 'Ask a question or describe a task.';
        announce('Conversation cleared');
        return;
      }

      if (event.target.closest('[data-session-extend]')) {
        const button = event.target.closest('[data-session-extend]');
        if (typeof refreshDashboardSession !== 'function') return;
        button.disabled = true;
        button.setAttribute('aria-busy', 'true');
        refreshDashboardSession()
          .then((renewed) => {
            announce(
              renewed
                ? 'Dashboard session extended. You can continue on this page.'
                : 'The session could not be extended. Your current page remains open.',
              !renewed,
              false
            );
          })
          .finally(() => {
            button.disabled = false;
            button.removeAttribute('aria-busy');
          });
        return;
      }

      if (event.target.closest('[data-session-reload]')) {
        const button = event.target.closest('[data-session-reload]');
        if (typeof refreshDashboardSession !== 'function') {
          window.location.reload();
          return;
        }
        button.disabled = true;
        button.setAttribute('aria-busy', 'true');
        refreshDashboardSession()
          .then((renewed) => {
            if (!renewed) announce('Session still locked. Reopen the dashboard URL printed by mayhem up.', true);
          })
          .finally(() => {
            button.disabled = false;
            button.removeAttribute('aria-busy');
          });
        return;
      }

      if (event.target.closest('[data-session-dismiss]')) {
        event.target.closest('[data-session-expired]')?.remove();
        body.classList.remove('session-expired-visible');
        body.dataset.sessionNoticeDismissed = 'true';
        announce('Session notice dismissed');
        return;
      }

      if (event.target.closest('[data-session-warning-dismiss]')) {
        event.target.closest('[data-session-warning]')?.remove();
        body.classList.remove('session-expired-visible');
        body.dataset.sessionWarningDismissed = 'true';
        announce('Session warning dismissed', false, false);
      }
    });

    document.addEventListener('submit', (event) => {
      const form = event.target.closest('[data-playground-form]');
      if (!form) return;
      event.preventDefault();
      const mode = currentPlaygroundMode();
      if (mode === 'image') runPlaygroundImage(form);
      else if (mode === 'speech') runPlaygroundSpeech(form);
      else runPlayground(form);
    });

    document.addEventListener('close', (event) => {
      const dialog = event.target instanceof HTMLDialogElement ? event.target : null;
      const trigger = dialog ? dialogTriggers.get(dialog) : null;
      if (trigger?.isConnected) trigger.focus();
    }, true);

    document.addEventListener('input', (event) => {
      const filter = event.target.closest('[data-table-filter]');
      if (filter) updateFilter(filter, true);
      if (event.target.matches('[data-playground-draft]')) {
        // A select emits `input` before `change`. Preserve the old price-mode
        // marker until the change handler can compare bases and clear a value
        // that would otherwise be silently reinterpreted.
        if (event.target.matches('[data-playground-model]')) return;
        savePlaygroundDraft();
        updatePreflight();
        syncPlaygroundInputs();
      }
    });

    document.addEventListener('change', (event) => {
      if (event.target.matches('[data-playground-draft]')) {
        if (event.target.matches('[data-playground-model]')) {
          const price = safeQuery('[data-playground-max-price]');
          const previousMode = price?.dataset.priceMode || '';
          const nextMode = selectedPlaygroundPriceMode();
          if (price?.value && previousMode && previousMode !== nextMode) {
            price.value = '';
            announce('Price ceiling cleared because this model uses a different price basis.', false, false);
          }
        }
        updatePreflight();
        savePlaygroundDraft();
        syncPlaygroundInputs();
      }
      if (event.target.matches('[data-playground-voice]')) {
        document.querySelectorAll('.pg-voice-option').forEach((option) => {
          option.classList.toggle('is-selected', Boolean(safeQuery('[data-playground-voice]', option)?.checked));
        });
        savePlaygroundDraft();
      }
    });

    document.addEventListener('keydown', (event) => {
      const modelPickerOption = event.target.closest?.('[data-playground-model-option]');
      if (modelPickerOption) {
        const options = Array.from(document.querySelectorAll('[data-playground-model-option]')).filter((option) => !option.hidden);
        const index = options.indexOf(modelPickerOption);
        let next = null;
        if (event.key === 'ArrowDown') next = Math.min(options.length - 1, index + 1);
        else if (event.key === 'ArrowUp') next = Math.max(0, index - 1);
        else if (event.key === 'Home') next = 0;
        else if (event.key === 'End') next = options.length - 1;
        else if (event.key === 'Escape') {
          event.preventDefault();
          closePlaygroundModelPicker(true);
          return;
        } else if (event.key === 'Tab') {
          closePlaygroundModelPicker(false);
        }
        if (next !== null && options[next]) {
          event.preventDefault();
          options[next].focus();
          return;
        }
      }
      const modelPickerTrigger = event.target.closest?.('[data-playground-model-trigger]');
      if (modelPickerTrigger && ['ArrowDown', 'ArrowUp'].includes(event.key)) {
        event.preventDefault();
        openPlaygroundModelPicker();
        return;
      }
      const modeTab = event.target.closest?.('[data-playground-mode-tab]');
      if (modeTab && ['ArrowRight', 'ArrowDown', 'ArrowLeft', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
        const tabs = Array.from(document.querySelectorAll('[data-playground-mode-tab]:not(:disabled)'));
        const current = tabs.indexOf(modeTab);
        let next = current;
        if (['ArrowRight', 'ArrowDown'].includes(event.key)) next = (current + 1) % tabs.length;
        else if (['ArrowLeft', 'ArrowUp'].includes(event.key)) next = (current - 1 + tabs.length) % tabs.length;
        else if (event.key === 'Home') next = 0;
        else if (event.key === 'End') next = tabs.length - 1;
        if (tabs[next]) {
          event.preventDefault();
          setPlaygroundMode(tabs[next].dataset.playgroundModeTab || 'chat', true);
          return;
        }
      }
      if (event.key === 'Escape' && safeQuery('[data-playground-model-trigger]')?.getAttribute('aria-expanded') === 'true') {
        event.preventDefault();
        closePlaygroundModelPicker(true);
        return;
      }
      if (event.key === 'Escape' && body.classList.contains('nav-open')) {
        event.preventDefault();
        setDrawer(false);
        return;
      }
      if (event.key === 'Tab' && body.classList.contains('nav-open') && drawer) {
        const focusable = drawerFocusable();
        if (!focusable.length) return;
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault(); last.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault(); first.focus();
        }
      }
      const target = event.target;
      const isTyping = target instanceof HTMLElement && (
        target.matches('input, textarea, select') || target.isContentEditable
      );
      if (event.key === '/' && !event.altKey && !event.ctrlKey && !event.metaKey && !isTyping) {
        const quickTarget = safeQuery('[data-table-filter], [data-playground-prompt]');
        if (quickTarget) {
          event.preventDefault();
          quickTarget.focus();
        }
      }
      if (event.key === 'Escape' && target?.matches?.('[data-table-filter]') && target.value) {
        event.preventDefault();
        target.value = '';
        updateFilter(target, true);
      }
    });

    mobileQuery.addEventListener('change', (event) => {
      if (!event.matches) setDrawer(false);
    });

    const session = safeQuery('[data-session-seconds]');
    if (session) {
      const initial = Number.parseInt(session.getAttribute('data-session-seconds') || '', 10);
      if (Number.isFinite(initial) && initial >= 0) {
        let deadline = Date.now() + initial * 1000;
        const clearSessionNotices = () => {
          safeQuery('[data-session-expired]')?.remove();
          safeQuery('[data-session-warning]')?.remove();
          body.classList.remove('session-expired-visible');
          delete body.dataset.sessionNoticeDismissed;
          delete body.dataset.sessionWarningDismissed;
        };
        const showExpiryWarning = () => {
          if (safeQuery('[data-session-warning]') || safeQuery('[data-session-expired]') || body.dataset.sessionWarningDismissed === 'true') return;
          const notice = document.createElement('div');
          notice.className = 'session-expired';
          notice.dataset.sessionWarning = '';
          notice.setAttribute('role', 'status');
          notice.setAttribute('aria-live', 'polite');
          notice.setAttribute('aria-atomic', 'true');
          const copy = document.createElement('div');
          copy.className = 'session-expired-copy';
          const title = document.createElement('strong');
          title.textContent = 'Dashboard session ends soon';
          const help = document.createElement('span');
          help.textContent = 'Extend access without reloading; this page, draft, filters, and scroll position stay in place.';
          copy.append(title, help);
          const actions = document.createElement('div');
          actions.className = 'session-expired-actions';
          const extend = document.createElement('button');
          extend.className = 'soft-button';
          extend.type = 'button';
          extend.dataset.sessionExtend = '';
          extend.textContent = 'Extend session';
          const dismiss = document.createElement('button');
          dismiss.className = 'quiet-button';
          dismiss.type = 'button';
          dismiss.dataset.sessionWarningDismiss = '';
          dismiss.textContent = 'Dismiss';
          actions.append(extend, dismiss);
          notice.append(copy, actions);
          body.classList.add('session-expired-visible');
          document.body.appendChild(notice);
        };
        const showExpiredNotice = () => {
          if (safeQuery('[data-session-expired]') || body.classList.contains('has-workbench') || body.dataset.sessionNoticeDismissed === 'true') return;
          const notice = document.createElement('div');
          notice.className = 'session-expired';
          notice.dataset.sessionExpired = '';
          notice.setAttribute('role', 'alert');
          const copy = document.createElement('div');
          copy.className = 'session-expired-copy';
          const title = document.createElement('strong');
          title.textContent = 'Dashboard session expired';
          const help = document.createElement('span');
          help.textContent = 'Reopen the dashboard URL printed by mayhem up, or retry after access was renewed in another tab.';
          copy.append(title, help);
          const actions = document.createElement('div');
          actions.className = 'session-expired-actions';
          const retry = document.createElement('button');
          retry.className = 'soft-button';
          retry.type = 'button';
          retry.dataset.sessionReload = '';
          retry.textContent = 'Retry access';
          const dismiss = document.createElement('button');
          dismiss.className = 'quiet-button';
          dismiss.type = 'button';
          dismiss.dataset.sessionDismiss = '';
          dismiss.textContent = 'Dismiss';
          actions.append(retry, dismiss);
          notice.append(copy, actions);
          body.classList.add('session-expired-visible');
          document.body.appendChild(notice);
        };
        const tick = () => {
          const remaining = Math.max(0, Math.ceil((deadline - Date.now()) / 1000));
          const status = remaining > 60
            ? 'Browser session active'
            : remaining > 0
              ? `Browser session · ${remaining}s remaining`
              : 'Browser session expired';
          if (session.textContent !== status) session.textContent = status;
          if (remaining === 0) {
            safeQuery('[data-session-warning]')?.remove();
            body.classList.remove('session-expired-visible');
            showExpiredNotice();
          } else if (remaining <= 60) {
            showExpiryWarning();
          }
        };
        recordDashboardSessionActivity = (expiresInSeconds = initial) => {
          const expires = Number.parseInt(String(expiresInSeconds), 10);
          if (!Number.isFinite(expires) || expires <= 0) return;
          deadline = Date.now() + expires * 1000;
          session.setAttribute('data-session-seconds', String(expires));
          clearSessionNotices();
          tick();
        };
        refreshDashboardSession = async () => {
          if (body.classList.contains('has-workbench')) {
            recordDashboardSessionActivity(initial);
            return true;
          }
          try {
            const response = await fetch('/mayhem/dashboard/session', {
              credentials: 'same-origin',
              cache: 'no-store',
              headers: { accept: 'application/json' }
            });
            if (!response.ok) return false;
            const payload = await response.json();
            const expires = Number.parseInt(String(payload?.expires_in_seconds ?? ''), 10);
            if (!payload?.ok || !Number.isFinite(expires) || expires <= 0) return false;
            recordDashboardSessionActivity(expires);
            return true;
          } catch (_) {
            return false;
          }
        };
        window.setInterval(tick, 1000);
        tick();
      }
    }

    // Volatile evidence freshness: degrade only source-backed capacity/status
    // claims. Immutable receipt and catalog facts deliberately have no markers.
    const formatEvidenceAge = (ageMillis) => {
      const seconds = Math.max(0, Math.floor(ageMillis / 1000));
      if (seconds < 60) return `${seconds}s ago`;
      const minutes = Math.floor(seconds / 60);
      if (minutes < 60) return `${minutes} min ago`;
      const hours = Math.floor(minutes / 60);
      if (hours < 24) return `${hours}h ago`;
      const days = Math.floor(hours / 24);
      if (days < 14) return `${days}d ago`;
      return `${Math.floor(days / 7)}w ago`;
    };

    const refreshVolatileEvidence = () => {
      const now = Date.now();
      document.querySelectorAll('[data-relative-time][data-observed-at-ms]').forEach((node) => {
        const observedAt = Number.parseInt(node.dataset.observedAtMs || '', 10);
        if (Number.isFinite(observedAt)) node.textContent = formatEvidenceAge(now - observedAt);
      });

      document.querySelectorAll('[data-volatile-expires-at-ms][data-expired-text]').forEach((node) => {
        const expiresAt = Number.parseInt(node.dataset.volatileExpiresAtMs || '', 10);
        if (!Number.isFinite(expiresAt) || now <= expiresAt) return;
        const expiredText = node.dataset.expiredText || 'Unavailable';
        node.textContent = expiredText;
        node.dataset.volatileExpired = 'true';
        const refreshHint = /refresh/i.test(expiredText) ? '' : '. Refresh to reconfirm.';
        node.setAttribute('aria-label', `${expiredText}${refreshHint}`);
        const badge = node.closest('.status-badge');
        if (badge) {
          badge.classList.remove('good', 'info', 'danger');
          badge.classList.add('warn');
        }
      });

      document.querySelectorAll('[data-volatile-step][data-expires-at-ms]').forEach((step) => {
        const expiresAt = Number.parseInt(step.dataset.expiresAtMs || '', 10);
        if (!Number.isFinite(expiresAt) || now <= expiresAt || step.dataset.volatileExpired === 'true') return;
        step.dataset.volatileExpired = 'true';
        step.classList.remove('done');
        step.classList.add('active');
        const mark = safeQuery('[data-check-mark]', step);
        const state = safeQuery('[data-check-state]', step);
        const label = safeQuery('[data-check-label]', step);
        const help = safeQuery('[data-check-help]', step);
        if (mark) mark.textContent = '!';
        if (state) state.textContent = 'Refresh needed: ';
        if (label) label.textContent = 'Refresh provider capacity';
        if (help) help.textContent = 'Capacity evidence expired in this tab; refresh to reconfirm.';
      });

      const pageMarker = safeQuery('[data-page-status-freshness][data-expires-at-ms]');
      const pageExpiresAt = Number.parseInt(pageMarker?.dataset.expiresAtMs || '', 10);
      if (pageMarker && Number.isFinite(pageExpiresAt) && now > pageExpiresAt) {
        pageMarker.dataset.freshnessExpired = 'true';
        const statusText = safeQuery('[data-page-status-text]');
        const indicator = safeQuery('.topbar-status .state-indicator');
        const pageSummary = safeQuery('.page-summary');
        if (statusText) {
          statusText.textContent = 'Refresh to reconfirm';
          statusText.dataset.volatileExpired = 'true';
        }
        if (indicator) {
          indicator.classList.remove('good', 'danger');
          indicator.classList.add('warn');
        }
        if (pageSummary) {
          pageSummary.textContent = pageMarker.dataset.expiredSummary || 'Live evidence expired in this tab. Refresh to reconfirm.';
          pageSummary.dataset.volatileExpired = 'true';
        }
        const playgroundAvailability = safeQuery('[data-preflight-value="availability"]');
        if (playgroundAvailability) {
          playgroundAvailability.textContent = 'Refresh to reconfirm';
          playgroundAvailability.dataset.volatileExpired = 'true';
        }
      }
    };

    if (safeQuery('[data-relative-time], [data-volatile-expires-at-ms], [data-page-status-freshness]')) {
      window.setInterval(refreshVolatileEvidence, 1000);
      refreshVolatileEvidence();
    }

    const savedDraft = readPlaygroundDraft();
    if (savedDraft) {
      const playgroundPrompt = safeQuery('[data-playground-prompt]');
      const playgroundImagePrompt = safeQuery('[data-playground-image-prompt]');
      const playgroundSpeechText = safeQuery('[data-playground-speech-text]');
      const playgroundSystem = safeQuery('[data-playground-system]');
      const playgroundMaxTokens = safeQuery('[data-playground-max-tokens]');
      const playgroundMaxPrice = safeQuery('[data-playground-max-price]');
      const playgroundMinAttTier = safeQuery('[data-playground-min-att-tier]');
      const playgroundModel = safeQuery('[data-playground-model]');
      let urlSelectsModel = false;
      try { urlSelectsModel = new URL(window.location.href).searchParams.has('model'); } catch (_) {}
      if (playgroundModel && !urlSelectsModel && typeof savedDraft.model === 'string'
        && Array.from(playgroundModel.options).some((option) => option.value === savedDraft.model)) {
        playgroundModel.value = savedDraft.model;
      }
      const restoredMode = ['chat', 'image', 'speech'].includes(savedDraft.mode)
        ? savedDraft.mode
        : playgroundModel?.selectedOptions?.[0]?.dataset.playgroundMode;
      if (restoredMode && playgroundOptionsForMode(restoredMode).length) setPlaygroundMode(restoredMode);
      if (playgroundPrompt && typeof savedDraft.prompt === 'string' && savedDraft.prompt && !playgroundPrompt.value) {
        playgroundPrompt.value = savedDraft.prompt;
        const help = safeQuery('#playground-prompt-help');
        if (help) help.textContent = 'Draft and request controls restored from this browser tab.';
      }
      if (playgroundImagePrompt && typeof savedDraft.imagePrompt === 'string') playgroundImagePrompt.value = savedDraft.imagePrompt;
      if (playgroundSpeechText && typeof savedDraft.speechText === 'string') playgroundSpeechText.value = savedDraft.speechText;
      if (['1:1', '4:3', '3:4', '16:9'].includes(savedDraft.aspectRatio)) {
        document.querySelectorAll('[data-playground-aspect-ratio]').forEach((button) => {
          const selected = button.dataset.playgroundAspectRatio === savedDraft.aspectRatio;
          button.classList.toggle('is-active', selected);
          button.setAttribute('aria-pressed', String(selected));
        });
      }
      if (typeof savedDraft.voice === 'string') {
        const voice = Array.from(document.querySelectorAll('[data-playground-voice]')).find((input) => input.value === savedDraft.voice);
        if (voice) {
          voice.checked = true;
          document.querySelectorAll('.pg-voice-option').forEach((option) => {
            option.classList.toggle('is-selected', Boolean(safeQuery('[data-playground-voice]', option)?.checked));
          });
        }
      }
      if (playgroundSystem && typeof savedDraft.system === 'string') playgroundSystem.value = savedDraft.system;
      if (playgroundMaxTokens && /^\d+$/.test(String(savedDraft.maxTokens || ''))) {
        const savedOutput = Number.parseInt(String(savedDraft.maxTokens), 10);
        if (savedOutput >= 64 && savedOutput <= 4096) playgroundMaxTokens.value = String(savedOutput);
      }
      const restoredPriceMode = selectedPlaygroundPriceMode();
      const savedPriceMode = savedDraft.maxPriceMode === 'fixed' ? 'fixed' : 'rate';
      const savedPrice = String(savedDraft.maxPrice || '');
      const validSavedPrice = restoredPriceMode === 'fixed'
        ? /^\d+(?:\.\d{1,18})?$/.test(savedPrice)
        : /^\d+(?:\.\d{1,15})?$/.test(savedPrice);
      if (playgroundMaxPrice && savedPriceMode === restoredPriceMode && validSavedPrice) {
        playgroundMaxPrice.value = savedPrice;
      }
      if (playgroundMinAttTier && ['', '1', '2', '3', '4'].includes(String(savedDraft.minAttTier ?? ''))) {
        playgroundMinAttTier.value = String(savedDraft.minAttTier ?? '');
      }
    }
    restorePlaygroundConversation();
    recordProductEvent('dashboard_page_view', { title: document.title });
    document.querySelectorAll('[data-table-filter]').forEach((input) => {
      enhanceTableTools(input);
      updateFilter(input);
    });
    syncPaginationParameters();
    applyPreferenceButtons();
    updatePreflight();
    syncPlaygroundInputs();
    const activeSubnavItem = safeQuery('.subnav a[aria-current="page"]');
    const activeSubnav = activeSubnavItem ? activeSubnavItem.closest('.subnav') : null;
    if (activeSubnavItem && activeSubnav) {
      window.requestAnimationFrame(() => {
        const centered = activeSubnavItem.offsetLeft
          - ((activeSubnav.clientWidth - activeSubnavItem.offsetWidth) / 2);
        activeSubnav.scrollLeft = Math.max(0, centered);
      });
    }
  };

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', ready, { once: true });
  else ready();
})();
"##;
