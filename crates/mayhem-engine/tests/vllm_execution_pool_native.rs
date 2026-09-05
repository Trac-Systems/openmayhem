#![cfg(feature = "vllm")]

//! Opt-in native evidence only, not signed calibration or billing proof.
//! The caller must authenticate the plan before explicitly running the ignored test.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mayhem_engine::{
    ArtifactFormat, CancellationToken, ConcurrentGenerationBackend, EngineBackend, EngineError,
    GenerateRequest, LoadConfig, TokenChunk, VllmBackend, VllmGenerationTopology,
};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

const PLAN_ENV: &str = "MAYHEM_NATIVE_VLLM_POOL_PLAN";
const MAX_PLAN_BYTES: u64 = 128 * 1024 * 1024;
const MAX_EVIDENCE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SUMMARY_BYTES: u64 = 1024 * 1024;
const MAX_ROUND_TIMEOUT_MS: u64 = 3_600_000;
const CANCEL_GRACE: Duration = Duration::from_secs(10);
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    load_config: LoadConfig,
    python: PathBuf,
    cases: Vec<Case>,
    rounds: Vec<Round>,
    round_timeout_ms: u64,
    output_dir: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    request: GenerateRequest,
    expected_prefix: Vec<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Round {
    case_ids: Vec<String>,
    require_overlap: bool,
}

// serde_json::Value normally accepts duplicate keys, including in nested objects.
struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate key {key:?}")));
                    }
                    values.insert(key, map.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueValue(Value::Number(number)))
                    .ok_or_else(|| de::Error::custom("non-finite number"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

fn reject_dropped_fields(input: &Value, decoded: &Value, path: &str) -> TestResult {
    match (input, decoded) {
        (Value::Object(input), Value::Object(decoded)) => {
            for (key, value) in input {
                let child = decoded.get(key).ok_or_else(|| {
                    format!("{path}/{key}: unknown or omitted field; omit absent optional values")
                })?;
                reject_dropped_fields(value, child, &format!("{path}/{key}"))?;
            }
        }
        (Value::Array(input), Value::Array(decoded)) if input.len() == decoded.len() => {
            for (index, (value, child)) in input.iter().zip(decoded).enumerate() {
                reject_dropped_fields(value, child, &format!("{path}/{index}"))?;
            }
        }
        (Value::Object(_) | Value::Array(_), _) => {
            return Err(format!("{path}: noncanonical structured value").into());
        }
        _ => {}
    }
    Ok(())
}

fn decode_plan(bytes: &[u8]) -> TestResult<Plan> {
    if bytes.len() as u64 > MAX_PLAN_BYTES {
        return Err("plan exceeds 128 MiB".into());
    }
    let raw = serde_json::from_slice::<UniqueValue>(bytes)?.0;
    let plan: Plan = serde_json::from_value(raw.clone())?;
    reject_dropped_fields(&raw, &serde_json::to_value(&plan)?, "")?;
    let config = &plan.load_config;
    let capacity = config.vllm_concurrent_generation_capacity.unwrap_or(0) as usize;
    if config.artifact.format != ArtifactFormat::VllmSafetensors
        || config.vllm_generation_topology != Some(VllmGenerationTopology::IsolatedWorkers)
        || !(2..=128).contains(&capacity)
        || config.vllm_max_num_seqs != Some(1)
        || config.memory_limit_bytes.unwrap_or(0) == 0
        || config.ctx_size == 0
    {
        return Err("plan requires an admitted isolated vLLM pool of 2..=128 workers".into());
    }
    let target = config.vllm_gpu_memory_utilization_pct.unwrap_or(0);
    let floor = config.vllm_gpu_memory_utilization_floor_pct.unwrap_or(0);
    if floor == 0 || floor > target || target > 100 {
        return Err("explicit valid utilization target and floor are required".into());
    }
    if !plan.python.is_absolute()
        || !config.artifact.path.is_absolute()
        || !plan.output_dir.is_absolute()
        || !(1..=MAX_ROUND_TIMEOUT_MS).contains(&plan.round_timeout_ms)
        || plan.cases.is_empty()
        || plan.cases.len() > 4096
        || plan.rounds.is_empty()
        || plan.rounds.len() > 1024
    {
        return Err("invalid paths, round deadline, or plan size".into());
    }
    let mut cases = BTreeMap::new();
    for case in &plan.cases {
        if case.id.is_empty()
            || case.id.len() > 128
            || !case
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
            || cases.insert(&case.id, case).is_some()
            || case.expected_prefix.is_empty()
            || case.expected_prefix.len() > case.request.max_new_tokens as usize
            || case.request.max_new_tokens == 0
            || case.request.max_new_tokens > config.ctx_size
        {
            return Err("invalid/duplicate case id, prefix, or original request budget".into());
        }
    }
    let mut requests = 0usize;
    let mut multi = false;
    for round in &plan.rounds {
        if round.case_ids.is_empty()
            || round.case_ids.len() > capacity
            || (round.require_overlap && round.case_ids.len() < 2)
        {
            return Err("round must contain 1..=capacity case ids".into());
        }
        multi |= round.case_ids.len() > 1;
        for id in &round.case_ids {
            cases
                .get(id)
                .ok_or_else(|| format!("unknown case {id:?}"))?;
            requests += 1;
        }
    }
    if !multi || requests > 4096 {
        return Err("plan needs a multi-request round and at most 4096 requests".into());
    }
    Ok(plan)
}

fn read_plan(path: &Path) -> TestResult<Vec<u8>> {
    if !path.is_absolute() {
        return Err("plan path must be absolute".into());
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_PLAN_BYTES {
        return Err("plan must be a regular file no larger than 128 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_PLAN_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PLAN_BYTES {
        return Err("plan grew beyond 128 MiB".into());
    }
    Ok(bytes)
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

struct BoundedFile {
    file: File,
    remaining: Arc<AtomicU64>,
}

impl Write for BoundedFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                left.checked_sub(bytes.len() as u64)
            })
            .map_err(|_| io::Error::other("evidence byte budget exhausted"))?;
        self.file.write_all(bytes)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[derive(Clone)]
struct Evidence {
    root: PathBuf,
    plan_sha256: String,
    remaining: Arc<AtomicU64>,
}

impl Evidence {
    fn create(plan: &Plan, bytes: &[u8]) -> TestResult<Self> {
        fs::create_dir(&plan.output_dir)?;
        let evidence = Self {
            root: plan.output_dir.clone(),
            plan_sha256: digest(bytes),
            remaining: Arc::new(AtomicU64::new(MAX_EVIDENCE_BYTES)),
        };
        let mut copy = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(evidence.root.join("plan.json"))?;
        copy.write_all(bytes)?;
        copy.sync_all()?;
        evidence.write(
            "start.json",
            json!({
                "schema_version": 1,
                "not_signed_calibration_or_billing_proof": true,
                "round_timeout_ms": plan.round_timeout_ms,
            }),
        )?;
        Ok(evidence)
    }
    fn open(&self, name: &str) -> TestResult<BoundedFile> {
        Ok(BoundedFile {
            file: OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.root.join(name))?,
            remaining: Arc::clone(&self.remaining),
        })
    }
    fn event(&self, file: &mut BoundedFile, value: Value) -> TestResult {
        serde_json::to_writer(
            &mut *file,
            &json!({
                "plan_sha256": self.plan_sha256,
                "event": value,
            }),
        )?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }
    fn write(&self, name: &str, value: Value) -> TestResult {
        self.event(&mut self.open(name)?, value)
    }
    fn finish(&self, value: Value) -> TestResult {
        // Reserve summary space independently, even if streaming exhausted its budget.
        let summary = Self {
            remaining: Arc::new(AtomicU64::new(MAX_SUMMARY_BYTES)),
            ..self.clone()
        };
        summary.write("result.json", value)
    }
}

#[derive(Debug, Serialize)]
struct RequestResult {
    lane: usize,
    case_id: String,
    original_max_new_tokens: u32,
    start_ns: u64,
    first_token_ns: Option<u64>,
    last_token_ns: Option<u64>,
    terminal_ns: u64,
    prompt_tokens: Option<u32>,
    output_tokens: Option<u32>,
    streamed_tokens: usize,
    error: Option<String>,
}

fn elapsed_ns(origin: Instant) -> u64 {
    u64::try_from(origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn check_token(case: &Case, tokens: &[i32], chunk: &TokenChunk) -> Result<(), String> {
    let index = tokens.len();
    if chunk.index as usize != index || index >= case.request.max_new_tokens as usize {
        return Err(format!(
            "nonsequential/out-of-budget token index {} (expected {index})",
            chunk.index
        ));
    }
    if case
        .expected_prefix
        .get(index)
        .is_some_and(|expected| *expected != chunk.token_id)
    {
        return Err(format!(
            "prefix mismatch at index {index}: expected {}, got {}",
            case.expected_prefix[index], chunk.token_id
        ));
    }
    Ok(())
}

struct WarmupGate {
    arrived: AtomicUsize,
    total: usize,
    deadline: Instant,
}

impl WarmupGate {
    fn wait(&self, cancellation: &CancellationToken) -> mayhem_engine::Result<()> {
        self.arrived.fetch_add(1, Ordering::AcqRel);
        while self.arrived.load(Ordering::Acquire) < self.total {
            if Instant::now() >= self.deadline {
                cancellation.cancel();
            }
            cancellation.check()?;
            thread::sleep(Duration::from_millis(1));
        }
        cancellation.check()
    }
}

fn generate_case(
    backend: &dyn ConcurrentGenerationBackend,
    case: &Case,
    lane: usize,
    cancellation: &CancellationToken,
    origin: Instant,
    evidence: &Evidence,
    log: &mut BoundedFile,
    warmup_gate: Option<&WarmupGate>,
) -> RequestResult {
    let mut record = RequestResult {
        lane,
        case_id: case.id.clone(),
        original_max_new_tokens: case.request.max_new_tokens,
        start_ns: elapsed_ns(origin),
        first_token_ns: None,
        last_token_ns: None,
        terminal_ns: 0,
        prompt_tokens: None,
        output_tokens: None,
        streamed_tokens: 0,
        error: None,
    };
    let mut tokens = Vec::new();
    let result = catch_unwind(AssertUnwindSafe(|| -> TestResult {
        evidence.event(
            log,
            json!({
                "kind": "request_start", "lane": lane, "case_id": case.id,
                "start_ns": record.start_ns, "max_new_tokens": case.request.max_new_tokens,
                "request_sha256": digest(&serde_json::to_vec(&case.request)?),
            }),
        )?;
        let output = backend.generate(
            case.request.clone(),
            &mut |chunk: TokenChunk| {
                let at = elapsed_ns(origin);
                let validation = check_token(case, &tokens, &chunk);
                evidence.event(log, json!({
                "kind": "token", "index": chunk.index, "token_id": chunk.token_id, "at_ns": at,
            })).map_err(|error| {
                cancellation.cancel();
                EngineError::InvalidOutput(error.to_string())
            })?;
                if let Err(error) = validation {
                    cancellation.cancel();
                    return Err(EngineError::InvalidOutput(error));
                }
                record.first_token_ns.get_or_insert(at);
                record.last_token_ns = Some(at);
                tokens.push(chunk.token_id);
                if tokens.len() == 1 {
                    if let Some(gate) = warmup_gate {
                        gate.wait(cancellation)?;
                    }
                }
                cancellation.check()
            },
            cancellation,
        )?;
        record.prompt_tokens = Some(output.usage.prompt_tokens);
        record.output_tokens = Some(output.usage.completion_tokens);
        if tokens.len() < case.expected_prefix.len() {
            return Err("output terminated before the complete expected prefix".into());
        }
        if output.usage.prompt_tokens == 0
            || output.usage.completion_tokens as usize != tokens.len()
            || output.usage.completion_tokens > case.request.max_new_tokens
        {
            return Err("terminal usage does not match streamed tokens/original budget".into());
        }
        Ok(())
    }));
    record.error = match result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error.to_string()),
        Err(_) => Some("generation thread panicked".to_owned()),
    };
    if record.error.is_some() {
        cancellation.cancel();
    }
    record.terminal_ns = elapsed_ns(origin);
    record.streamed_tokens = tokens.len();
    if let Err(error) = evidence.event(log, json!({"kind": "terminal", "result": record})) {
        record.error = Some(format!("terminal evidence write failed: {error}"));
        cancellation.cancel();
    }
    record
}

fn token_overlap(results: &[RequestResult]) -> u64 {
    let first = results
        .iter()
        .map(|item| item.first_token_ns)
        .collect::<Option<Vec<_>>>();
    let last = results
        .iter()
        .map(|item| item.last_token_ns)
        .collect::<Option<Vec<_>>>();
    match (first, last) {
        (Some(first), Some(last)) => last
            .into_iter()
            .min()
            .unwrap_or(0)
            .saturating_sub(first.into_iter().max().unwrap_or(u64::MAX)),
        _ => 0,
    }
}

fn run_round(
    backend: &Arc<dyn ConcurrentGenerationBackend>,
    plan: &Plan,
    round_index: usize,
    evidence: &Evidence,
    handles: &mut Vec<JoinHandle<()>>,
) -> TestResult {
    let round = &plan.rounds[round_index];
    let ids = &round.case_ids;
    let barrier = Arc::new(Barrier::new(ids.len() + 1));
    let cancellation = CancellationToken::new();
    let origin = Instant::now();
    // Holding each first-token callback retains its distinct engine lease. A full
    // duplicate warmup therefore exercises every worker, even for tiny outputs.
    // Never gate measured rounds: buffering would distort token-interval evidence.
    let warmup_gate = (!round.require_overlap).then(|| {
        Arc::new(WarmupGate {
            arrived: AtomicUsize::new(0),
            total: ids.len(),
            deadline: origin + Duration::from_millis(plan.round_timeout_ms),
        })
    });
    let (tx, rx) = mpsc::channel();
    let mut starters = Vec::new();
    for (lane, id) in ids.iter().enumerate() {
        let case = plan
            .cases
            .iter()
            .find(|case| &case.id == id)
            .unwrap()
            .clone();
        let mut log = evidence.open(&format!("round-{round_index:04}-lane-{lane:03}.jsonl"))?;
        let backend = Arc::clone(backend);
        let barrier = Arc::clone(&barrier);
        let cancellation = cancellation.clone();
        let evidence = evidence.clone();
        let tx = tx.clone();
        let warmup_gate = warmup_gate.clone();
        // Do not enter the barrier until every thread was successfully spawned.
        let (start_tx, start_rx) = mpsc::channel();
        handles.push(
            thread::Builder::new()
                .name(format!("pool-{round_index}-{lane}"))
                .spawn(move || {
                    if start_rx.recv().is_err() {
                        return;
                    }
                    barrier.wait();
                    let result = generate_case(
                        backend.as_ref(),
                        &case,
                        lane,
                        &cancellation,
                        origin,
                        &evidence,
                        &mut log,
                        warmup_gate.as_deref(),
                    );
                    let _ = tx.send(result);
                })?,
        );
        starters.push(start_tx);
    }
    drop(tx);
    for starter in starters {
        starter.send(())?;
    }
    barrier.wait();
    let deadline = Instant::now() + Duration::from_millis(plan.round_timeout_ms);
    let mut results = Vec::new();
    let mut failure = None;
    while results.len() < ids.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(result) => {
                let failed = result.error.is_some();
                if failed {
                    cancellation.cancel();
                }
                results.push(result);
                if failed {
                    failure = Some("request failed; cancelling sibling requests".to_owned());
                    break;
                }
            }
            Err(error) => {
                cancellation.cancel();
                failure = Some(format!("round deadline/channel failure: {error}"));
                break;
            }
        }
    }
    if results.len() < ids.len() {
        let grace = Instant::now() + CANCEL_GRACE;
        while results.len() < ids.len() {
            match rx.recv_timeout(grace.saturating_duration_since(Instant::now())) {
                Ok(result) => results.push(result),
                Err(_) => break,
            }
        }
    }
    results.sort_by_key(|result| result.lane);
    let overlap_ns = token_overlap(&results);
    if results.iter().any(|result| result.error.is_some()) {
        failure.get_or_insert_with(|| "one or more requests failed; siblings cancelled".to_owned());
    }
    if round.require_overlap && overlap_ns == 0 {
        failure.get_or_insert_with(|| "no positive common token-interval overlap".to_owned());
    }
    evidence.write(
        &format!("round-{round_index:04}.json"),
        json!({
            "results": results, "expected_requests": ids.len(),
            "overlap_ns": round.require_overlap.then_some(overlap_ns),
            "require_overlap": round.require_overlap,
            "overlap_proven": round.require_overlap && failure.is_none() && overlap_ns > 0,
            "warmup_first_token_gate": !round.require_overlap,
            "error": failure,
        }),
    )?;
    match failure {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
}

fn process_table() -> TestResult<BTreeMap<u32, u32>> {
    if !cfg!(unix) {
        return Err("native PID verification requires Unix".into());
    }
    let output = Command::new("ps").args(["-axo", "pid=,ppid="]).output()?;
    if !output.status.success() || output.stdout.len() > 4 * 1024 * 1024 {
        return Err("cannot read bounded process table".into());
    }
    let mut processes = BTreeMap::new();
    for line in std::str::from_utf8(&output.stdout)?.lines() {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() != 2 {
            return Err("malformed process table".into());
        }
        processes.insert(columns[0].parse()?, columns[1].parse()?);
    }
    Ok(processes)
}

fn remember_descendants(known: &mut BTreeSet<u32>) -> TestResult {
    let table = process_table()?;
    loop {
        let previous = known.len();
        for (pid, parent) in &table {
            if known.contains(parent) {
                known.insert(*pid);
            }
        }
        if known.len() == previous {
            return Ok(());
        }
    }
}

fn healthy_evidence(backend: &mut VllmBackend, capacity: usize) -> TestResult<Value> {
    let evidence = backend
        .loaded_backend_evidence()
        .ok_or("missing loaded evidence")?;
    let workers = evidence
        .pointer("/generation/per_worker")
        .and_then(Value::as_array)
        .ok_or("missing per-worker evidence")?;
    if !backend.component_healthy()
        || workers.len() != capacity
        || workers
            .iter()
            .any(|worker| worker["healthy"] != json!(true))
        || evidence.pointer("/generation/topology") != Some(&json!("isolated_workers"))
        || backend.process_ids().len() != capacity
        || backend
            .process_ids()
            .into_iter()
            .collect::<BTreeSet<_>>()
            .len()
            != capacity
    {
        return Err("isolated pool did not retain all healthy workers".into());
    }
    Ok(evidence)
}

fn execute(plan: &Plan, evidence: &Evidence) -> TestResult {
    process_table()?;
    let mut backend = VllmBackend::with_python(&plan.python)?;
    let mut processes = BTreeSet::new();
    let mut handles = Vec::new();
    let run = catch_unwind(AssertUnwindSafe(|| -> TestResult {
        let loaded = backend.load(plan.load_config.clone())?;
        processes.extend(backend.process_ids());
        remember_descendants(&mut processes)?;
        let concurrent = backend
            .concurrent_generation_backend()
            .ok_or("pool did not expose concurrent_generation_backend")?;
        let capacity = plan
            .load_config
            .vllm_concurrent_generation_capacity
            .unwrap() as usize;
        if concurrent.capacity() != capacity {
            return Err("runtime capacity differs from plan".into());
        }
        evidence.write(
            "loaded.json",
            json!({
                "loaded": loaded, "evidence": healthy_evidence(&mut backend, capacity)?,
                "process_ids": processes,
            }),
        )?;
        for index in 0..plan.rounds.len() {
            run_round(&concurrent, plan, index, evidence, &mut handles)?;
            remember_descendants(&mut processes)?;
            evidence.write(
                &format!("round-{index:04}-health.json"),
                healthy_evidence(&mut backend, capacity)?,
            )?;
        }
        Ok(())
    }));
    processes.extend(backend.process_ids());
    let capture_error = remember_descendants(&mut processes)
        .err()
        .map(|error| error.to_string());
    // Drop the real backend before joining potentially cancelled native callers.
    drop(backend);
    let teardown_deadline = Instant::now() + CANCEL_GRACE;
    while handles.iter().any(|handle| !handle.is_finished()) && Instant::now() < teardown_deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let mut failures = Vec::new();
    match run {
        Ok(Ok(())) => {}
        Ok(Err(error)) => failures.push(error.to_string()),
        Err(_) => failures.push("native run panicked".to_owned()),
    }
    if let Some(error) = capture_error {
        failures.push(error);
    }
    for handle in handles {
        if !handle.is_finished() {
            failures.push("generation thread survived backend drop".to_owned());
        } else if handle.join().is_err() {
            failures.push("generation thread panicked".to_owned());
        }
    }
    let mut alive = processes.clone();
    while !alive.is_empty() {
        let table = process_table()?;
        alive.retain(|pid| table.contains_key(pid));
        if alive.is_empty() || Instant::now() >= teardown_deadline {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if !alive.is_empty() {
        failures.push(format!("processes survived backend drop: {alive:?}"));
    }
    evidence.finish(json!({
        "ok": failures.is_empty(), "errors": failures, "observed_process_ids": processes,
        "surviving_process_ids": alive, "not_signed_calibration_or_billing_proof": true,
    }))?;
    if !failures.is_empty() {
        return Err(failures.join("; ").into());
    }
    Ok(())
}

#[test]
#[ignore = "requires an authenticated local plan and explicit native/GPU opt-in"]
fn native_execution_pool() -> TestResult {
    let path = env::var_os(PLAN_ENV).ok_or_else(|| format!("{PLAN_ENV} is required"))?;
    let bytes = read_plan(Path::new(&path))?;
    let plan = decode_plan(&bytes)?;
    let evidence = Evidence::create(&plan, &bytes)?;
    let result = execute(&plan, &evidence);
    if let Err(error) = &result {
        if !evidence.root.join("result.json").exists() {
            evidence.finish(json!({"ok": false, "error": error.to_string(),
                "not_signed_calibration_or_billing_proof": true}))?;
        }
    }
    result
}

fn fixture_plan() -> Plan {
    let mut config = LoadConfig::vllm_safetensors("/not-loaded/model");
    config.vllm_generation_topology = Some(VllmGenerationTopology::IsolatedWorkers);
    config.vllm_concurrent_generation_capacity = Some(2);
    config.vllm_max_num_seqs = Some(1);
    config.memory_limit_bytes = Some(1024 * 1024);
    config.vllm_gpu_memory_utilization_pct = Some(20);
    config.vllm_gpu_memory_utilization_floor_pct = Some(10);
    Plan {
        load_config: config,
        python: "/not-run/python".into(),
        cases: vec![Case {
            id: "a".to_owned(),
            request: GenerateRequest::new("hello").with_max_new_tokens(4),
            expected_prefix: vec![10, 20],
        }],
        rounds: vec![Round {
            case_ids: vec!["a".to_owned(), "a".to_owned()],
            require_overlap: false,
        }],
        round_timeout_ms: 1000,
        output_dir: env::temp_dir().join("not-created-native-pool"),
    }
}

#[test]
fn plan_is_strict_and_preserves_original_request_budget() {
    let raw = serde_json::to_value(fixture_plan()).unwrap();
    let plan = decode_plan(&serde_json::to_vec(&raw).unwrap()).unwrap();
    assert_eq!(plan.cases[0].request.max_new_tokens, 4);
    for pointer in [
        "",
        "/load_config",
        "/load_config/artifact",
        "/cases/0",
        "/cases/0/request",
        "/rounds/0",
    ] {
        let mut invalid = raw.clone();
        invalid
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unexpected".to_owned(), json!(true));
        assert!(
            decode_plan(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{pointer}"
        );
    }
    assert!(serde_json::from_slice::<UniqueValue>(br#"{"x":{"a":1,"a":2}}"#).is_err());
    for (pointer, value) in [
        (
            "/load_config/vllm_generation_topology",
            json!("shared_worker"),
        ),
        ("/load_config/vllm_generation_topology", json!("unknown")),
        ("/load_config/vllm_concurrent_generation_capacity", json!(1)),
        ("/round_timeout_ms", json!(0)),
        ("/round_timeout_ms", json!(MAX_ROUND_TIMEOUT_MS + 1)),
        ("/rounds/0/case_ids", json!(["missing", "a"])),
        ("/rounds/0/case_ids", json!(["a", "a", "a"])),
        ("/cases/0/expected_prefix", json!([1, 2, 3, 4, 5])),
    ] {
        let mut invalid = raw.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(
            decode_plan(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn long_context_round_deadline_remains_explicit_and_bounded() {
    let mut raw = serde_json::to_value(fixture_plan()).unwrap();
    for timeout_ms in [1, 600_000, 1_800_000, MAX_ROUND_TIMEOUT_MS] {
        raw["round_timeout_ms"] = json!(timeout_ms);
        let plan = decode_plan(&serde_json::to_vec(&raw).unwrap()).unwrap();
        assert_eq!(plan.round_timeout_ms, timeout_ms);
    }
}

#[test]
fn indexed_prefix_rejects_gaps_mismatches_and_budget_overrun() {
    let case = fixture_plan().cases.remove(0);
    let chunk = |index, token_id| TokenChunk {
        index,
        token_id,
        text: String::new(),
    };
    assert!(check_token(&case, &[], &chunk(0, 10)).is_ok());
    assert!(check_token(&case, &[10], &chunk(1, 20)).is_ok());
    assert!(check_token(&case, &[], &chunk(1, 10)).is_err());
    assert!(check_token(&case, &[10], &chunk(1, 99)).is_err());
    assert!(check_token(&case, &[10, 20, 30, 40], &chunk(4, 50)).is_err());
}

#[test]
fn token_overlap_uses_tokens_not_request_lifetimes() {
    let result = |first, last| RequestResult {
        lane: 0,
        case_id: "a".to_owned(),
        original_max_new_tokens: 4,
        start_ns: 0,
        first_token_ns: Some(first),
        last_token_ns: Some(last),
        terminal_ns: 100,
        prompt_tokens: Some(1),
        output_tokens: Some(2),
        streamed_tokens: 2,
        error: None,
    };
    assert_eq!(token_overlap(&[result(10, 30), result(20, 40)]), 10);
    assert_eq!(token_overlap(&[result(10, 20), result(20, 40)]), 0);
    assert_eq!(token_overlap(&[result(10, 15), result(20, 40)]), 0);
}

#[test]
fn evidence_is_exclusive_hash_bound_and_bounded() {
    let mut plan = fixture_plan();
    plan.output_dir = env::temp_dir().join(format!(
        "mayhem-native-evidence-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let bytes = serde_json::to_vec(&plan).unwrap();
    let evidence = Evidence::create(&plan, &bytes).unwrap();
    assert!(Evidence::create(&plan, &bytes).is_err());
    evidence
        .write("partial.jsonl", json!({"token_id": 10}))
        .unwrap();
    assert!(evidence
        .write("partial.jsonl", json!({"token_id": 20}))
        .is_err());
    let partial: Value =
        serde_json::from_slice(&fs::read(plan.output_dir.join("partial.jsonl")).unwrap()).unwrap();
    assert_eq!(partial["plan_sha256"], digest(&bytes));
    assert_eq!(partial["event"]["token_id"], 10);
    evidence.remaining.store(0, Ordering::Release);
    assert!(evidence.write("exhausted.json", json!({"x": 1})).is_err());
    evidence
        .finish(json!({"ok": false, "error": "bounded test failure"}))
        .unwrap();
    assert!(plan.output_dir.join("result.json").exists());
    fs::remove_dir_all(plan.output_dir).unwrap();
}

struct FakePool {
    started: AtomicUsize,
    cancelled: AtomicUsize,
}

impl ConcurrentGenerationBackend for FakePool {
    fn capacity(&self) -> usize {
        2
    }

    fn generate(
        &self,
        request: GenerateRequest,
        sink: &mut dyn mayhem_engine::TokenSink,
        cancellation: &CancellationToken,
    ) -> mayhem_engine::Result<mayhem_engine::GenerateOutput> {
        self.started.fetch_add(1, Ordering::AcqRel);
        if request.max_new_tokens != 4 {
            return Err(EngineError::InvalidRequest(
                "original budget changed".to_owned(),
            ));
        }
        if request.prompt == "fail" {
            while self.started.load(Ordering::Acquire) < 2 {
                cancellation.check()?;
                thread::sleep(Duration::from_millis(1));
            }
            return Err(EngineError::InvalidOutput(
                "deliberate sibling failure".to_owned(),
            ));
        }
        if request.prompt == "wait" {
            while !cancellation.is_cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            self.cancelled.fetch_add(1, Ordering::AcqRel);
            return Err(EngineError::Cancelled);
        }
        for (index, token_id) in [10, 20].into_iter().enumerate() {
            sink.on_token(TokenChunk {
                index: index as u32,
                token_id,
                text: String::new(),
            })?;
        }
        Ok(mayhem_engine::GenerateOutput {
            text: "ok".to_owned(),
            usage: mayhem_engine::UsageCounters::new(1, 2),
            finish_reason: mayhem_engine::FinishReason::Stop,
        })
    }
}

#[test]
fn warmups_keep_full_prefixes_without_claiming_measured_overlap() {
    let mut plan = fixture_plan();
    plan.output_dir = env::temp_dir().join(format!(
        "mayhem-pool-warmup-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let evidence = Evidence::create(&plan, &serde_json::to_vec(&plan).unwrap()).unwrap();
    let backend: Arc<dyn ConcurrentGenerationBackend> = Arc::new(FakePool {
        started: AtomicUsize::new(0),
        cancelled: AtomicUsize::new(0),
    });
    let mut handles = Vec::new();
    run_round(&backend, &plan, 0, &evidence, &mut handles).unwrap();
    for handle in handles {
        handle.join().unwrap();
    }
    let result: Value =
        serde_json::from_slice(&fs::read(plan.output_dir.join("round-0000.json")).unwrap())
            .unwrap();
    assert_eq!(result["event"]["overlap_proven"], false);
    assert!(result["event"]["overlap_ns"].is_null());
    assert_eq!(result["event"]["results"][0]["streamed_tokens"], 2);
    fs::remove_dir_all(plan.output_dir).unwrap();
}

#[test]
fn round_errors_and_deadlines_cancel_siblings_and_preserve_partial_evidence() {
    for fail in [false, true] {
        let mut plan = fixture_plan();
        plan.output_dir = env::temp_dir().join(format!(
            "mayhem-pool-cancel-{}-{fail}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        plan.round_timeout_ms = 50;
        plan.cases[0].request.prompt = if fail { "fail" } else { "wait" }.to_owned();
        let mut sibling = plan.cases[0].clone();
        sibling.id = "b".to_owned();
        sibling.request.prompt = "wait".to_owned();
        plan.cases.push(sibling);
        plan.rounds[0] = Round {
            case_ids: vec!["a".to_owned(), "b".to_owned()],
            require_overlap: true,
        };
        let evidence = Evidence::create(&plan, &serde_json::to_vec(&plan).unwrap()).unwrap();
        let fake = Arc::new(FakePool {
            started: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
        });
        let backend: Arc<dyn ConcurrentGenerationBackend> = fake.clone();
        let mut handles = Vec::new();
        assert!(run_round(&backend, &plan, 0, &evidence, &mut handles).is_err());
        for handle in handles {
            handle.join().unwrap();
        }
        assert!(fake.cancelled.load(Ordering::Acquire) >= 1);
        let partial =
            fs::read_to_string(plan.output_dir.join("round-0000-lane-001.jsonl")).unwrap();
        assert!(partial.contains("request_start"));
        assert!(partial.contains("terminal"));
        fs::remove_dir_all(plan.output_dir).unwrap();
    }
}
