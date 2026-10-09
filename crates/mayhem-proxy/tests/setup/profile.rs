use super::*;
use mayhem_proxy::setup::{
    profiles, EndpointProfile, MembershipInput, OfferInput, ProfileInput, ProfileMarket,
};

fn profile(f: &Fixture, custom: bool) -> ProfileInput {
    let input = &f.input;
    ProfileInput {
        schema_version: 1,
        network: input.network.clone(),
        provider_pubkey: input.provider_pubkey.clone(),
        connection_file: input.connection_file.clone(),
        profile: if custom {
            EndpointProfile::Custom {
                endpoint: input.adapter.endpoint,
                contract: input.adapter.contract.clone(),
            }
        } else {
            EndpointProfile::Standard {
                endpoint: input.adapter.endpoint,
            }
        },
        upstream_model: input.adapter.upstream_model.clone(),
        limits: input.adapter.limits,
        market: ProfileMarket::CreateMarket {
            slug: input.market.slug.clone(),
            model: input.market.model.clone(),
        },
        membership: MembershipInput {
            revision: input.membership.revision,
            served_context: input.membership.served_context,
            max_concurrency: input.membership.max_concurrency,
            capacity_group: input.membership.capacity_group.clone(),
            accepted_rails: input.membership.accepted_rails.clone(),
        },
        offers: input
            .offers
            .iter()
            .map(|o| OfferInput {
                revision: o.revision,
                ctx_bracket: o.ctx_bracket.clone(),
                outcome_class: o.outcome_class.clone(),
                rates: o.rates.clone(),
                per_request_au: o.per_request_au,
                min_session_au: o.min_session_au,
                accepted_rails: o.accepted_rails.clone(),
            })
            .collect(),
        sequence: input.sequence,
        settlement_policy: input.settlement_policy.clone(),
    }
}

#[test]
fn standard_and_custom_profiles_derive_exact_existing_bindings_offline_for_four_families() {
    let templates = profiles().unwrap();
    assert_eq!(templates.len(), 4);
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let f = Fixture::new(endpoint);
        let custom = profile(&f, true).prepare().unwrap();
        assert!(
            serde_json::to_value(&custom).unwrap() == serde_json::to_value(&f.input).unwrap(),
            "profile must derive the original exact declaration"
        );
        let built = profile(&f, false).prepare().unwrap();
        let template = templates.iter().find(|t| t.endpoint == endpoint).unwrap();
        assert_eq!(json!(built.adapter.contract), json!(template.contract));
        built.validate().unwrap();
        let created = f.store().prepare(profile(&f, false), None).unwrap();
        assert_eq!(created.state, State::Unchecked);
        assert_eq!(created.claim_status, "operator_declared");
        assert_eq!(created.probe_status, "not_run");
        assert_eq!(created.admission_status, "not_checked");
        assert_eq!(created.publication_status, "not_submitted");
        assert_eq!(created.serving_status, "not_started");
        assert_eq!(f.store().inspect().unwrap().draft_id, created.draft_id);
        let review = serde_json::to_string(&created).unwrap();
        for hidden in [
            "private-upstream-model",
            "never-read-secret",
            "127.0.0.1",
            "connection_file",
        ] {
            assert!(!review.contains(hidden));
        }
        assert!(!f.store.join("discovery.json").exists());
        f.no_network_or_secret();
    }
}

#[test]
fn prepare_load_cas_rates_and_join_preserve_identity_and_reject_mismatched_contracts() {
    let f = Fixture::new(ProxyEndpoint::Chat);
    let path = f.dir.path().join("profile.json");
    let mut value = serde_json::to_value(profile(&f, true)).unwrap();
    value["connection_file"] = json!("private-connection.json");
    private(&path, &serde_json::to_vec(&value).unwrap());
    let loaded = ProfileInput::load(&path).unwrap();
    let created = f.store().prepare(loaded, None).unwrap();
    let checked = f.store().check(created.revision).unwrap();
    let mut revised = profile(&f, true);
    revised.offers[0].revision = 2;
    revised.offers[0].rates[0].per_unit_au += 1;
    assert!(matches!(
        f.store().prepare(revised.clone(), None),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        f.store().prepare(revised.clone(), Some(created.revision)),
        Err(Error::Conflict)
    ));
    let updated = f.store().prepare(revised, Some(checked.revision)).unwrap();
    assert_eq!(updated.draft_id, created.draft_id);
    assert_eq!(updated.market, created.market);
    assert_eq!(updated.state, State::Unchecked);
    assert!(updated.admission_handoff.is_none());
    assert_eq!(created.offers[0].revision, 1);
    assert_eq!(updated.offers[0].revision, 2);
    let mut joined = profile(&f, true);
    joined.market = ProfileMarket::JoinMarket {
        market: f.input.market.clone(),
    };
    assert!(matches!(
        joined.clone().prepare().unwrap().selection,
        Selection::JoinMarket
    ));
    joined.profile = EndpointProfile::Standard {
        endpoint: ProxyEndpoint::Chat,
    };
    assert!(
        joined.prepare().is_err(),
        "join cannot substitute the standard contract for the exact custom market"
    );
    let mut invalid = value.clone();
    invalid["profile"]["upstream_secret"] = json!("rejected");
    assert!(serde_json::from_value::<ProfileInput>(invalid).is_err());
    let mut invalid = profile(&f, true);
    invalid.membership.accepted_rails = vec![ProxyRail::Fiat];
    assert!(
        invalid.prepare().is_err(),
        "profile cannot expand offers beyond declared rails"
    );
    f.no_network_or_secret();
}
