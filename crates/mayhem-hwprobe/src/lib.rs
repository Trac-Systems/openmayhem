#![forbid(unsafe_code)]

use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const CRATE_NAME: &str = "mayhem-hwprobe";

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ProbeOptions {
    pub disk_path: PathBuf,
    pub run_disk_bench: bool,
    pub disk_bench_mib: u64,
    pub fixture: Option<FixtureProfile>,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        Self {
            disk_path: env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            run_disk_bench: true,
            disk_bench_mib: 16,
            fixture: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureProfile {
    AppleSilicon,
    LinuxNvidia,
    CpuOnly,
}

impl FixtureProfile {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "apple-silicon" | "apple" | "mac" | "macos" => Some(Self::AppleSilicon),
            "linux-nvidia" | "nvidia" | "blackwell" => Some(Self::LinuxNvidia),
            "cpu-only" | "cpu" => Some(Self::CpuOnly),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppleSilicon => "apple-silicon",
            Self::LinuxNvidia => "linux-nvidia",
            Self::CpuOnly => "cpu-only",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HardwareReport {
    pub schema_version: u32,
    pub source: ReportSource,
    pub host: HostInfo,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub disk: DiskInfo,
    pub gpus: Vec<GpuInfo>,
    pub tee: TeeInfo,
    pub backend_verdicts: Vec<BackendVerdict>,
    pub selected_backend: Option<String>,
    pub summary: SummaryVerdict,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReportSource {
    pub kind: String,
    pub fixture: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostInfo {
    pub os: String,
    pub arch: String,
    pub family: String,
    pub kernel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CpuInfo {
    pub model: Option<String>,
    pub physical_cores: Option<usize>,
    pub logical_cores: usize,
    pub flags: CpuFlags,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CpuFlags {
    pub avx2: bool,
    pub avx512: bool,
    pub neon: bool,
    pub amx: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub available_bytes: Option<u64>,
    pub unified_memory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiskInfo {
    pub path: PathBuf,
    pub free_bytes: Option<u64>,
    pub write_mib_s: Option<f64>,
    pub bench_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GpuInfo {
    pub vendor: GpuVendor,
    pub name: String,
    pub backend: GpuBackend,
    pub memory_bytes: Option<u64>,
    pub unified_memory: bool,
    pub compute_capability: Option<String>,
    pub supports_nvfp4: bool,
    pub supports_fp8: bool,
    pub supports_tensor_parallel: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    Apple,
    Nvidia,
    Amd,
    Vulkan,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GpuBackend {
    Metal,
    Nvml,
    Rocm,
    Vulkan,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeeInfo {
    pub tier: u8,
    pub sev_snp: bool,
    pub tdx: bool,
    pub gpu_confidential_compute: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendVerdict {
    pub backend: String,
    pub status: VerdictStatus,
    pub reason: Option<String>,
    pub est_tok_s: Option<f64>,
    pub n_layers_gpu: Option<u32>,
    pub max_sessions: u32,
    pub kv_cache_bytes_budget: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerdictStatus {
    FullOffload,
    PartialOffload,
    CpuOnly,
    Insufficient,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SummaryVerdict {
    pub can_serve: bool,
    pub status: VerdictStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct HardwareProfile {
    source: ReportSource,
    host: HostInfo,
    cpu: CpuInfo,
    memory: MemoryInfo,
    disk: DiskInfo,
    gpus: Vec<GpuInfo>,
    tee: TeeInfo,
    warnings: Vec<String>,
}

pub fn probe(options: ProbeOptions) -> HardwareReport {
    let profile = match options.fixture {
        Some(fixture) => fixture_profile(fixture, &options.disk_path),
        None => probe_host(options),
    };
    report_from_profile(profile)
}

fn report_from_profile(profile: HardwareProfile) -> HardwareReport {
    let backend_verdicts = compute_backend_verdicts(&profile);
    let selected_backend = backend_verdicts
        .iter()
        .find(|verdict| verdict.status != VerdictStatus::Insufficient)
        .map(|verdict| verdict.backend.clone());
    let summary = backend_verdicts
        .iter()
        .find(|verdict| verdict.status != VerdictStatus::Insufficient)
        .map(|verdict| SummaryVerdict {
            can_serve: true,
            status: verdict.status.clone(),
            reason: verdict.reason.clone(),
        })
        .unwrap_or_else(|| SummaryVerdict {
            can_serve: false,
            status: VerdictStatus::Insufficient,
            reason: Some("no eligible backend found".to_owned()),
        });

    HardwareReport {
        schema_version: 1,
        source: profile.source,
        host: profile.host,
        cpu: profile.cpu,
        memory: profile.memory,
        disk: profile.disk,
        gpus: profile.gpus,
        tee: profile.tee,
        backend_verdicts,
        selected_backend,
        summary,
        warnings: profile.warnings,
    }
}

fn compute_backend_verdicts(profile: &HardwareProfile) -> Vec<BackendVerdict> {
    vec![
        trt_llm_verdict(profile),
        mlx_verdict(profile),
        llama_cpp_verdict(profile),
        stable_diffusion_cpp_verdict(profile),
        whisper_cpp_verdict(profile),
        piper_verdict(profile),
    ]
}

fn trt_llm_verdict(profile: &HardwareProfile) -> BackendVerdict {
    let nvidia = profile
        .gpus
        .iter()
        .filter(|gpu| gpu.vendor == GpuVendor::Nvidia)
        .collect::<Vec<_>>();
    if nvidia.is_empty() {
        return insufficient("trt-llm", "no NVIDIA GPU detected");
    }

    let total_vram = nvidia
        .iter()
        .filter_map(|gpu| gpu.memory_bytes)
        .sum::<u64>();
    let best_cc = nvidia
        .iter()
        .filter_map(|gpu| gpu.compute_capability.as_deref())
        .filter_map(parse_compute_capability)
        .fold(None, |best: Option<(u32, u32)>, cc| {
            Some(best.map_or(cc, |best| best.max(cc)))
        });
    let Some(cc) = best_cc else {
        return BackendVerdict {
            backend: "trt-llm".to_owned(),
            status: VerdictStatus::PartialOffload,
            reason: Some("NVIDIA GPU detected but compute capability is unknown".to_owned()),
            est_tok_s: Some(28.0),
            n_layers_gpu: Some(24),
            max_sessions: 2,
            kv_cache_bytes_budget: total_vram / 4,
        };
    };

    let tensor_parallel = nvidia.first().is_some_and(|first| {
        nvidia.len() >= 2
            && nvidia.iter().all(|gpu| {
                gpu.name == first.name
                    && gpu.compute_capability == first.compute_capability
                    && gpu.memory_bytes == first.memory_bytes
            })
    });
    let supports_nvfp4 = cc >= (10, 0);
    let supports_fp8 = cc >= (8, 9);
    if supports_nvfp4 && total_vram >= 24 * GIB {
        BackendVerdict {
            backend: "trt-llm".to_owned(),
            status: VerdictStatus::FullOffload,
            reason: Some(if tensor_parallel {
                "Blackwell-class NVIDIA GPU set with NVFP4 and tensor parallel eligibility"
                    .to_owned()
            } else {
                "Blackwell-class NVIDIA GPU with NVFP4".to_owned()
            }),
            est_tok_s: Some(if tensor_parallel { 260.0 } else { 160.0 }),
            n_layers_gpu: None,
            max_sessions: if tensor_parallel { 12 } else { 8 },
            kv_cache_bytes_budget: total_vram / 2,
        }
    } else if supports_fp8 && total_vram >= 16 * GIB {
        BackendVerdict {
            backend: "trt-llm".to_owned(),
            status: VerdictStatus::FullOffload,
            reason: Some("FP8-capable NVIDIA GPU".to_owned()),
            est_tok_s: Some(95.0),
            n_layers_gpu: None,
            max_sessions: 6,
            kv_cache_bytes_budget: total_vram / 2,
        }
    } else {
        BackendVerdict {
            backend: "trt-llm".to_owned(),
            status: VerdictStatus::PartialOffload,
            reason: Some(
                "NVIDIA GPU lacks launch quantization target or has limited VRAM".to_owned(),
            ),
            est_tok_s: Some(35.0),
            n_layers_gpu: Some(24),
            max_sessions: 2,
            kv_cache_bytes_budget: total_vram / 3,
        }
    }
}

fn mlx_verdict(profile: &HardwareProfile) -> BackendVerdict {
    let apple_metal = profile
        .gpus
        .iter()
        .find(|gpu| gpu.vendor == GpuVendor::Apple && gpu.backend == GpuBackend::Metal);
    let Some(gpu) = apple_metal else {
        return insufficient("mlx", "no Apple Metal unified-memory GPU detected");
    };

    let memory = gpu.memory_bytes.unwrap_or(profile.memory.total_bytes);
    if memory >= 16 * GIB {
        BackendVerdict {
            backend: "mlx".to_owned(),
            status: VerdictStatus::FullOffload,
            reason: Some("Apple Silicon unified memory supports MLX artifact serving".to_owned()),
            est_tok_s: Some(if memory >= 64 * GIB { 75.0 } else { 42.0 }),
            n_layers_gpu: None,
            max_sessions: if memory >= 64 * GIB { 6 } else { 3 },
            kv_cache_bytes_budget: memory / 3,
        }
    } else {
        BackendVerdict {
            backend: "mlx".to_owned(),
            status: VerdictStatus::PartialOffload,
            reason: Some("Apple Metal is present but unified memory is tight".to_owned()),
            est_tok_s: Some(18.0),
            n_layers_gpu: Some(20),
            max_sessions: 1,
            kv_cache_bytes_budget: memory / 4,
        }
    }
}

fn llama_cpp_verdict(profile: &HardwareProfile) -> BackendVerdict {
    let total_gpu_mem = profile
        .gpus
        .iter()
        .filter(|gpu| gpu.vendor != GpuVendor::Apple)
        .filter_map(|gpu| gpu.memory_bytes)
        .sum::<u64>();
    let has_accel = profile.gpus.iter().any(|gpu| {
        matches!(
            gpu.backend,
            GpuBackend::Metal | GpuBackend::Nvml | GpuBackend::Rocm | GpuBackend::Vulkan
        )
    });

    if has_accel && total_gpu_mem >= 8 * GIB {
        BackendVerdict {
            backend: "llama.cpp".to_owned(),
            status: VerdictStatus::PartialOffload,
            reason: Some(
                "GPU acceleration available for GGUF partial/full layer offload".to_owned(),
            ),
            est_tok_s: Some(26.0),
            n_layers_gpu: Some(28),
            max_sessions: 2,
            kv_cache_bytes_budget: total_gpu_mem / 3,
        }
    } else if has_accel && profile.memory.total_bytes >= 16 * GIB {
        BackendVerdict {
            backend: "llama.cpp".to_owned(),
            status: VerdictStatus::PartialOffload,
            reason: Some(
                "accelerator detected but dedicated VRAM is unknown or limited".to_owned(),
            ),
            est_tok_s: Some(16.0),
            n_layers_gpu: Some(12),
            max_sessions: 1,
            kv_cache_bytes_budget: profile
                .memory
                .available_bytes
                .unwrap_or(profile.memory.total_bytes)
                / 4,
        }
    } else if profile.memory.total_bytes >= 8 * GIB {
        BackendVerdict {
            backend: "llama.cpp".to_owned(),
            status: VerdictStatus::CpuOnly,
            reason: Some(cpu_reason(&profile.cpu)),
            est_tok_s: Some(if profile.cpu.flags.avx2 || profile.cpu.flags.neon {
                7.0
            } else {
                3.5
            }),
            n_layers_gpu: Some(0),
            max_sessions: 1,
            kv_cache_bytes_budget: profile
                .memory
                .available_bytes
                .unwrap_or(profile.memory.total_bytes)
                / 5,
        }
    } else {
        insufficient(
            "llama.cpp",
            "less than 8 GiB RAM available for baseline GGUF serving",
        )
    }
}

fn stable_diffusion_cpp_verdict(profile: &HardwareProfile) -> BackendVerdict {
    if profile.memory.total_bytes < 4 * GIB {
        return insufficient(
            "stable-diffusion.cpp",
            "less than 4 GiB RAM available for small diffusion serving",
        );
    }
    let has_accel = profile.gpus.iter().any(|gpu| {
        matches!(
            gpu.backend,
            GpuBackend::Metal | GpuBackend::Nvml | GpuBackend::Rocm | GpuBackend::Vulkan
        )
    });
    if has_accel {
        BackendVerdict {
            backend: "stable-diffusion.cpp".to_owned(),
            status: VerdictStatus::PartialOffload,
            reason: Some("local accelerator available for stable-diffusion.cpp".to_owned()),
            est_tok_s: None,
            n_layers_gpu: None,
            max_sessions: 1,
            kv_cache_bytes_budget: profile
                .memory
                .available_bytes
                .unwrap_or(profile.memory.total_bytes)
                / 4,
        }
    } else {
        BackendVerdict {
            backend: "stable-diffusion.cpp".to_owned(),
            status: VerdictStatus::CpuOnly,
            reason: Some(cpu_reason(&profile.cpu)),
            est_tok_s: None,
            n_layers_gpu: Some(0),
            max_sessions: 1,
            kv_cache_bytes_budget: profile
                .memory
                .available_bytes
                .unwrap_or(profile.memory.total_bytes)
                / 5,
        }
    }
}

fn whisper_cpp_verdict(profile: &HardwareProfile) -> BackendVerdict {
    if profile.memory.total_bytes < GIB {
        return insufficient(
            "whisper.cpp",
            "less than 1 GiB RAM available for small Whisper serving",
        );
    }
    BackendVerdict {
        backend: "whisper.cpp".to_owned(),
        status: VerdictStatus::CpuOnly,
        reason: Some(cpu_reason(&profile.cpu)),
        est_tok_s: None,
        n_layers_gpu: Some(0),
        max_sessions: 2,
        kv_cache_bytes_budget: profile
            .memory
            .available_bytes
            .unwrap_or(profile.memory.total_bytes)
            / 8,
    }
}

fn piper_verdict(profile: &HardwareProfile) -> BackendVerdict {
    if profile.memory.total_bytes < GIB {
        return insufficient(
            "piper",
            "less than 1 GiB RAM available for small Piper TTS serving",
        );
    }
    BackendVerdict {
        backend: "piper".to_owned(),
        status: VerdictStatus::CpuOnly,
        reason: Some(cpu_reason(&profile.cpu)),
        est_tok_s: None,
        n_layers_gpu: Some(0),
        max_sessions: 2,
        kv_cache_bytes_budget: profile
            .memory
            .available_bytes
            .unwrap_or(profile.memory.total_bytes)
            / 8,
    }
}

fn insufficient(backend: &str, reason: &str) -> BackendVerdict {
    BackendVerdict {
        backend: backend.to_owned(),
        status: VerdictStatus::Insufficient,
        reason: Some(reason.to_owned()),
        est_tok_s: None,
        n_layers_gpu: None,
        max_sessions: 0,
        kv_cache_bytes_budget: 0,
    }
}

fn cpu_reason(cpu: &CpuInfo) -> String {
    if cpu.flags.avx2 {
        "CPU-only fallback with AVX2".to_owned()
    } else if cpu.flags.neon {
        "CPU-only fallback with NEON".to_owned()
    } else {
        "CPU-only fallback without preferred SIMD flags".to_owned()
    }
}

fn probe_host(options: ProbeOptions) -> HardwareProfile {
    let mut warnings = Vec::new();
    let host = HostInfo {
        os: env::consts::OS.to_owned(),
        arch: env::consts::ARCH.to_owned(),
        family: env::consts::FAMILY.to_owned(),
        kernel: command_stdout("uname", &["-r"]).or_else(|| command_stdout("cmd", &["/C", "ver"])),
    };
    let cpu = probe_cpu(&host);
    let memory = probe_memory(&host);
    let disk = probe_disk(
        &options.disk_path,
        options.run_disk_bench,
        options.disk_bench_mib,
    );
    if disk.free_bytes.is_none() {
        warnings.push("disk free space probe unavailable".to_owned());
    }
    if options.run_disk_bench && disk.write_mib_s.is_none() {
        warnings.push("disk write benchmark unavailable".to_owned());
    }

    let mut gpus = Vec::new();
    gpus.extend(probe_apple_metal(&host, &memory));
    gpus.extend(probe_nvidia());
    gpus.extend(probe_rocm());
    gpus.extend(probe_vulkan());
    dedupe_gpus(&mut gpus);

    let tee = probe_tee(&gpus);
    HardwareProfile {
        source: ReportSource {
            kind: "host".to_owned(),
            fixture: None,
        },
        host,
        cpu,
        memory,
        disk,
        gpus,
        tee,
        warnings,
    }
}

fn probe_cpu(host: &HostInfo) -> CpuInfo {
    let logical_cores = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    let model = cpu_model(host);
    let physical_cores = physical_core_count(host);
    let flags = cpu_flags(host);
    CpuInfo {
        model,
        physical_cores,
        logical_cores,
        flags,
    }
}

fn cpu_model(host: &HostInfo) -> Option<String> {
    if host.os == "linux" {
        parse_proc_cpuinfo_value("model name")
            .or_else(|| parse_proc_cpuinfo_value("Hardware"))
            .or_else(|| parse_proc_cpuinfo_value("Processor"))
    } else if host.os == "macos" {
        command_stdout("sysctl", &["-n", "machdep.cpu.brand_string"])
            .or_else(|| command_stdout("sysctl", &["-n", "hw.model"]))
    } else if host.os == "windows" {
        env::var("PROCESSOR_IDENTIFIER").ok()
    } else {
        None
    }
}

fn physical_core_count(host: &HostInfo) -> Option<usize> {
    if host.os == "macos" {
        command_stdout("sysctl", &["-n", "hw.physicalcpu"])
            .and_then(|value| value.parse::<usize>().ok())
    } else if host.os == "linux" {
        let text = fs::read_to_string("/proc/cpuinfo").ok()?;
        let mut ids = std::collections::BTreeSet::new();
        let mut physical = None::<String>;
        let mut core = None::<String>;
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("physical id") {
                physical = value.split(':').nth(1).map(|value| value.trim().to_owned());
            } else if let Some(value) = line.strip_prefix("core id") {
                core = value.split(':').nth(1).map(|value| value.trim().to_owned());
            } else if line.trim().is_empty() {
                if let (Some(physical), Some(core)) = (physical.take(), core.take()) {
                    ids.insert(format!("{physical}:{core}"));
                }
            }
        }
        if let (Some(physical), Some(core)) = (physical.take(), core.take()) {
            ids.insert(format!("{physical}:{core}"));
        }
        (!ids.is_empty()).then_some(ids.len())
    } else {
        None
    }
}

fn cpu_flags(host: &HostInfo) -> CpuFlags {
    let mut flags = CpuFlags {
        avx2: cfg!(target_feature = "avx2"),
        avx512: cfg!(target_feature = "avx512f"),
        neon: cfg!(target_feature = "neon"),
        amx: false,
    };

    if host.os == "linux" {
        if let Ok(text) = fs::read_to_string("/proc/cpuinfo") {
            let lower = text.to_ascii_lowercase();
            flags.avx2 |= lower.contains(" avx2") || lower.contains("\tavx2");
            flags.avx512 |= lower.contains(" avx512");
            flags.neon |= lower.contains(" neon") || lower.contains(" asimd");
            flags.amx |= lower.contains(" amx_");
        }
    } else if host.os == "macos" {
        let text = command_stdout("sysctl", &["-a"])
            .unwrap_or_default()
            .to_ascii_lowercase();
        flags.avx2 |= text.contains("avx2");
        flags.avx512 |= text.contains("avx512");
        flags.amx |= text.contains("amx");
        flags.neon |= host.arch == "aarch64" || host.arch == "arm64";
    } else if host.arch == "aarch64" || host.arch == "arm64" {
        flags.neon = true;
    }
    flags
}

fn parse_proc_cpuinfo_value(key: &str) -> Option<String> {
    let text = fs::read_to_string("/proc/cpuinfo").ok()?;
    text.lines().find_map(|line| {
        let (line_key, value) = line.split_once(':')?;
        line_key
            .trim()
            .eq_ignore_ascii_case(key)
            .then_some(value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn probe_memory(host: &HostInfo) -> MemoryInfo {
    if host.os == "linux" {
        let text = fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let total = meminfo_kib(&text, "MemTotal").unwrap_or(0) * 1024;
        let available = meminfo_kib(&text, "MemAvailable").map(|kib| kib * 1024);
        MemoryInfo {
            total_bytes: total,
            available_bytes: available,
            unified_memory: false,
        }
    } else if host.os == "macos" {
        let total = command_stdout("sysctl", &["-n", "hw.memsize"])
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        MemoryInfo {
            total_bytes: total,
            available_bytes: None,
            unified_memory: host.arch == "aarch64" || host.arch == "arm64",
        }
    } else {
        MemoryInfo {
            total_bytes: 0,
            available_bytes: None,
            unified_memory: false,
        }
    }
}

fn meminfo_kib(text: &str, key: &str) -> Option<u64> {
    let prefix = format!("{key}:");
    text.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
    })
}

fn probe_disk(path: &Path, run_bench: bool, bench_mib: u64) -> DiskInfo {
    let free_bytes = disk_free_bytes(path);
    let (write_mib_s, bench_bytes) = if run_bench {
        let bytes = bench_mib.saturating_mul(MIB);
        match disk_write_bench(path, bytes) {
            Some(value) => (Some(value), Some(bytes)),
            None => (None, None),
        }
    } else {
        (None, None)
    };
    DiskInfo {
        path: path.to_path_buf(),
        free_bytes,
        write_mib_s,
        bench_bytes,
    }
}

fn disk_free_bytes(path: &Path) -> Option<u64> {
    let output = if cfg!(target_os = "windows") {
        command_stdout(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "(Get-PSDrive -Name ((Get-Location).Path.Substring(0,1))).Free",
            ],
        )
    } else {
        command_stdout("df", &["-k", path.to_str().unwrap_or(".")])
    }?;
    if cfg!(target_os = "windows") {
        output.trim().parse::<u64>().ok()
    } else {
        output
            .lines()
            .nth(1)
            .and_then(|line| line.split_whitespace().nth(3))
            .and_then(|kib| kib.parse::<u64>().ok())
            .map(|kib| kib * 1024)
    }
}

fn disk_write_bench(path: &Path, bytes: u64) -> Option<f64> {
    if bytes == 0 {
        return None;
    }
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or_else(|| Path::new("."))
    };
    let bench_path = dir.join(format!(
        ".mayhem-hwprobe-{}-{}.tmp",
        std::process::id(),
        monotonic_nanos()
    ));
    let mut file = fs::File::create(&bench_path).ok()?;
    let chunk = vec![0u8; MIB as usize];
    let start = Instant::now();
    let mut remaining = bytes;
    while remaining > 0 {
        let n = remaining.min(chunk.len() as u64) as usize;
        file.write_all(&chunk[..n]).ok()?;
        remaining -= n as u64;
    }
    file.sync_all().ok()?;
    let elapsed = start.elapsed().as_secs_f64();
    drop(file);
    let _ = fs::remove_file(&bench_path);
    (elapsed > 0.0).then_some((bytes as f64 / MIB as f64) / elapsed)
}

fn monotonic_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn probe_apple_metal(host: &HostInfo, memory: &MemoryInfo) -> Vec<GpuInfo> {
    if host.os != "macos" {
        return Vec::new();
    }
    let is_apple_silicon = host.arch == "aarch64" || host.arch == "arm64";
    if !is_apple_silicon {
        return Vec::new();
    }
    vec![GpuInfo {
        vendor: GpuVendor::Apple,
        name: command_stdout("sysctl", &["-n", "machdep.cpu.brand_string"])
            .or_else(|| command_stdout("sysctl", &["-n", "hw.model"]))
            .unwrap_or_else(|| "Apple Silicon GPU".to_owned()),
        backend: GpuBackend::Metal,
        memory_bytes: Some(memory.total_bytes),
        unified_memory: true,
        compute_capability: None,
        supports_nvfp4: false,
        supports_fp8: false,
        supports_tensor_parallel: false,
    }]
}

fn probe_nvidia() -> Vec<GpuInfo> {
    let Some(output) = command_stdout(
        "nvidia-smi",
        &[
            "--query-gpu=name,memory.total,compute_cap",
            "--format=csv,noheader,nounits",
        ],
    ) else {
        return Vec::new();
    };
    let mut gpus = Vec::new();
    for line in output.lines() {
        let parts = line.split(',').map(str::trim).collect::<Vec<_>>();
        if parts.is_empty() || parts[0].is_empty() {
            continue;
        }
        let memory_bytes = parts
            .get(1)
            .and_then(|value| value.parse::<u64>().ok())
            .map(|mib| mib * MIB);
        let compute_capability = parts
            .get(2)
            .map(|value| value.to_string())
            .filter(|value| value.chars().all(|ch| ch.is_ascii_digit() || ch == '.'));
        let cc = compute_capability
            .as_deref()
            .and_then(parse_compute_capability);
        gpus.push(GpuInfo {
            vendor: GpuVendor::Nvidia,
            name: parts[0].to_owned(),
            backend: GpuBackend::Nvml,
            memory_bytes,
            unified_memory: false,
            compute_capability,
            supports_nvfp4: cc.is_some_and(|cc| cc >= (10, 0)),
            supports_fp8: cc.is_some_and(|cc| cc >= (8, 9)),
            supports_tensor_parallel: false,
        });
    }
    let tensor_parallel = gpus.len() >= 2
        && gpus
            .windows(2)
            .all(|pair| pair[0].compute_capability == pair[1].compute_capability);
    if tensor_parallel {
        for gpu in &mut gpus {
            gpu.supports_tensor_parallel = true;
        }
    }
    gpus
}

fn probe_rocm() -> Vec<GpuInfo> {
    let Some(output) = command_stdout("rocm-smi", &["--showproductname"]) else {
        return Vec::new();
    };
    output
        .lines()
        .filter(|line| line.contains("GPU") && line.contains(':'))
        .map(|line| GpuInfo {
            vendor: GpuVendor::Amd,
            name: line
                .split(':')
                .nth(1)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("AMD ROCm GPU")
                .to_owned(),
            backend: GpuBackend::Rocm,
            memory_bytes: None,
            unified_memory: false,
            compute_capability: None,
            supports_nvfp4: false,
            supports_fp8: false,
            supports_tensor_parallel: false,
        })
        .collect()
}

fn probe_vulkan() -> Vec<GpuInfo> {
    let Some(output) = command_stdout("vulkaninfo", &["--summary"]) else {
        return Vec::new();
    };
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if !(trimmed.starts_with("deviceName") || trimmed.starts_with("GPU id")) {
                return None;
            }
            let name = trimmed
                .split('=')
                .nth(1)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("Vulkan GPU");
            Some(GpuInfo {
                vendor: GpuVendor::Vulkan,
                name: name.to_owned(),
                backend: GpuBackend::Vulkan,
                memory_bytes: None,
                unified_memory: false,
                compute_capability: None,
                supports_nvfp4: false,
                supports_fp8: false,
                supports_tensor_parallel: false,
            })
        })
        .collect()
}

fn dedupe_gpus(gpus: &mut Vec<GpuInfo>) {
    let dedicated_names = gpus
        .iter()
        .filter(|gpu| gpu.backend != GpuBackend::Vulkan)
        .map(|gpu| gpu.name.to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    let mut seen_vulkan = std::collections::BTreeSet::new();
    gpus.retain(|gpu| {
        if gpu.backend != GpuBackend::Vulkan {
            return true;
        }
        let key = gpu.name.to_ascii_lowercase();
        !dedicated_names.contains(&key) && seen_vulkan.insert(key)
    });
}

fn probe_tee(gpus: &[GpuInfo]) -> TeeInfo {
    let sev_snp = path_has_y("/sys/module/kvm_amd/parameters/sev_snp")
        || path_has_y("/sys/module/kvm_amd/parameters/sev");
    let tdx = Path::new("/sys/firmware/tdx").exists()
        || fs::read_to_string("/proc/cpuinfo")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("tdx");
    let gpu_confidential_compute = gpus.iter().any(|gpu| {
        gpu.vendor == GpuVendor::Nvidia
            && command_stdout(
                "nvidia-smi",
                &[
                    "--query-gpu=confidential_compute.current_status",
                    "--format=csv,noheader",
                ],
            )
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("enabled")
    });
    let apple_device_identity = cfg!(target_os = "macos")
        && gpus
            .iter()
            .any(|gpu| gpu.vendor == GpuVendor::Apple && gpu.backend == GpuBackend::Metal);
    let gb10_device_identity = gpus.iter().any(|gpu| {
        gpu.vendor == GpuVendor::Nvidia && {
            let name = gpu.name.to_ascii_lowercase();
            name.contains("gb10") || name.contains("dgx spark")
        }
    });
    let tier = if sev_snp
        || tdx
        || gpu_confidential_compute
        || apple_device_identity
        || gb10_device_identity
    {
        2
    } else {
        1
    };
    let mut notes = Vec::new();
    if sev_snp {
        notes.push("AMD SEV/SEV-SNP signal detected".to_owned());
    }
    if tdx {
        notes.push("Intel TDX signal detected".to_owned());
    }
    if gpu_confidential_compute {
        notes.push("NVIDIA confidential compute enabled".to_owned());
    }
    if apple_device_identity {
        notes.push("Apple Metal device identity can supply Tier 2 App Attest evidence".to_owned());
    }
    if gb10_device_identity {
        notes.push("NVIDIA GB10 device identity can supply Tier 2 evidence".to_owned());
    }
    if notes.is_empty() {
        notes.push("hardware TEE not detected; Tier 1 software-rooted attestation".to_owned());
    }
    TeeInfo {
        tier,
        sev_snp,
        tdx,
        gpu_confidential_compute,
        notes,
    }
}

fn path_has_y(path: &str) -> bool {
    fs::read_to_string(path)
        .map(|value| matches!(value.trim(), "Y" | "y" | "1"))
        .unwrap_or(false)
}

fn parse_compute_capability(value: &str) -> Option<(u32, u32)> {
    let mut parts = value.trim().split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    Some((major, minor))
}

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then_some(trimmed.to_owned())
}

fn fixture_profile(fixture: FixtureProfile, disk_path: &Path) -> HardwareProfile {
    match fixture {
        FixtureProfile::AppleSilicon => HardwareProfile {
            source: ReportSource {
                kind: "fixture".to_owned(),
                fixture: Some(fixture.as_str().to_owned()),
            },
            host: HostInfo {
                os: "macos".to_owned(),
                arch: "aarch64".to_owned(),
                family: "unix".to_owned(),
                kernel: Some("Darwin 25.0.0".to_owned()),
            },
            cpu: CpuInfo {
                model: Some("Apple M3 Max".to_owned()),
                physical_cores: Some(16),
                logical_cores: 16,
                flags: CpuFlags {
                    neon: true,
                    ..CpuFlags::default()
                },
            },
            memory: MemoryInfo {
                total_bytes: 64 * GIB,
                available_bytes: Some(52 * GIB),
                unified_memory: true,
            },
            disk: fixture_disk(disk_path),
            gpus: vec![GpuInfo {
                vendor: GpuVendor::Apple,
                name: "Apple M3 Max GPU".to_owned(),
                backend: GpuBackend::Metal,
                memory_bytes: Some(64 * GIB),
                unified_memory: true,
                compute_capability: None,
                supports_nvfp4: false,
                supports_fp8: false,
                supports_tensor_parallel: false,
            }],
            tee: fixture_tee(2),
            warnings: Vec::new(),
        },
        FixtureProfile::LinuxNvidia => HardwareProfile {
            source: ReportSource {
                kind: "fixture".to_owned(),
                fixture: Some(fixture.as_str().to_owned()),
            },
            host: HostInfo {
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                family: "unix".to_owned(),
                kernel: Some("6.14.0".to_owned()),
            },
            cpu: CpuInfo {
                model: Some("AMD Ryzen Threadripper PRO".to_owned()),
                physical_cores: Some(32),
                logical_cores: 64,
                flags: CpuFlags {
                    avx2: true,
                    avx512: true,
                    ..CpuFlags::default()
                },
            },
            memory: MemoryInfo {
                total_bytes: 256 * GIB,
                available_bytes: Some(220 * GIB),
                unified_memory: false,
            },
            disk: fixture_disk(disk_path),
            gpus: vec![
                GpuInfo {
                    vendor: GpuVendor::Nvidia,
                    name: "NVIDIA RTX PRO 6000 Blackwell".to_owned(),
                    backend: GpuBackend::Nvml,
                    memory_bytes: Some(96 * GIB),
                    unified_memory: false,
                    compute_capability: Some("10.0".to_owned()),
                    supports_nvfp4: true,
                    supports_fp8: true,
                    supports_tensor_parallel: true,
                },
                GpuInfo {
                    vendor: GpuVendor::Nvidia,
                    name: "NVIDIA RTX PRO 6000 Blackwell".to_owned(),
                    backend: GpuBackend::Nvml,
                    memory_bytes: Some(96 * GIB),
                    unified_memory: false,
                    compute_capability: Some("10.0".to_owned()),
                    supports_nvfp4: true,
                    supports_fp8: true,
                    supports_tensor_parallel: true,
                },
            ],
            tee: fixture_tee(2),
            warnings: Vec::new(),
        },
        FixtureProfile::CpuOnly => HardwareProfile {
            source: ReportSource {
                kind: "fixture".to_owned(),
                fixture: Some(fixture.as_str().to_owned()),
            },
            host: HostInfo {
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                family: "unix".to_owned(),
                kernel: Some("6.8.0".to_owned()),
            },
            cpu: CpuInfo {
                model: Some("Intel Core i7 CPU-only fixture".to_owned()),
                physical_cores: Some(8),
                logical_cores: 16,
                flags: CpuFlags {
                    avx2: true,
                    ..CpuFlags::default()
                },
            },
            memory: MemoryInfo {
                total_bytes: 32 * GIB,
                available_bytes: Some(22 * GIB),
                unified_memory: false,
            },
            disk: fixture_disk(disk_path),
            gpus: Vec::new(),
            tee: fixture_tee(1),
            warnings: Vec::new(),
        },
    }
}

fn fixture_disk(path: &Path) -> DiskInfo {
    DiskInfo {
        path: path.to_path_buf(),
        free_bytes: Some(1_000 * GIB),
        write_mib_s: Some(950.0),
        bench_bytes: Some(16 * MIB),
    }
}

fn fixture_tee(tier: u8) -> TeeInfo {
    TeeInfo {
        tier,
        sev_snp: tier >= 2,
        tdx: false,
        gpu_confidential_compute: tier >= 2,
        notes: if tier >= 2 {
            vec!["fixture Tier 2 hardware attestation signals".to_owned()]
        } else {
            vec!["fixture Tier 1 software-rooted attestation".to_owned()]
        },
    }
}

pub fn human_report(report: &HardwareReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Mayhem doctor");
    let _ = writeln!(
        out,
        "Host: {} {} ({})",
        report.host.os, report.host.arch, report.host.family
    );
    if let Some(model) = &report.cpu.model {
        let _ = writeln!(out, "CPU: {model}");
    }
    let _ = writeln!(
        out,
        "Cores: logical={} physical={}",
        report.cpu.logical_cores,
        report
            .cpu
            .physical_cores
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_owned())
    );
    let _ = writeln!(
        out,
        "CPU flags: avx2={} avx512={} neon={} amx={}",
        yes_no(report.cpu.flags.avx2),
        yes_no(report.cpu.flags.avx512),
        yes_no(report.cpu.flags.neon),
        yes_no(report.cpu.flags.amx)
    );
    let _ = writeln!(
        out,
        "RAM: total={} available={} unified={}",
        format_bytes(report.memory.total_bytes),
        report
            .memory
            .available_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "unknown".to_owned()),
        yes_no(report.memory.unified_memory)
    );
    let _ = writeln!(
        out,
        "Disk: path={} free={} write={}",
        report.disk.path.display(),
        report
            .disk
            .free_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "unknown".to_owned()),
        report
            .disk
            .write_mib_s
            .map(|value| format!("{value:.1} MiB/s"))
            .unwrap_or_else(|| "not measured".to_owned())
    );
    if report.gpus.is_empty() {
        let _ = writeln!(out, "GPUs: none detected");
    } else {
        let _ = writeln!(out, "GPUs:");
        for gpu in &report.gpus {
            let _ = writeln!(
                out,
                "  - {:?} {:?}: {} mem={} cc={} nvfp4={} fp8={} tp={}",
                gpu.vendor,
                gpu.backend,
                gpu.name,
                gpu.memory_bytes
                    .map(format_bytes)
                    .unwrap_or_else(|| "unknown".to_owned()),
                gpu.compute_capability.as_deref().unwrap_or("n/a"),
                yes_no(gpu.supports_nvfp4),
                yes_no(gpu.supports_fp8),
                yes_no(gpu.supports_tensor_parallel)
            );
        }
    }
    let _ = writeln!(
        out,
        "TEE: tier={} sev_snp={} tdx={} gpu_cc={}",
        report.tee.tier,
        yes_no(report.tee.sev_snp),
        yes_no(report.tee.tdx),
        yes_no(report.tee.gpu_confidential_compute)
    );
    let _ = writeln!(out, "Backend verdicts:");
    for verdict in &report.backend_verdicts {
        let _ = writeln!(
            out,
            "  - {}: {:?} est_tok_s={} max_sessions={} kv_cache_budget={} reason={}",
            verdict.backend,
            verdict.status,
            verdict
                .est_tok_s
                .map(|value| format!("{value:.1}"))
                .unwrap_or_else(|| "n/a".to_owned()),
            verdict.max_sessions,
            format_bytes(verdict.kv_cache_bytes_budget),
            verdict.reason.as_deref().unwrap_or("n/a")
        );
    }
    let _ = writeln!(
        out,
        "Selected backend: {}",
        report.selected_backend.as_deref().unwrap_or("none")
    );
    let _ = writeln!(
        out,
        "Summary: {:?} can_serve={} reason={}",
        report.summary.status,
        yes_no(report.summary.can_serve),
        report.summary.reason.as_deref().unwrap_or("n/a")
    );
    if !report.warnings.is_empty() {
        let _ = writeln!(out, "Warnings:");
        for warning in &report.warnings {
            let _ = writeln!(out, "  - {warning}");
        }
    }
    out
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn format_bytes(value: u64) -> String {
    if value >= GIB {
        format!("{:.1} GiB", value as f64 / GIB as f64)
    } else if value >= MIB {
        format!("{:.1} MiB", value as f64 / MIB as f64)
    } else {
        format!("{value} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_report(fixture: FixtureProfile) -> HardwareReport {
        probe(ProbeOptions {
            fixture: Some(fixture),
            run_disk_bench: false,
            ..ProbeOptions::default()
        })
    }

    #[test]
    fn exposes_crate_name() {
        assert_eq!(CRATE_NAME, "mayhem-hwprobe");
    }

    #[test]
    fn apple_silicon_prefers_mlx_full_offload() {
        let report = fixture_report(FixtureProfile::AppleSilicon);
        assert_eq!(report.selected_backend.as_deref(), Some("mlx"));
        let mlx = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "mlx")
            .unwrap();
        assert_eq!(mlx.status, VerdictStatus::FullOffload);
        assert!(report.memory.unified_memory);
        assert_eq!(report.gpus[0].backend, GpuBackend::Metal);
        assert_eq!(report.tee.tier, 2);
    }

    #[test]
    fn blackwell_fixture_prefers_trt_llm_with_nvfp4_and_tensor_parallel() {
        let report = fixture_report(FixtureProfile::LinuxNvidia);
        assert_eq!(report.selected_backend.as_deref(), Some("trt-llm"));
        let trt = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "trt-llm")
            .unwrap();
        assert_eq!(trt.status, VerdictStatus::FullOffload);
        assert!(report.gpus.iter().all(|gpu| gpu.supports_nvfp4));
        assert!(report.gpus.iter().all(|gpu| gpu.supports_tensor_parallel));
        assert_eq!(report.tee.tier, 2);
    }

    #[test]
    fn cpu_only_fixture_falls_back_to_llama_cpp_cpu_only() {
        let report = fixture_report(FixtureProfile::CpuOnly);
        assert_eq!(report.selected_backend.as_deref(), Some("llama.cpp"));
        let llama = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "llama.cpp")
            .unwrap();
        assert_eq!(llama.status, VerdictStatus::CpuOnly);
        assert!(report.gpus.is_empty());
    }

    #[test]
    fn parses_compute_capability() {
        assert_eq!(parse_compute_capability("10.0"), Some((10, 0)));
        assert_eq!(parse_compute_capability("8.9"), Some((8, 9)));
        assert_eq!(parse_compute_capability("bad"), None);
    }
}
