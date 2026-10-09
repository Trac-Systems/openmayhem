use super::*;
use crate::managed_provider::{config, private, save};
use mayhem_proxy::managed::{Health, Prepared};
use tokio::sync::watch;

#[tokio::test]
async fn managed_provider_startup_negotiates_executes_and_closes_paid_decisions_on_each_rail() {
    for rail in [ProxyRail::Fiat, ProxyRail::Tnk, ProxyRail::Tap] {
        let (endpoint, bytes, response) = cases()
            .into_iter()
            .find(|(e, _, _)| *e == ProxyEndpoint::Decisions)
            .unwrap();
        let backend = backend(200, response, Duration::ZERO).await;
        let f = Fixture::new(&backend.base, endpoint);
        let mut peer = Peer::start(rail, &f, &bytes, false, None).await;
        let s = Controlled::new(&f, &mut peer, limits(), 128 * 1024 * 1024).await;
        let context = context(&peer);
        let bridge = Bridge::start(context.buyer.as_str(), &context.offer.provider_pubkey).await;
        let root = dir();
        let mut value = config(
            root.path(),
            &backend.base,
            bridge.config(false).url.as_str(),
            endpoint,
        );
        value["provider_pubkey"] = json!(peer.identity.controller_pubkey);
        value["network"] = json!({"network_id":peer.identity.network_id,"msb_bootstrap":peer.identity.msb_bootstrap,"subnet_bootstrap":peer.identity.subnet_bootstrap,"contract_version":mayhem_proto::CONTRACT_VERSION});
        value["peer_rpc_url"] = json!(peer.rpc_url);
        value["connections"][0]["group"] = json!(d(200));
        value["routes"][0]["connection"] = json!(d(200));
        value["routes"][0]["adapter"] = json!(f.adapter.snapshot());
        value["routes"][0]["offers"] = json!([context.offer]);
        value["routes"][0]["settlement_policy"] = json!(s.runtime.approved_policy);
        value["routes"][0]["recovery"]["request"] = serde_json::from_slice(&bytes).unwrap();
        // Exact fixture connection identity: recovery and paid negotiation must
        // bind the same configuration, rather than silently bypassing its hash.
        private(&root.path().join("connection.json"), &serde_json::to_vec(&json!({"schema_version":1,"id":"fixture","revision":1,"base_url":backend.base,
            "network":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},"paths":{"chat_completions":"chat/completions","completions":"completions","responses":"responses","decisions":"decisions"},"error_profile":"open_ai"})).unwrap());
        let prepared = Prepared::load(&save(root.path(), &value)).unwrap();
        let provider = prepared.open(s.signing.clone()).unwrap();
        assert_eq!(provider.route_status(&d(20)).unwrap().1.available, 0);
        let (stop, rx) = watch::channel(false);
        let (updates, mut health) = watch::channel(Health::default());
        let task = tokio::spawn(provider.run(rx, updates));
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if health
                    .borrow()
                    .routes
                    .get(&d(20))
                    .is_some_and(|v| v.allowance > 0)
                {
                    break;
                }
                health.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "one operator probe"
        );
        assert_eq!(
            peer.command("status").await["publications"],
            0,
            "probes must not reserve buyer money or generate ledger demand"
        );
        let buyer = n::Channel::dial(
            bridge.config(true),
            context.clone(),
            &identity(&peer),
            exchange::Limits {
                max_message_bytes: 1024 * 1024,
            },
        )
        .await
        .unwrap();
        let (mut buyer, saved) = purchase(&s, &peer, &bytes, buyer, &context).await;
        s.buyer
            .publish(saved.key().clone(), &recovery(&f, &peer), 1002)
            .await
            .unwrap();
        buyer
            .send(&exchange::Message::Execute {
                request: serde_json::from_slice(&bytes).unwrap(),
                streaming: false,
            })
            .await
            .unwrap();
        let result = next(&mut buyer).await;
        settle(&s, &f, &peer, &mut buyer, &bytes, result).await;
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            2,
            "one probe and exactly one funded generation"
        );
        assert_eq!(
            peer.command("status").await["publications"],
            2,
            "one reservation and one closure"
        );
        stop.send_replace(true);
        task.await.unwrap().unwrap();
        assert_eq!(health.borrow().serving.accepted, 1);
        assert_eq!(health.borrow().serving.failed, 0);
        peer.stop().await;
    }
}
