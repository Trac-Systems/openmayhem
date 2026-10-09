use super::*;
use crate::db;
use redb::{
    Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
};
use std::{path::Path, sync::Mutex, time::Instant};
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("conformance_identity_v1");
const RECORDS: TableDefinition<&str, &[u8]> = TableDefinition::new("conformance_records_v1");
const EXPIRY: TableDefinition<&str, &str> = TableDefinition::new("conformance_expiry_v1");

pub enum Lookup {
    Missing,
    Expired,
    Restarted,
    ConfigurationChanged,
    Present(Box<Signed>),
}
pub struct Store {
    database: Database,
    pub(super) network: discovery::Identity,
    pub(super) config: Config,
    pub(super) configuration: Digest,
    pub(super) boot: Digest,
    pub(super) opened: Instant,
    clock: Mutex<(u64, Instant)>,
    pub(super) tokenizer: Option<Arc<health::native::Source>>,
}
impl Store {
    /// Durable records survive restart for inspection, but cannot become fresh
    /// samples after a restart or clock rollback. No restored telemetry is live.
    pub fn open(path: &Path, network: discovery::Identity, config: Config) -> Result<Self> {
        network.validate()?;
        config.validate()?;
        let configuration = config.digest()?;
        let tokenizer = config
            .tokenizer
            .as_ref()
            .map(|t| {
                let bytes =
                    crate::connector::config::private_file(&t.file, t.limits.artifact_bytes)
                        .map_err(|_| crate::invalid("conformance tokenizer protection failed"))?;
                health::native::Source::from_bytes(
                    &bytes,
                    t.digest.clone(),
                    configuration.clone(),
                    configuration.clone(),
                    t.limits,
                )
                .map(Arc::new)
                .map_err(|_| crate::invalid("conformance tokenizer rejected"))
            })
            .transpose()?;
        let file = crate::attempts::private_file(path)
            .map_err(|_| crate::invalid("conformance store protection failed"))?;
        let mut builder = Database::builder();
        builder.set_cache_size(8 * 1024 * 1024);
        let database = db(builder.create_file(file))?;
        let tx = db(database.begin_write())?;
        {
            let mut m = db(tx.open_table(META))?;
            let expected = serde_json::to_vec(&(network.clone(), config.tester.clone()))?;
            let prior = db(m.get("identity"))?.map(|v| v.value().to_vec());
            require(
                prior.as_ref().is_none_or(|v| v == &expected),
                "conformance store identity differs",
            )?;
            if prior.is_none() {
                db(m.insert("identity", expected.as_slice()))?;
                db(m.insert("bytes", &0u64.to_le_bytes()[..]))?;
            }
            require(
                db(db(tx.open_table(RECORDS))?.len())? <= config.maximum_records
                    && stored_bytes(&m)? <= config.maximum_bytes,
                "conformance store exceeds resource bounds",
            )?;
            db(tx.open_table(EXPIRY))?;
        }
        db(tx.commit())?;
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(|_| crate::invalid("conformance boot unavailable"))?;
        Ok(Self {
            database,
            network,
            config,
            configuration,
            boot: Digest::hash("mayhem/proxy/conformance-boot/v1", &[&random]),
            opened: Instant::now(),
            clock: Mutex::new((crate::supervisor::unix_ms(), Instant::now())),
            tokenizer,
        })
    }
    pub fn network(&self) -> &discovery::Identity {
        &self.network
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn configuration(&self) -> &Digest {
        &self.configuration
    }
    pub fn now_ms(&self) -> u64 {
        self.effective_time(crate::supervisor::unix_ms())
    }
    pub(super) fn effective_time(&self, wall_ms: u64) -> u64 {
        let Ok(mut clock) = self.clock.lock() else {
            return u64::MAX;
        };
        let monotonic = clock
            .0
            .saturating_add(clock.1.elapsed().as_millis().min(u64::MAX as u128) as u64);
        if wall_ms > monotonic {
            *clock = (wall_ms, Instant::now());
            wall_ms
        } else {
            monotonic
        }
    }
    fn key(subject: &Subject, class: &Class) -> Result<String> {
        Ok(
            digest("mayhem/proxy/conformance-index/v1", &(subject, class))?
                .as_str()
                .into(),
        )
    }
    pub fn lookup(&self, subject: &Subject, class: &Class) -> Result<Lookup> {
        let tx = db(self.database.begin_read())?;
        let table = db(tx.open_table(RECORDS))?;
        let key = Self::key(subject, class)?;
        let Some(row) = db(table.get(key.as_str()))? else {
            return Ok(Lookup::Missing);
        };
        require(
            row.value().len() <= MAX_RECORD_BYTES,
            "conformance record exceeds byte bound",
        )?;
        let signed: Signed = serde_json::from_slice(row.value())?;
        signed.verify()?;
        let b = &signed.body;
        require(
            b.network == self.network
                && b.tester == self.config.tester
                && &b.subject == subject
                && &b.class == class,
            "conformance stored binding differs",
        )?;
        if b.configuration != self.configuration {
            return Ok(Lookup::ConfigurationChanged);
        }
        if b.boot != self.boot {
            return Ok(Lookup::Restarted);
        }
        let now = self.now_ms();
        if now < b.observed_at_ms || now >= b.expires_at_ms {
            return Ok(Lookup::Expired);
        }
        Ok(Lookup::Present(Box::new(signed)))
    }
    // Only a sealed completion inside this module can reach this writer.
    pub(super) fn retain(&self, signed: Signed) -> Result<()> {
        signed.verify()?;
        let b = &signed.body;
        require(
            b.network == self.network
                && b.tester == self.config.tester
                && b.boot == self.boot
                && b.configuration == self.configuration
                && b.expires_at_ms - b.observed_at_ms == self.config.ttl_ms,
            "conformance completion owner differs",
        )?;
        let bytes = serde_json::to_vec(&signed)?;
        require(
            bytes.len() <= MAX_RECORD_BYTES,
            "conformance record exceeds byte bound",
        )?;
        let key = Self::key(&b.subject, &b.class)?;
        let expiry = |at: u64, key: &str| format!("{at:016x}/{key}");
        let mut tx = db(self.database.begin_write())?;
        db(tx.set_durability(Durability::Immediate))?;
        {
            let mut meta = db(tx.open_table(META))?;
            let mut total = stored_bytes(&meta)?;
            let mut records = db(tx.open_table(RECORDS))?;
            let mut expired = db(tx.open_table(EXPIRY))?;
            // Indexed bounded housekeeping, independent of total registry size.
            let old_keys = db(expired.range(..expiry(self.now_ms(), "").as_str()))?
                .take(32)
                .map(|v| {
                    let (k, v) = db(v)?;
                    Ok((k.value().to_owned(), v.value().to_owned()))
                })
                .collect::<Result<Vec<_>>>()?;
            for (index, id) in old_keys {
                if let Some(old) = db(records.remove(id.as_str()))? {
                    total = total.saturating_sub(old.value().len() as u64);
                }
                db(expired.remove(index.as_str()))?;
            }
            let prior = db(records.get(key.as_str()))?.map(|v| v.value().to_vec());
            if let Some(prior) = prior {
                let old: Signed = serde_json::from_slice(&prior)?;
                // Repeated callbacks cannot turn one execution into a fresh sample.
                if old.body.session == b.session {
                    return Ok(());
                }
                total = total
                    .checked_sub(prior.len() as u64)
                    .ok_or_else(|| crate::invalid("conformance size metadata differs"))?;
                db(expired.remove(expiry(old.body.expires_at_ms, &key).as_str()))?;
            } else {
                require(
                    db(records.len())? < self.config.maximum_records,
                    "conformance record quota exhausted",
                )?;
            }
            total = total
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| crate::invalid("conformance bytes overflow"))?;
            require(
                total <= self.config.maximum_bytes,
                "conformance byte quota exhausted",
            )?;
            db(records.insert(key.as_str(), bytes.as_slice()))?;
            db(expired.insert(expiry(b.expires_at_ms, &key).as_str(), key.as_str()))?;
            db(meta.insert("bytes", total.to_le_bytes().as_slice()))?;
        }
        db(tx.commit())?;
        Ok(())
    }
    /// Exact pinned semantics only. Unknown assertions never satisfy a predicate.
    pub fn evaluate(
        &self,
        definition: &registry::Definition,
        predicate: &registry::Predicate,
        record: &Signed,
    ) -> Result<registry::Match> {
        record.verify()?;
        let b = &record.body;
        require(
            b.network == self.network
                && b.tester == self.config.tester
                && b.boot == self.boot
                && b.configuration == self.configuration,
            "conformance assessment authority differs",
        )?;
        let mapping = self.config.mappings.iter().find(|m| {
            m.field_id == definition.field_id && m.schema_revision == definition.schema_revision
        });
        let Some(mapping) = mapping else {
            return Ok(registry::Match::Unknown);
        };
        require(
            definition.digest()? == mapping.definition_digest.as_str()
                && matches!(definition.value_schema, registry::ValueSchema::Boolean)
                && matches!(definition.usage, registry::Usage::FilterOnly),
            "conformance assertion semantic binding differs",
        )?;
        let supported = b.assertions.contains(&mapping.assertion);
        let observation = registry::Observation {
            field_id: mapping.field_id.clone(),
            schema_revision: mapping.schema_revision,
            endpoint: b.subject.endpoint,
            status: if supported {
                registry::Support::Supported
            } else {
                registry::Support::Unknown
            },
            value: supported.then_some(registry::TypedValue::Boolean(true)),
            observed_at_ms: b.observed_at_ms,
            expires_at_ms: Some(b.expires_at_ms),
            source_id: "gateway_observation".into(),
        };
        registry::evaluate(
            definition,
            predicate,
            Some(&observation),
            if b.provenance == Provenance::GatewayObservation && b.tester != b.subject.provider {
                registry::Assurance::Probed
            } else {
                registry::Assurance::Declared
            },
            b.subject.endpoint,
            self.now_ms(),
        )
    }
    pub fn semantics(&self) -> serde_json::Value {
        serde_json::json!({"schema_version":1,"kind":"conformance_policy","suite":SUITE,"configuration_hash":self.configuration,
            "source":"gateway_observation","maximum_assurance":"probed","mappings":self.config.mappings,
            "mapping_status":"requires_pinned_registry_validation",
            "ttl_ms":self.config.ttl_ms,"speed_basis":"locally_tokenized_generation_rate",
            "tokenizer":self.config.tokenizer.as_ref().map(|t| &t.digest),"minimum_interval_tokens":self.config.minimum_interval_tokens,
            "minimum_interval_us":self.config.minimum_interval_us,"remote_cache":"unknown","remote_concurrency":"unknown",
            "restart_behavior":"fresh_observation_required","authorizes_execution":false})
    }
}
fn stored_bytes<T: ReadableTable<&'static str, &'static [u8]>>(m: &T) -> Result<u64> {
    let v = db(m.get("bytes"))?.ok_or_else(|| crate::invalid("conformance metadata missing"))?;
    Ok(u64::from_le_bytes(v.value().try_into().map_err(|_| {
        crate::invalid("conformance metadata invalid")
    })?))
}
