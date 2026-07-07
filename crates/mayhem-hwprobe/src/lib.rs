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
        vllm_verdict(profile),
        trt_llm_verdict(profile),
        mlx_verdict(profile),
        llama_cpp_verdict(profile),
        stable_diffusion_cpp_verdict(profile),
        whisper_cpp_verdict(profile),
        piper_verdict(profile),
    ]
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

fn windows_memory_bytes(index: usize) -> Option<u64> {
    if !cfg!(target_os = "windows") {
        return None;
    }
    let output = command_stdout(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "$m=Get-CimInstance Win32_OperatingSystem; [Console]::WriteLine([string]$m.TotalVisibleMemorySize + ',' + [string]$m.FreePhysicalMemory)",
        ],
    )?;
    let kib = output.split(',').nth(index)?.trim().parse::<u64>().ok()?;
    kib.checked_mul(1024)
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
        let split = windows_memory_split_for_name(&windows_memory, parts[0]);
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
        notes.push("AMD SEV/SEV-SNP signal detected; Mayhem launch keeps this at Tier 1 until real vendor quote generation is wired".to_owned());
    }
    if tdx {
        notes.push("Intel TDX signal detected; Mayhem launch keeps this at Tier 1 until real vendor quote generation is wired".to_owned());
    }
    if gpu_confidential_compute {
        notes.push("NVIDIA confidential compute signal detected; Mayhem launch keeps this at Tier 1 until real vendor quote generation is wired".to_owned());
    }
    if apple_device_identity {
        notes.push("Apple Metal device identity signal detected; not advertised above Tier 1 until real App Attest quote generation is wired".to_owned());
    }
    if gb10_device_identity {
        notes.push("NVIDIA GB10 device identity signal detected; not advertised above Tier 1 until real device quote generation is wired".to_owned());
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
            vec!["fixture hardware identity signals; launch advertises Tier 1 until real quote generation is wired".to_owned()]
        } else {
            vec!["fixture Tier 1 software-rooted attestation; launch advertises Tier 1 until real quote generation is wired".to_owned()]
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
        assert_eq!(report.tee.tier, 1);
        assert!(report
            .tee
            .notes
            .iter()
            .any(|note| note.contains("launch advertises Tier 1")));
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
            .any(|note| note.contains("launch advertises Tier 1")));
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
        assert!(parsed.get("Software Rasterizer").is_none());
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
