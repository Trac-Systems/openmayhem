use mayhem_proxy::registry::publication::{taxonomy::*, Limits, Reader, TrustedOrigin};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/taxonomy-membership-v1.json")).unwrap()
}
fn selection_fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/taxonomy-selection-v1.json")).unwrap()
}
fn hash(domain: &str, v: &Value) -> String {
    let mut bytes = domain.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend(mayhem_proto::stable_json_bytes(v).unwrap());
    blake3::hash(&bytes).to_hex().to_string()
}
fn reference(f: &Value, which: &str) -> Reference {
    serde_json::from_value(json!({"release_id":f[which]["release"]["release_id"],"release_hash":f[which]["release"]["release_hash"],"entry_id":f[which]["response"]["category"]["entry_id"],"schema_revision":1})).unwrap()
}
struct Server {
    origin: String,
    mutate: Arc<Mutex<Option<fn(&mut Value)>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort()
    }
}
impl Server {
    async fn start() -> Self {
        Self::start_fixture(false).await
    }
    async fn start_fixture(selection: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mutate = Arc::new(Mutex::new(None::<fn(&mut Value)>));
        let m = mutate.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let m = m.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut b = [0u8; 4096];
                    let (header, length) = loop {
                        let n = s.read(&mut b).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        buf.extend(&b[..n]);
                        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let h = String::from_utf8(buf[..end].to_vec()).unwrap();
                            let len = h
                                .lines()
                                .find_map(|l| {
                                    l.to_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|s| s.trim().parse::<usize>().unwrap())
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + len {
                                break (h, (end + 4, len));
                            }
                        }
                    };
                    let path = header
                        .lines()
                        .next()
                        .unwrap()
                        .split_whitespace()
                        .nth(1)
                        .unwrap();
                    let url = url::Url::parse(&format!("http://127.0.0.1{path}")).unwrap();
                    let f = if selection {
                        selection_fixture()
                    } else {
                        fixture()
                    };
                    let which = if path
                        .contains(f["original"]["release"]["release_id"].as_str().unwrap())
                    {
                        "original"
                    } else {
                        "updated"
                    };
                    let release = &f[which]["release"];
                    let (mut out, kind, selector) = if url.path().ends_with("/members") {
                        let q: std::collections::BTreeMap<_, _> =
                            url.query_pairs().into_owned().collect();
                        (
                            f[which]["response"].clone(),
                            "members",
                            json!({"entry_id":q["entry_id"],"schema_revision":q["schema_revision"].parse::<u32>().unwrap(),"limit":q["limit"].parse::<usize>().unwrap(),"cursor":q.get("cursor")}),
                        )
                    } else if url.path().ends_with("/selection") {
                        let body: Value =
                            serde_json::from_slice(&buf[length.0..length.0 + length.1]).unwrap();
                        let selector = json!({"variants":body["variants"],"tags":body["tags"],"models":body["models"].as_array().unwrap().iter().map(|m|hash("mayhem/proxy/taxonomy-match-model/v1",m)).collect::<Vec<_>>()});
                        (
                            f[format!("{which}_selection")].clone(),
                            "selection",
                            selector,
                        )
                    } else if url.path().ends_with("/match") {
                        let body: Value =
                            serde_json::from_slice(&buf[length.0..length.0 + length.1]).unwrap();
                        let selector = json!({"entry_id":body["entry_id"],"schema_revision":body["schema_revision"],"models":body["models"].as_array().unwrap().iter().map(|m|hash("mayhem/proxy/taxonomy-match-model/v1",m)).collect::<Vec<_>>()});
                        (f[format!("{which}_match")].clone(), "match", selector)
                    } else {
                        (release.clone(), "release", Value::Null)
                    };
                    let etag = hash(
                        "mayhem/proxy/taxonomy-representation/v1",
                        &json!({"release_id":release["release_id"],"release_hash":release["release_hash"],"kind":kind,"selector":selector}),
                    );
                    if let Some(change) = *m.lock().unwrap() {
                        change(&mut out)
                    }
                    let bytes = serde_json::to_vec(&out).unwrap();
                    s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"{etag}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len()).as_bytes()).await.unwrap();
                    s.write_all(&bytes).await.unwrap();
                });
            }
        });
        Self {
            origin,
            mutate,
            task,
        }
    }
    fn reader(&self) -> Reader {
        Reader::new(
            TrustedOrigin::local_loopback_http(&self.origin).unwrap(),
            Limits::default(),
        )
        .unwrap()
    }
}

fn filters(f: &Value, which: &str) -> Filters {
    serde_json::from_value(json!({"release_id":f[which]["release"]["release_id"],"release_hash":f[which]["release"]["release_hash"],
        "variants":f["request"]["variants"],"tags":f["request"]["tags"]})).unwrap()
}
fn selection_reference(filters: &Filters) -> Reference {
    Reference {
        release_id: filters.release_id.clone(),
        release_hash: filters.release_hash.clone(),
        entry_id: filters.tags[0].entry_id.clone(),
        schema_revision: filters.tags[0].schema_revision,
    }
}
#[tokio::test]
async fn actual_site_selection_preserves_exact_filters_and_old_release() {
    let f = selection_fixture();
    let server = Server::start_fixture(true).await;
    let reader = server.reader();
    let models: Vec<Model> = serde_json::from_value(f["request"]["models"].clone()).unwrap();
    for which in ["original", "updated", "original"] {
        let filters = filters(&f, which);
        let pin = reader
            .pin_taxonomy(&selection_reference(&filters))
            .await
            .unwrap();
        let selection = reader
            .taxonomy_selection(&pin, &filters, &models)
            .await
            .unwrap();
        for (index, model) in models.iter().enumerate() {
            assert_eq!(
                selection.allows(&filters, model),
                f[format!("{which}_selection")]["matches"][index]["matches"]
                    .as_bool()
                    .unwrap()
            );
        }
        let mut changed = filters.clone();
        changed.tags.clear();
        assert!(!selection.allows(&changed, &models[0]));
        let mut changed = models[0].clone();
        changed.quantization.push('x');
        assert!(!selection.allows(&filters, &changed));
    }
}
#[tokio::test]
async fn selection_rejects_substitutions_and_malformed_or_partial_evidence() {
    let f = selection_fixture();
    let server = Server::start_fixture(true).await;
    let reader = server.reader();
    let filters = filters(&f, "original");
    let pin = reader
        .pin_taxonomy(&selection_reference(&filters))
        .await
        .unwrap();
    let models: Vec<Model> = serde_json::from_value(f["request"]["models"].clone()).unwrap();
    for mutation in [
        (|v: &mut Value| {
            v["release"]["release_hash"] = json!("f".repeat(64));
        }) as fn(&mut Value),
        |v| {
            v["authorizes_execution"] = json!(true);
        },
        |v| {
            v["capacity_reserved"] = json!(true);
        },
        |v| {
            v["matches"].as_array_mut().unwrap().reverse();
        },
        |v| {
            v["matches"][0]["matches"] = json!(false);
        },
        |v| {
            v["matches"][0]["variant"] = Value::Null;
        },
        |v| {
            v["matches"][0]["variant"]["entry_id"] = json!("different");
        },
        |v| {
            v["tags"][0]["schema_revision"] = json!(2);
        },
        |v| {
            v["matches"][0]["tags"][0]["scope"] = Value::Null;
        },
        |v| {
            v["matches"][0]["tags"][0]
                .as_object_mut()
                .unwrap()
                .remove("source");
        },
        |v| {
            v["matches"][0]["tags"].as_array_mut().unwrap().pop();
        },
        |v| {
            v["unknown"] = json!(true);
        },
    ] {
        *server.mutate.lock().unwrap() = Some(mutation);
        assert!(reader
            .taxonomy_selection(&pin, &filters, &models)
            .await
            .is_err());
    }
    *server.mutate.lock().unwrap() = None;
    let mut duplicate = models.clone();
    duplicate.push(models[0].clone());
    assert!(reader
        .taxonomy_selection(&pin, &filters, &duplicate)
        .await
        .is_err());
    let other = Server::start_fixture(true).await;
    assert!(other
        .reader()
        .taxonomy_selection(&pin, &filters, &models)
        .await
        .is_err());
    let mut empty = filters.clone();
    empty.tags.clear();
    empty.variants.clear();
    assert!(empty.validate().is_err());
    let mut wrong = filters.clone();
    wrong.release_hash = "f".repeat(64);
    assert!(reader
        .taxonomy_selection(&pin, &wrong, &models)
        .await
        .is_err());
}
#[tokio::test]
async fn real_site_release_members_and_exact_lookup_preserve_old_pins() {
    let f = fixture();
    let server = Server::start().await;
    let reader = server.reader();
    let models: Vec<Model> = serde_json::from_value(f["match_request"]["models"].clone()).unwrap();
    for which in ["original", "updated", "original"] {
        let r = reference(&f, which);
        let pin = reader.pin_taxonomy(&r).await.unwrap();
        let p = reader.taxonomy_members(&pin, &r, None, 100).await.unwrap();
        assert_eq!(serde_json::to_value(p).unwrap(), f[which]["response"]);
        let matches = reader.taxonomy_match(&pin, &r, &models).await.unwrap();
        assert_eq!(matches.len(), models.len());
        for (i, m) in matches.iter().enumerate() {
            assert_eq!(
                m.contains(&r, &models[i]),
                !f[format!("{which}_match")]["matches"][i]["scope"].is_null()
            );
            let mut other = r.clone();
            other.schema_revision += 1;
            assert!(!m.contains(&other, &models[i]));
        }
    }
    let mut wrong = reference(&f, "original");
    wrong.release_hash = "f".repeat(64);
    assert!(reader.pin_taxonomy(&wrong).await.is_err());
}
#[tokio::test]
async fn malformed_or_substituted_admin_metadata_never_mints_membership() {
    let f = fixture();
    let server = Server::start().await;
    let reader = server.reader();
    let r = reference(&f, "original");
    let pin = reader.pin_taxonomy(&r).await.unwrap();
    let models: Vec<Model> = serde_json::from_value(f["match_request"]["models"].clone()).unwrap();
    for mutation in [
        (|v: &mut Value| {
            v["release"]["release_hash"] = json!("f".repeat(64));
        }) as fn(&mut Value),
        |v| {
            v["category"]["schema_revision"] = json!(2);
        },
        |v| {
            v["authorizes_execution"] = json!(true);
        },
        |v| {
            v["unexpected"] = json!(true);
        },
        |v| {
            v["matches"].as_array_mut().unwrap().reverse();
        },
        |v| {
            v["matches"][0]["scope"] = Value::Null;
        },
        |v| {
            v["matches"][0].as_object_mut().unwrap().remove("source");
        },
    ] {
        *server.mutate.lock().unwrap() = Some(mutation);
        assert!(reader.taxonomy_match(&pin, &r, &models).await.is_err());
    }
    for mutation in [
        (|v: &mut Value| {
            v["scanned_entries"] = json!(257);
        }) as fn(&mut Value),
        |v| {
            v["next_cursor"] = json!("more");
        },
        |v| {
            v["capacity_reserved"] = json!(true);
        },
        |v| {
            v.as_object_mut().unwrap().remove("next_cursor");
        },
        |v| {
            let duplicate = v["scopes"][0].clone();
            v["scopes"].as_array_mut().unwrap().push(duplicate);
        },
    ] {
        *server.mutate.lock().unwrap() = Some(mutation);
        assert!(reader.taxonomy_members(&pin, &r, None, 100).await.is_err());
    }
    *server.mutate.lock().unwrap() = None;
    let mut duplicate = models.clone();
    duplicate.push(models[0].clone());
    assert!(reader.taxonomy_match(&pin, &r, &duplicate).await.is_err());
    let other = Server::start().await;
    assert!(other
        .reader()
        .taxonomy_members(&pin, &r, None, 100)
        .await
        .is_err());
}
#[test]
fn strict_release_hash_network_and_required_nulls() {
    let f = fixture();
    let v = f["original"]["release"].clone();
    let release: Release = serde_json::from_value(v.clone()).unwrap();
    release.validate().unwrap();
    for mutation in [
        (|v: &mut Value| {
            v["manifest"]["changes"][0]["document_hash"] = json!("f".repeat(64));
        }) as fn(&mut Value),
        |v| {
            v["canonical_observation"]["network"]["network_id"] = json!("other");
        },
        |v| {
            v["manifest"].as_object_mut().unwrap().remove("network");
        },
        |v| {
            v.as_object_mut().unwrap().remove("canonical_observation");
        },
        |v| {
            v["publication_state"] = json!("draft");
        },
        |v| {
            v["revision"] = json!("08");
        },
    ] {
        let mut changed = v.clone();
        mutation(&mut changed);
        assert!(serde_json::from_value::<Release>(changed).map_or(true, |r| r.validate().is_err()));
    }
}
