use super::*;
use std::os::unix::fs::PermissionsExt;

// This executable ignores the embedded-worker arguments and never imports vLLM.
const MOCK_WORKER: &str = r#"#!/usr/bin/env python3
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import time

root = Path(__file__).resolve().parents[1]
index = len(list(root.glob('spawn-*.json')))
plans = json.loads((root / 'plans.json').read_text())
plan = plans[min(index, len(plans) - 1)]
pid = os.getpid()

def record(name, value):
    path = root / name
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(value))
    temporary.replace(path)

record(f'spawn-{index}.json', {
    'pid': pid,
    'memory_limit': os.environ.get('MAYHEM_ENGINE_MEMORY_LIMIT_BYTES'),
    'previous_loaded': index == 0 or (root / f'loaded-{index - 1}.json').exists(),
})
if plan.get('descendant'):
    child = subprocess.Popen(['sleep', '30'])
    record(f'descendant-{index}.json', child.pid)

def send(value):
    print(json.dumps(value), flush=True)

def complete(request):
    text = f"{pid}:{request['payload']['prompt']}"
    send({'id': request['id'], 'type': 'token',
          'chunk': {'index': 0, 'token_id': 1, 'text': text}})
    send({'id': request['id'], 'ok': True, 'result': {
        'text': text, 'usage': {'prompt_tokens': 1, 'completion_tokens': 1, 'total_tokens': 2},
        'finish_reason': 'stop'}})

active = None
deadline = time.monotonic() + 15
while time.monotonic() < deadline:
    if active and (root / ('release-' + active['payload']['prompt'])).exists():
        complete(active)
        active = None
    if not select.select([sys.stdin], [], [], 0.01)[0]:
        continue
    line = sys.stdin.readline()
    if not line:
        break
    request = json.loads(line)
    op = request['op']
    if op == 'shutdown':
        break
    if op == 'load':
        record(f'load-{index}.json', request)
        record(f'loaded-{index}.json', True)
        if plan.get('remove_executable'):
            Path(__file__).unlink()
        send(dict(plan['load'], id=request['id']))
    elif op == 'tokenize':
        send({'id': request['id'], 'ok': True, 'result': {'token_ids': [1, 2]}})
    elif op == 'generate':
        if active:
            raise RuntimeError('overlapping requests on an isolated worker')
        prompt = request['payload']['prompt']
        record('seen-' + prompt + '.json', {'pid': pid, 'request': request})
        if prompt.startswith('hold-'):
            active = request
        else:
            complete(request)
    elif op == 'cancel':
        record(f'cancel-{index}.json', request)
        assert active and active['id'] == request['payload']['request_id']
        send({'id': active['id'], 'cancelled': True,
              'abort_failed': plan.get('abort_failed', False)})
        active = None
"#;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(plans: Value) -> Self {
        let root = env::temp_dir().join(format!(
            "mayhem-vllm-isolated-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("checkpoint")).unwrap();
        fs::write(root.join("plans.json"), serde_json::to_vec(&plans).unwrap()).unwrap();
        let python = root.join("bin/python");
        fs::write(&python, MOCK_WORKER).unwrap();
        fs::set_permissions(&python, fs::Permissions::from_mode(0o700)).unwrap();
        let header = br#"{"weight":{"dtype":"F32","shape":[1],"data_offsets":[0,4]}}"#;
        let mut weights = (header.len() as u64).to_le_bytes().to_vec();
        weights.extend_from_slice(header);
        weights.extend_from_slice(&[0; 4]);
        fs::write(root.join("checkpoint/model.safetensors"), weights).unwrap();
        Self { root }
    }

    fn backend(&self) -> VllmBackend {
        VllmBackend::with_python(self.root.join("bin/python")).unwrap()
    }

    fn config(&self, count: u32) -> LoadConfig {
        let mut config =
            LoadConfig::vllm_safetensors(self.root.join("checkpoint/model.safetensors"));
        config.ctx_size = 4096;
        config.batch_size = 1;
        config.vllm_generation_topology = Some(VllmGenerationTopology::IsolatedWorkers);
        config.vllm_worker_address_space_limit_bytes = Some(104_630_093_824);
        config.vllm_concurrent_generation_capacity = Some(count);
        config.vllm_gpu_memory_utilization_pct = Some(31);
        config.vllm_gpu_memory_utilization_floor_pct = Some(26);
        config.backend_cache_dir = Some(self.root.join("cache"));
        config
    }

    fn read(&self, name: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(bytes) = fs::read(self.root.join(name)) {
                return serde_json::from_slice(&bytes).unwrap();
            }
            assert!(Instant::now() < deadline, "missing mock event {name}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn release(&self, prompt: &str) {
        fs::write(self.root.join(format!("release-{prompt}")), b"").unwrap();
    }

    fn assert_exited(&self, count: usize) {
        for index in 0..count {
            let pid = self.read(&format!("spawn-{index}.json"))["pid"]
                .as_u64()
                .unwrap();
            assert_process_exited(pid);
        }
        assert!(!self.root.join(format!("spawn-{count}.json")).exists());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn assert_process_exited(pid: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout);
        if state.trim().is_empty() || state.trim().starts_with('Z') {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "worker {pid} still running: {state}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn plan(tokens: u64) -> Value {
    json!({"load": {"ok": true, "result": {
        "n_vocab": 32000, "n_ctx_train": 4096, "kv_cache_size_tokens": tokens,
        "determinism": {"batch_invariant": true},
    }}})
}

fn generate(
    backend: Arc<dyn ConcurrentGenerationBackend>,
    prompt: &str,
    cancellation: CancellationToken,
) -> JoinHandle<Result<GenerateOutput>> {
    let prompt = prompt.to_owned();
    thread::spawn(move || {
        backend.generate(GenerateRequest::new(prompt), &mut |_| Ok(()), &cancellation)
    })
}

#[test]
fn topology_defaults_preserve_wire_and_shared_capacity() {
    let mut config = LoadConfig::vllm_safetensors("/tmp/checkpoint");
    assert_eq!(config.vllm_generation_topology, None);
    assert_eq!(
        VllmGenerationTopology::default(),
        VllmGenerationTopology::SharedWorker
    );
    let legacy = serde_json::to_value(&config).unwrap();
    assert!(legacy.get("vllm_generation_topology").is_none());
    assert!(legacy
        .get("vllm_worker_address_space_limit_bytes")
        .is_none());
    let restored: LoadConfig = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), legacy);
    for (topology, name) in [
        (VllmGenerationTopology::SharedWorker, "shared_worker"),
        (VllmGenerationTopology::IsolatedWorkers, "isolated_workers"),
    ] {
        config.vllm_generation_topology = Some(topology);
        let wire = serde_json::to_value(&config).unwrap();
        assert_eq!(wire["vllm_generation_topology"], name);
        assert_eq!(serde_json::from_value::<LoadConfig>(wire).unwrap(), config);
    }
    assert!(serde_json::from_value::<VllmGenerationTopology>(json!("isolated")).is_err());
    config.vllm_generation_topology = Some(VllmGenerationTopology::SharedWorker);
    config.vllm_max_num_seqs = Some(1);
    config.vllm_concurrent_generation_capacity = Some(3);
    assert!(validate_load_config(&config).is_err());
}

#[test]
fn isolated_address_envelope_is_explicit_finite_and_not_a_worker_payload_property() {
    let fixture = Fixture::new(json!([plan(8192)]));
    let mut config = fixture.config(3);
    config.memory_limit_bytes = Some(70 * 1024 * 1024 * 1024);
    let wire = serde_json::to_value(&config).unwrap();
    assert_eq!(
        wire["vllm_worker_address_space_limit_bytes"],
        104_630_093_824_u64
    );
    assert_eq!(serde_json::from_value::<LoadConfig>(wire).unwrap(), config);
    assert!(vllm_load_payload(&config, Path::new("/tmp/checkpoint"))
        .get("vllm_worker_address_space_limit_bytes")
        .is_none());
    for invalid in [None, Some(0), Some(1023), Some(u64::MAX)] {
        config.vllm_worker_address_space_limit_bytes = invalid;
        assert!(validate_load_config(&config).is_err());
    }
    config.vllm_worker_address_space_limit_bytes = Some(104_630_093_824);
    config.vllm_generation_topology = None;
    config.vllm_concurrent_generation_capacity = None;
    assert!(validate_load_config(&config).is_err());
    config.vllm_worker_address_space_limit_bytes = None;
    validate_load_config(&config).unwrap();
}

#[test]
#[cfg(target_os = "linux")]
fn isolated_linux_fallback_keeps_stdin_and_uses_the_separate_finite_envelope() {
    let fixture = Fixture::new(json!([plan(8192)]));
    let envelope = 104_630_093_824_u64;
    for physical in [None, Some(35 * 1024 * 1024 * 1024_u64)] {
        let mut command = linux_isolated_worker_command(
            Path::new("/bin/sh"),
            physical,
            envelope,
            &fixture.root.join("no-cgroup-delegation"),
        );
        command
            .arg("-c")
            .arg(r#"read line; printf '%s\n' "$line"; ulimit -v"#)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"preserved-input\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        let mut lines = text.lines();
        let report: IsolatedContainmentReport =
            serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(report.mode, "linux-rlimit-as");
        assert_eq!(report.address_space_limit_bytes, Some(envelope));
        assert_eq!(report.physical_limit_bytes, None);
        assert_eq!(report.cgroup_path, None);
        assert_eq!(lines.next(), Some("preserved-input"));
        assert_eq!(
            lines.next().unwrap().parse::<u64>().unwrap(),
            envelope / 1024
        );
    }
}

#[test]
fn isolated_linux_command_establishes_membership_before_exec_without_backgrounding() {
    let command = linux_isolated_worker_command(
        Path::new("/usr/bin/python3"),
        Some(35 * 1024 * 1024 * 1024),
        104_630_093_824,
        Path::new("/sys/fs/cgroup"),
    );
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>();
    let script = &args[1];
    assert!(
        script.find(r#"echo "$$" > "$cg/cgroup.procs""#).unwrap()
            < script.find(r#"exec "$@""#).unwrap()
    );
    assert!(!script.contains(r#""$@" &"#));
    assert!(script.contains(r#"ulimit -v "$address_kib""#));
    assert!(script.contains("physical_cap=null"));
    assert_eq!(args[3], (35 * 1024 * 1024 * 1024_u64).to_string());
    assert_eq!(args[4], (104_630_093_824_u64 / 1024).to_string());
}

#[test]
fn isolated_admission_requires_count_and_one_sequence_per_worker() {
    let mut config = LoadConfig::vllm_safetensors("/tmp/checkpoint");
    config.vllm_generation_topology = Some(VllmGenerationTopology::IsolatedWorkers);
    assert!(validate_load_config(&config).is_err());
    config.vllm_worker_address_space_limit_bytes = Some(104_630_093_824);
    config.vllm_concurrent_generation_capacity = Some(0);
    assert!(validate_load_config(&config).is_err());
    for count in [1, 3] {
        config.vllm_concurrent_generation_capacity = Some(count);
        validate_load_config(&config).unwrap();
        assert_eq!(effective_vllm_max_num_seqs(&config), 1);
    }
    for invalid in [0, 2, 4] {
        config.vllm_max_num_seqs = Some(invalid);
        assert!(validate_load_config(&config).is_err());
    }
    config.vllm_max_num_seqs = Some(1);
    for invalid in [0, 1, 3071] {
        config.memory_limit_bytes = Some(invalid);
        assert!(validate_load_config(&config).is_err());
    }
    config.memory_limit_bytes = Some(3072);
    validate_load_config(&config).unwrap();
    config.artifact.format = ArtifactFormat::Gguf;
    assert!(validate_load_config(&config).is_err());
}

#[test]
fn limiter_slots_are_exclusive_cancellable_and_have_a_capacity_floor() {
    let cancellation = CancellationToken::new();
    let floor = Arc::new(GenerationLimiter::new(0));
    assert_eq!(floor.capacity(), 1);
    assert_eq!(floor.acquire(&cancellation).unwrap().slot, 0);
    let limiter = Arc::new(GenerationLimiter::new(3));
    let first = limiter.acquire(&cancellation).unwrap();
    let second = limiter.acquire(&cancellation).unwrap();
    let third = limiter.acquire(&cancellation).unwrap();
    assert_eq!([first.slot, second.slot, third.slot], [0, 1, 2]);
    let waiting = Arc::clone(&limiter);
    let waiting_cancellation = cancellation.clone();
    let (sender, receiver) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let result = waiting.acquire(&waiting_cancellation);
        sender
            .send(matches!(result, Err(EngineError::Cancelled)))
            .unwrap();
    });
    assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
    cancellation.cancel();
    assert!(receiver.recv_timeout(Duration::from_secs(1)).unwrap());
    waiter.join().unwrap();
    drop(second);
    assert_eq!(limiter.acquire(&CancellationToken::new()).unwrap().slot, 1);
}

#[test]
fn isolated_load_is_sequential_and_reports_each_capacity_and_containment_share() {
    let fixture = Fixture::new(json!([plan(4096), plan(6144), plan(40960)]));
    let mut backend = fixture.backend();
    let mut config = fixture.config(3);
    let child_limit = 256 * 1024 * 1024_u64;
    config.memory_limit_bytes = Some(child_limit * 3 + 2);
    backend.load(config).unwrap();
    let pids = backend.process_ids();
    assert_eq!(pids.len(), 3);
    assert_eq!(pids.iter().collect::<HashSet<_>>().len(), 3);
    assert!(backend.component_healthy());
    assert_eq!(backend.tokenize("check").unwrap().token_ids, [1, 2]);
    assert_eq!(
        backend.concurrent_generation_backend().unwrap().capacity(),
        3
    );
    let evidence = backend.loaded_backend_evidence().unwrap();
    let generation = &evidence["generation"];
    assert_eq!(generation["topology"], "isolated_workers");
    assert_eq!(generation["worker_count"], 3);
    assert_eq!(generation["capacity"], 3);
    assert!(generation.get("runtime_kv_token_capacity").is_none());
    assert!(generation.get("runtime_full_context_capacity").is_none());
    assert_eq!(evidence["memory_limit_bytes"], child_limit * 3 + 2);
    for (index, tokens) in [4096, 6144, 40960].into_iter().enumerate() {
        let info = &generation["per_worker"][index];
        assert_eq!(info["capacity"], 1);
        assert_eq!(info["runtime_kv_token_capacity"], tokens);
        assert_eq!(info["process_id"], pids[index]);
        assert_eq!(info["healthy"], true);
        assert_eq!(info["memory_limit_bytes"], child_limit);
        assert_eq!(
            info["vllm_worker_address_space_limit_bytes"],
            104_630_093_824_u64
        );
        assert!(info["containment"]["mode"].as_str().is_some());
        #[cfg(target_os = "macos")]
        {
            assert_eq!(info["containment"]["mode"], "macos-provider-watchdog");
            assert!(info["containment"]["physical_limit_bytes"].is_null());
            assert!(info["containment"]["address_space_limit_bytes"].is_null());
        }
        assert_eq!(info["vllm_max_num_seqs"], 1);
        assert_eq!(info["vllm_gpu_memory_utilization_pct"], 31);
        assert_eq!(info["vllm_gpu_memory_utilization_floor_pct"], 26);
        let spawn = fixture.read(&format!("spawn-{index}.json"));
        assert_eq!(spawn["memory_limit"], child_limit.to_string());
        assert_eq!(spawn["previous_loaded"], true);
        let load = fixture.read(&format!("load-{index}.json"));
        assert_eq!(load["payload"], info["load_payload"]);
        assert_eq!(load["payload"]["max_batch_size"], 1);
        assert_eq!(load["payload"]["gpu_memory_utilization"], 0.31);
    }
    drop(backend);
    fixture.assert_exited(3);
}

#[test]
fn isolated_partial_failure_never_aggregates_kv_or_keeps_fewer_workers() {
    let missing = json!({"load": {"ok": true, "result": {"n_vocab": 32000, "n_ctx_train": 4096}}});
    let failure = json!({"load": {"ok": false, "error": "mock load failed"}});
    for second in [plan(4095), missing, failure] {
        let fixture = Fixture::new(json!([plan(40960), second]));
        let mut backend = fixture.backend();
        assert!(backend.load(fixture.config(3)).is_err());
        assert!(backend.process_ids().is_empty());
        assert!(backend.loaded_backend_evidence().is_none());
        assert!(backend.concurrent_generation_backend().is_none());
        assert!(backend.loaded.is_none());
        fixture.assert_exited(2);
    }
    let fixture = Fixture::new(json!([plan(4095)]));
    let mut backend = fixture.backend();
    assert!(backend.load(fixture.config(1)).is_err());
    fixture.assert_exited(1);
}

#[test]
fn isolated_backoff_is_per_worker_and_cleans_up_at_the_floor() {
    let oom = json!({"load": {"ok": false, "error": "CUDA out of memory"}});
    for succeeds in [true, false] {
        let last = if succeeds { plan(4096) } else { oom.clone() };
        let fixture = Fixture::new(json!([plan(8192), oom, last]));
        let mut backend = fixture.backend();
        let result = backend.load(fixture.config(2));
        assert_eq!(result.is_ok(), succeeds);
        for (index, pct) in [0.31, 0.31, 0.26].into_iter().enumerate() {
            let load = fixture.read(&format!("load-{index}.json"));
            assert_eq!(load["payload"]["gpu_memory_utilization"], pct);
            assert_eq!(load["payload"]["ctx_size"], 4096);
        }
        if succeeds {
            let evidence = backend.loaded_backend_evidence().unwrap();
            assert_eq!(evidence["generation"]["capacity"], 2);
            assert_eq!(
                evidence["generation"]["per_worker"][0]["vllm_gpu_memory_utilization_pct"],
                31
            );
            assert_eq!(
                evidence["generation"]["per_worker"][1]["vllm_gpu_memory_utilization_pct"],
                26
            );
        } else {
            assert!(backend.process_ids().is_empty());
            assert!(backend.loaded_backend_evidence().is_none());
        }
        drop(backend);
        fixture.assert_exited(3);
    }
}

#[test]
fn isolated_dispatch_cancels_waiters_and_only_the_leased_worker() {
    let fixture = Fixture::new(json!([plan(8192)]));
    let mut backend = fixture.backend();
    backend.load(fixture.config(2)).unwrap();
    let handle = backend.concurrent_generation_backend().unwrap();
    let cancellation = CancellationToken::new();
    let first = generate(Arc::clone(&handle), "hold-first", cancellation.clone());
    let first_seen = fixture.read("seen-hold-first.json");
    let second = generate(Arc::clone(&handle), "hold-second", CancellationToken::new());
    let second_seen = fixture.read("seen-hold-second.json");
    assert_ne!(first_seen["pid"], second_seen["pid"]);
    let waiting_cancellation = CancellationToken::new();
    let waiting = generate(Arc::clone(&handle), "waiting", waiting_cancellation.clone());
    thread::sleep(Duration::from_millis(50));
    waiting_cancellation.cancel();
    assert!(matches!(
        waiting.join().unwrap(),
        Err(EngineError::Cancelled)
    ));
    assert!(!fixture.root.join("seen-waiting.json").exists());
    cancellation.cancel();
    assert!(matches!(first.join().unwrap(), Err(EngineError::Cancelled)));
    let cancel = fixture.read("cancel-0.json");
    assert_eq!(cancel["payload"]["request_id"], first_seen["request"]["id"]);
    assert!(!fixture.root.join("cancel-1.json").exists());
    let reuse = generate(Arc::clone(&handle), "reuse", CancellationToken::new());
    assert_eq!(
        reuse.join().unwrap().unwrap().text,
        format!("{}:reuse", first_seen["pid"])
    );
    fixture.release("hold-second");
    assert_eq!(
        second.join().unwrap().unwrap().text,
        format!("{}:hold-second", second_seen["pid"])
    );
    assert!(backend.component_healthy());
}

#[test]
fn isolated_abort_failure_quarantines_only_its_worker() {
    let mut failed_abort = plan(4096);
    failed_abort["abort_failed"] = json!(true);
    let fixture = Fixture::new(json!([failed_abort, plan(4096)]));
    let mut backend = fixture.backend();
    backend.load(fixture.config(2)).unwrap();
    let handle = backend.concurrent_generation_backend().unwrap();
    let cancellation = CancellationToken::new();
    let first = generate(Arc::clone(&handle), "hold-cancel", cancellation.clone());
    fixture.read("seen-hold-cancel.json");
    let second = generate(handle, "hold-survivor", CancellationToken::new());
    fixture.read("seen-hold-survivor.json");
    cancellation.cancel();
    assert!(first
        .join()
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("quarantined"));
    fixture.release("hold-survivor");
    assert!(second.join().unwrap().is_ok());
    assert!(!backend.component_healthy());
    let evidence = backend.loaded_backend_evidence().unwrap();
    assert_eq!(evidence["generation"]["capacity"], 2);
    assert_eq!(evidence["generation"]["per_worker"][0]["healthy"], false);
    assert_eq!(evidence["generation"]["per_worker"][1]["healthy"], true);
    assert!(backend
        .generate(
            GenerateRequest::new("no-fallback"),
            &mut |_| Ok(()),
            &CancellationToken::new()
        )
        .is_err());
    assert!(!fixture.root.join("seen-no-fallback.json").exists());
}

#[test]
fn isolated_reload_and_drop_invalidate_handles_and_own_processes() {
    let mut with_descendant = plan(16384);
    with_descendant["descendant"] = json!(true);
    let fixture = Fixture::new(json!([with_descendant]));
    let mut backend = fixture.backend();
    backend.load(fixture.config(2)).unwrap();
    let stale = backend.concurrent_generation_backend().unwrap();
    let old_workers = backend.isolated_workers.clone();
    backend.load(fixture.config(1)).unwrap();
    assert_eq!(backend.process_ids().len(), 1);
    assert!(backend.concurrent_generation_backend().is_none());
    for worker in old_workers {
        assert!(!worker.component_healthy());
    }
    for tokens in [0, 1] {
        let mut request = GenerateRequest::new("stale");
        request.max_new_tokens = tokens;
        assert!(matches!(
            stale.generate(request, &mut |_| Ok(()), &CancellationToken::new()),
            Err(EngineError::NotLoaded)
        ));
    }
    let current = Arc::clone(backend.concurrent_generation.as_ref().unwrap());
    assert_eq!(current.capacity(), 1);
    drop(backend);
    assert!(matches!(
        current.generate(
            GenerateRequest::new("dropped"),
            &mut |_| Ok(()),
            &CancellationToken::new()
        ),
        Err(EngineError::NotLoaded)
    ));
    fixture.assert_exited(3);
    for index in 0..3 {
        assert_process_exited(
            fixture
                .read(&format!("descendant-{index}.json"))
                .as_u64()
                .unwrap(),
        );
    }
}

#[test]
fn isolated_execution_report_is_checked_for_every_worker() {
    let mut reported = plan(8192);
    reported["load"]["result"]["execution"] = json!({"vllm_enforce_eager": false});
    let fixture = Fixture::new(json!([reported, plan(8192)]));
    let mut backend = fixture.backend();
    let mut config = fixture.config(2);
    config.vllm_enforce_eager = Some(false);
    let error = backend.load(config).unwrap_err();
    assert!(error
        .to_string()
        .contains("did not report effective execution"));
    assert!(backend.process_ids().is_empty());
    fixture.assert_exited(2);
}

#[test]
fn isolated_drop_terminates_active_requests_without_waiting_for_the_gate() {
    let fixture = Fixture::new(json!([plan(8192)]));
    let mut backend = fixture.backend();
    backend.load(fixture.config(2)).unwrap();
    let handle = backend.concurrent_generation_backend().unwrap();
    let active = generate(handle, "hold-drop", CancellationToken::new());
    fixture.read("seen-hold-drop.json");
    let (sender, receiver) = mpsc::channel();
    let teardown = thread::spawn(move || {
        drop(backend);
        sender.send(()).unwrap();
    });
    receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    teardown.join().unwrap();
    assert!(active.join().unwrap().is_err());
    fixture.assert_exited(2);
}

#[test]
fn isolated_spawn_failure_cleans_up_already_loaded_workers() {
    let mut first = plan(8192);
    first["remove_executable"] = json!(true);
    let fixture = Fixture::new(json!([first]));
    let mut backend = fixture.backend();
    backend
        .load(fixture.config(3))
        .expect_err("missing worker executable");
    assert!(backend.process_ids().is_empty());
    fixture.assert_exited(1);
}

#[test]
fn isolated_failed_reload_invalidates_old_handles_and_cleans_up_both_pools() {
    let failure = json!({"load": {"ok": false, "error": "mock reload failed"}});
    let fixture = Fixture::new(json!([plan(8192), plan(8192), failure]));
    let mut backend = fixture.backend();
    backend.load(fixture.config(2)).unwrap();
    let stale = backend.concurrent_generation_backend().unwrap();
    assert!(backend.load(fixture.config(3)).is_err());
    assert!(backend.process_ids().is_empty());
    assert!(backend.loaded_backend_evidence().is_none());
    assert!(matches!(
        stale.generate(
            GenerateRequest::new("stale"),
            &mut |_| Ok(()),
            &CancellationToken::new()
        ),
        Err(EngineError::NotLoaded)
    ));
    fixture.assert_exited(3);
}

#[test]
fn isolated_topology_transitions_preserve_shared_dispatch_and_gate_reload() {
    let fixture = Fixture::new(json!([plan(8192)]));
    let mut backend = fixture.backend();
    let mut shared = fixture.config(2);
    shared.vllm_generation_topology = Some(VllmGenerationTopology::SharedWorker);
    shared.vllm_worker_address_space_limit_bytes = None;
    shared.vllm_max_num_seqs = Some(2);
    backend.load(shared.clone()).unwrap();
    assert_eq!(backend.process_ids().len(), 1);
    let shared_handle = backend.concurrent_generation_backend().unwrap();
    assert_eq!(shared_handle.capacity(), 2);
    let evidence = backend.loaded_backend_evidence().unwrap();
    assert_eq!(evidence["generation"]["topology"], "shared_worker");
    assert_eq!(evidence["generation"]["worker_count"], 1);
    backend.load(fixture.config(2)).unwrap();
    assert!(matches!(
        shared_handle.generate(
            GenerateRequest::new("stale-shared"),
            &mut |_| Ok(()),
            &CancellationToken::new()
        ),
        Err(EngineError::NotLoaded)
    ));
    let isolated_handle = backend.concurrent_generation_backend().unwrap();
    let active = generate(
        Arc::clone(&isolated_handle),
        "hold-reload",
        CancellationToken::new(),
    );
    fixture.read("seen-hold-reload.json");
    let (sender, receiver) = mpsc::channel();
    let reload = thread::spawn(move || {
        shared.vllm_generation_topology = None;
        backend.load(shared).unwrap();
        sender.send(backend).unwrap();
    });
    assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(!fixture.root.join("spawn-3.json").exists());
    fixture.release("hold-reload");
    active.join().unwrap().unwrap();
    let backend = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    reload.join().unwrap();
    assert_eq!(backend.process_ids().len(), 1);
    assert_eq!(
        backend.concurrent_generation_backend().unwrap().capacity(),
        2
    );
    assert!(backend.loaded_backend_evidence().unwrap()["generation"]
        .get("topology")
        .is_none());
    assert!(matches!(
        isolated_handle.generate(
            GenerateRequest::new("stale-isolated"),
            &mut |_| Ok(()),
            &CancellationToken::new()
        ),
        Err(EngineError::NotLoaded)
    ));
    drop(backend);
    fixture.assert_exited(4);
}
