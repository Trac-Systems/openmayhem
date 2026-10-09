//! Shared canonical and guarded upstream reads, presented as bounded CLI pages.
use super::*;
use mayhem_proxy::setup::guided::{compatible, Browse};
pub(super) async fn model(
    input: &mut impl BufRead,
    output: &mut impl Write,
    home: &Path,
    base: &str,
    network: &NetworkPolicy,
    credential: &Credential,
) -> Result<String> {
    if yes(
        input,
        output,
        "Read this server's /models once before choosing? yes/no (no probe or generation)",
    )? {
        let credential = match credential {
            Credential::None => Credential::None,
            Credential::BearerFile(p) => Credential::BearerFile(p.clone()),
            Credential::BearerValue(_) => {
                anyhow::bail!("CLI preview accepts protected key references")
            }
        };
        let parent = home.to_path_buf();
        let base = base.to_owned();
        let network = network.clone();
        let client = tokio::task::spawn_blocking(move || {
            bootstrap::models_connection(&parent, base, network, credential)
        })
        .await??;
        let preview = mayhem_proxy::setup::preview_models(client).await;
        writeln!(output,"Model listing: {:?}. Names are unverified; this is not a compatibility or speed probe.",preview.state)?;
        for (i, name) in preview.model_ids.iter().enumerate() {
            writeln!(output, "{}: {}", i + 1, name)?;
        }
        if preview.truncated {
            writeln!(output,"This bounded response may omit additional models; an exact manual ID remains available.")?;
        }
        if !preview.model_ids.is_empty() {
            let choice = ask(input, output, "Choose model number, or manual", "")?;
            if choice != "manual" {
                let i = choice
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .context("invalid model number")?;
                return preview
                    .model_ids
                    .get(i)
                    .cloned()
                    .context("model number is outside this page");
            }
        }
    }
    ask(input, output, "Exact upstream model ID", "")
}
pub(super) async fn market(
    input: &mut impl BufRead,
    output: &mut impl Write,
    canonical: &Canonical,
    endpoint: ProxyEndpoint,
    upstream: &str,
) -> Result<ProfileMarket> {
    let mut cursor = None;
    let family = loop {
        let page = canonical.browse(Browse::Families { cursor }).await?;
        let enabled = page
            .entries
            .iter()
            .filter(|e| e.value["enabled"] == true)
            .collect::<Vec<_>>();
        for (i, e) in enabled.iter().enumerate() {
            writeln!(
                output,
                "{}: {} ({})",
                i + 1,
                e.value["label"].as_str().unwrap(),
                e.key.rsplit('/').next().unwrap()
            )?;
        }
        if page.next_cursor.is_some() {
            writeln!(output, "More canonical families available: choose next.")?;
        }
        let choice = ask(
            input,
            output,
            "Choose family number on this page, or next",
            "",
        )?;
        if choice == "next" {
            cursor = Some(page.next_cursor.context("end of canonical families")?);
            continue;
        }
        let i = choice
            .parse::<usize>()
            .ok()
            .and_then(|i| i.checked_sub(1))
            .context("invalid family number")?;
        break enabled
            .get(i)
            .context("family number is outside this page")?
            .key
            .rsplit('/')
            .next()
            .unwrap()
            .to_owned();
    };
    match ask(
        input,
        output,
        "Create a new market or join an existing market? create/join",
        "",
    )?
    .as_str()
    {
        "create" => Ok(ProfileMarket::CreateMarket {
            slug: ask(input, output, "New market slug", "")?,
            model: ProxyModelClaim {
                family_id: family,
                model_id: ask(input, output, "Declared public model label", upstream)?,
                revision: String::new(),
                quantization: String::new(),
            },
        }),
        "join" => {
            let mut cursor = None;
            loop {
                let page = canonical
                    .browse(Browse::Markets {
                        family_id: family.clone(),
                        endpoint,
                        cursor,
                    })
                    .await?;
                let mut choices = Vec::new();
                for e in page.entries {
                    let market: ProxyMarketDescriptor = serde_json::from_value(e.value)?;
                    if compatible(&market, endpoint)? {
                        choices.push(market);
                    }
                }
                for (i, m) in choices.iter().enumerate() {
                    writeln!(
                        output,
                        "{}: {} / {} ({})",
                        i + 1,
                        m.model.model_id,
                        m.slug,
                        m.id().map_err(anyhow::Error::msg)?
                    )?;
                }
                if page.next_cursor.is_some() {
                    writeln!(
                        output,
                        "More pages exist; later compatible matches may remain."
                    )?;
                }
                let choice = ask(
                    input,
                    output,
                    "Choose compatible market number on this page, or next",
                    "",
                )?;
                if choice == "next" {
                    cursor = Some(
                        page.next_cursor
                            .context("end of this market scope; rerun to create your own market")?,
                    );
                    continue;
                }
                let i = choice
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .context("invalid market number")?;
                return Ok(ProfileMarket::JoinMarket {
                    market: choices
                        .get(i)
                        .cloned()
                        .context("market number is outside this page")?,
                });
            }
        }
        _ => anyhow::bail!("choose create or join explicitly"),
    }
}
