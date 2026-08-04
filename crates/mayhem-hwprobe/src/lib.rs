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
const ACE_STEP_RAM_FLOOR: u64 = 16 * GIB;
const ACE_STEP_CUDA_MEMORY_FLOOR: u64 = 4 * GIB;
const ACE_STEP_CUDA_FULL_OFFLOAD_FLOOR: u64 = 20 * GIB;
const ACE_STEP_APPLE_UNIFIED_MEMORY_FLOOR: u64 = 16 * GIB;
const CHATTERBOX_RAM_FLOOR: u64 = 8 * GIB;
const CHATTERBOX_ACCELERATOR_MEMORY_FLOOR: u64 = 6 * GIB;
const NEEDLE_RAM_FLOOR: u64 = GIB;
const NEEDLE_GPU_MEMORY_FLOOR: u64 = 512 * MIB;
const COMFYUI_RAM_FLOOR: u64 = 4 * GIB;
const TRANSFORMERS_ASR_RAM_FLOOR: u64 = 8 * GIB;
const TRANSFORMERS_ASR_CUDA_MEMORY_FLOOR: u64 = 4 * GIB;
const SULPHUR_NVIDIA_PARTIAL_OFFLOAD_FLOOR: u64 = 16 * GIB;
const SULPHUR_NVIDIA_FULL_OFFLOAD_FLOOR: u64 = 24 * GIB;
const SULPHUR_APPLE_UNIFIED_MEMORY_FLOOR: u64 = 64 * GIB;

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
    LinuxNvidiaArm64,
    WindowsNvidia,
    CpuOnly,
    UnsupportedHost,
    InsufficientHost,
}

impl FixtureProfile {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "apple-silicon" | "apple" | "mac" | "macos" => Some(Self::AppleSilicon),
            "linux-nvidia" | "nvidia" | "blackwell" => Some(Self::LinuxNvidia),
            "linux-nvidia-arm64" | "linux-aarch64" | "nvidia-arm64" => Some(Self::LinuxNvidiaArm64),
            "windows-nvidia" | "windows-4090" | "rtx-4090" => Some(Self::WindowsNvidia),
            "cpu-only" | "cpu" => Some(Self::CpuOnly),
            "unsupported" | "unsupported-host" => Some(Self::UnsupportedHost),
            "insufficient" | "insufficient-host" => Some(Self::InsufficientHost),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppleSilicon => "apple-silicon",
            Self::LinuxNvidia => "linux-nvidia",
            Self::LinuxNvidiaArm64 => "linux-nvidia-arm64",
            Self::WindowsNvidia => "windows-nvidia",
            Self::CpuOnly => "cpu-only",
            Self::UnsupportedHost => "unsupported-host",
            Self::InsufficientHost => "insufficient-host",
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
    /// Usable memory for provider capacity math. For discrete GPUs this is
    /// dedicated VRAM; shared/WDDM system memory is reported separately.
    pub memory_bytes: Option<u64>,
    pub dedicated_memory_bytes: Option<u64>,
    pub shared_memory_bytes: Option<u64>,
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
    Intel,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatterboxManagedDevice {
    Cpu,
    Cuda,
    Mps,
}

impl ChatterboxManagedDevice {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
            Self::Mps => "mps",
        }
    }
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
pub struct ModelMemoryFit {
    pub requested_context: u64,
    pub max_safe_context: u64,
    pub required_bytes: u64,
    pub usable_bytes: u64,
    pub status: VerdictStatus,
}

pub fn model_memory_fit(
    usable_bytes: u64,
    weights_bytes: u64,
    runtime_overhead_bytes: u64,
    kv_bytes_per_token: u64,
    requested_context: u64,
) -> ModelMemoryFit {
    let fixed_bytes = weights_bytes.saturating_add(runtime_overhead_bytes);
    let max_safe_context = if fixed_bytes > usable_bytes {
        0
    } else if kv_bytes_per_token == 0 {
        u64::MAX
    } else {
        usable_bytes.saturating_sub(fixed_bytes) / kv_bytes_per_token
    };
    let required_bytes =
        fixed_bytes.saturating_add(requested_context.saturating_mul(kv_bytes_per_token));
    ModelMemoryFit {
        requested_context,
        max_safe_context,
        required_bytes,
        usable_bytes,
        status: if required_bytes <= usable_bytes {
            VerdictStatus::FullOffload
        } else {
            VerdictStatus::Insufficient
        },
    }
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
        vllm_verdict(profile),
        trt_llm_verdict(profile),
        mlx_verdict(profile),
        llama_cpp_verdict(profile),
        stable_diffusion_cpp_verdict(profile),
        comfyui_verdict(profile),
        ace_step_verdict(profile),
        chatterbox_verdict(profile),
        transformers_asr_verdict(profile),
        whisper_cpp_verdict(profile),
        piper_verdict(profile),
        needle_cpu_verdict(profile),
        needle_gpu_verdict(profile),
        sulphur_verdict(profile),
    ]
}

fn chatterbox_verdict(profile: &HardwareProfile) -> BackendVerdict {
    let host_supported = match profile.host.os.as_str() {
        "linux" => matches!(profile.host.arch.as_str(), "x86_64" | "aarch64" | "arm64"),
        "windows" => profile.host.arch == "x86_64",
        "macos" => matches!(profile.host.arch.as_str(), "aarch64" | "arm64"),
        _ => false,
    };
    if !host_supported {
        return insufficient(
            "chatterbox",
            &format!(
                "original Chatterbox has no supported PyTorch runtime path for {}/{}",
                profile.host.os, profile.host.arch
            ),
        );
    }

    let host_memory = profile
        .memory
        .available_bytes
        .unwrap_or(profile.memory.total_bytes);
    if host_memory < CHATTERBOX_RAM_FLOOR {
        return insufficient(
            "chatterbox",
            &format!(
                "original Chatterbox needs at least 8 GiB available host memory; {} detected",
                format_bytes(host_memory)
            ),
        );
    }

    match chatterbox_managed_device_for_parts(
        &profile.host,
        host_memory,
        &profile.gpus,
    ) {
        ChatterboxManagedDevice::Cuda => BackendVerdict {
            backend: "chatterbox".to_owned(),
            status: VerdictStatus::FullOffload,
            reason: Some(
                "original Chatterbox can use its pinned CUDA runtime; model-specific memory checks still apply"
                    .to_owned(),
            ),
            est_tok_s: None,
            n_layers_gpu: None,
            max_sessions: 1,
            kv_cache_bytes_budget: host_memory / 8,
        },
        ChatterboxManagedDevice::Mps => BackendVerdict {
            backend: "chatterbox".to_owned(),
            status: VerdictStatus::FullOffload,
            reason: Some(
                "original Chatterbox can use its pinned Apple Metal/MPS runtime; model-specific memory checks still apply"
                    .to_owned(),
            ),
            est_tok_s: None,
            n_layers_gpu: None,
            max_sessions: 1,
            kv_cache_bytes_budget: host_memory / 8,
        },
        ChatterboxManagedDevice::Cpu => BackendVerdict {
            backend: "chatterbox".to_owned(),
            status: VerdictStatus::CpuOnly,
            reason: Some(
                "original Chatterbox will use its supported CPU fallback; model-specific memory checks still apply"
                    .to_owned(),
            ),
            est_tok_s: None,
            n_layers_gpu: Some(0),
            max_sessions: 1,
            kv_cache_bytes_budget: host_memory / 8,
        },
    }
}

#[must_use]
pub fn chatterbox_managed_device(report: &HardwareReport) -> Option<ChatterboxManagedDevice> {
    report
        .backend_verdicts
        .iter()
        .find(|verdict| verdict.backend == "chatterbox")
        .filter(|verdict| verdict.status != VerdictStatus::Insufficient)?;
    Some(chatterbox_managed_device_for_parts(
        &report.host,
        report
            .memory
            .available_bytes
            .unwrap_or(report.memory.total_bytes),
        &report.gpus,
    ))
}

fn chatterbox_managed_device_for_parts(
    host: &HostInfo,
    host_memory: u64,
    gpus: &[GpuInfo],
) -> ChatterboxManagedDevice {
    let managed_cuda_runtime = match host.os.as_str() {
        "linux" => matches!(host.arch.as_str(), "x86_64" | "aarch64" | "arm64"),
        "windows" => host.arch == "x86_64",
        _ => false,
    };
    let cuda_memory = managed_cuda_runtime
        .then(|| {
            gpus.iter()
                .filter(|gpu| gpu.vendor == GpuVendor::Nvidia && gpu.backend == GpuBackend::Nvml)
                .map(|gpu| {
                    if gpu.unified_memory || nvidia_host_unified_memory_signal(host, gpu) {
                        gpu.memory_bytes.unwrap_or(host_memory).min(host_memory)
                    } else {
                        gpu.dedicated_memory_bytes.or(gpu.memory_bytes).unwrap_or(0)
                    }
                })
                .max()
        })
        .flatten();
    if cuda_memory.is_some_and(|memory| memory >= CHATTERBOX_ACCELERATOR_MEMORY_FLOOR) {
        return ChatterboxManagedDevice::Cuda;
    }

    let managed_mps_runtime =
        host.os == "macos" && matches!(host.arch.as_str(), "aarch64" | "arm64");
    if managed_mps_runtime
        && gpus.iter().any(|gpu| {
            gpu.vendor == GpuVendor::Apple
                && gpu.backend == GpuBackend::Metal
                && gpu.unified_memory
                && gpu.memory_bytes.unwrap_or(host_memory).min(host_memory) >= CHATTERBOX_RAM_FLOOR
        })
    {
        return ChatterboxManagedDevice::Mps;
    }

    ChatterboxManagedDevice::Cpu
}

fn vllm_verdict(profile: &HardwareProfile) -> BackendVerdict {
    let nvidia = profile
        .gpus
        .iter()
        .filter(|gpu| gpu.vendor == GpuVendor::Nvidia)
        .collect::<Vec<_>>();
    if nvidia.is_empty() {
        return insufficient("vllm", "no NVIDIA GPU detected");
    }
    if profile.host.os == "windows" {
        return insufficient(
            "vllm",
            "vLLM is not selected on Windows provider builds yet; Mayhem uses llama.cpp there until a supported Windows vLLM runtime is verified",
        );
    }

    let total_cuda_memory = nvidia_usable_memory_bytes(profile, &nvidia);
    let best_cc = nvidia
        .iter()
        .filter_map(|gpu| gpu.compute_capability.as_deref())
        .filter_map(parse_compute_capability)
        .fold(None, |best: Option<(u32, u32)>, cc| {
            Some(best.map_or(cc, |best| best.max(cc)))
        });
    let Some(cc) = best_cc else {
        return BackendVerdict {
            backend: "vllm".to_owned(),
            status: VerdictStatus::PartialOffload,
            reason: Some(format!(
                "NVIDIA GPU detected but compute capability is unknown{}",
                nvidia_memory_reason_suffix(profile, &nvidia)
            )),
            est_tok_s: Some(30.0),
            n_layers_gpu: None,
            max_sessions: 2,
            kv_cache_bytes_budget: total_cuda_memory / 4,
        };
    };
    if cc < (7, 0) {
        return insufficient(
            "vllm",
            "NVIDIA compute capability is below the launch vLLM floor",
        );
    }

    let tensor_parallel = nvidia.first().is_some_and(|first| {
        nvidia.len() >= 2
            && nvidia.iter().all(|gpu| {
                gpu.name == first.name
                    && gpu.compute_capability == first.compute_capability
                    && gpu.memory_bytes == first.memory_bytes
            })
    });
    let uses_unified_memory = nvidia
        .iter()
        .any(|gpu| nvidia_gpu_uses_host_unified_memory(profile, gpu));
    if total_cuda_memory >= 16 * GIB {
        BackendVerdict {
            backend: "vllm".to_owned(),
            status: VerdictStatus::FullOffload,
            reason: Some(format!(
                "{}{}",
                if tensor_parallel {
                    "NVIDIA CUDA GPU set supports vLLM continuous batching and tensor parallelism"
                } else if uses_unified_memory {
                    "NVIDIA CUDA GPU with unified memory supports vLLM continuous batching"
                } else {
                    "NVIDIA CUDA GPU supports vLLM continuous batching"
                },
                nvidia_memory_reason_suffix(profile, &nvidia)
            )),
            est_tok_s: Some(if cc >= (10, 0) {
                if tensor_parallel {
                    300.0
                } else {
                    180.0
                }
            } else if cc >= (8, 9) {
                120.0
            } else {
                70.0
            }),
            n_layers_gpu: None,
            max_sessions: if tensor_parallel { 16 } else { 8 },
            kv_cache_bytes_budget: total_cuda_memory / 2,
        }
    } else if total_cuda_memory >= 8 * GIB {
        BackendVerdict {
            backend: "vllm".to_owned(),
            status: VerdictStatus::PartialOffload,
            reason: Some(format!(
                "NVIDIA CUDA GPU can run small vLLM launch artifacts with limited concurrency{}",
                nvidia_memory_reason_suffix(profile, &nvidia)
            )),
            est_tok_s: Some(45.0),
            n_layers_gpu: None,
            max_sessions: 2,
            kv_cache_bytes_budget: total_cuda_memory / 3,
        }
    } else {
        insufficient(
            "vllm",
            "less than 8 GiB NVIDIA dedicated or unified memory available",
        )
    }
}

fn nvidia_usable_memory_bytes(profile: &HardwareProfile, gpus: &[&GpuInfo]) -> u64 {
    gpus.iter()
        .map(|gpu| {
            if nvidia_gpu_uses_host_unified_memory(profile, gpu) {
                gpu.memory_bytes.unwrap_or_else(|| {
                    profile
                        .memory
                        .available_bytes
                        .unwrap_or(profile.memory.total_bytes)
                })
            } else {
                gpu.dedicated_memory_bytes.or(gpu.memory_bytes).unwrap_or(0)
            }
        })
        .sum()
}

fn nvidia_memory_reason_suffix(profile: &HardwareProfile, gpus: &[&GpuInfo]) -> &'static str {
    if profile.host.os == "windows"
        && gpus
            .iter()
            .any(|gpu| gpu.shared_memory_bytes.unwrap_or(0) > 0 && !gpu.unified_memory)
    {
        "; Windows WDDM shared GPU memory is reported but capacity uses dedicated VRAM only to avoid silent paging"
    } else {
        ""
    }
}

fn nvidia_gpu_uses_host_unified_memory(profile: &HardwareProfile, gpu: &GpuInfo) -> bool {
    gpu.unified_memory || nvidia_host_unified_memory_signal(&profile.host, gpu)
}

fn sulphur_verdict(profile: &HardwareProfile) -> BackendVerdict {
    if profile.host.os == "macos" && profile.host.arch == "aarch64" {
        let has_apple_mlx = profile.gpus.iter().any(|gpu| {
            gpu.vendor == GpuVendor::Apple
                && gpu.backend == GpuBackend::Metal
                && gpu.unified_memory
                && profile.memory.unified_memory
        });
        if !has_apple_mlx {
            return insufficient(
                "sulphur",
                "Sulphur on macOS arm64 requires Apple Silicon Metal with unified memory; no CPU fallback is claimed",
            );
        }

        let available = profile
            .memory
            .available_bytes
            .unwrap_or(profile.memory.total_bytes);
        if available < SULPHUR_APPLE_UNIFIED_MEMORY_FLOOR {
            return insufficient(
                "sulphur",
                &format!(
                    "Sulphur MLX requires at least 64 GiB available unified memory; {} is available, and no CPU fallback is claimed",
                    format_bytes(available)
                ),
            );
        }

        return BackendVerdict {
            backend: "sulphur".to_owned(),
            status: VerdictStatus::FullOffload,
            reason: Some(format!(
                "Apple Silicon MLX hardware path has {} available unified memory; admission remains artifact-calibration-gated",
                format_bytes(available)
            )),
            est_tok_s: None,
            n_layers_gpu: None,
            max_sessions: 1,
            kv_cache_bytes_budget: 0,
        };
    }

    let nvidia_platform = matches!(
        (profile.host.os.as_str(), profile.host.arch.as_str()),
        ("windows", "x86_64") | ("linux", "x86_64") | ("linux", "aarch64")
    );
    if !nvidia_platform {
        return insufficient(
            "sulphur",
            &format!(
                "Sulphur has no supported accelerator runtime for {}/{} and no CPU fallback is claimed",
                profile.host.os, profile.host.arch
            ),
        );
    }

    let nvidia = profile
        .gpus
        .iter()
        .filter(|gpu| gpu.vendor == GpuVendor::Nvidia && gpu.backend == GpuBackend::Nvml)
        .collect::<Vec<_>>();
    let best_cuda_memory = nvidia
        .iter()
        .map(|gpu| {
            let unified = nvidia_gpu_uses_host_unified_memory(profile, gpu);
            let memory = if unified {
                profile
                    .memory
                    .available_bytes
                    .unwrap_or(profile.memory.total_bytes)
            } else {
                gpu.dedicated_memory_bytes.or(gpu.memory_bytes).unwrap_or(0)
            };
            (memory, unified)
        })
        .max();
    let Some((cuda_memory, unified)) = best_cuda_memory else {
        return insufficient(
            "sulphur",
            "Sulphur on this platform requires an NVIDIA CUDA GPU; no CPU fallback is claimed",
        );
    };

    let memory_kind = if unified {
        "available unified memory"
    } else {
        "dedicated device memory"
    };
    let status = if cuda_memory >= SULPHUR_NVIDIA_FULL_OFFLOAD_FLOOR {
        VerdictStatus::FullOffload
    } else if cuda_memory >= SULPHUR_NVIDIA_PARTIAL_OFFLOAD_FLOOR {
        VerdictStatus::PartialOffload
    } else {
        return insufficient(
            "sulphur",
            &format!(
                "Sulphur CUDA GGUF requires at least 16 GiB usable accelerator memory for conservative partial offload and 24 GiB for full offload; {} {} is detected, and no CPU fallback is claimed{}",
                format_bytes(cuda_memory),
                memory_kind,
                nvidia_memory_reason_suffix(profile, &nvidia)
            ),
        );
    };

    BackendVerdict {
        backend: "sulphur".to_owned(),
        status,
        reason: Some(format!(
            "NVIDIA CUDA GGUF hardware path has {} {}; admission remains artifact-calibration-gated{}",
            format_bytes(cuda_memory),
            memory_kind,
            nvidia_memory_reason_suffix(profile, &nvidia)
        )),
        est_tok_s: None,
        n_layers_gpu: None,
        max_sessions: 1,
        kv_cache_bytes_budget: 0,
    }
}

fn os_memory_reserve_bytes(memory_bytes: u64, unified: bool) -> u64 {
    let (bps, floor) = if unified {
        (1_500_u64, 4 * GIB)
    } else {
        (1_000_u64, 2 * GIB)
    };
    let percent = ((u128::from(memory_bytes) * u128::from(bps)) / 10_000)
        .try_into()
        .unwrap_or(u64::MAX);
    percent.max(floor)
}

fn estimated_partial_offload_layers(memory_bytes: u64, unified: bool) -> Option<u32> {
    if memory_bytes == 0 {
        return None;
    }
    let usable = memory_bytes.saturating_sub(os_memory_reserve_bytes(memory_bytes, unified));
    let per_layer_bytes = if unified { 768 * MIB } else { 512 * MIB };
    let layers = usable / per_layer_bytes;
    (layers > 0).then_some(u32::try_from(layers.min(80)).unwrap_or(80))
}

fn estimated_partial_offload_tok_s(layers: Option<u32>, base: f64, per_layer: f64) -> Option<f64> {
    Some(base + f64::from(layers.unwrap_or(0)) * per_layer)
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
    if profile.host.os == "windows" {
        return insufficient(
            "trt-llm",
            "TensorRT-LLM is not selected on Windows provider builds yet; Mayhem uses llama.cpp there until a supported Windows TensorRT-LLM runtime is verified",
        );
    }

    let total_vram = nvidia
        .iter()
        .filter_map(|gpu| gpu.dedicated_memory_bytes.or(gpu.memory_bytes))
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
            est_tok_s: estimated_partial_offload_tok_s(
                estimated_partial_offload_layers(total_vram, false),
                12.0,
                0.9,
            ),
            n_layers_gpu: estimated_partial_offload_layers(total_vram, false),
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
        let n_layers_gpu = estimated_partial_offload_layers(total_vram, false);
        BackendVerdict {
            backend: "trt-llm".to_owned(),
            status: VerdictStatus::PartialOffload,
            reason: Some(
                "NVIDIA GPU lacks launch quantization target or has limited VRAM".to_owned(),
            ),
            est_tok_s: estimated_partial_offload_tok_s(n_layers_gpu, 14.0, 0.9),
            n_layers_gpu,
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
        let n_layers_gpu = estimated_partial_offload_layers(memory, true);
        BackendVerdict {
            backend: "mlx".to_owned(),
            status: VerdictStatus::PartialOffload,
            reason: Some("Apple Metal is present but unified memory is tight".to_owned()),
            est_tok_s: estimated_partial_offload_tok_s(n_layers_gpu, 6.0, 0.8),
            n_layers_gpu,
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
        .filter_map(|gpu| gpu.dedicated_memory_bytes.or(gpu.memory_bytes))
        .sum::<u64>();
    let has_accel = profile.gpus.iter().any(|gpu| {
        matches!(
            gpu.backend,
            GpuBackend::Metal | GpuBackend::Nvml | GpuBackend::Rocm | GpuBackend::Vulkan
        )
    });

    if has_accel && total_gpu_mem >= 8 * GIB {
        let n_layers_gpu = estimated_partial_offload_layers(total_gpu_mem, false);
        BackendVerdict {
            backend: "llama.cpp".to_owned(),
            status: VerdictStatus::PartialOffload,
            reason: Some(
                "GPU acceleration available for GGUF partial/full layer offload".to_owned(),
            ),
            est_tok_s: estimated_partial_offload_tok_s(n_layers_gpu, 7.0, 0.75),
            n_layers_gpu,
            max_sessions: 2,
            kv_cache_bytes_budget: total_gpu_mem / 3,
        }
    } else if windows_wddm_shared_memory_without_dedicated_capacity(profile, total_gpu_mem)
        && profile.memory.total_bytes >= 8 * GIB
    {
        BackendVerdict {
            backend: "llama.cpp".to_owned(),
            status: VerdictStatus::CpuOnly,
            reason: Some(format!(
                "{}; Windows WDDM shared GPU memory is ignored to avoid silent paging",
                cpu_reason(&profile.cpu)
            )),
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
    } else if has_accel && profile.memory.total_bytes >= 16 * GIB {
        let unified = profile.memory.unified_memory
            || profile
                .gpus
                .iter()
                .any(|gpu| gpu.unified_memory || gpu.vendor == GpuVendor::Apple);
        let memory = profile
            .memory
            .available_bytes
            .unwrap_or(profile.memory.total_bytes);
        let n_layers_gpu = estimated_partial_offload_layers(memory, unified);
        BackendVerdict {
            backend: "llama.cpp".to_owned(),
            status: VerdictStatus::PartialOffload,
            reason: Some(
                "accelerator detected but dedicated VRAM is unknown or limited".to_owned(),
            ),
            est_tok_s: estimated_partial_offload_tok_s(n_layers_gpu, 5.0, 0.55),
            n_layers_gpu,
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

fn windows_wddm_shared_memory_without_dedicated_capacity(
    profile: &HardwareProfile,
    total_gpu_mem: u64,
) -> bool {
    profile.host.os == "windows"
        && total_gpu_mem < 8 * GIB
        && profile
            .gpus
            .iter()
            .any(|gpu| gpu.shared_memory_bytes.unwrap_or(0) > 0 && !gpu.unified_memory)
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

fn comfyui_verdict(profile: &HardwareProfile) -> BackendVerdict {
    let host_supported = match profile.host.os.as_str() {
        "linux" | "macos" => matches!(profile.host.arch.as_str(), "x86_64" | "aarch64" | "arm64"),
        "windows" => profile.host.arch == "x86_64",
        _ => false,
    };
    if !host_supported {
        return insufficient(
            "comfyui",
            &format!(
                "ComfyUI runtime has no supported launch path for {}/{}",
                profile.host.os, profile.host.arch
            ),
        );
    }
    if profile.memory.total_bytes < COMFYUI_RAM_FLOOR {
        return insufficient(
            "comfyui",
            "less than 4 GiB RAM available for ComfyUI runtime serving",
        );
    }
    let has_accel = profile.gpus.iter().any(|gpu| {
        matches!(
            gpu.backend,
            GpuBackend::Metal | GpuBackend::Nvml | GpuBackend::Rocm | GpuBackend::Vulkan
        )
    });
    BackendVerdict {
        backend: "comfyui".to_owned(),
        status: if has_accel {
            VerdictStatus::PartialOffload
        } else {
            VerdictStatus::CpuOnly
        },
        reason: Some(if has_accel {
            "local accelerator available for ComfyUI workflows".to_owned()
        } else {
            cpu_reason(&profile.cpu)
        }),
        est_tok_s: None,
        n_layers_gpu: None,
        max_sessions: 1,
        kv_cache_bytes_budget: profile
            .memory
            .available_bytes
            .unwrap_or(profile.memory.total_bytes)
            / 5,
    }
}

fn ace_step_verdict(profile: &HardwareProfile) -> BackendVerdict {
    if !ace_step_cpu_platform_supported(&profile.host) {
        return insufficient(
            "ace-step",
            &format!(
                "ACE-Step 1.5 v0.1.8 has no supported runtime path for {}/{}",
                profile.host.os, profile.host.arch
            ),
        );
    }

    let host_memory = profile
        .memory
        .available_bytes
        .unwrap_or(profile.memory.total_bytes);
    if host_memory < ACE_STEP_RAM_FLOOR {
        return insufficient(
            "ace-step",
            &format!(
                "ACE-Step needs at least 16 GiB available host RAM for the generic runtime; {} detected; model-specific CLI RAM/VRAM checks still apply",
                format_bytes(host_memory)
            ),
        );
    }

    let cuda_gpu = ace_step_cuda_platform_supported(&profile.host)
        .then(|| {
            profile
                .gpus
                .iter()
                .filter(|gpu| gpu.vendor == GpuVendor::Nvidia && gpu.backend == GpuBackend::Nvml)
                .max_by_key(|gpu| ace_step_nvidia_memory_bytes(profile, gpu))
        })
        .flatten();
    if let Some(gpu) = cuda_gpu {
        let cuda_memory = ace_step_nvidia_memory_bytes(profile, gpu);
        if cuda_memory >= ACE_STEP_CUDA_FULL_OFFLOAD_FLOOR {
            return BackendVerdict {
                backend: "ace-step".to_owned(),
                status: VerdictStatus::FullOffload,
                reason: Some(format!(
                    "ACE-Step CUDA hardware path available on {}/{} with {} detected device memory; the worker rechecks load-time free memory before selecting its pinned no-offload or offload policy, and throughput comes from artifact calibration; model-specific CLI RAM/VRAM checks still apply",
                    profile.host.os,
                    profile.host.arch,
                    format_bytes(cuda_memory)
                )),
                est_tok_s: None,
                n_layers_gpu: None,
                max_sessions: 1,
                kv_cache_bytes_budget: cuda_memory / 8,
            };
        }
        if cuda_memory >= ACE_STEP_CUDA_MEMORY_FLOOR {
            return BackendVerdict {
                backend: "ace-step".to_owned(),
                status: VerdictStatus::PartialOffload,
                reason: Some(format!(
                    "ACE-Step CUDA hardware path available on {}/{} with {} detected device memory; the pinned runtime supports CPU/INT8 offload and the worker rechecks load-time free memory before selecting it, while throughput comes from artifact calibration; model-specific CLI RAM/VRAM checks still apply",
                    profile.host.os,
                    profile.host.arch,
                    format_bytes(cuda_memory)
                )),
                est_tok_s: None,
                n_layers_gpu: None,
                max_sessions: 1,
                kv_cache_bytes_budget: cuda_memory / 8,
            };
        }
    }

    let apple_metal = (profile.host.os == "macos"
        && matches!(profile.host.arch.as_str(), "aarch64" | "arm64"))
    .then(|| {
        profile.gpus.iter().find(|gpu| {
            gpu.vendor == GpuVendor::Apple && gpu.backend == GpuBackend::Metal && gpu.unified_memory
        })
    })
    .flatten();
    if let Some(gpu) = apple_metal {
        let unified_memory = gpu.memory_bytes.unwrap_or(host_memory).min(host_memory);
        if unified_memory >= ACE_STEP_APPLE_UNIFIED_MEMORY_FLOOR {
            return BackendVerdict {
                backend: "ace-step".to_owned(),
                status: VerdictStatus::FullOffload,
                reason: Some(format!(
                    "ACE-Step Apple Silicon Metal/MPS hardware path available with {} detected available unified memory; the pinned runtime keeps CPU offload disabled for unified memory, and throughput comes from artifact calibration; model-specific CLI RAM/VRAM checks still apply",
                    format_bytes(unified_memory)
                )),
                est_tok_s: None,
                n_layers_gpu: None,
                max_sessions: 1,
                kv_cache_bytes_budget: unified_memory / 8,
            };
        }
    }

    let accelerator_note = if let Some(gpu) = cuda_gpu {
        let memory = ace_step_nvidia_memory_bytes(profile, gpu);
        if memory == 0 {
            "NVIDIA/NVML device detected but usable CUDA memory is unknown; "
        } else {
            "NVIDIA/NVML device detected but usable CUDA memory is below 4 GiB; "
        }
    } else if apple_metal.is_some() {
        "Apple Metal device detected but usable unified memory is below 16 GiB; "
    } else {
        "no supported accelerator probe was detected; "
    };
    BackendVerdict {
        backend: "ace-step".to_owned(),
        status: VerdictStatus::CpuOnly,
        reason: Some(format!(
            "{accelerator_note}ACE-Step will use the supported CPU fallback on {}/{} with at least 16 GiB available host memory; throughput comes from artifact calibration; model-specific CLI RAM/VRAM checks still apply",
            profile.host.os, profile.host.arch
        )),
        est_tok_s: None,
        n_layers_gpu: Some(0),
        max_sessions: 1,
        kv_cache_bytes_budget: host_memory / 16,
    }
}

fn ace_step_cpu_platform_supported(host: &HostInfo) -> bool {
    match host.os.as_str() {
        "linux" => matches!(host.arch.as_str(), "x86_64" | "aarch64" | "arm64"),
        "windows" => host.arch == "x86_64",
        "macos" => matches!(host.arch.as_str(), "x86_64" | "aarch64" | "arm64"),
        _ => false,
    }
}

fn ace_step_cuda_platform_supported(host: &HostInfo) -> bool {
    match host.os.as_str() {
        "linux" => matches!(host.arch.as_str(), "x86_64" | "aarch64" | "arm64"),
        "windows" => host.arch == "x86_64",
        _ => false,
    }
}

fn ace_step_nvidia_memory_bytes(profile: &HardwareProfile, gpu: &GpuInfo) -> u64 {
    if nvidia_gpu_uses_host_unified_memory(profile, gpu) {
        let host_memory = profile
            .memory
            .available_bytes
            .unwrap_or(profile.memory.total_bytes);
        gpu.memory_bytes.unwrap_or(host_memory).min(host_memory)
    } else {
        gpu.dedicated_memory_bytes.or(gpu.memory_bytes).unwrap_or(0)
    }
}

fn transformers_asr_verdict(profile: &HardwareProfile) -> BackendVerdict {
    if profile.memory.total_bytes < TRANSFORMERS_ASR_RAM_FLOOR {
        return insufficient(
            "transformers-asr",
            "less than 8 GiB RAM available for float32 transformers ASR serving",
        );
    }

    let cuda_memory = matches!(profile.host.os.as_str(), "linux" | "windows")
        .then(|| {
            profile
                .gpus
                .iter()
                .filter(|gpu| gpu.vendor == GpuVendor::Nvidia && gpu.backend == GpuBackend::Nvml)
                .map(|gpu| {
                    if nvidia_gpu_uses_host_unified_memory(profile, gpu) {
                        gpu.memory_bytes.unwrap_or_else(|| {
                            profile
                                .memory
                                .available_bytes
                                .unwrap_or(profile.memory.total_bytes)
                        })
                    } else {
                        gpu.dedicated_memory_bytes.or(gpu.memory_bytes).unwrap_or(0)
                    }
                })
                .max()
        })
        .flatten();
    if cuda_memory.is_some_and(|memory| memory >= TRANSFORMERS_ASR_CUDA_MEMORY_FLOOR) {
        return BackendVerdict {
            backend: "transformers-asr".to_owned(),
            status: VerdictStatus::FullOffload,
            reason: Some(
                "CUDA accelerator has enough usable device memory for transformers ASR".to_owned(),
            ),
            est_tok_s: None,
            n_layers_gpu: None,
            max_sessions: 1,
            kv_cache_bytes_budget: profile
                .memory
                .available_bytes
                .unwrap_or(profile.memory.total_bytes)
                / 8,
        };
    }

    let has_mps = profile.host.os == "macos"
        && profile.gpus.iter().any(|gpu| {
            gpu.vendor == GpuVendor::Apple
                && gpu.backend == GpuBackend::Metal
                && gpu.unified_memory
                && gpu.memory_bytes.unwrap_or(profile.memory.total_bytes)
                    >= TRANSFORMERS_ASR_RAM_FLOOR
        });
    if has_mps {
        return BackendVerdict {
            backend: "transformers-asr".to_owned(),
            status: VerdictStatus::FullOffload,
            reason: Some(
                "Apple Metal/MPS unified-memory acceleration available for transformers ASR"
                    .to_owned(),
            ),
            est_tok_s: None,
            n_layers_gpu: None,
            max_sessions: 1,
            kv_cache_bytes_budget: profile
                .memory
                .available_bytes
                .unwrap_or(profile.memory.total_bytes)
                / 8,
        };
    }

    if !matches!(profile.host.os.as_str(), "linux" | "windows" | "macos") {
        return insufficient(
            "transformers-asr",
            "transformers ASR CPU serving is supported only on Linux, Windows, and macOS",
        );
    }

    let accelerator_note =
        if cuda_memory.is_some_and(|memory| memory < TRANSFORMERS_ASR_CUDA_MEMORY_FLOOR) {
            "CUDA device detected but usable device memory is below 4 GiB; "
        } else {
            ""
        };
    let simd = if profile.cpu.flags.avx2 {
        " with AVX2"
    } else if profile.cpu.flags.neon {
        " with NEON"
    } else {
        ""
    };
    BackendVerdict {
        backend: "transformers-asr".to_owned(),
        status: VerdictStatus::CpuOnly,
        reason: Some(format!(
            "{accelerator_note}transformers ASR will use CPU execution{simd}"
        )),
        est_tok_s: None,
        n_layers_gpu: Some(0),
        max_sessions: 1,
        kv_cache_bytes_budget: profile
            .memory
            .available_bytes
            .unwrap_or(profile.memory.total_bytes)
            / 8,
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

fn needle_cpu_verdict(profile: &HardwareProfile) -> BackendVerdict {
    let platform_supported = match profile.host.os.as_str() {
        "linux" => {
            matches!(profile.host.arch.as_str(), "x86_64" | "aarch64" | "arm64")
        }
        "macos" => matches!(profile.host.arch.as_str(), "aarch64" | "arm64"),
        "windows" => profile.host.arch == "x86_64",
        _ => false,
    };
    if !platform_supported {
        return insufficient(
            "needle-cpu",
            &format!(
                "Needle CPU execution has no supported 64-bit runtime for {}/{}",
                profile.host.os, profile.host.arch
            ),
        );
    }

    let available_memory = profile
        .memory
        .available_bytes
        .unwrap_or(profile.memory.total_bytes);
    if available_memory < NEEDLE_RAM_FLOOR {
        return insufficient(
            "needle-cpu",
            &format!(
                "Needle CPU execution needs at least 1 GiB available host memory; {} detected",
                format_bytes(available_memory)
            ),
        );
    }

    BackendVerdict {
        backend: "needle-cpu".to_owned(),
        status: VerdictStatus::CpuOnly,
        reason: Some(format!(
            "Needle's 30.4M BF16 model and 1,024-token combined context fit CPU execution{}",
            if profile.cpu.flags.avx2 {
                " with AVX2"
            } else if profile.cpu.flags.neon {
                " with NEON"
            } else {
                ""
            }
        )),
        est_tok_s: Some(50.0),
        n_layers_gpu: Some(0),
        max_sessions: 2,
        kv_cache_bytes_budget: 64 * MIB,
    }
}

fn needle_gpu_verdict(profile: &HardwareProfile) -> BackendVerdict {
    let platform_supported = match profile.host.os.as_str() {
        "macos" => matches!(profile.host.arch.as_str(), "aarch64" | "arm64"),
        "linux" => matches!(profile.host.arch.as_str(), "aarch64" | "arm64" | "x86_64"),
        "windows" => profile.host.arch == "x86_64",
        _ => false,
    };
    if !platform_supported {
        return insufficient(
            "needle-gpu",
            &format!(
                "Needle GPU execution has no frozen runtime for {}/{}",
                profile.host.os, profile.host.arch
            ),
        );
    }

    let available_memory = profile
        .memory
        .available_bytes
        .unwrap_or(profile.memory.total_bytes);
    if available_memory < NEEDLE_RAM_FLOOR {
        return insufficient(
            "needle-gpu",
            &format!(
                "Needle GPU execution needs at least 1 GiB available host memory; {} detected",
                format_bytes(available_memory)
            ),
        );
    }

    if profile.host.os == "macos" {
        return insufficient(
            "needle-gpu",
            "Needle's calibrated Apple MPS path is slower than CPU; use needle-cpu",
        );
    }

    let nvidia = profile
        .gpus
        .iter()
        .filter(|gpu| {
            gpu.vendor == GpuVendor::Nvidia
                && gpu.backend == GpuBackend::Nvml
                && gpu
                    .compute_capability
                    .as_deref()
                    .and_then(parse_compute_capability)
                    .is_some_and(|capability| capability >= (7, 5))
        })
        .collect::<Vec<_>>();
    if nvidia.is_empty() {
        return insufficient(
            "needle-gpu",
            "no compatible NVIDIA GPU with compute capability >= 7.5 detected",
        );
    }

    let cuda_memory = nvidia_usable_memory_bytes(profile, &nvidia);
    if cuda_memory < NEEDLE_GPU_MEMORY_FLOOR {
        return insufficient(
            "needle-gpu",
            &format!(
                "Needle CUDA execution needs at least 512 MiB usable NVIDIA memory; {} detected",
                format_bytes(cuda_memory)
            ),
        );
    }

    BackendVerdict {
        backend: "needle-gpu".to_owned(),
        status: VerdictStatus::FullOffload,
        reason: Some(format!(
            "NVIDIA CUDA GPU has enough memory for Needle's 30.4M BF16 model and 1,024-token combined context{}",
            nvidia_memory_reason_suffix(profile, &nvidia)
        )),
        est_tok_s: Some(200.0),
        n_layers_gpu: None,
        max_sessions: 4,
        kv_cache_bytes_budget: 64 * MIB,
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
    let mut memory = probe_memory(&host);
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
    let windows_gpu_memory = probe_windows_gpu_memory();
    gpus.extend(probe_apple_metal(&host, &memory));
    gpus.extend(probe_windows_dxdiag_adapters(&windows_gpu_memory));
    gpus.extend(probe_nvidia(&windows_gpu_memory));
    gpus.extend(probe_rocm());
    gpus.extend(probe_vulkan());
    dedupe_gpus(&mut gpus);
    mark_nvidia_host_unified_memory(&host, &mut memory, &mut gpus);
    if host.os == "windows"
        && gpus
            .iter()
            .any(|gpu| gpu.shared_memory_bytes.unwrap_or(0) > 0 && !gpu.unified_memory)
    {
        warnings.push(
            "Windows WDDM shared GPU memory detected; provider capacity ignores shared memory to avoid silent paging".to_owned(),
        );
    }

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

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        flags.avx2 |= std::is_x86_feature_detected!("avx2");
        flags.avx512 |= std::is_x86_feature_detected!("avx512f");
    }

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
            total_bytes: windows_memory_bytes(0).unwrap_or(0),
            available_bytes: windows_memory_bytes(1),
            unified_memory: false,
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_memory_bytes(index: usize) -> Option<u64> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    match index {
        0 => (system.total_memory() > 0).then(|| system.total_memory()),
        1 => Some(system.available_memory()),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn windows_memory_bytes(_index: usize) -> Option<u64> {
    None
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
    fs2::available_space(path).ok()
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
        dedicated_memory_bytes: None,
        shared_memory_bytes: None,
        unified_memory: true,
        compute_capability: None,
        supports_nvfp4: false,
        supports_fp8: false,
        supports_tensor_parallel: false,
    }]
}

fn probe_nvidia(
    windows_memory: &std::collections::BTreeMap<String, WindowsGpuMemorySplit>,
) -> Vec<GpuInfo> {
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
        let split = windows_memory_split_for_name(windows_memory, parts[0]);
        let dedicated_memory_bytes =
            memory_bytes.or_else(|| split.as_ref().and_then(|split| split.dedicated_bytes));
        gpus.push(GpuInfo {
            vendor: GpuVendor::Nvidia,
            name: parts[0].to_owned(),
            backend: GpuBackend::Nvml,
            memory_bytes: dedicated_memory_bytes,
            dedicated_memory_bytes,
            shared_memory_bytes: split.and_then(|split| split.shared_bytes),
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

fn mark_nvidia_host_unified_memory(host: &HostInfo, memory: &mut MemoryInfo, gpus: &mut [GpuInfo]) {
    let mut found = false;
    for gpu in gpus.iter_mut() {
        if nvidia_host_unified_memory_signal(host, gpu) {
            gpu.unified_memory = true;
            found = true;
        }
    }
    if found {
        memory.unified_memory = true;
    }
}

fn nvidia_host_unified_memory_signal(host: &HostInfo, gpu: &GpuInfo) -> bool {
    gpu.vendor == GpuVendor::Nvidia
        && gpu.memory_bytes.is_none()
        && host.os == "linux"
        && matches!(host.arch.as_str(), "aarch64" | "arm64")
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct WindowsGpuMemorySplit {
    name: String,
    manufacturer: Option<String>,
    dedicated_bytes: Option<u64>,
    shared_bytes: Option<u64>,
}

fn windows_memory_split_for_name(
    memory: &std::collections::BTreeMap<String, WindowsGpuMemorySplit>,
    name: &str,
) -> Option<WindowsGpuMemorySplit> {
    let key = normalize_gpu_name(name);
    memory.get(&key).cloned().or_else(|| {
        memory
            .iter()
            .find(|(candidate, _)| candidate.contains(&key) || key.contains(*candidate))
            .map(|(_, split)| split.clone())
    })
}

fn normalize_gpu_name(name: &str) -> String {
    name.to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn probe_windows_gpu_memory() -> std::collections::BTreeMap<String, WindowsGpuMemorySplit> {
    let dxdiag = probe_windows_dxdiag_memory();
    if !dxdiag.is_empty() {
        return dxdiag;
    }
    probe_windows_cim_gpu_memory()
}

fn probe_windows_dxdiag_memory() -> std::collections::BTreeMap<String, WindowsGpuMemorySplit> {
    if !cfg!(target_os = "windows") {
        return std::collections::BTreeMap::new();
    }
    let path = env::temp_dir().join(format!(
        "mayhem-dxdiag-{}-{}.txt",
        std::process::id(),
        monotonic_nanos()
    ));
    let status = Command::new("dxdiag")
        .args(["/whql:off", "/t", path.to_string_lossy().as_ref()])
        .status();
    if !status.is_ok_and(|status| status.success()) {
        let _ = fs::remove_file(&path);
        return std::collections::BTreeMap::new();
    }
    let text = fs::read_to_string(&path).unwrap_or_default();
    let _ = fs::remove_file(&path);
    parse_dxdiag_memory_splits(&text)
}

fn probe_windows_cim_gpu_memory() -> std::collections::BTreeMap<String, WindowsGpuMemorySplit> {
    if !cfg!(target_os = "windows") {
        return std::collections::BTreeMap::new();
    }
    let Some(output) = command_stdout(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_VideoController | ForEach-Object { [Console]::WriteLine(($_.Name -replace '\\|',' ') + '|' + ($_.AdapterCompatibility -replace '\\|',' ') + '|' + [string]$_.AdapterRAM) }",
        ],
    ) else {
        return std::collections::BTreeMap::new();
    };
    parse_windows_cim_video_controller_memory(
        &output,
        windows_memory_bytes(0).map(|bytes| bytes / 2),
    )
}

fn probe_windows_dxdiag_adapters(
    memory: &std::collections::BTreeMap<String, WindowsGpuMemorySplit>,
) -> Vec<GpuInfo> {
    if !cfg!(target_os = "windows") {
        return Vec::new();
    }
    windows_dxdiag_adapters_from_memory(memory)
}

fn windows_dxdiag_adapters_from_memory(
    memory: &std::collections::BTreeMap<String, WindowsGpuMemorySplit>,
) -> Vec<GpuInfo> {
    memory
        .values()
        .filter(|split| !windows_dxdiag_is_software_adapter(split))
        .map(|split| {
            let vendor = windows_dxdiag_vendor(split);
            let cc = None;
            GpuInfo {
                vendor,
                name: split.name.clone(),
                backend: GpuBackend::Vulkan,
                memory_bytes: split.dedicated_bytes,
                dedicated_memory_bytes: split.dedicated_bytes,
                shared_memory_bytes: split.shared_bytes,
                unified_memory: false,
                compute_capability: cc,
                supports_nvfp4: false,
                supports_fp8: false,
                supports_tensor_parallel: false,
            }
        })
        .collect()
}

fn windows_dxdiag_vendor(split: &WindowsGpuMemorySplit) -> GpuVendor {
    let haystack = format!(
        "{} {}",
        split.name,
        split.manufacturer.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase();
    if haystack.contains("nvidia") {
        GpuVendor::Nvidia
    } else if haystack.contains("amd")
        || haystack.contains("advanced micro devices")
        || haystack.contains("radeon")
    {
        GpuVendor::Amd
    } else if haystack.contains("intel") || haystack.contains("arc graphics") {
        GpuVendor::Intel
    } else {
        GpuVendor::Unknown
    }
}

fn windows_dxdiag_is_software_adapter(split: &WindowsGpuMemorySplit) -> bool {
    let name = split.name.to_ascii_lowercase();
    name.contains("microsoft basic render")
        || name.contains("software")
        || (split.dedicated_bytes.unwrap_or(0) == 0 && split.shared_bytes.unwrap_or(0) == 0)
}

fn parse_dxdiag_memory_splits(
    text: &str,
) -> std::collections::BTreeMap<String, WindowsGpuMemorySplit> {
    let mut out = std::collections::BTreeMap::<String, WindowsGpuMemorySplit>::new();
    let mut current_name: Option<String> = None;
    let mut current = WindowsGpuMemorySplit::default();

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix("Card name:")
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            if let Some(name) = current_name.take() {
                current.name = name.clone();
                out.insert(normalize_gpu_name(&name), current);
                current = WindowsGpuMemorySplit::default();
            }
            current_name = Some(name.to_owned());
            continue;
        }
        if current_name.is_none() {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Dedicated Memory:") {
            current.dedicated_bytes = parse_memory_bytes_line(value.trim());
        } else if let Some(value) = trimmed.strip_prefix("Shared Memory:") {
            current.shared_bytes = parse_memory_bytes_line(value.trim());
        } else if let Some(value) = trimmed.strip_prefix("Manufacturer:") {
            current.manufacturer = Some(value.trim().to_owned()).filter(|value| !value.is_empty());
        }
    }
    if let Some(name) = current_name.take() {
        current.name = name.clone();
        out.insert(normalize_gpu_name(&name), current);
    }
    out
}

fn parse_windows_cim_video_controller_memory(
    text: &str,
    shared_bytes: Option<u64>,
) -> std::collections::BTreeMap<String, WindowsGpuMemorySplit> {
    let mut out = std::collections::BTreeMap::<String, WindowsGpuMemorySplit>::new();
    for line in text.lines() {
        let mut parts = line.split('|').map(str::trim);
        let Some(name) = parts.next().filter(|value| !value.is_empty()) else {
            continue;
        };
        let manufacturer = parts
            .next()
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let dedicated_bytes = parts
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|bytes| *bytes > 0);
        out.insert(
            normalize_gpu_name(name),
            WindowsGpuMemorySplit {
                name: name.to_owned(),
                manufacturer,
                dedicated_bytes,
                shared_bytes,
            },
        );
    }
    out
}

fn probe_rocm() -> Vec<GpuInfo> {
    let Some(output) = command_stdout("rocm-smi", &["--showproductname"]) else {
        return Vec::new();
    };
    let memory_bytes = command_stdout("rocm-smi", &["--showmeminfo", "vram"])
        .map(|output| parse_rocm_vram_bytes(&output))
        .unwrap_or_default();
    output
        .lines()
        .filter(|line| line.contains("GPU") && line.contains(':'))
        .enumerate()
        .map(|(idx, line)| {
            let name = line
                .split(':')
                .nth(1)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("AMD ROCm GPU")
                .to_owned();
            GpuInfo {
                vendor: GpuVendor::Amd,
                name,
                backend: GpuBackend::Rocm,
                memory_bytes: memory_bytes.get(idx).copied(),
                dedicated_memory_bytes: memory_bytes.get(idx).copied(),
                shared_memory_bytes: None,
                unified_memory: false,
                compute_capability: None,
                supports_nvfp4: false,
                supports_fp8: false,
                supports_tensor_parallel: false,
            }
        })
        .collect()
}

fn probe_vulkan() -> Vec<GpuInfo> {
    let Some(output) = command_stdout("vulkaninfo", &["--summary"]) else {
        return Vec::new();
    };
    let memory_by_name = command_stdout("vulkaninfo", &[])
        .map(|output| parse_vulkan_device_local_memory_bytes(&output))
        .unwrap_or_default();
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
                memory_bytes: memory_by_name.get(name).copied(),
                dedicated_memory_bytes: memory_by_name.get(name).copied(),
                shared_memory_bytes: None,
                unified_memory: false,
                compute_capability: None,
                supports_nvfp4: false,
                supports_fp8: false,
                supports_tensor_parallel: false,
            })
        })
        .collect()
}

fn parse_rocm_vram_bytes(output: &str) -> Vec<u64> {
    output
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("vram") && lower.contains("total")
        })
        .filter_map(parse_memory_bytes_line)
        .collect()
}

fn parse_vulkan_device_local_memory_bytes(output: &str) -> std::collections::BTreeMap<String, u64> {
    let mut memory_by_name = std::collections::BTreeMap::<String, u64>::new();
    let mut current_name: Option<String> = None;
    let mut pending_heap_size: Option<u64> = None;
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix("deviceName")
            .and_then(|line| line.split('=').nth(1))
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            current_name = Some(name.to_owned());
            pending_heap_size = None;
            continue;
        }
        if current_name.is_none() {
            continue;
        }
        if trimmed.starts_with("size") {
            pending_heap_size =
                parse_memory_bytes_line(trimmed.split('(').next().unwrap_or(trimmed));
            continue;
        }
        if trimmed.contains("DEVICE_LOCAL") {
            if let (Some(name), Some(size)) = (current_name.as_ref(), pending_heap_size.take()) {
                memory_by_name
                    .entry(name.clone())
                    .and_modify(|existing| *existing = (*existing).max(size))
                    .or_insert(size);
            }
        }
    }
    memory_by_name
}

fn parse_memory_bytes_line(line: &str) -> Option<u64> {
    let lower = line.to_ascii_lowercase();
    let number = line
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .filter(|part| !part.is_empty())
        .next_back()?;
    let value = number.parse::<f64>().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let multiplier = if lower.contains("gib") || lower.contains(" gb") || lower.ends_with("gb") {
        GIB as f64
    } else if lower.contains("mib") || lower.contains(" mb") || lower.ends_with("mb") {
        MIB as f64
    } else if lower.contains("kib") || lower.contains(" kb") || lower.ends_with("kb") {
        1024.0
    } else {
        1.0
    };
    let bytes = value * multiplier;
    (bytes <= u64::MAX as f64).then_some(bytes.round() as u64)
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
    let tier = 1;
    let mut notes = Vec::new();
    if sev_snp {
        notes.push("AMD SEV/SEV-SNP signal detected; hwprobe is signal-only and does not advertise higher tiers without a configured bound quote command".to_owned());
    }
    if tdx {
        notes.push("Intel TDX signal detected; hwprobe is signal-only and does not advertise higher tiers without a configured bound quote command".to_owned());
    }
    if gpu_confidential_compute {
        notes.push("NVIDIA confidential compute signal detected; hwprobe is signal-only and does not advertise higher tiers without a configured bound quote command".to_owned());
    }
    if apple_device_identity {
        notes.push("Apple Metal device identity signal detected; hwprobe is signal-only and does not advertise higher tiers without a configured bound quote command".to_owned());
    }
    if gb10_device_identity {
        notes.push("NVIDIA GB10 device identity signal detected; hwprobe is signal-only and does not advertise higher tiers without a configured bound quote command".to_owned());
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
                dedicated_memory_bytes: None,
                shared_memory_bytes: None,
                unified_memory: true,
                compute_capability: None,
                supports_nvfp4: false,
                supports_fp8: false,
                supports_tensor_parallel: false,
            }],
            tee: fixture_tee(1),
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
                    dedicated_memory_bytes: Some(96 * GIB),
                    shared_memory_bytes: None,
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
                    dedicated_memory_bytes: Some(96 * GIB),
                    shared_memory_bytes: None,
                    unified_memory: false,
                    compute_capability: Some("10.0".to_owned()),
                    supports_nvfp4: true,
                    supports_fp8: true,
                    supports_tensor_parallel: true,
                },
            ],
            tee: fixture_tee(1),
            warnings: Vec::new(),
        },
        FixtureProfile::LinuxNvidiaArm64 => {
            let mut profile = fixture_profile(FixtureProfile::LinuxNvidia, disk_path);
            profile.source.fixture = Some(fixture.as_str().to_owned());
            profile.host.arch = "aarch64".to_owned();
            profile.cpu = CpuInfo {
                model: Some("NVIDIA Grace".to_owned()),
                physical_cores: Some(20),
                logical_cores: 20,
                flags: CpuFlags {
                    neon: true,
                    ..CpuFlags::default()
                },
            };
            profile.memory = MemoryInfo {
                total_bytes: 128 * GIB,
                available_bytes: Some(112 * GIB),
                unified_memory: true,
            };
            profile.gpus.truncate(1);
            profile.gpus[0] = GpuInfo {
                vendor: GpuVendor::Nvidia,
                name: "NVIDIA GB10".to_owned(),
                backend: GpuBackend::Nvml,
                memory_bytes: Some(128 * GIB),
                dedicated_memory_bytes: None,
                shared_memory_bytes: None,
                unified_memory: true,
                compute_capability: Some("12.1".to_owned()),
                supports_nvfp4: true,
                supports_fp8: true,
                supports_tensor_parallel: false,
            };
            profile
        }
        FixtureProfile::WindowsNvidia => {
            let mut profile = fixture_profile(FixtureProfile::LinuxNvidia, disk_path);
            profile.source.fixture = Some(fixture.as_str().to_owned());
            profile.host = HostInfo {
                os: "windows".to_owned(),
                arch: "x86_64".to_owned(),
                family: "windows".to_owned(),
                kernel: Some("Windows 11 24H2".to_owned()),
            };
            profile.cpu = CpuInfo {
                model: Some("AMD Ryzen 9 7950X3D".to_owned()),
                physical_cores: Some(16),
                logical_cores: 32,
                flags: CpuFlags {
                    avx2: true,
                    ..CpuFlags::default()
                },
            };
            profile.memory = MemoryInfo {
                total_bytes: 64 * GIB,
                available_bytes: Some(48 * GIB),
                unified_memory: false,
            };
            profile.gpus.truncate(1);
            profile.gpus[0] = GpuInfo {
                vendor: GpuVendor::Nvidia,
                name: "NVIDIA GeForce RTX 4090".to_owned(),
                backend: GpuBackend::Nvml,
                memory_bytes: Some(24 * GIB),
                dedicated_memory_bytes: Some(24 * GIB),
                shared_memory_bytes: Some(32 * GIB),
                unified_memory: false,
                compute_capability: Some("8.9".to_owned()),
                supports_nvfp4: false,
                supports_fp8: true,
                supports_tensor_parallel: false,
            };
            profile.warnings = vec![
                "Windows WDDM shared GPU memory detected; provider capacity ignores shared memory to avoid silent paging".to_owned(),
            ];
            profile
        }
        FixtureProfile::UnsupportedHost => {
            let mut profile = fixture_profile(FixtureProfile::CpuOnly, disk_path);
            profile.source.fixture = Some(fixture.as_str().to_owned());
            profile.host = HostInfo {
                os: "freebsd".to_owned(),
                arch: "x86_64".to_owned(),
                family: "unix".to_owned(),
                kernel: Some("FreeBSD 14.2".to_owned()),
            };
            profile
        }
        FixtureProfile::InsufficientHost => {
            let mut profile = fixture_profile(FixtureProfile::CpuOnly, disk_path);
            profile.source.fixture = Some(fixture.as_str().to_owned());
            profile.memory = MemoryInfo {
                total_bytes: 8 * GIB,
                available_bytes: Some(6 * GIB),
                unified_memory: false,
            };
            profile
        }
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
            vec!["fixture hardware identity signals; hwprobe remains signal-only without a configured bound quote command".to_owned()]
        } else {
            vec![
                "fixture Tier 1 software-rooted attestation; no hardware quote command configured"
                    .to_owned(),
            ]
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
                "  - {:?} {:?}: {} usable={} dedicated={} shared={} unified={} cc={} nvfp4={} fp8={} tp={}",
                gpu.vendor,
                gpu.backend,
                gpu.name,
                gpu.memory_bytes
                    .map(format_bytes)
                    .unwrap_or_else(|| "unknown".to_owned()),
                gpu.dedicated_memory_bytes
                    .map(format_bytes)
                    .unwrap_or_else(|| "n/a".to_owned()),
                gpu.shared_memory_bytes
                    .map(format_bytes)
                    .unwrap_or_else(|| "n/a".to_owned()),
                yes_no(gpu.unified_memory),
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

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_memory_probe_reports_physical_memory_without_cim() {
        let total = windows_memory_bytes(0).expect("Windows physical memory");
        let available = windows_memory_bytes(1).expect("Windows available memory");
        assert!(total > 0);
        assert!(available <= total);
    }

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
    fn model_memory_fit_exposes_the_context_ceiling() {
        let fit = model_memory_fit(10_000, 4_000, 1_000, 10, 500);
        assert_eq!(fit.max_safe_context, 500);
        assert_eq!(fit.required_bytes, 10_000);
        assert_eq!(fit.status, VerdictStatus::FullOffload);

        let too_large = model_memory_fit(10_000, 4_000, 1_000, 10, 501);
        assert_eq!(too_large.max_safe_context, 500);
        assert_eq!(too_large.status, VerdictStatus::Insufficient);
    }

    #[test]
    fn model_memory_fit_refuses_when_fixed_memory_does_not_fit() {
        let fit = model_memory_fit(4_999, 4_000, 1_000, 10, 0);
        assert_eq!(fit.max_safe_context, 0);
        assert_eq!(fit.status, VerdictStatus::Insufficient);
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
        assert_eq!(report.tee.tier, 1);
        assert!(report
            .tee
            .notes
            .iter()
            .any(|note| note.contains("hardware quote command")));
    }

    #[test]
    fn blackwell_fixture_prefers_vllm_with_nvfp4_and_tensor_parallel() {
        let report = fixture_report(FixtureProfile::LinuxNvidia);
        assert_eq!(report.selected_backend.as_deref(), Some("vllm"));
        let vllm = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "vllm")
            .unwrap();
        assert_eq!(vllm.status, VerdictStatus::FullOffload);
        let trt = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "trt-llm")
            .unwrap();
        assert_eq!(trt.status, VerdictStatus::FullOffload);
        assert!(report.gpus.iter().all(|gpu| gpu.supports_nvfp4));
        assert!(report.gpus.iter().all(|gpu| gpu.supports_tensor_parallel));
        assert_eq!(report.tee.tier, 1);
        assert!(report
            .tee
            .notes
            .iter()
            .any(|note| note.contains("hardware quote command")));
    }

    #[test]
    fn nvidia_unified_memory_supports_vllm_without_dedicated_vram_counter() {
        let mut profile = fixture_profile(FixtureProfile::LinuxNvidia, Path::new("."));
        profile.host.arch = "aarch64".to_owned();
        profile.memory.total_bytes = 128 * GIB;
        profile.memory.available_bytes = Some(112 * GIB);
        profile.memory.unified_memory = false;
        profile.gpus.truncate(1);
        profile.gpus[0].name = "NVIDIA GB10".to_owned();
        profile.gpus[0].memory_bytes = None;
        profile.gpus[0].dedicated_memory_bytes = None;
        profile.gpus[0].unified_memory = false;
        profile.gpus[0].compute_capability = Some("12.1".to_owned());
        let report = report_from_profile(profile);
        let vllm = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "vllm")
            .unwrap();
        assert_eq!(vllm.status, VerdictStatus::FullOffload);
        assert!(vllm
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("unified memory"));
        assert_eq!(report.selected_backend.as_deref(), Some("vllm"));
    }

    #[test]
    fn nvidia_arm64_no_vram_counter_is_marked_unified_memory() {
        let mut profile = fixture_profile(FixtureProfile::LinuxNvidia, Path::new("."));
        profile.host.arch = "aarch64".to_owned();
        profile.memory.unified_memory = false;
        profile.gpus.truncate(1);
        profile.gpus[0].name = "NVIDIA GB10".to_owned();
        profile.gpus[0].memory_bytes = None;
        profile.gpus[0].dedicated_memory_bytes = None;
        profile.gpus[0].unified_memory = false;

        mark_nvidia_host_unified_memory(&profile.host, &mut profile.memory, &mut profile.gpus);

        assert!(profile.memory.unified_memory);
        assert!(profile.gpus[0].unified_memory);
    }

    #[test]
    fn partial_offload_layers_scale_with_gpu_memory() {
        let mut small = fixture_profile(FixtureProfile::LinuxNvidia, Path::new("."));
        small.gpus.truncate(1);
        small.gpus[0].memory_bytes = Some(8 * GIB);
        small.gpus[0].dedicated_memory_bytes = Some(8 * GIB);
        let small_report = report_from_profile(small);
        let small_layers = small_report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "llama.cpp")
            .and_then(|verdict| verdict.n_layers_gpu)
            .unwrap();

        let mut large = fixture_profile(FixtureProfile::LinuxNvidia, Path::new("."));
        large.gpus.truncate(1);
        large.gpus[0].memory_bytes = Some(24 * GIB);
        large.gpus[0].dedicated_memory_bytes = Some(24 * GIB);
        let large_report = report_from_profile(large);
        let large_layers = large_report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "llama.cpp")
            .and_then(|verdict| verdict.n_layers_gpu)
            .unwrap();

        assert!(large_layers > small_layers);
    }

    #[test]
    fn sulphur_verdict_is_appended_without_changing_backend_precedence() {
        for (fixture, selected) in [
            (FixtureProfile::AppleSilicon, "mlx"),
            (FixtureProfile::LinuxNvidia, "vllm"),
            (FixtureProfile::LinuxNvidiaArm64, "vllm"),
            (FixtureProfile::WindowsNvidia, "llama.cpp"),
        ] {
            let report = fixture_report(fixture);
            assert_eq!(report.selected_backend.as_deref(), Some(selected));
            assert_eq!(
                report
                    .backend_verdicts
                    .last()
                    .map(|verdict| verdict.backend.as_str()),
                Some("sulphur")
            );
        }
    }

    #[test]
    fn comfyui_verdict_is_available_without_changing_backend_precedence() {
        for (fixture, selected) in [
            (FixtureProfile::AppleSilicon, "mlx"),
            (FixtureProfile::LinuxNvidia, "vllm"),
            (FixtureProfile::LinuxNvidiaArm64, "vllm"),
            (FixtureProfile::WindowsNvidia, "llama.cpp"),
            (FixtureProfile::CpuOnly, "llama.cpp"),
        ] {
            let report = fixture_report(fixture);
            assert_eq!(report.selected_backend.as_deref(), Some(selected));
            let verdict = report
                .backend_verdicts
                .iter()
                .find(|verdict| verdict.backend == "comfyui")
                .expect("comfyui verdict");
            assert_ne!(verdict.status, VerdictStatus::Insufficient);
            assert_eq!(verdict.max_sessions, 1);
        }
    }

    #[test]
    fn sulphur_windows_x86_64_cuda_uses_only_dedicated_memory() {
        let profile = fixture_profile(FixtureProfile::WindowsNvidia, Path::new("."));
        let verdict = sulphur_verdict(&profile);

        assert_eq!(verdict.status, VerdictStatus::FullOffload);
        assert_eq!(verdict.est_tok_s, None);
        assert_eq!(verdict.n_layers_gpu, None);
        assert_eq!(verdict.max_sessions, 1);
        assert_eq!(verdict.kv_cache_bytes_budget, 0);
        let reason = verdict.reason.unwrap_or_default();
        assert!(reason.contains("CUDA GGUF"));
        assert!(reason.contains("24.0 GiB dedicated device memory"));
        assert!(reason.contains("artifact-calibration-gated"));

        let mut partial = fixture_profile(FixtureProfile::WindowsNvidia, Path::new("."));
        partial.gpus[0].memory_bytes = Some(20 * GIB);
        partial.gpus[0].dedicated_memory_bytes = Some(20 * GIB);
        partial.gpus[0].shared_memory_bytes = Some(128 * GIB);
        assert_eq!(
            sulphur_verdict(&partial).status,
            VerdictStatus::PartialOffload
        );

        let mut insufficient = partial;
        insufficient.gpus[0].memory_bytes = Some(12 * GIB);
        insufficient.gpus[0].dedicated_memory_bytes = Some(12 * GIB);
        let verdict = sulphur_verdict(&insufficient);
        assert_eq!(verdict.status, VerdictStatus::Insufficient);
        let reason = verdict.reason.unwrap_or_default();
        assert!(reason.contains("12.0 GiB dedicated device memory"));
        assert!(reason.contains("WDDM shared GPU memory"));
        assert!(reason.contains("no CPU fallback"));
    }

    #[test]
    fn sulphur_linux_cuda_supports_x86_64_and_gb10_aarch64() {
        for fixture in [
            FixtureProfile::LinuxNvidia,
            FixtureProfile::LinuxNvidiaArm64,
        ] {
            let profile = fixture_profile(fixture, Path::new("."));
            let verdict = sulphur_verdict(&profile);
            assert_eq!(verdict.status, VerdictStatus::FullOffload);
            assert_eq!(verdict.est_tok_s, None);
            assert_eq!(verdict.max_sessions, 1);
            assert!(verdict
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("artifact-calibration-gated"));

            let mut insufficient = profile;
            for gpu in &mut insufficient.gpus {
                gpu.memory_bytes = Some(12 * GIB);
                gpu.dedicated_memory_bytes = Some(12 * GIB);
            }
            insufficient.memory.available_bytes = Some(12 * GIB);
            let verdict = sulphur_verdict(&insufficient);
            assert_eq!(verdict.status, VerdictStatus::Insufficient);
            assert!(verdict
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("no CPU fallback"));
        }
    }

    #[test]
    fn sulphur_macos_arm64_mlx_requires_64_gib_available_unified_memory() {
        let insufficient = fixture_profile(FixtureProfile::AppleSilicon, Path::new("."));
        let verdict = sulphur_verdict(&insufficient);
        assert_eq!(verdict.status, VerdictStatus::Insufficient);
        assert!(verdict
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("52.0 GiB is available"));

        let mut viable = insufficient;
        viable.memory.total_bytes = 96 * GIB;
        viable.memory.available_bytes = Some(64 * GIB);
        viable.gpus[0].memory_bytes = Some(96 * GIB);
        let verdict = sulphur_verdict(&viable);
        assert_eq!(verdict.status, VerdictStatus::FullOffload);
        assert_eq!(verdict.est_tok_s, None);
        assert_eq!(verdict.max_sessions, 1);
        let reason = verdict.reason.unwrap_or_default();
        assert!(reason.contains("Apple Silicon MLX"));
        assert!(reason.contains("64.0 GiB available unified memory"));
        assert!(reason.contains("artifact-calibration-gated"));
    }

    #[test]
    fn sulphur_never_claims_a_cpu_fallback() {
        let profile = fixture_profile(FixtureProfile::CpuOnly, Path::new("."));
        let verdict = sulphur_verdict(&profile);

        assert_eq!(verdict.status, VerdictStatus::Insufficient);
        assert_eq!(verdict.max_sessions, 0);
        assert!(verdict
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("no CPU fallback"));
    }

    #[test]
    fn ace_step_windows_4090_uses_cuda_without_counting_wddm_shared_memory() {
        let report = fixture_report(FixtureProfile::WindowsNvidia);
        let verdict = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "ace-step")
            .expect("ace-step verdict");

        assert_eq!(verdict.backend, "ace-step");
        assert_eq!(verdict.status, VerdictStatus::FullOffload);
        assert_eq!(verdict.max_sessions, 1);
        assert_eq!(verdict.est_tok_s, None);
        let reason = verdict.reason.as_deref().unwrap_or_default();
        assert!(reason.contains("CUDA"));
        assert!(reason.contains("windows/x86_64"));
        assert!(reason.contains("24.0 GiB detected device memory"));
        assert!(reason.contains("rechecks load-time free memory"));
        assert!(reason.contains("throughput comes from artifact calibration"));
        assert!(reason.contains("model-specific CLI RAM/VRAM checks still apply"));
    }

    #[test]
    fn ace_step_uses_cuda_on_linux_x86_and_aarch64() {
        for (fixture, arch) in [
            (FixtureProfile::LinuxNvidia, "x86_64"),
            (FixtureProfile::LinuxNvidiaArm64, "aarch64"),
        ] {
            let profile = fixture_profile(fixture, Path::new("."));

            let verdict = ace_step_verdict(&profile);

            assert_eq!(verdict.status, VerdictStatus::FullOffload, "{arch}");
            let reason = verdict.reason.unwrap_or_default();
            assert!(reason.contains("CUDA"), "{arch}");
            assert!(reason.contains(&format!("linux/{arch}")), "{arch}");
            assert!(reason.contains("detected device memory"), "{arch}");
            assert!(reason.contains("rechecks load-time free memory"), "{arch}");
            assert!(
                reason.contains("throughput comes from artifact calibration"),
                "{arch}"
            );
        }
    }

    #[test]
    fn ace_step_uses_apple_silicon_mps() {
        let profile = fixture_profile(FixtureProfile::AppleSilicon, Path::new("."));

        let verdict = ace_step_verdict(&profile);

        assert_eq!(verdict.status, VerdictStatus::FullOffload);
        assert_eq!(verdict.est_tok_s, None);
        let reason = verdict.reason.unwrap_or_default();
        assert!(reason.contains("Apple Silicon Metal/MPS"));
        assert!(reason.contains("detected available unified memory"));
        assert!(reason.contains("throughput comes from artifact calibration"));
    }

    #[test]
    fn ace_step_cpu_only_fixture_reports_slow_supported_fallback() {
        let profile = fixture_profile(FixtureProfile::CpuOnly, Path::new("."));

        let verdict = ace_step_verdict(&profile);

        assert_eq!(verdict.status, VerdictStatus::CpuOnly);
        assert_eq!(verdict.n_layers_gpu, Some(0));
        assert_eq!(verdict.est_tok_s, None);
        let reason = verdict.reason.unwrap_or_default();
        assert!(reason.contains("CPU fallback"));
        assert!(reason.contains("at least 16 GiB available host memory"));
        assert!(reason.contains("throughput comes from artifact calibration"));
        assert!(reason.contains("no supported accelerator probe was detected"));
    }

    #[test]
    fn ace_step_low_vram_cuda_reports_partial_offload() {
        let mut profile = fixture_profile(FixtureProfile::LinuxNvidia, Path::new("."));
        profile.gpus.truncate(1);
        profile.gpus[0].memory_bytes = Some(6 * GIB);
        profile.gpus[0].dedicated_memory_bytes = Some(6 * GIB);

        let verdict = ace_step_verdict(&profile);

        assert_eq!(verdict.status, VerdictStatus::PartialOffload);
        let reason = verdict.reason.unwrap_or_default();
        assert!(reason.contains("6.0 GiB detected device memory"));
        assert!(reason.contains("supports CPU/INT8 offload"));
        assert!(reason.contains("rechecks load-time free memory"));
        assert!(reason.contains("throughput comes from artifact calibration"));
    }

    #[test]
    fn ace_step_does_not_infer_acceleration_from_linux_arm_or_nvidia_name() {
        let mut profile = fixture_profile(FixtureProfile::LinuxNvidiaArm64, Path::new("."));
        profile.gpus[0].backend = GpuBackend::Vulkan;

        let verdict = ace_step_verdict(&profile);

        assert_eq!(verdict.status, VerdictStatus::CpuOnly);
        let reason = verdict.reason.unwrap_or_default();
        assert!(reason.contains("no supported accelerator probe was detected"));
        assert!(!reason.contains("CUDA hardware path available"));
    }

    #[test]
    fn ace_step_does_not_count_windows_shared_memory_as_cuda_capacity() {
        let mut profile = fixture_profile(FixtureProfile::WindowsNvidia, Path::new("."));
        profile.gpus[0].memory_bytes = Some(3 * GIB);
        profile.gpus[0].dedicated_memory_bytes = Some(3 * GIB);
        profile.gpus[0].shared_memory_bytes = Some(64 * GIB);

        let verdict = ace_step_verdict(&profile);

        assert_eq!(verdict.status, VerdictStatus::CpuOnly);
        assert!(verdict
            .reason
            .unwrap_or_default()
            .contains("usable CUDA memory is below 4 GiB"));
    }

    #[test]
    fn ace_step_rejects_unsupported_and_insufficient_fixtures() {
        let unsupported = fixture_profile(FixtureProfile::UnsupportedHost, Path::new("."));
        let unsupported_verdict = ace_step_verdict(&unsupported);
        assert_eq!(unsupported_verdict.status, VerdictStatus::Insufficient);
        assert!(unsupported_verdict
            .reason
            .unwrap_or_default()
            .contains("no supported runtime path for freebsd/x86_64"));

        let insufficient = fixture_profile(FixtureProfile::InsufficientHost, Path::new("."));
        let insufficient_verdict = ace_step_verdict(&insufficient);
        assert_eq!(insufficient_verdict.status, VerdictStatus::Insufficient);
        assert!(insufficient_verdict
            .reason
            .unwrap_or_default()
            .contains("at least 16 GiB available host RAM"));
    }

    #[test]
    fn ace_step_fixture_names_are_publicly_parseable() {
        for (name, expected) in [
            ("windows-4090", FixtureProfile::WindowsNvidia),
            ("linux-nvidia", FixtureProfile::LinuxNvidia),
            ("linux-aarch64", FixtureProfile::LinuxNvidiaArm64),
            ("apple-silicon", FixtureProfile::AppleSilicon),
            ("cpu-only", FixtureProfile::CpuOnly),
            ("unsupported-host", FixtureProfile::UnsupportedHost),
            ("insufficient-host", FixtureProfile::InsufficientHost),
        ] {
            assert_eq!(FixtureProfile::parse(name), Some(expected), "{name}");
        }
    }

    #[test]
    fn chatterbox_supports_cuda_mps_and_cpu_without_admin_setup() {
        for fixture in [
            FixtureProfile::LinuxNvidia,
            FixtureProfile::LinuxNvidiaArm64,
            FixtureProfile::WindowsNvidia,
        ] {
            let profile = fixture_profile(fixture, Path::new("."));
            let verdict = chatterbox_verdict(&profile);
            assert_eq!(verdict.status, VerdictStatus::FullOffload, "{fixture:?}");
            assert_eq!(verdict.max_sessions, 1);
            assert!(verdict
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("CUDA"));
        }

        let apple = chatterbox_verdict(&fixture_profile(
            FixtureProfile::AppleSilicon,
            Path::new("."),
        ));
        assert_eq!(apple.status, VerdictStatus::FullOffload);
        assert!(apple
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("Metal/MPS"));

        let cpu = chatterbox_verdict(&fixture_profile(FixtureProfile::CpuOnly, Path::new(".")));
        assert_eq!(cpu.status, VerdictStatus::CpuOnly);
        assert_eq!(cpu.n_layers_gpu, Some(0));
        assert_eq!(cpu.max_sessions, 1);
        assert!(cpu
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("CPU fallback"));
    }

    #[test]
    fn chatterbox_public_managed_device_matches_runtime_artifacts() {
        for (fixture, expected) in [
            (FixtureProfile::LinuxNvidia, ChatterboxManagedDevice::Cuda),
            (
                FixtureProfile::LinuxNvidiaArm64,
                ChatterboxManagedDevice::Cuda,
            ),
            (FixtureProfile::WindowsNvidia, ChatterboxManagedDevice::Cuda),
            (FixtureProfile::AppleSilicon, ChatterboxManagedDevice::Mps),
            (FixtureProfile::CpuOnly, ChatterboxManagedDevice::Cpu),
        ] {
            let report = report_from_profile(fixture_profile(fixture, Path::new(".")));
            assert_eq!(
                chatterbox_managed_device(&report),
                Some(expected),
                "{fixture:?}"
            );
        }
        let unsupported = report_from_profile(fixture_profile(
            FixtureProfile::UnsupportedHost,
            Path::new("."),
        ));
        assert_eq!(chatterbox_managed_device(&unsupported), None);
    }

    #[test]
    fn chatterbox_rejects_unsupported_or_low_memory_hosts() {
        let unsupported = chatterbox_verdict(&fixture_profile(
            FixtureProfile::UnsupportedHost,
            Path::new("."),
        ));
        assert_eq!(unsupported.status, VerdictStatus::Insufficient);
        assert!(unsupported
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("freebsd/x86_64"));

        let mut intel_macos = fixture_profile(FixtureProfile::AppleSilicon, Path::new("."));
        intel_macos.host.arch = "x86_64".to_owned();
        let intel_macos = chatterbox_verdict(&intel_macos);
        assert_eq!(intel_macos.status, VerdictStatus::Insufficient);
        assert!(intel_macos
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("macos/x86_64"));

        let low_memory = chatterbox_verdict(&fixture_profile(
            FixtureProfile::InsufficientHost,
            Path::new("."),
        ));
        assert_eq!(low_memory.status, VerdictStatus::Insufficient);
        assert!(low_memory
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("at least 8 GiB"));
    }

    #[test]
    fn transformers_asr_uses_cuda_on_linux_and_windows() {
        for os in ["linux", "windows"] {
            let mut profile = fixture_profile(FixtureProfile::LinuxNvidia, Path::new("."));
            profile.host.os = os.to_owned();
            profile.host.family = if os == "windows" { "windows" } else { "unix" }.to_owned();
            profile.gpus.truncate(1);
            profile.gpus[0].memory_bytes = Some(4 * GIB);
            profile.gpus[0].dedicated_memory_bytes = Some(4 * GIB);

            let verdict = transformers_asr_verdict(&profile);

            assert_eq!(verdict.status, VerdictStatus::FullOffload, "{os}");
            assert!(verdict.reason.unwrap_or_default().contains("CUDA"), "{os}");
            assert_eq!(verdict.max_sessions, 1, "{os}");
            assert_eq!(verdict.n_layers_gpu, None, "{os}");
        }
    }

    #[test]
    fn transformers_asr_uses_mps_only_for_apple_unified_metal() {
        let profile = fixture_profile(FixtureProfile::AppleSilicon, Path::new("."));

        let verdict = transformers_asr_verdict(&profile);

        assert_eq!(verdict.status, VerdictStatus::FullOffload);
        assert!(verdict.reason.unwrap_or_default().contains("Metal/MPS"));
        assert_eq!(verdict.max_sessions, 1);
        assert_eq!(verdict.n_layers_gpu, None);
    }

    #[test]
    fn transformers_asr_supports_desktop_cpu_execution() {
        for os in ["linux", "windows", "macos"] {
            let mut profile = fixture_profile(FixtureProfile::CpuOnly, Path::new("."));
            profile.host.os = os.to_owned();
            profile.host.family = if os == "windows" { "windows" } else { "unix" }.to_owned();

            let verdict = transformers_asr_verdict(&profile);

            assert_eq!(verdict.status, VerdictStatus::CpuOnly, "{os}");
            assert!(verdict.reason.unwrap_or_default().contains("CPU execution"));
            assert_eq!(verdict.max_sessions, 1, "{os}");
            assert_eq!(verdict.n_layers_gpu, Some(0), "{os}");
        }
    }

    #[test]
    fn transformers_asr_enforces_float32_host_ram_floor() {
        let mut profile = fixture_profile(FixtureProfile::LinuxNvidia, Path::new("."));
        profile.memory.total_bytes = TRANSFORMERS_ASR_RAM_FLOOR - 1;
        profile.memory.available_bytes = Some(TRANSFORMERS_ASR_RAM_FLOOR - 1);

        let verdict = transformers_asr_verdict(&profile);

        assert_eq!(verdict.status, VerdictStatus::Insufficient);
        assert_eq!(verdict.max_sessions, 0);
        assert!(verdict.reason.unwrap_or_default().contains("8 GiB RAM"));
    }

    #[test]
    fn transformers_asr_does_not_count_wddm_shared_memory_as_cuda_capacity() {
        let mut profile = fixture_profile(FixtureProfile::LinuxNvidia, Path::new("."));
        profile.host.os = "windows".to_owned();
        profile.host.family = "windows".to_owned();
        profile.gpus.truncate(1);
        profile.gpus[0].memory_bytes = Some(3 * GIB);
        profile.gpus[0].dedicated_memory_bytes = Some(3 * GIB);
        profile.gpus[0].shared_memory_bytes = Some(32 * GIB);

        let verdict = transformers_asr_verdict(&profile);

        assert_eq!(verdict.status, VerdictStatus::CpuOnly);
        assert_eq!(verdict.n_layers_gpu, Some(0));
        assert!(verdict.reason.unwrap_or_default().contains("below 4 GiB"));
    }

    #[test]
    fn needle_cpu_only_host_gets_cpu_but_not_gpu() {
        let report = fixture_report(FixtureProfile::CpuOnly);
        let cpu = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "needle-cpu")
            .unwrap();
        let gpu = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "needle-gpu")
            .unwrap();

        assert_eq!(cpu.status, VerdictStatus::CpuOnly);
        assert_eq!(cpu.est_tok_s, Some(50.0));
        assert_eq!(gpu.status, VerdictStatus::Insufficient);
    }

    #[test]
    fn needle_nvidia_hosts_get_cpu_and_gpu_in_deterministic_order() {
        for fixture in [
            FixtureProfile::LinuxNvidiaArm64,
            FixtureProfile::LinuxNvidia,
            FixtureProfile::WindowsNvidia,
        ] {
            let report = fixture_report(fixture);
            let needle = report
                .backend_verdicts
                .iter()
                .filter(|verdict| verdict.backend.starts_with("needle-"))
                .collect::<Vec<_>>();

            assert_eq!(needle.len(), 2, "{}", fixture.as_str());
            assert_eq!(needle[0].backend, "needle-cpu", "{}", fixture.as_str());
            assert_eq!(
                needle[0].status,
                VerdictStatus::CpuOnly,
                "{}",
                fixture.as_str()
            );
            assert_eq!(needle[1].backend, "needle-gpu", "{}", fixture.as_str());
            assert_eq!(
                needle[1].status,
                VerdictStatus::FullOffload,
                "{}",
                fixture.as_str()
            );
            assert_eq!(needle[1].est_tok_s, Some(200.0), "{}", fixture.as_str());
        }
    }

    #[test]
    fn needle_apple_host_uses_cpu_and_rejects_inefficient_mps() {
        let report = fixture_report(FixtureProfile::AppleSilicon);
        let cpu = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "needle-cpu")
            .unwrap();
        let gpu = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "needle-gpu")
            .unwrap();

        assert_eq!(cpu.status, VerdictStatus::CpuOnly);
        assert_eq!(cpu.n_layers_gpu, Some(0));
        assert_eq!(gpu.status, VerdictStatus::Insufficient);
        assert_eq!(gpu.est_tok_s, None);
        assert!(gpu
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("slower than CPU"));
    }

    #[test]
    fn needle_rejects_unsupported_architectures() {
        let mut profile = fixture_profile(FixtureProfile::CpuOnly, Path::new("."));
        profile.host.arch = "i686".to_owned();

        assert_eq!(
            needle_cpu_verdict(&profile).status,
            VerdictStatus::Insufficient
        );
        assert_eq!(
            needle_gpu_verdict(&profile).status,
            VerdictStatus::Insufficient
        );

        profile.host.os = "macos".to_owned();
        profile.host.arch = "x86_64".to_owned();
        assert_eq!(
            needle_cpu_verdict(&profile).status,
            VerdictStatus::Insufficient
        );
    }

    #[test]
    fn parses_rocm_vram_totals() {
        let output = r#"
GPU[0]          : VRAM Total Memory (B): 17163091968
GPU[1]          : VRAM Total Memory (MiB): 24576
"#;
        assert_eq!(
            parse_rocm_vram_bytes(output),
            vec![17_163_091_968, 24_576 * MIB]
        );
    }

    #[test]
    fn parses_vulkan_device_local_heap_memory() {
        let output = r#"
VkPhysicalDeviceProperties:
    deviceName        = AMD Radeon Test
VkPhysicalDeviceMemoryProperties:
    memoryHeaps[0]:
        size          = 8589934592 (0x200000000)
        flags: count = 1
            MEMORY_HEAP_DEVICE_LOCAL_BIT
VkPhysicalDeviceProperties:
    deviceName        = Software Rasterizer
VkPhysicalDeviceMemoryProperties:
    memoryHeaps[0]:
        size          = 1024 MiB
        flags: count = 0
"#;
        let parsed = parse_vulkan_device_local_memory_bytes(output);
        assert_eq!(parsed.get("AMD Radeon Test"), Some(&(8 * GIB)));
        assert!(!parsed.contains_key("Software Rasterizer"));
    }

    #[test]
    fn parses_dxdiag_dedicated_and_shared_memory() {
        let output = r#"
---------------
Display Devices
---------------
           Card name: NVIDIA GeForce RTX 4090
        Manufacturer: NVIDIA
        Display Memory: 56950 MB
      Dedicated Memory: 24142 MB
          Shared Memory: 32808 MB
           Card name: Microsoft Basic Render Driver
        Manufacturer: Microsoft
      Dedicated Memory: 0 MB
          Shared Memory: 32768 MB
"#;
        let parsed = parse_dxdiag_memory_splits(output);
        let split = windows_memory_split_for_name(&parsed, "NVIDIA GeForce RTX 4090").unwrap();
        assert_eq!(split.name, "NVIDIA GeForce RTX 4090");
        assert_eq!(split.manufacturer.as_deref(), Some("NVIDIA"));
        assert_eq!(split.dedicated_bytes, Some(24_142 * MIB));
        assert_eq!(split.shared_bytes, Some(32_808 * MIB));
    }

    #[test]
    fn windows_dxdiag_reports_non_nvidia_adapters_without_counting_shared_memory() {
        let output = r#"
           Card name: AMD Radeon RX 7900 XTX
        Manufacturer: Advanced Micro Devices, Inc.
      Dedicated Memory: 24576 MB
          Shared Memory: 32768 MB
           Card name: Intel(R) UHD Graphics
        Manufacturer: Intel Corporation
      Dedicated Memory: 128 MB
          Shared Memory: 32768 MB
           Card name: Microsoft Basic Render Driver
        Manufacturer: Microsoft
      Dedicated Memory: 0 MB
          Shared Memory: 32768 MB
"#;
        let parsed = parse_dxdiag_memory_splits(output);
        let adapters = windows_dxdiag_adapters_from_memory(&parsed);

        assert_eq!(adapters.len(), 2);
        let amd = adapters
            .iter()
            .find(|gpu| gpu.name.contains("7900"))
            .expect("amd adapter");
        assert_eq!(amd.vendor, GpuVendor::Amd);
        assert_eq!(amd.backend, GpuBackend::Vulkan);
        assert_eq!(amd.memory_bytes, Some(24_576 * MIB));
        assert_eq!(amd.shared_memory_bytes, Some(32_768 * MIB));

        let intel = adapters
            .iter()
            .find(|gpu| gpu.name.contains("Intel"))
            .expect("intel adapter");
        assert_eq!(intel.vendor, GpuVendor::Intel);
        assert_eq!(intel.memory_bytes, Some(128 * MIB));
        assert_eq!(intel.shared_memory_bytes, Some(32_768 * MIB));
    }

    #[test]
    fn parses_windows_cim_video_controller_memory_with_shared_ceiling() {
        let output = r#"
NVIDIA GeForce RTX 4090|NVIDIA|4293918720
AMD Radeon(TM) Graphics|Advanced Micro Devices, Inc.|536870912
"#;
        let parsed = parse_windows_cim_video_controller_memory(output, Some(32 * GIB));

        let nvidia = windows_memory_split_for_name(&parsed, "NVIDIA GeForce RTX 4090").unwrap();
        assert_eq!(nvidia.manufacturer.as_deref(), Some("NVIDIA"));
        assert_eq!(nvidia.dedicated_bytes, Some(4_293_918_720));
        assert_eq!(nvidia.shared_bytes, Some(32 * GIB));

        let amd = windows_memory_split_for_name(&parsed, "AMD Radeon(TM) Graphics").unwrap();
        assert_eq!(
            amd.manufacturer.as_deref(),
            Some("Advanced Micro Devices, Inc.")
        );
        assert_eq!(amd.dedicated_bytes, Some(536_870_912));
        assert_eq!(amd.shared_bytes, Some(32 * GIB));
    }

    #[test]
    fn windows_shared_memory_only_gpu_falls_back_to_cpu_not_partial_offload() {
        let mut profile = fixture_profile(FixtureProfile::CpuOnly, Path::new("."));
        profile.host.os = "windows".to_owned();
        profile.host.family = "windows".to_owned();
        profile.memory.total_bytes = 32 * GIB;
        profile.memory.available_bytes = Some(24 * GIB);
        profile.gpus = vec![GpuInfo {
            vendor: GpuVendor::Unknown,
            name: "Intel(R) UHD Graphics".to_owned(),
            backend: GpuBackend::Vulkan,
            memory_bytes: Some(128 * MIB),
            dedicated_memory_bytes: Some(128 * MIB),
            shared_memory_bytes: Some(16 * GIB),
            unified_memory: false,
            compute_capability: None,
            supports_nvfp4: false,
            supports_fp8: false,
            supports_tensor_parallel: false,
        }];

        let report = report_from_profile(profile);
        let llama = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "llama.cpp")
            .unwrap();
        assert_eq!(llama.status, VerdictStatus::CpuOnly);
        assert_eq!(llama.n_layers_gpu, Some(0));
        assert!(llama
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("WDDM shared GPU memory is ignored"));
    }

    #[test]
    fn windows_wddm_shared_memory_is_reported_but_not_used_for_capacity() {
        let mut profile = fixture_profile(FixtureProfile::LinuxNvidia, Path::new("."));
        profile.host.os = "windows".to_owned();
        profile.host.family = "windows".to_owned();
        profile.memory.total_bytes = 64 * GIB;
        profile.memory.available_bytes = Some(48 * GIB);
        profile.gpus.truncate(1);
        profile.gpus[0].name = "NVIDIA GeForce RTX 4090".to_owned();
        profile.gpus[0].memory_bytes = Some(24 * GIB);
        profile.gpus[0].dedicated_memory_bytes = Some(24 * GIB);
        profile.gpus[0].shared_memory_bytes = Some(32 * GIB);
        profile.gpus[0].compute_capability = Some("8.9".to_owned());
        profile.warnings.push(
            "Windows WDDM shared GPU memory detected; provider capacity ignores shared memory to avoid silent paging".to_owned(),
        );

        let report = report_from_profile(profile);
        let vllm = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "vllm")
            .unwrap();
        let trt = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "trt-llm")
            .unwrap();
        let llama = report
            .backend_verdicts
            .iter()
            .find(|verdict| verdict.backend == "llama.cpp")
            .unwrap();

        assert_eq!(report.selected_backend.as_deref(), Some("llama.cpp"));
        assert_eq!(vllm.status, VerdictStatus::Insufficient);
        assert_eq!(trt.status, VerdictStatus::Insufficient);
        assert_eq!(llama.status, VerdictStatus::PartialOffload);
        assert_eq!(llama.kv_cache_bytes_budget, 8 * GIB);
        assert!(vllm
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("not selected on Windows"));
        let human = human_report(&report);
        assert!(human.contains("dedicated=24.0 GiB"));
        assert!(human.contains("shared=32.0 GiB"));
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
