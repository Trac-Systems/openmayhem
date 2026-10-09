//! Explicit local reads. Same session/Host/Origin/CSRF and bounded work as create.
use super::*;
use mayhem_proxy::setup::guided::{self, Browse, Canonical};
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Read {
    Catalog {
        browse: Browse,
    },
    Sequence,
    Models {
        base_url: String,
        network_policy: NetworkPolicy,
        credential: CredentialInput,
    },
    Amounts {
        rates: Vec<HumanRate>,
        per_request_usd: String,
        min_session_usd: String,
        probe_total_usd: String,
        probe_per_attempt_usd: String,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HumanRate {
    unit: String,
    granularity: u64,
    usd: String,
}
fn amounts(
    rates: Vec<HumanRate>,
    request: String,
    minimum: String,
    total: String,
    attempt: String,
) -> Result<Value, ()> {
    if rates.is_empty() || rates.len() > 2 || rates.windows(2).any(|v| v[0].unit >= v[1].unit) {
        return Err(());
    }
    let rates=rates.into_iter().map(|r| {
        if !["input_token","output_token","decision"].contains(&r.unit.as_str()) || r.granularity==0 || r.granularity>9_007_199_254_740_991 {return Err(());}
        Ok(json!({"unit":r.unit,"granularity":r.granularity,"per_unit_au":guided::usd_to_au(&r.usd).map_err(|_|())?.to_string()}))
    }).collect::<Result<Vec<_>,_>>()?;
    Ok(
        json!({"rates":rates,"per_request_au":guided::usd_to_au(&request).map_err(|_|())?.to_string(),
        "min_session_au":guided::usd_to_au(&minimum).map_err(|_|())?.to_string(),
        "max_cost_microusd":guided::usd_to_microusd(&total).map_err(|_|())?,
        "per_attempt_cost_microusd":guided::usd_to_microusd(&attempt).map_err(|_|())?,
        "usd_per_au":"0.000000000000000001","rounding":"none","exchange_rate_applied":false}),
    )
}
pub(crate) async fn read(State(state): State<SharedState>, request: Request) -> Response {
    let control = match mutation_access(&state, &request) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(config) = control.bootstrap.clone() else {
        return failure(StatusCode::CONFLICT, "setup_original_exists");
    };
    if control.flow.get().is_some() {
        return failure(StatusCode::CONFLICT, "setup_original_exists");
    }
    let Ok(permit) = control.create_gate.clone().try_acquire_owned() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "setup_busy");
    };
    let bytes = match body(request).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let input = match serde_json::from_slice::<Read>(&bytes) {
        Ok(v) => v,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "setup_invalid_guidance"),
    };
    let value = match input {
        Read::Amounts {
            rates,
            per_request_usd,
            min_session_usd,
            probe_total_usd,
            probe_per_attempt_usd,
        } => {
            match amounts(
                rates,
                per_request_usd,
                min_session_usd,
                probe_total_usd,
                probe_per_attempt_usd,
            ) {
                Ok(v) => v,
                Err(_) => return failure(StatusCode::BAD_REQUEST, "setup_invalid_exact_amount"),
            }
        }
        Read::Models {
            base_url,
            network_policy,
            credential,
        } => {
            let credential = match config.credential(credential) {
                Ok(v) => v,
                Err(e) => return failure(StatusCode::BAD_REQUEST, e),
            };
            // Hold the permit inside blocking key/client construction, including
            // when a disconnected HTTP future no longer awaits its result.
            let initialized = tokio::task::spawn_blocking(move || {
                let result = bootstrap::models_connection(
                    config.destination.parent().unwrap(),
                    base_url,
                    network_policy,
                    credential,
                );
                (permit, result)
            })
            .await;
            let (_permit, connection) = match initialized {
                Ok((p, Ok(c))) => (p, c),
                _ => {
                    return failure(
                        StatusCode::BAD_REQUEST,
                        "setup_preview_configuration_rejected",
                    )
                }
            };
            return dashboard_json_response(
                StatusCode::OK,
                json!({"schema_version":1,"preview":mayhem_proxy::setup::preview_models(connection).await,"authorizes_probe":false}),
                None,
            );
        }
        other => {
            let canonical = match Canonical::new(&config.host) {
                Ok(v) => v,
                Err(_) => return failure(StatusCode::CONFLICT, "setup_canonical_unavailable"),
            };
            match other {
                Read::Catalog { browse } => {
                    let endpoint = match &browse {
                        Browse::Markets { endpoint, .. } => Some(*endpoint),
                        _ => None,
                    };
                    match canonical.browse(browse).await {
                        Ok(page) => {
                            let compatible = page
                                .entries
                                .iter()
                                .filter_map(|e| {
                                    let endpoint = endpoint?;
                                    let market =
                                        serde_json::from_value::<
                                            mayhem_proto::proxy::ProxyMarketDescriptor,
                                        >(e.value.clone())
                                        .ok()?;
                                    guided::compatible(&market, endpoint)
                                        .ok()?
                                        .then(|| e.key.rsplit('/').next().unwrap().to_owned())
                                })
                                .collect::<Vec<_>>();
                            json!({"page":page,"compatible_market_ids":compatible})
                        }
                        Err(_) => {
                            return failure(
                                StatusCode::SERVICE_UNAVAILABLE,
                                "setup_canonical_unavailable",
                            )
                        }
                    }
                }
                Read::Sequence => match canonical.next_sequence().await {
                    Ok(sequence) => json!({"sequence":sequence}),
                    Err(_) => {
                        return failure(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "setup_canonical_unavailable",
                        )
                    }
                },
                _ => unreachable!(),
            }
        }
    };
    drop(permit);
    dashboard_json_response(
        StatusCode::OK,
        json!({"schema_version":1,"result":value,"authorizes_probe":false,"authorizes_publication":false}),
        None,
    )
}
