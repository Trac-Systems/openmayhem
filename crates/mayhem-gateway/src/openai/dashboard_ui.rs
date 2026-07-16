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
    fn nav_items() -> [(Self, &'static str, &'static str); 8] {
        [
            (Self::Home, "Home", "/mayhem/dashboard"),
            (
                Self::Playground,
                "Playground",
                "/mayhem/dashboard/playground",
            ),
            (Self::Models, "Models", "/mayhem/dashboard/models"),
            (Self::Activity, "Activity", "/mayhem/dashboard/activity"),
            (Self::Wallet, "Wallet", "/mayhem/dashboard/wallet"),
            (Self::Connect, "Connect", "/mayhem/dashboard/connect"),
            (Self::Earn, "Earn", "/mayhem/dashboard/earn"),
            (Self::Network, "Network", "/mayhem/dashboard/network"),
        ]
    }

    fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Playground => "Playground",
            Self::Models => "Models",
            Self::Activity => "Activity",
            Self::Wallet => "Wallet",
            Self::Connect => "Connect",
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
                r#"<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="3"/><path d="M12 3v2m0 14v2M3 12h2m14 0h2M5.6 5.6 7 7m10 10 1.4 1.4M18.4 5.6 17 7M7 17l-1.4 1.4"/></svg>"#
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
    <nav class="app-nav" aria-label="Primary"><span class="app-nav-label">Workspace</span>{navigation}<span class="app-nav-label">System</span><a href="/mayhem/dashboard/help" aria-label="Help"{help_current}><span class="nav-icon">{help_icon}</span><span class="nav-text">Help</span></a><a href="/mayhem/dashboard/settings" aria-label="Settings"{settings_current}><span class="nav-icon">{settings_icon}</span><span class="nav-text">Settings</span></a></nav>
  </aside>
  <button class="nav-scrim js-only" type="button" data-nav-close aria-label="Close navigation"></button>
  <div class="app-frame">
    <header class="app-topbar"><div class="topbar-context"><button class="icon-button mobile-menu-button js-only" type="button" data-nav-toggle aria-label="Open navigation" aria-controls="app-navigation" aria-expanded="false"><span aria-hidden="true">&#9776;</span></button><strong>{page_label}</strong><span class="topbar-status"><span class="state-indicator {status_tone}" aria-hidden="true"></span><span data-page-status-text>{status}</span></span></div><div class="topbar-actions"><button class="icon-button sidebar-collapse-button js-only" type="button" data-sidebar-toggle aria-label="Collapse navigation" aria-controls="app-navigation" aria-expanded="true"><span aria-hidden="true">&#8592;</span></button>{amount_control}</div></header>
    <main class="app-main" id="main-content" tabindex="-1"><header class="page-head"><div><p class="page-eyebrow">{eyebrow}</p><h1>{heading}</h1><p class="page-summary">{summary}</p></div><div class="page-head-actions">{actions}</div></header>{content}</main>
    <footer class="app-footer"><span>{footer}</span><span class="mono" data-session-seconds="{expires}" data-session-status>Browser session active</span></footer>
  </div>
</div>
<nav class="mobile-bottom-nav" aria-label="Mobile primary"><a href="/mayhem/dashboard"{mobile_home}>Home</a><a href="/mayhem/dashboard/models"{mobile_models}>Models</a><a href="/mayhem/dashboard/activity"{mobile_activity}>Activity</a><a href="/mayhem/dashboard/earn"{mobile_earn}>Earn</a><button class="js-only" type="button" data-nav-toggle aria-label="Open all navigation" aria-controls="app-navigation" aria-expanded="false"{mobile_more}>More</button></nav>
<dialog class="verify-dialog" id="dashboard-evidence-dialog" aria-labelledby="dashboard-evidence-title"><header class="verify-head"><div><h2 id="dashboard-evidence-title" data-evidence-title>Evidence</h2><p data-evidence-summary>Loading the requested snapshot&hellip;</p></div><div class="verify-actions"><button class="quiet-button js-only" type="button" data-copy data-copy-target="[data-evidence-raw]" data-evidence-copy disabled><span data-copy-label>Copy raw JSON</span></button><button class="quiet-button js-only" type="button" data-evidence-download disabled>Download evidence</button><button class="icon-button" type="button" data-dialog-close aria-label="Close evidence">&times;</button></div></header><div class="verify-body" data-evidence-body><p class="notice" data-evidence-state role="status">Loading evidence&hellip;</p><section class="verify-level" data-evidence-facts-section hidden><h3>Structured facts</h3><div class="verify-grid" data-evidence-facts></div></section><section class="verify-level" data-evidence-raw-section hidden><h3>Raw gateway snapshot</h3><pre class="raw-evidence" data-evidence-raw></pre></section></div></dialog>"##,
        navigation = navigation,
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
        mobile_models = if shell.page == DashboardAppPage::Models {
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
        mobile_more = if matches!(
            shell.page,
            DashboardAppPage::Playground
                | DashboardAppPage::Wallet
                | DashboardAppPage::Connect
                | DashboardAppPage::Network
                | DashboardAppPage::Help
                | DashboardAppPage::Settings
        ) {
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
  --app-danger:#ff6678;
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
a{color:var(--app-accent-strong)}
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
.app-nav-label{margin:11px 10px 5px;color:var(--app-text-muted);font-size:11px;letter-spacing:.1em;text-transform:uppercase}
.app-nav a{min-height:44px;display:flex;align-items:center;gap:11px;padding:10px 12px;border:1px solid transparent;border-radius:12px;color:var(--app-text-soft);text-decoration:none;font-weight:600}
.app-nav a:hover{background:var(--app-panel);color:var(--app-text)}
.app-nav a[aria-current="page"]{background:linear-gradient(110deg,rgba(255,107,122,.15),rgba(255,107,122,.04));border-color:rgba(255,107,122,.25);color:var(--app-text)}
.nav-icon{width:20px;height:20px;display:grid;place-items:center;color:var(--app-text-muted)}
.nav-icon svg{width:19px;height:19px;fill:none;stroke:currentColor;stroke-width:1.8;stroke-linecap:round;stroke-linejoin:round}
.app-nav a[aria-current="page"] .nav-icon{color:var(--app-accent-strong)}
.state-indicator{width:9px;height:9px;border-radius:999px;background:var(--app-text-muted);box-shadow:0 0 0 4px rgba(126,135,148,.1)}
.state-indicator.good{background:var(--app-good);box-shadow:0 0 0 4px rgba(88,214,168,.1)}
.state-indicator.warn{background:var(--app-warn);box-shadow:0 0 0 4px rgba(245,184,92,.1)}
.state-indicator.danger{background:var(--app-danger);box-shadow:0 0 0 4px rgba(255,102,120,.1)}

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

.app-main{width:100%;padding:clamp(24px,3.7vw,56px) max(clamp(18px,3.1vw,52px),env(safe-area-inset-right)) 72px max(clamp(18px,3.1vw,52px),env(safe-area-inset-left))}
.page-head{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:24px;align-items:end;margin:0 0 clamp(24px,3vw,42px)}
.page-eyebrow{margin:0 0 7px;color:var(--app-accent-strong);font-size:12px;font-weight:800;letter-spacing:.1em;text-transform:uppercase}
.page-head h1{max-width:850px;margin:0;font-size:clamp(32px,4vw,56px);line-height:1.03;letter-spacing:-.045em}
.page-summary{max-width:720px;margin:13px 0 0;color:var(--app-text-soft);font-size:clamp(15px,1.25vw,18px)}
.page-head-actions{display:flex;gap:10px;align-items:center;justify-content:flex-end;flex-wrap:wrap}

.attention-card{margin-bottom:24px;padding:17px 18px;border:1px solid rgba(110,168,255,.28);border-radius:16px;background:linear-gradient(110deg,rgba(110,168,255,.12),rgba(110,168,255,.035));display:grid;grid-template-columns:auto minmax(0,1fr) auto;gap:14px;align-items:center}
.attention-card.warn{border-color:rgba(245,184,92,.35);background:linear-gradient(110deg,rgba(245,184,92,.13),rgba(245,184,92,.035))}
.attention-card.danger{border-color:rgba(255,102,120,.35);background:linear-gradient(110deg,rgba(255,102,120,.13),rgba(255,102,120,.035))}
.attention-icon{width:36px;height:36px;border-radius:11px;display:grid;place-items:center;background:rgba(110,168,255,.13);color:var(--app-info);font-weight:900}
.attention-card.warn .attention-icon{background:rgba(245,184,92,.13);color:var(--app-warn)}
.attention-card.danger .attention-icon{background:rgba(255,102,120,.13);color:var(--app-danger)}
.attention-copy strong{display:block}
.attention-copy p{margin:3px 0 0;color:var(--app-text-soft);font-size:13px}

.metric-grid{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:14px;margin-bottom:24px}
.metric{min-width:0;padding:17px;border:1px solid var(--app-border);border-radius:16px;background:linear-gradient(145deg,var(--app-panel-strong),var(--app-panel));box-shadow:0 14px 40px rgba(0,0,0,.12)}
.metric-top{display:flex;align-items:center;justify-content:space-between;gap:10px}
.metric-label{color:var(--app-text-muted);font-size:12px;font-weight:700}
.metric-state{font-size:11px;color:var(--app-text-muted)}
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

.activity-list{display:grid}
.activity-row{min-width:0;padding:14px 18px;display:grid;grid-template-columns:auto minmax(0,1fr) auto;gap:13px;align-items:center;border-bottom:1px solid var(--app-border)}
.activity-row:last-child{border-bottom:0}
.activity-state{width:34px;height:34px;border-radius:11px;display:grid;place-items:center;background:rgba(88,214,168,.1);color:var(--app-good);font-weight:800}
.activity-state.pending{background:rgba(110,168,255,.1);color:var(--app-info)}
.activity-state.failed{background:rgba(255,102,120,.1);color:var(--app-danger)}
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
.check-copy strong{display:block;font-size:13px}
.check-copy span{display:block;margin-top:2px;color:var(--app-text-muted);font-size:12px}
.check-copy .soft-button{margin-top:9px}

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
.data-table thead th{position:sticky;top:0;background:var(--app-panel);color:var(--app-text-muted);font-size:11px;letter-spacing:.06em;text-transform:uppercase;z-index:1}
.table-sort-button{width:100%;min-height:44px;margin:-6px -8px;padding:6px 8px;border:0;border-radius:8px;background:transparent;color:inherit;text-align:left;text-transform:inherit;letter-spacing:inherit;font-weight:inherit;display:inline-flex;align-items:center;gap:6px}
.table-sort-button:hover{background:rgba(255,255,255,.035);color:var(--app-text-soft)}
.table-sort-button::after{content:"↕";opacity:.42;font-size:10px}
th[aria-sort="ascending"] .table-sort-button::after{content:"↑";opacity:1;color:var(--app-accent-strong)}
th[aria-sort="descending"] .table-sort-button::after{content:"↓";opacity:1;color:var(--app-accent-strong)}
.data-table tbody tr:last-child>*{border-bottom:0}
.data-table tbody tr:hover{background:rgba(255,255,255,.018)}
.table-primary{font-weight:700}
.table-secondary{display:block;margin-top:2px;color:var(--app-text-muted);font-size:12px}
.status-badge{display:inline-flex;align-items:center;gap:6px;min-height:26px;padding:4px 8px;border:1px solid var(--app-border);border-radius:999px;color:var(--app-text-soft);font-size:11px;font-weight:700;white-space:nowrap}
.status-badge.good{border-color:rgba(88,214,168,.32);background:rgba(88,214,168,.08);color:var(--app-good)}
.status-badge.info{border-color:rgba(110,168,255,.32);background:rgba(110,168,255,.08);color:var(--app-info)}
.status-badge.warn{border-color:rgba(245,184,92,.32);background:rgba(245,184,92,.08);color:var(--app-warn)}
.status-badge.danger{border-color:rgba(255,102,120,.32);background:rgba(255,102,120,.08);color:var(--app-danger)}

.search-field{min-height:44px;min-width:min(260px,100%);padding:9px 12px;border:1px solid var(--app-border);border-radius:12px;background:var(--app-panel-soft);color:var(--app-text)}

.subnav{margin:-10px 0 24px;display:flex;gap:6px;overflow-x:auto;scrollbar-width:thin;padding:4px 0}
.subnav a{min-height:44px;padding:10px 11px;border:1px solid transparent;border-radius:10px;color:var(--app-text-muted);font-size:12px;font-weight:700;text-decoration:none;white-space:nowrap;display:inline-flex;align-items:center}
.subnav a:hover{color:var(--app-text);background:var(--app-panel)}
.subnav a[aria-current="page"]{border-color:var(--app-border);background:var(--app-panel-strong);color:var(--app-text)}
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
.notice.danger{border-color:rgba(255,102,120,.3);background:rgba(255,102,120,.07)}
.code-block{position:relative;margin:0;padding:15px 54px 15px 15px;border:1px solid var(--app-border);border-radius:13px;background:#0b0d10;color:#cdd3db;white-space:pre-wrap;overflow-wrap:anywhere;font:12px/1.58 ui-monospace,SFMono-Regular,Menlo,monospace}
.code-block .copy-corner{position:absolute;right:8px;top:8px}
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
.playground-layout{display:grid;grid-template-columns:minmax(0,1.4fr) minmax(270px,.6fr);gap:18px;align-items:start}
html.js-ready .playground-interactive.js-only{display:block!important}
.playground-thread{min-height:280px;max-height:52vh;padding:18px;overflow:auto;display:grid;align-content:start;gap:13px;background:linear-gradient(180deg,rgba(9,10,13,.45),rgba(16,18,23,.5))}
.message{max-width:min(85%,720px);padding:12px 14px;border:1px solid var(--app-border);border-radius:15px;white-space:pre-wrap;overflow-wrap:anywhere}
.message.user{justify-self:end;background:rgba(110,168,255,.1);border-color:rgba(110,168,255,.24)}
.message.assistant{justify-self:start;background:var(--app-panel-strong)}
.message.failed{border-color:rgba(255,102,120,.38);background:rgba(255,102,120,.08);color:var(--app-danger)}
.message.completed{animation:message-complete 420ms cubic-bezier(0,0,.38,.9) both}
.message .message-label{display:block;margin-bottom:5px;color:var(--app-text-muted);font-size:11px;font-weight:800;text-transform:uppercase;letter-spacing:.06em}
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
.message.failed .message-result.incomplete .message-result-mark{background:rgba(255,102,120,.14)}
.message-recovery-impact{margin:8px 0 0;color:var(--app-text-soft)}
.recovery-actions{display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin-top:10px}
@keyframes message-complete{from{border-color:rgba(88,214,168,.55);background:rgba(88,214,168,.1);transform:translateY(4px)}to{border-color:var(--app-border);background:var(--app-panel-strong);transform:none}}
.playground-empty{min-height:230px;display:grid;place-items:center;text-align:center;color:var(--app-text-muted)}
.playground-composer{padding:15px;border-top:1px solid var(--app-border);display:grid;gap:11px}
.playground-meta{padding:13px 18px;border-top:1px solid var(--app-border);display:flex;align-items:center;justify-content:space-between;gap:12px;color:var(--app-text-muted);font-size:12px}
.settings-list{display:grid}
.settings-row{padding:16px 18px;border-bottom:1px solid var(--app-border);display:grid;grid-template-columns:minmax(0,1fr) auto;gap:16px;align-items:center}
.settings-row:last-child{border-bottom:0}
.settings-copy strong{display:block}
.settings-copy span{display:block;margin-top:3px;color:var(--app-text-muted);font-size:12px}
.settings-control[aria-pressed="true"]{border-color:rgba(88,214,168,.4);background:rgba(88,214,168,.09);color:var(--app-good)}
.fact-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px}
.fact{padding:12px;border:1px solid var(--app-border);border-radius:12px;background:var(--app-panel-soft)}
.fact span{display:block;color:var(--app-text-muted);font-size:11px}
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

.app-footer{padding:18px max(clamp(18px,3.1vw,52px),env(safe-area-inset-right)) max(18px,env(safe-area-inset-bottom)) max(clamp(18px,3.1vw,52px),env(safe-area-inset-left));border-top:1px solid var(--app-border);display:flex;align-items:center;justify-content:space-between;gap:12px;color:var(--app-text-muted);font-size:12px}

.verify-dialog{width:min(700px,calc(100vw - 28px));max-height:min(84vh,900px);padding:0;border:1px solid var(--app-border-strong);border-radius:22px;background:var(--app-panel);color:var(--app-text);box-shadow:var(--app-shadow);overflow:hidden}
.verify-dialog::backdrop{background:rgba(4,5,7,.72);backdrop-filter:blur(5px)}
.verify-head{padding:18px 20px;border-bottom:1px solid var(--app-border);display:flex;align-items:flex-start;justify-content:space-between;gap:16px}
.verify-head h2{margin:0;font-size:20px}
.verify-head p{margin:4px 0 0;color:var(--app-text-muted);font-size:12px}
.verify-actions{display:flex;align-items:center;justify-content:flex-end;gap:8px;flex-wrap:wrap}
.verify-body{padding:20px;overflow:auto;max-height:calc(84vh - 145px)}
.verify-level{padding:15px 0;border-bottom:1px solid var(--app-border)}
.verify-level:first-child{padding-top:0}.verify-level:last-child{border-bottom:0;padding-bottom:0}
.verify-level h3{margin:0 0 9px;font-size:14px}
.verify-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px}
.verify-fact{padding:11px;border:1px solid var(--app-border);border-radius:12px;background:var(--app-panel-soft)}
.verify-fact span{display:block;color:var(--app-text-muted);font-size:11px}
.verify-fact strong{display:block;margin-top:4px;overflow-wrap:anywhere;font-size:13px}
.verify-fact small{display:block;margin-top:6px;color:var(--app-text-muted);font-size:10px;line-height:1.35}
.raw-evidence{margin:0;padding:13px;border:1px solid var(--app-border);border-radius:12px;background:#0b0d10;color:#c8d0da;white-space:pre-wrap;overflow-wrap:anywhere;font:12px/1.55 ui-monospace,SFMono-Regular,Menlo,monospace}
.evidence-standalone{width:min(920px,calc(100% - 36px));margin:0 auto;padding:36px 0 72px}
.evidence-page-body{max-height:none}
.evidence-page-body .verify-level h2{margin:0 0 10px;font-size:15px}

.toast-region{position:fixed;right:max(18px,env(safe-area-inset-right));bottom:max(18px,env(safe-area-inset-bottom));z-index:100;display:grid;gap:8px;pointer-events:none}
body.session-expired-visible .toast-region{bottom:max(112px,calc(env(safe-area-inset-bottom) + 96px))}
.app-toast{padding:11px 13px;border:1px solid var(--app-border-strong);border-radius:12px;background:var(--app-panel-strong);box-shadow:var(--app-shadow);color:var(--app-text);font-size:13px;animation:toast-in var(--app-standard) cubic-bezier(.2,0,.38,.9) both}
@keyframes toast-in{from{opacity:0;transform:translateY(6px)}to{opacity:1;transform:none}}

@media(max-width:1120px){
  .app-shell{grid-template-columns:218px minmax(0,1fr)}
  .metric-grid{grid-template-columns:repeat(2,minmax(0,1fr))}
  .dashboard-layout{grid-template-columns:1fr}
}

@media(min-width:781px){
  html.sidebar-collapsed .app-shell{grid-template-columns:84px minmax(0,1fr)}
  html.sidebar-collapsed .app-sidebar{padding-inline:14px;align-items:stretch}
  html.sidebar-collapsed .app-brand{justify-content:center;padding-inline:0}
  html.sidebar-collapsed .app-brand-text,html.sidebar-collapsed .app-nav-label,html.sidebar-collapsed .app-nav .nav-text{display:none}
  html.sidebar-collapsed .app-nav a{justify-content:center;padding-inline:10px}
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
  .topbar-context .topbar-status{display:inline-flex;min-width:0;max-width:min(42vw,240px);gap:6px;font-size:11px;overflow:hidden}
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
  .verify-grid{grid-template-columns:1fr}
  .app-footer{padding-inline:max(14px,env(safe-area-inset-left)) max(14px,env(safe-area-inset-right));align-items:flex-start;flex-direction:column}
  html.js-ready .mobile-bottom-nav{position:fixed;left:max(10px,env(safe-area-inset-left));right:max(10px,env(safe-area-inset-right));bottom:max(10px,env(safe-area-inset-bottom));z-index:18;min-height:60px;padding:6px;border:1px solid var(--app-border);border-radius:18px;background:rgba(18,20,25,.95);box-shadow:0 16px 50px rgba(0,0,0,.38);backdrop-filter:blur(18px);display:grid;grid-template-columns:repeat(5,minmax(0,1fr));gap:3px}
  .session-expired,.toast-region{bottom:calc(max(10px,env(safe-area-inset-bottom)) + 72px)}
  body.session-expired-visible .toast-region{bottom:calc(max(10px,env(safe-area-inset-bottom)) + 206px)}
  .mobile-bottom-nav a,.mobile-bottom-nav button{min-width:0;border:0;border-radius:12px;background:transparent;color:var(--app-text-muted);display:grid;place-items:center;padding:8px 3px;text-decoration:none;font-size:11px;font-weight:700}
  .mobile-bottom-nav a[aria-current="page"],.mobile-bottom-nav button[aria-current="page"]{background:rgba(255,107,122,.11);color:var(--app-accent-strong)}
  html.js-ready .mobile-bottom-nav .js-only{display:grid!important}
  .playground-layout{grid-template-columns:1fr}
}

@media(max-width:520px){
  .topbar-actions .soft-button .button-label{display:none}
  .metric-grid{grid-template-columns:1fr}
  .metric{padding:15px}
  .page-head h1{font-size:34px}
  .panel-head{align-items:flex-start;flex-direction:column}
  .panel-actions{width:100%}
  .search-field{width:100%;min-width:0}
  .activity-row{grid-template-columns:auto minmax(0,1fr)}
  .activity-value{grid-column:2;text-align:left}
  .data-table{min-width:620px}
  .form-grid,.fact-grid{grid-template-columns:1fr}
  .settings-row{grid-template-columns:1fr;align-items:start}
  .session-expired{align-items:stretch;flex-direction:column}
  .session-expired-actions{justify-content:flex-end}
  .verify-head{align-items:stretch;flex-direction:column}
  .verify-actions{justify-content:flex-start}
}

@media(max-width:360px){
  .topbar-context strong{display:none}
  .topbar-context .topbar-status{max-width:46vw}
}

@media(forced-colors:active){
  :focus-visible{outline:3px solid Highlight}
  .app-sidebar,.app-topbar,.panel,.metric,.notice,.attention-card,.message,.mobile-bottom-nav,.verify-dialog{background:Canvas;color:CanvasText;box-shadow:none}
  .state-indicator,.state-indicator.good,.state-indicator.warn,.state-indicator.danger{background:CanvasText;box-shadow:none;border:1px solid Canvas}
  .status-badge,.status-badge.good,.status-badge.info,.status-badge.warn,.status-badge.danger,.check-mark{background:Canvas;color:CanvasText;border-color:CanvasText}
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

    const selectedPlaygroundPriceMode = (scope = document) => {
      const model = safeQuery('[data-playground-model]', scope);
      return model?.selectedOptions?.[0]?.dataset.priceMode === 'fixed' ? 'fixed' : 'rate';
    };

    const playgroundDraftSnapshot = () => ({
      version: 2,
      model: safeQuery('[data-playground-model]')?.value || '',
      prompt: safeQuery('[data-playground-prompt]')?.value || '',
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
          button.setAttribute('aria-pressed', String(value));
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
      actions.className = 'message-actions';
      const button = document.createElement('button');
      button.className = 'quiet-button';
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
      const content = safeQuery('.message-content', message);
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

    const runPlayground = async (form) => {
      const model = safeQuery('[data-playground-model]', form);
      const prompt = safeQuery('[data-playground-prompt]', form);
      const system = safeQuery('[data-playground-system]', form);
      const token = safeQuery('[data-playground-token]', form);
      const thread = safeQuery('[data-playground-thread]');
      const send = safeQuery('[data-playground-send]', form);
      const stop = safeQuery('[data-playground-stop]', form);
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
        const message = document.createElement('div');
        message.className = `message ${role}`;
        message.setAttribute('role', 'article');
        message.setAttribute('aria-label', labelText);
        const label = document.createElement('span');
        label.className = 'message-label';
        label.setAttribute('aria-hidden', 'true');
        label.textContent = labelText;
        const content = document.createElement('span');
        content.className = 'message-content';
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
      thread.append(userMessage, assistantMessage);
      thread.scrollTop = thread.scrollHeight;
      send.disabled = true;
      send.setAttribute('aria-busy', 'true');
      stop.hidden = false;
      if (metadata) metadata.textContent = 'Request in progress';

      const messages = [];
      if (system && system.value.trim()) messages.push({ role: 'system', content: system.value.trim() });
      messages.push(...playgroundConversation.slice(-16));
      messages.push({ role: 'user', content: promptValue });
      const headers = { 'content-type': 'application/json' };
      if (token && token.value.trim()) headers.authorization = `Bearer ${token.value.trim()}`;
      if (requestControls.maxPriceAu) headers['x-mayhem-max-price-au'] = requestControls.maxPriceAu;
      if (requestControls.minAttTier) headers['x-mayhem-min-att-tier'] = requestControls.minAttTier;
      const requestedMaxTokens = requestControls.outputTokens;
      let assembled = '';
      let reportedModel = '';
      let reportedSessionId = '';
      let reportedCharge = '';
      let reportedFinishReason = '';
      let failureStatus = 0;
      const startedAt = Date.now();
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
        if (stoppedNormally) assistantMessage.classList.add('completed');
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
        resultActions.className = 'message-actions';
        const copy = document.createElement('button');
        copy.className = 'quiet-button';
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
          continuation.className = 'soft-button';
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
          receipt.className = 'quiet-button';
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
        prompt.value = '';
        savePlaygroundDraft();
        const promptHelp = safeQuery('#playground-prompt-help');
        if (promptHelp) promptHelp.textContent = 'Ask a question or describe a task.';
        if (metadata) metadata.textContent = `${finishLabel} with ${reportedModel || model.value}. Actual metering appears in Activity when the route supplies a receipt.`;
        announcePlaygroundAnswer(outputLimitReached ? 'Mayhem output limit reached.' : toolRequested ? 'Mayhem requested a tool.' : stoppedNormally ? 'Mayhem response complete.' : `Mayhem response ended: ${reportedFinishReason || 'finish reason unreported'}.`);
        if (stoppedNormally) window.setTimeout(() => assistantMessage.classList.remove('completed'), 520);
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
        assistantMessage.classList.toggle('failed', !aborted);
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
      } finally {
        if (playgroundController === controller) {
          send.disabled = false;
          send.removeAttribute('aria-busy');
          stop.hidden = true;
          playgroundController = null;
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
      const target = safeQuery('[data-playground-preflight]');
      const summary = safeQuery('[data-playground-request-summary]', target || document);
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
      updatePlaygroundPriceMode();
      updatePlaygroundControlSummary();
      applyAmountPreference();
    };

    const openEvidence = async (link, dialog) => {
      const title = safeQuery('[data-evidence-title]', dialog);
      const summary = safeQuery('[data-evidence-summary]', dialog);
      const state = safeQuery('[data-evidence-state]', dialog);
      const factsSection = safeQuery('[data-evidence-facts-section]', dialog);
      const facts = safeQuery('[data-evidence-facts]', dialog);
      const rawSection = safeQuery('[data-evidence-raw-section]', dialog);
      const raw = safeQuery('[data-evidence-raw]', dialog);
      const copyButton = safeQuery('[data-evidence-copy]', dialog);
      const downloadButton = safeQuery('[data-evidence-download]', dialog);
      if (!title || !summary || !state || !factsSection || !facts || !rawSection || !raw) return;

      title.textContent = 'Evidence';
      summary.textContent = 'Loading the requested snapshot…';
      state.hidden = false;
      state.className = 'notice';
      state.setAttribute('role', 'status');
      state.textContent = 'Loading evidence…';
      factsSection.hidden = true;
      rawSection.hidden = true;
      facts.replaceChildren();
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
        summary.textContent = evidenceInterpretation ? `${evidenceSummary} \u2014 ${evidenceInterpretation}` : evidenceSummary;
        const factItems = Array.isArray(payload.facts) ? payload.facts : [];
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
        applyAmountPreference();
        if (copyButton) copyButton.disabled = false;
        if (downloadButton) downloadButton.disabled = false;
      } catch (error) {
        state.hidden = false;
        state.className = 'notice danger';
        state.setAttribute('role', 'alert');
        state.textContent = error instanceof Error ? error.message : 'Evidence could not be loaded.';
        summary.textContent = 'The requested snapshot could not be opened.';
        if (copyButton) copyButton.disabled = true;
        if (downloadButton) downloadButton.disabled = true;
      } finally {
        dialog.removeAttribute('aria-busy');
      }
    };

    document.addEventListener('click', async (event) => {
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
        const system = safeQuery('[data-playground-system]');
        const output = safeQuery('[data-playground-max-tokens]');
        const price = safeQuery('[data-playground-max-price]');
        const tier = safeQuery('[data-playground-min-att-tier]');
        if (model) {
          const defaultValue = model.dataset.defaultValue || model.options[0]?.value || '';
          if (Array.from(model.options).some((option) => option.value === defaultValue)) model.value = defaultValue;
        }
        if (prompt) prompt.value = '';
        if (system) system.value = '';
        if (output) output.value = '512';
        if (price) price.value = '';
        if (tier) tier.value = '';
        taskStorage.remove(playgroundDraftKey);
        const promptHelp = safeQuery('#playground-prompt-help');
        if (promptHelp) promptHelp.textContent = 'Ask a question or describe a task.';
        const metadata = safeQuery('[data-playground-meta]');
        if (metadata) metadata.textContent = 'Saved draft and controls reset';
        updatePreflight();
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
          document.querySelectorAll('.message[data-conversation-offset]').forEach((message) => {
            const messageOffset = Number.parseInt(message.dataset.conversationOffset || '', 10);
            if (Number.isInteger(messageOffset) && messageOffset >= offset) message.remove();
          });
          const thread = safeQuery('[data-playground-thread]');
          if (thread && !safeQuery('.message', thread)) {
            const empty = document.createElement('div');
            empty.className = 'playground-empty';
            empty.dataset.playgroundEmpty = '';
            empty.innerHTML = '<div><strong>Message ready to revise</strong><p>Edit it below, then send when ready.</p></div>';
            thread.append(empty);
          }
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
        const thread = safeQuery('[data-playground-thread]');
        if (thread) {
          const empty = document.createElement('div');
          empty.className = 'playground-empty';
          empty.dataset.playgroundEmpty = '';
          empty.innerHTML = '<div><strong>Start with a real task</strong><p>Conversation context was cleared.</p></div>';
          thread.replaceChildren(empty);
        }
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
      runPlayground(form);
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
      }
    });

    document.addEventListener('keydown', (event) => {
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
      if (minutes < 60) return `${minutes}m ago`;
      const hours = Math.floor(minutes / 60);
      if (hours < 24) return `${hours}h ago`;
      return `${Math.floor(hours / 24)}d ago`;
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
        node.setAttribute('aria-label', `${expiredText}. Evidence expired; refresh to reconfirm.`);
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
      if (playgroundPrompt && typeof savedDraft.prompt === 'string' && savedDraft.prompt && !playgroundPrompt.value) {
        playgroundPrompt.value = savedDraft.prompt;
        const help = safeQuery('#playground-prompt-help');
        if (help) help.textContent = 'Draft and request controls restored from this browser tab.';
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
    document.querySelectorAll('[data-table-filter]').forEach((input) => {
      enhanceTableTools(input);
      updateFilter(input);
    });
    syncPaginationParameters();
    applyPreferenceButtons();
    updatePreflight();
  };

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', ready, { once: true });
  else ready();
})();
"##;
