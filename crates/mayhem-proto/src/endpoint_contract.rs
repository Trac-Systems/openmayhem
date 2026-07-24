use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::{json, Value};

use crate::{
    validated_audio_metadata, EndpointAttributeSpec, EndpointFamilyContract, EndpointValueType,
    ValidatedAudioFormat, ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION, ENDPOINT_HF_FEATURE_EXTRACTION,
    ENDPOINT_HF_MULTIMODAL_CHAT, ENDPOINT_HF_TEXT_TO_AUDIO, ENDPOINT_HF_TEXT_TO_IMAGE,
    ENDPOINT_HF_TEXT_TO_SPEECH, ENDPOINT_HF_TEXT_TO_VIDEO, ENDPOINT_MAYHEM_AUDIO_GENERATIONS,
    ENDPOINT_MAYHEM_MUSIC_GENERATIONS, ENDPOINT_OPENAI_AUDIO_SPEECH,
    ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS, ENDPOINT_OPENAI_CHAT_COMPLETIONS,
    ENDPOINT_OPENAI_COMPLETIONS, ENDPOINT_OPENAI_EMBEDDINGS, ENDPOINT_OPENAI_IMAGE_GENERATIONS,
    ENDPOINT_OPENAI_RESPONSES, ENDPOINT_OPENAI_VIDEOS,
};

const MAX_MUSIC_CAPTION_CHARS: u64 = 512;
const MAX_MUSIC_LYRICS_CHARS: u64 = 4_096;
const MAX_MUSIC_AUDIO_CODE_VALUE: u64 = 63_999;
const MAX_MUSIC_AUDIO_CODE_COUNT: u64 = 600 * 5;
const MAX_MUSIC_INLINE_AUDIO_SECONDS: u64 = 600;
const MAX_MUSIC_AUDIO_CODES_CHARS: u64 =
    MAX_MUSIC_AUDIO_CODE_COUNT * "<|audio_code_63999|>".len() as u64;
const MAX_MUSIC_INLINE_AUDIO_BYTES: usize = 64 * 1024 * 1024;
const MAX_MUSIC_INLINE_AUDIO_BASE64_CHARS: u64 =
    (MAX_MUSIC_INLINE_AUDIO_BYTES.div_ceil(3) * 4) as u64;
const MIN_MUSIC_INLINE_AUDIO_BYTES: usize = 44 + (8_000 / 10 * 2);
const MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS: u64 =
    (MIN_MUSIC_INLINE_AUDIO_BYTES.div_ceil(3) * 4) as u64;
const MAX_TTS_REFERENCE_AUDIO_SECONDS: u64 = 10;
const MAX_TTS_REFERENCE_AUDIO_BYTES: usize = 16 * 1024 * 1024;
const MAX_TTS_REFERENCE_AUDIO_BASE64_CHARS: u64 =
    (MAX_TTS_REFERENCE_AUDIO_BYTES.div_ceil(3) * 4) as u64;
const MAX_MUSIC_CUSTOM_TIMESTEPS: u64 = 200;
const MAX_MUSIC_SEED: f64 = u32::MAX as f64;
const MUSIC_VALID_LANGUAGES: &[&str] = &[
    "ar", "az", "bg", "bn", "ca", "cs", "da", "de", "el", "en", "es", "fa", "fi", "fr", "he", "hi",
    "hr", "ht", "hu", "id", "is", "it", "ja", "ko", "la", "lt", "ms", "ne", "nl", "no", "pa", "pl",
    "pt", "ro", "ru", "sa", "sk", "sr", "sv", "sw", "ta", "te", "th", "tl", "tr", "uk", "ur", "vi",
    "yue", "zh", "unknown",
];

const MUSIC_REQUEST_ALIAS_GROUPS: &[(&str, &[&str])] = &[
    ("prompt", &["caption"]),
    ("language", &["vocal_language"]),
    ("key", &["keyscale", "key_scale"]),
    ("time_signature", &["timesignature"]),
    ("duration_seconds", &["duration", "audio_duration"]),
    ("steps", &["inference_steps"]),
    ("n", &["batch", "batch_size"]),
    ("custom_timesteps", &["timesteps"]),
    ("adg", &["use_adg"]),
    ("sample_query", &["description", "desc"]),
    ("use_format", &["format"]),
    ("repaint_start", &["repainting_start"]),
    ("repaint_end", &["repainting_end"]),
    ("cover_strength", &["audio_cover_strength"]),
    ("response_format", &["audio_format"]),
    ("source_audio", &["src_audio", "ctx_audio"]),
    ("reference_audio", &["melody", "ref_audio"]),
    ("audio_codes", &["audio_code_string"]),
    ("infer_method", &["inference_method"]),
    ("sampler", &["sampler_mode"]),
    ("flow_edit_morph", &["flow_edit"]),
    ("lm_temperature", &["temperature"]),
    ("lm_cfg_scale", &["lm_cfg"]),
    ("lm_top_k", &["top_k"]),
    ("lm_top_p", &["top_p"]),
    ("lm_negative_prompt", &["negative_prompt"]),
    ("constrained_decoding", &["use_constrained_decoding"]),
];

const MUSIC_INLINE_AUDIO_ROOTS: &[&str] = &[
    "source_audio",
    "src_audio",
    "ctx_audio",
    "reference_audio",
    "melody",
    "ref_audio",
];
const MUSIC_INLINE_AUDIO_CONTENT_TYPES: &[&str] = &[
    "audio/aac",
    "audio/flac",
    "audio/m4a",
    "audio/mp4",
    "audio/mpeg",
    "audio/mp3",
    "audio/ogg",
    "audio/opus",
    "audio/wav",
    "audio/x-wav",
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ArtifactGenerationInlineAudioLoad {
    pub item_count: u32,
    pub max_item_bytes: u64,
    pub max_item_seconds: u64,
}

pub fn artifact_generation_inline_audio_load(
    request: &Value,
) -> Result<ArtifactGenerationInlineAudioLoad, String> {
    let mut load = ArtifactGenerationInlineAudioLoad::default();
    for root in ["source_audio", "reference_audio"] {
        let Some(audio) = request.get(root) else {
            continue;
        };
        let (bytes, seconds) = validated_music_inline_audio_item(root, audio)?;
        load.item_count = load
            .item_count
            .checked_add(1)
            .ok_or_else(|| "inline audio item count overflowed u32".to_owned())?;
        load.max_item_bytes = load.max_item_bytes.max(bytes);
        load.max_item_seconds = load.max_item_seconds.max(seconds);
    }
    Ok(load)
}

pub fn endpoint_family_contract_template(family: &str) -> Option<EndpointFamilyContract> {
    let (request, required, response): (&[&str], &[&str], &[&str]) = match family {
        ENDPOINT_OPENAI_CHAT_COMPLETIONS => (
            &[
                "model",
                "messages",
                "metadata",
                "stream",
                "stream_options.include_usage",
                "max_tokens",
                "max_completion_tokens",
                "temperature",
                "top_p",
                "top_k",
                "min_p",
                "seed",
                "frequency_penalty",
                "presence_penalty",
                "repeat_penalty",
                "stop",
                "tools",
                "tool_choice",
                "parallel_tool_calls",
                "response_format",
                "user",
                "messages.role",
                "messages.name",
                "messages.tool_calls",
                "messages.tool_call_id",
                "messages.content.type",
                "messages.content.text",
                "messages.content.image_url.url",
                "messages.content.input_audio.data",
                "messages.content.input_audio.format",
            ],
            &["model", "messages", "messages.role"],
            &[
                "id", "object", "created", "model", "choices", "usage", "mayhem",
            ],
        ),
        ENDPOINT_OPENAI_COMPLETIONS => (
            &[
                "model",
                "prompt",
                "stream",
                "max_tokens",
                "temperature",
                "top_p",
                "top_k",
                "min_p",
                "seed",
                "frequency_penalty",
                "presence_penalty",
                "stop",
                "user",
            ],
            &["model", "prompt"],
            &[
                "id", "object", "created", "model", "choices", "usage", "mayhem",
            ],
        ),
        ENDPOINT_OPENAI_RESPONSES => (
            &[
                "model",
                "input",
                "stream",
                "max_output_tokens",
                "temperature",
                "top_p",
                "top_k",
                "min_p",
                "tools",
                "tool_choice",
                "parallel_tool_calls",
                "text",
                "reasoning",
                "metadata",
                "user",
            ],
            &["model", "input"],
            &[
                "id",
                "object",
                "created_at",
                "status",
                "model",
                "output",
                "usage",
                "mayhem",
            ],
        ),
        ENDPOINT_HF_MULTIMODAL_CHAT => (
            &[
                "model",
                "messages",
                "messages.role",
                "messages.name",
                "messages.tool_calls",
                "messages.tool_call_id",
                "messages.content.type",
                "messages.content.text",
                "messages.content.image_url.url",
                "messages.content.input_audio.data",
                "messages.content.input_audio.format",
                "messages.content.video.data",
                "messages.content.video.frames",
                "messages.content.video.content_type",
                "messages.content.video.num_frames",
                "messages.content.video.fps",
                "stream",
                "max_tokens",
                "temperature",
                "top_p",
                "top_k",
                "min_p",
                "seed",
                "tools",
                "tool_choice",
                "parallel_tool_calls",
                "response_format",
            ],
            &["model", "messages", "messages.role"],
            &[
                "id", "object", "created", "model", "choices", "usage", "mayhem",
            ],
        ),
        ENDPOINT_OPENAI_EMBEDDINGS => (
            &["model", "input", "encoding_format", "dimensions", "user"],
            &["model", "input"],
            &["object", "data", "model", "usage", "mayhem"],
        ),
        ENDPOINT_HF_FEATURE_EXTRACTION => (
            &[
                "inputs",
                "normalize",
                "prompt_name",
                "truncate",
                "truncation_direction",
                "dimensions",
            ],
            &["inputs"],
            &["embeddings", "usage", "mayhem"],
        ),
        ENDPOINT_OPENAI_IMAGE_GENERATIONS => (
            &[
                "model",
                "prompt",
                "background",
                "moderation",
                "n",
                "output_compression",
                "output_format",
                "partial_images",
                "quality",
                "response_format",
                "size",
                "width",
                "height",
                "stream",
                "style",
                "user",
                "negative_prompt",
                "steps",
                "cfg_scale",
                "shift",
                "seed",
                "scheduler",
            ],
            &["model", "prompt"],
            &[
                "id",
                "object",
                "created",
                "model",
                "background",
                "data",
                "output_format",
                "quality",
                "size",
                "usage",
                "mayhem",
            ],
        ),
        ENDPOINT_HF_TEXT_TO_IMAGE => (
            &[
                "inputs",
                "parameters.guidance_scale",
                "parameters.negative_prompt",
                "parameters.num_images_per_prompt",
                "parameters.num_inference_steps",
                "parameters.width",
                "parameters.height",
                "parameters.shift",
                "parameters.scheduler",
                "parameters.seed",
            ],
            &["inputs"],
            &["image", "content_type", "usage", "mayhem"],
        ),
        ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS => (
            &[
                "file",
                "model",
                "language",
                "prompt",
                "response_format",
                "temperature",
                "timestamp_granularities",
                "stream",
            ],
            &["file", "model"],
            &[
                "text", "task", "language", "duration", "words", "segments", "usage", "mayhem",
            ],
        ),
        ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION => (
            &[
                "inputs",
                "parameters.return_timestamps",
                "parameters.generation_parameters.temperature",
                "parameters.generation_parameters.top_k",
                "parameters.generation_parameters.top_p",
                "parameters.generation_parameters.typical_p",
                "parameters.generation_parameters.max_length",
                "parameters.generation_parameters.max_new_tokens",
                "parameters.generation_parameters.min_length",
                "parameters.generation_parameters.min_new_tokens",
                "parameters.generation_parameters.do_sample",
                "parameters.generation_parameters.num_beams",
            ],
            &["inputs"],
            &["text", "chunks", "usage", "mayhem"],
        ),
        ENDPOINT_OPENAI_AUDIO_SPEECH => (
            &[
                "model",
                "input",
                "voice",
                "response_format",
                "speed",
                "instructions",
                "stream_format",
                "reference_audio",
                "reference_audio.data",
                "reference_audio.encoding",
                "reference_audio.content_type",
                "exaggeration",
                "cfg_weight",
                "temperature",
                "min_p",
                "top_p",
                "repetition_penalty",
                "seed",
            ],
            &["model", "input", "voice"],
            &["audio", "content_type", "usage", "mayhem"],
        ),
        ENDPOINT_HF_TEXT_TO_SPEECH => (
            &[
                "inputs",
                "parameters.generation_parameters.temperature",
                "parameters.generation_parameters.top_k",
                "parameters.generation_parameters.top_p",
                "parameters.generation_parameters.typical_p",
                "parameters.generation_parameters.max_length",
                "parameters.generation_parameters.max_new_tokens",
                "parameters.generation_parameters.min_length",
                "parameters.generation_parameters.min_new_tokens",
                "parameters.generation_parameters.do_sample",
                "parameters.generation_parameters.num_beams",
                "parameters.voice",
                "parameters.speed",
                "parameters.language",
                "parameters.speaker_id",
                "parameters.reference_audio",
                "parameters.reference_audio.data",
                "parameters.reference_audio.encoding",
                "parameters.reference_audio.content_type",
                "parameters.exaggeration",
                "parameters.cfg_weight",
                "parameters.seed",
                "parameters.generation_parameters.min_p",
                "parameters.generation_parameters.repetition_penalty",
            ],
            &["inputs"],
            &["audio", "content_type", "usage", "mayhem"],
        ),
        ENDPOINT_OPENAI_VIDEOS => (
            &[
                "model",
                "prompt",
                "input_reference",
                "conditions",
                "negative_prompt",
                "n",
                "seconds",
                "size",
                "width",
                "height",
                "num_frames",
                "fps",
                "seed",
                "enhance_prompt",
            ],
            &["model", "prompt"],
            &[
                "id",
                "object",
                "model",
                "status",
                "progress",
                "created_at",
                "completed_at",
                "expires_at",
                "error",
                "prompt",
                "size",
                "seconds",
                "usage",
                "mayhem",
            ],
        ),
        ENDPOINT_HF_TEXT_TO_VIDEO => (
            &[
                "inputs",
                "parameters.num_frames",
                "parameters.conditions",
                "parameters.guidance_scale",
                "parameters.negative_prompt",
                "parameters.n",
                "parameters.num_inference_steps",
                "parameters.seed",
                "parameters.width",
                "parameters.height",
                "parameters.fps",
                "parameters.enhance_prompt",
            ],
            &["inputs"],
            &["video", "content_type", "usage", "mayhem"],
        ),
        ENDPOINT_MAYHEM_AUDIO_GENERATIONS => (
            &[
                "model",
                "prompt",
                "input_audio",
                "duration_seconds",
                "response_format",
                "temperature",
                "top_k",
                "top_p",
                "typical_p",
                "guidance_scale",
                "negative_prompt",
                "seed",
                "do_sample",
                "max_new_tokens",
            ],
            &["model", "prompt"],
            &["id", "object", "audio", "content_type", "usage", "mayhem"],
        ),
        ENDPOINT_MAYHEM_MUSIC_GENERATIONS => (
            &[
                "model",
                "prompt",
                "caption",
                "lyrics",
                "instrumental",
                "style",
                "genre",
                "tags",
                "sample_mode",
                "sample_query",
                "description",
                "desc",
                "use_format",
                "format",
                "audio_codes",
                "audio_code_string",
                "language",
                "vocal_language",
                "bpm",
                "key",
                "keyscale",
                "key_scale",
                "time_signature",
                "timesignature",
                "duration",
                "audio_duration",
                "melody",
                "duration_seconds",
                "steps",
                "inference_steps",
                "n",
                "batch",
                "batch_size",
                "response_format",
                "audio_format",
                "mp3_bitrate",
                "mp3_sample_rate",
                "temperature",
                "top_k",
                "top_p",
                "guidance_scale",
                "seed",
                "task_type",
                "thinking",
                "instruction",
                "lm_temperature",
                "lm_cfg_scale",
                "lm_cfg",
                "lm_top_k",
                "lm_top_p",
                "lm_negative_prompt",
                "use_cot_metas",
                "use_cot_caption",
                "use_cot_language",
                "constrained_decoding",
                "use_constrained_decoding",
                "infer_method",
                "inference_method",
                "sampler",
                "sampler_mode",
                "velocity_norm_threshold",
                "velocity_ema_factor",
                "dcw_enabled",
                "dcw_mode",
                "dcw_scaler",
                "dcw_high_scaler",
                "dcw_wavelet",
                "shift",
                "custom_timesteps",
                "timesteps",
                "adg",
                "use_adg",
                "cfg_interval_start",
                "cfg_interval_end",
                "repaint_start",
                "repainting_start",
                "repaint_end",
                "repainting_end",
                "repaint_mode",
                "repaint_strength",
                "chunk_mask_mode",
                "cover_strength",
                "audio_cover_strength",
                "cover_noise_strength",
                "enable_normalization",
                "normalization_db",
                "fade_in_duration",
                "fade_out_duration",
                "latent_shift",
                "latent_rescale",
                "retake_seed",
                "retake_variance",
                "flow_edit_morph",
                "flow_edit",
                "flow_edit_source_caption",
                "flow_edit_source_lyrics",
                "flow_edit_n_min",
                "flow_edit_n_max",
                "flow_edit_n_avg",
                "seeds",
                "source_audio",
                "source_audio.data",
                "source_audio.encoding",
                "source_audio.content_type",
                "src_audio",
                "src_audio.data",
                "src_audio.encoding",
                "src_audio.content_type",
                "ctx_audio",
                "ctx_audio.data",
                "ctx_audio.encoding",
                "ctx_audio.content_type",
                "reference_audio",
                "reference_audio.data",
                "reference_audio.encoding",
                "reference_audio.content_type",
                "melody.data",
                "melody.encoding",
                "melody.content_type",
                "ref_audio",
                "ref_audio.data",
                "ref_audio.encoding",
                "ref_audio.content_type",
                "no_fsq",
                "negative_prompt",
            ],
            &["model"],
            &["id", "object", "music", "content_type", "usage", "mayhem"],
        ),
        ENDPOINT_HF_TEXT_TO_AUDIO => (
            &[
                "inputs",
                "parameters.audio",
                "parameters.duration_seconds",
                "parameters.generation_parameters.temperature",
                "parameters.generation_parameters.top_k",
                "parameters.generation_parameters.top_p",
                "parameters.generation_parameters.typical_p",
                "parameters.generation_parameters.max_length",
                "parameters.generation_parameters.max_new_tokens",
                "parameters.generation_parameters.min_length",
                "parameters.generation_parameters.min_new_tokens",
                "parameters.generation_parameters.do_sample",
                "parameters.generation_parameters.early_stopping",
                "parameters.generation_parameters.num_beams",
                "parameters.generation_parameters.num_beam_groups",
                "parameters.generation_parameters.penalty_alpha",
                "parameters.generation_parameters.use_cache",
                "parameters.guidance_scale",
                "parameters.seed",
            ],
            &["inputs"],
            &["audio", "sampling_rate", "content_type", "usage", "mayhem"],
        ),
        _ => return None,
    };

    let request_attributes = strings(request);
    let required_request_attributes = strings(required);
    let response_attributes = strings(response);
    let required_response_attributes = response
        .iter()
        .filter(|path| !endpoint_response_attribute_optional(family, path))
        .map(|path| (*path).to_owned())
        .collect::<Vec<_>>();
    let request_attribute_specs = request
        .iter()
        .map(|path| {
            request_attribute_spec(family, path)
                .map(|spec| ((*path).to_owned(), spec))
                .ok_or(())
        })
        .collect::<Result<BTreeMap<_, _>, _>>()
        .ok()?;
    let response_attribute_specs = response
        .iter()
        .map(|path| {
            response_attribute_spec(path)
                .map(|spec| ((*path).to_owned(), spec))
                .ok_or(())
        })
        .collect::<Result<BTreeMap<_, _>, _>>()
        .ok()?;

    let interaction_groups = endpoint_interaction_groups(family)
        .into_iter()
        .map(|group| {
            group
                .into_iter()
                .filter(|path| request_attributes.contains(path))
                .collect::<Vec<_>>()
        })
        .filter(|group| group.len() > 1)
        .collect();
    Some(EndpointFamilyContract {
        family: family.to_owned(),
        request_attributes,
        required_request_attributes,
        response_attributes,
        required_response_attributes,
        request_attribute_specs,
        response_attribute_specs,
        interaction_groups,
        speciality_mappings: BTreeMap::new(),
    })
}

fn endpoint_response_attribute_optional(family: &str, path: &str) -> bool {
    matches!(
        (family, path),
        (
            ENDPOINT_OPENAI_IMAGE_GENERATIONS,
            "background" | "output_format" | "quality" | "size"
        ) | (
            ENDPOINT_OPENAI_VIDEOS,
            "completed_at" | "expires_at" | "error"
        ) | (
            ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS,
            "task" | "language" | "duration" | "words" | "segments"
        ) | (ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION, "chunks")
    )
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn endpoint_interaction_groups(family: &str) -> Vec<Vec<String>> {
    let groups: &[&[&str]] = match family {
        ENDPOINT_OPENAI_CHAT_COMPLETIONS | ENDPOINT_HF_MULTIMODAL_CHAT => &[
            &["tools", "tool_choice", "parallel_tool_calls"],
            &["response_format", "stream"],
            &["temperature", "top_p", "top_k", "min_p", "seed"],
        ],
        ENDPOINT_OPENAI_COMPLETIONS | ENDPOINT_OPENAI_RESPONSES => &[
            &["temperature", "top_p", "top_k", "min_p", "seed"],
            &["tools", "tool_choice", "parallel_tool_calls"],
        ],
        ENDPOINT_OPENAI_IMAGE_GENERATIONS => &[
            &["background", "output_format", "response_format"],
            &["quality", "size", "steps", "cfg_scale", "shift"],
            &["width", "height"],
            &["negative_prompt", "scheduler", "seed"],
        ],
        ENDPOINT_HF_TEXT_TO_IMAGE => &[
            &[
                "parameters.width",
                "parameters.height",
                "parameters.num_images_per_prompt",
                "parameters.num_inference_steps",
                "parameters.guidance_scale",
                "parameters.shift",
            ],
            &[
                "parameters.negative_prompt",
                "parameters.scheduler",
                "parameters.seed",
            ],
        ],
        ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS => &[&[
            "response_format",
            "timestamp_granularities",
            "stream",
            "temperature",
        ]],
        ENDPOINT_OPENAI_AUDIO_SPEECH => &[
            &[
                "voice",
                "response_format",
                "speed",
                "instructions",
                "stream_format",
            ],
            &[
                "reference_audio",
                "exaggeration",
                "cfg_weight",
                "temperature",
                "min_p",
                "top_p",
                "repetition_penalty",
                "seed",
            ],
        ],
        ENDPOINT_HF_TEXT_TO_SPEECH => &[
            &[
                "parameters.generation_parameters.temperature",
                "parameters.generation_parameters.top_k",
                "parameters.generation_parameters.top_p",
                "parameters.generation_parameters.typical_p",
                "parameters.generation_parameters.do_sample",
                "parameters.generation_parameters.max_new_tokens",
            ],
            &[
                "parameters.reference_audio",
                "parameters.exaggeration",
                "parameters.cfg_weight",
                "parameters.seed",
                "parameters.generation_parameters.temperature",
                "parameters.generation_parameters.min_p",
                "parameters.generation_parameters.top_p",
                "parameters.generation_parameters.repetition_penalty",
            ],
        ],
        ENDPOINT_OPENAI_VIDEOS => &[
            &[
                "input_reference",
                "size",
                "num_frames",
                "fps",
                "seed",
                "negative_prompt",
                "n",
            ],
            &[
                "conditions",
                "size",
                "num_frames",
                "fps",
                "seed",
                "negative_prompt",
                "n",
            ],
            &[
                "input_reference",
                "seconds",
                "size",
                "fps",
                "seed",
                "negative_prompt",
                "n",
            ],
            &[
                "conditions",
                "width",
                "height",
                "num_frames",
                "fps",
                "seed",
                "negative_prompt",
                "n",
            ],
            &[
                "input_reference",
                "seconds",
                "width",
                "height",
                "fps",
                "seed",
                "negative_prompt",
                "n",
            ],
        ],
        ENDPOINT_HF_TEXT_TO_VIDEO => &[
            &[
                "parameters.num_frames",
                "parameters.conditions",
                "parameters.width",
                "parameters.height",
                "parameters.fps",
                "parameters.num_inference_steps",
            ],
            &[
                "parameters.guidance_scale",
                "parameters.negative_prompt",
                "parameters.n",
                "parameters.seed",
            ],
        ],
        ENDPOINT_MAYHEM_AUDIO_GENERATIONS => &[&[
            "temperature",
            "top_k",
            "top_p",
            "typical_p",
            "do_sample",
            "max_new_tokens",
        ]],
        ENDPOINT_MAYHEM_MUSIC_GENERATIONS => &[
            &[
                "steps",
                "guidance_scale",
                "seed",
                "shift",
                "infer_method",
                "adg",
                "cfg_interval_start",
                "cfg_interval_end",
            ],
            &[
                "thinking",
                "lm_temperature",
                "lm_cfg_scale",
                "lm_top_k",
                "lm_top_p",
                "lm_negative_prompt",
                "use_cot_metas",
                "use_cot_caption",
                "use_cot_language",
                "constrained_decoding",
            ],
            &["task_type", "source_audio", "reference_audio"],
            &[
                "repaint_start",
                "repaint_end",
                "repaint_mode",
                "chunk_mask_mode",
                "source_audio",
            ],
            &["repaint_strength", "source_audio"],
            &["cover_strength", "cover_noise_strength", "source_audio"],
            &[
                "sampler",
                "velocity_norm_threshold",
                "velocity_ema_factor",
                "dcw_enabled",
                "dcw_mode",
                "dcw_scaler",
                "dcw_high_scaler",
                "dcw_wavelet",
            ],
            &[
                "response_format",
                "mp3_bitrate",
                "mp3_sample_rate",
                "enable_normalization",
                "normalization_db",
                "fade_in_duration",
                "fade_out_duration",
                "latent_shift",
                "latent_rescale",
            ],
            &[
                "retake_seed",
                "retake_variance",
                "flow_edit_morph",
                "flow_edit_source_caption",
                "flow_edit_source_lyrics",
                "flow_edit_n_min",
                "flow_edit_n_max",
                "flow_edit_n_avg",
            ],
        ],
        ENDPOINT_HF_TEXT_TO_AUDIO | ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION => &[&[
            "parameters.generation_parameters.temperature",
            "parameters.generation_parameters.top_k",
            "parameters.generation_parameters.top_p",
            "parameters.generation_parameters.typical_p",
            "parameters.generation_parameters.do_sample",
            "parameters.generation_parameters.max_new_tokens",
        ]],
        _ => &[],
    };
    groups
        .iter()
        .map(|group| group.iter().map(|path| (*path).to_owned()).collect())
        .collect()
}

fn request_attribute_spec(family: &str, path: &str) -> Option<EndpointAttributeSpec> {
    if family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS {
        if let Some(spec) = music_request_attribute_spec(path) {
            return Some(spec);
        }
    }
    let leaf = path.rsplit('.').next().unwrap_or(path);
    let spec = match path {
        "model" => marker_string_spec(json!("$MODEL")),
        "messages" => array_spec(
            1,
            4096,
            json!([
                {"role":"user","content":"Mayhem calibration context"},
                {"role":"user","content":"Mayhem calibration"}
            ]),
        ),
        "metadata" => object_spec_with_default(json!({})),
        "text" | "reasoning" | "response_format"
            if matches!(
                family,
                ENDPOINT_OPENAI_CHAT_COMPLETIONS
                    | ENDPOINT_OPENAI_RESPONSES
                    | ENDPOINT_HF_MULTIMODAL_CHAT
            ) =>
        {
            object_spec(json!({}))
        }
        "stream_options.include_usage" => boolean_spec(None),
        "tools" => array_spec(
            1,
            128,
            json!([{"type":"function","function":{"name":"calibration_tool","parameters":{"type":"object"}}}]),
        ),
        "tool_choice" => union_spec(
            &[EndpointValueType::String, EndpointValueType::Object],
            &[
                json!("auto"),
                json!("none"),
                json!({"type":"function","function":{"name":"calibration_tool"}}),
            ],
        ),
        "parallel_tool_calls" => boolean_spec(Some(true)),
        "stream" => boolean_spec(Some(false)),
        "prompt" => string_spec(1, 32_000, json!("Mayhem calibration")),
        "input" if family == ENDPOINT_OPENAI_EMBEDDINGS => union_spec(
            &[EndpointValueType::String, EndpointValueType::Array],
            &[
                json!("Mayhem calibration"),
                json!(["Mayhem calibration A", "Mayhem calibration B"]),
            ],
        ),
        "input" if family == ENDPOINT_OPENAI_RESPONSES => union_spec(
            &[EndpointValueType::String, EndpointValueType::Array],
            &[
                json!("Mayhem calibration"),
                json!([{"role":"user","content":"Mayhem calibration"}]),
            ],
        ),
        "input" => string_spec(1, 32_000, json!("Mayhem calibration")),
        "inputs" if family == ENDPOINT_HF_FEATURE_EXTRACTION => union_spec(
            &[EndpointValueType::String, EndpointValueType::Array],
            &[
                json!("Mayhem calibration"),
                json!(["Mayhem calibration A", "Mayhem calibration B"]),
            ],
        ),
        "inputs" if family == ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION => union_spec(
            &[EndpointValueType::String, EndpointValueType::Object],
            &[
                json!("$AUDIO_BASE64"),
                json!({
                    "data": "$AUDIO_BASE64",
                    "content_type": "$AUDIO_CONTENT_TYPE",
                    "filename": "$AUDIO_FILENAME"
                }),
            ],
        ),
        "inputs" => string_spec(1, 32_000, json!("Mayhem calibration")),
        "file" => object_spec(json!({
            "filename": "$AUDIO_FILENAME",
            "content_type": "$AUDIO_CONTENT_TYPE",
            "bytes": "$AUDIO_BYTES",
            "blake3": "$AUDIO_BLAKE3"
        })),
        "user" => string_spec(1, 512, json!("mayhem-calibration-user")),
        "language" | "parameters.language" => string_spec(1, 128, json!("en")),
        "prompt_name" => string_spec(1, 256, json!("query")),
        "instructions" => string_spec(1, 4096, json!("Speak clearly")),
        "negative_prompt" | "parameters.negative_prompt" => {
            if family == ENDPOINT_HF_TEXT_TO_VIDEO {
                let mut spec = union_spec(
                    &[EndpointValueType::String, EndpointValueType::Array],
                    &[json!("blur"), json!(["blur"])],
                );
                spec.min_length = Some(0);
                spec.max_length = Some(32_000);
                spec.min_items = Some(1);
                spec.max_items = Some(1);
                spec
            } else {
                string_spec(0, 32_000, json!("blur"))
            }
        }
        "input_audio" | "melody" | "parameters.audio" => object_spec(json!({
            "data": "$AUDIO_BASE64",
            "format": "wav"
        })),
        "reference_audio" if family == ENDPOINT_OPENAI_AUDIO_SPEECH => {
            tts_reference_audio_object_spec()
        }
        "parameters.reference_audio" if family == ENDPOINT_HF_TEXT_TO_SPEECH => {
            tts_reference_audio_object_spec()
        }
        "reference_audio.data" if family == ENDPOINT_OPENAI_AUDIO_SPEECH => string_spec(
            MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS,
            MAX_TTS_REFERENCE_AUDIO_BASE64_CHARS,
            json!(endpoint_calibration_wav_base64_fixture(
                MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize
            )
            .expect("minimum speech reference-audio fixture is constructible")),
        ),
        "parameters.reference_audio.data" if family == ENDPOINT_HF_TEXT_TO_SPEECH => string_spec(
            MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS,
            MAX_TTS_REFERENCE_AUDIO_BASE64_CHARS,
            json!(endpoint_calibration_wav_base64_fixture(
                MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize
            )
            .expect("minimum speech reference-audio fixture is constructible")),
        ),
        "reference_audio.encoding" if family == ENDPOINT_OPENAI_AUDIO_SPEECH => {
            enum_spec(None, &[json!("base64")])
        }
        "parameters.reference_audio.encoding" if family == ENDPOINT_HF_TEXT_TO_SPEECH => {
            enum_spec(None, &[json!("base64")])
        }
        "reference_audio.content_type" if family == ENDPOINT_OPENAI_AUDIO_SPEECH => {
            tts_reference_audio_content_type_spec()
        }
        "parameters.reference_audio.content_type" if family == ENDPOINT_HF_TEXT_TO_SPEECH => {
            tts_reference_audio_content_type_spec()
        }
        "input_reference" => union_spec(
            &[EndpointValueType::String, EndpointValueType::Object],
            &[json!("$IMAGE_FILE"), json!({"image_url":"$IMAGE_DATA_URL"})],
        ),
        "conditions" | "parameters.conditions" => array_spec(
            0,
            16,
            json!([{
                "image_url": "$IMAGE_DATA_URL",
                "frame_index": 0,
                "strength": 1.0,
                "crf": 33
            }]),
        ),
        "voice" => union_spec(
            &[EndpointValueType::String, EndpointValueType::Object],
            &[json!("$VOICE"), json!({"id":"$VOICE"})],
        ),
        "parameters.voice" | "parameters.speaker_id" | "scheduler" | "parameters.scheduler" => {
            string_spec(1, 256, json!("$MODEL_VALUE"))
        }
        "exaggeration" if family == ENDPOINT_OPENAI_AUDIO_SPEECH => {
            with_default(number_spec(0.25, 2.0, 0.5), json!(0.5))
        }
        "parameters.exaggeration" if family == ENDPOINT_HF_TEXT_TO_SPEECH => {
            with_default(number_spec(0.25, 2.0, 0.5), json!(0.5))
        }
        "cfg_weight" if family == ENDPOINT_OPENAI_AUDIO_SPEECH => {
            with_default(number_spec(0.0, 1.0, 0.5), json!(0.5))
        }
        "parameters.cfg_weight" if family == ENDPOINT_HF_TEXT_TO_SPEECH => {
            with_default(number_spec(0.0, 1.0, 0.5), json!(0.5))
        }
        "repetition_penalty" if family == ENDPOINT_OPENAI_AUDIO_SPEECH => {
            with_default(number_spec(1.0, 2.0, 1.2), json!(1.2))
        }
        "parameters.generation_parameters.repetition_penalty"
            if family == ENDPOINT_HF_TEXT_TO_SPEECH =>
        {
            with_default(number_spec(1.0, 2.0, 1.2), json!(1.2))
        }
        "temperature" if family == ENDPOINT_OPENAI_AUDIO_SPEECH => {
            with_default(number_spec(0.05, 5.0, 0.8), json!(0.8))
        }
        "parameters.generation_parameters.temperature" if family == ENDPOINT_HF_TEXT_TO_SPEECH => {
            with_default(number_spec(0.05, 5.0, 0.8), json!(0.8))
        }
        "min_p" if family == ENDPOINT_OPENAI_AUDIO_SPEECH => {
            with_default(number_spec(0.0, 1.0, 0.05), json!(0.05))
        }
        "parameters.generation_parameters.min_p" if family == ENDPOINT_HF_TEXT_TO_SPEECH => {
            with_default(number_spec(0.0, 1.0, 0.05), json!(0.05))
        }
        "top_p" if family == ENDPOINT_OPENAI_AUDIO_SPEECH => {
            with_default(number_spec(0.0, 1.0, 1.0), json!(1.0))
        }
        "parameters.generation_parameters.top_p" if family == ENDPOINT_HF_TEXT_TO_SPEECH => {
            with_default(number_spec(0.0, 1.0, 1.0), json!(1.0))
        }
        "seed" if family == ENDPOINT_OPENAI_AUDIO_SPEECH => {
            with_default(integer_spec(0.0, u32::MAX as f64, 7), json!(7))
        }
        "parameters.seed" if family == ENDPOINT_HF_TEXT_TO_SPEECH => {
            with_default(integer_spec(0.0, u32::MAX as f64, 7), json!(7))
        }
        "messages.content.type" => {
            let values = if family == ENDPOINT_OPENAI_CHAT_COMPLETIONS {
                vec![json!("text"), json!("image_url"), json!("input_audio")]
            } else {
                vec![
                    json!("text"),
                    json!("image_url"),
                    json!("input_audio"),
                    json!("video"),
                ]
            };
            enum_spec(None, &values)
        }
        "messages.role" => enum_spec(
            None,
            &[
                json!("system"),
                json!("developer"),
                json!("user"),
                json!("assistant"),
                json!("tool"),
            ],
        ),
        "messages.name" | "messages.tool_call_id" => string_spec(1, 256, json!("calibration")),
        "messages.tool_calls" => array_spec(
            1,
            128,
            json!([{"id":"call-calibration","type":"function","function":{"name":"calibration_tool","arguments":"{}"}}]),
        ),
        "messages.content.text" => string_spec(1, 32_000, json!("Mayhem calibration")),
        "messages.content.image_url.url" => marker_string_spec(json!("$IMAGE_DATA_URL")),
        "messages.content.input_audio.data" => marker_string_spec(json!("$AUDIO_BASE64")),
        "messages.content.input_audio.format" => enum_spec(
            None,
            &[json!("wav"), json!("mp3"), json!("flac"), json!("ogg")],
        ),
        "messages.content.video.data" => marker_string_spec(json!("$VIDEO_BASE64")),
        "messages.content.video.frames" => array_spec(1, 64, json!(["$IMAGE_DATA_URL"])),
        "messages.content.video.content_type" => {
            enum_spec(None, &[json!("video/mp4"), json!("video/webm")])
        }
        "messages.content.video.num_frames" | "num_frames" | "parameters.num_frames" => {
            integer_spec(1.0, 4096.0, 16)
        }
        "messages.content.video.fps" | "fps" | "parameters.fps" => number_spec(0.01, 240.0, 8.0),
        "enhance_prompt" | "parameters.enhance_prompt" => boolean_spec(Some(false)),
        "encoding_format" => enum_spec(Some(json!("float")), &[json!("float"), json!("base64")]),
        "truncation_direction" => enum_spec(Some(json!("right")), &[json!("left"), json!("right")]),
        "background" => enum_spec(
            Some(json!("auto")),
            &[json!("transparent"), json!("opaque"), json!("auto")],
        ),
        "moderation" => enum_spec(Some(json!("auto")), &[json!("low"), json!("auto")]),
        "output_format" => enum_spec(None, &[json!("png"), json!("jpeg"), json!("webp")]),
        "quality" => enum_spec(
            Some(json!("auto")),
            &[
                json!("standard"),
                json!("hd"),
                json!("low"),
                json!("medium"),
                json!("high"),
                json!("auto"),
            ],
        ),
        "style" => enum_spec(None, &[json!("vivid"), json!("natural")]),
        "response_format" if family == ENDPOINT_OPENAI_IMAGE_GENERATIONS => {
            enum_spec(Some(json!("b64_json")), &[json!("url"), json!("b64_json")])
        }
        "response_format" if family == ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS => enum_spec(
            Some(json!("json")),
            &[
                json!("json"),
                json!("text"),
                json!("srt"),
                json!("verbose_json"),
                json!("vtt"),
            ],
        ),
        "response_format"
            if matches!(
                family,
                ENDPOINT_OPENAI_AUDIO_SPEECH
                    | ENDPOINT_MAYHEM_AUDIO_GENERATIONS
                    | ENDPOINT_MAYHEM_MUSIC_GENERATIONS
            ) =>
        {
            enum_spec(
                Some(json!("wav")),
                &[
                    json!("mp3"),
                    json!("opus"),
                    json!("aac"),
                    json!("flac"),
                    json!("wav"),
                    json!("pcm"),
                ],
            )
        }
        "stream_format" => enum_spec(Some(json!("audio")), &[json!("audio"), json!("sse")]),
        "timestamp_granularities" => array_enum_spec(
            Some(json!(["segment"])),
            &[json!("word"), json!("segment")],
            1,
            2,
        ),
        "parameters.return_timestamps" if family == ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION => {
            enum_spec(
                None,
                &[json!(false), json!(true), json!("word"), json!("segment")],
            )
        }
        "size" if family == ENDPOINT_OPENAI_VIDEOS => {
            let mut spec = EndpointAttributeSpec::new(EndpointValueType::String);
            spec.calibration_values = vec![
                json!("720x1280"),
                json!("1280x720"),
                json!("1024x1792"),
                json!("1792x1024"),
            ];
            spec
        }
        "seconds" if family == ENDPOINT_OPENAI_VIDEOS => enum_spec(
            None,
            &[
                json!("1"),
                json!("2"),
                json!("3"),
                json!("4"),
                json!("5"),
                json!("6"),
                json!("7"),
                json!("8"),
                json!("9"),
                json!("10"),
                json!("11"),
                json!("12"),
            ],
        ),
        "seconds" => enum_spec(Some(json!("4")), &[json!("4"), json!("8"), json!("12")]),
        "size" => {
            let mut spec = EndpointAttributeSpec::new(EndpointValueType::String);
            spec.calibration_values.push(json!("512x512"));
            spec
        }
        "stop" => union_spec(
            &[
                EndpointValueType::String,
                EndpointValueType::Array,
                EndpointValueType::Null,
            ],
            &[
                json!("CALIBRATION_STOP"),
                json!(["CALIBRATION_STOP", "CALIBRATION_END"]),
                Value::Null,
            ],
        ),
        "early_stopping" | "parameters.generation_parameters.early_stopping" => union_spec(
            &[EndpointValueType::Boolean, EndpointValueType::String],
            &[json!(false), json!(true), json!("never")],
        ),
        "n" if family == ENDPOINT_OPENAI_IMAGE_GENERATIONS => integer_spec(1.0, 4.0, 1),
        "n" if family == ENDPOINT_OPENAI_VIDEOS => {
            with_default(integer_spec(1.0, 1.0, 1), json!(1))
        }
        "parameters.n" if family == ENDPOINT_HF_TEXT_TO_VIDEO => {
            with_default(integer_spec(1.0, 1.0, 1), json!(1))
        }
        "parameters.num_images_per_prompt" if family == ENDPOINT_HF_TEXT_TO_IMAGE => {
            integer_spec(1.0, 4.0, 1)
        }
        "steps" if family == ENDPOINT_OPENAI_IMAGE_GENERATIONS => integer_spec(1.0, 150.0, 9),
        "parameters.num_inference_steps" if family == ENDPOINT_HF_TEXT_TO_IMAGE => {
            integer_spec(1.0, 150.0, 9)
        }
        "width" | "height" if family == ENDPOINT_OPENAI_IMAGE_GENERATIONS => {
            integer_spec(64.0, 2_048.0, 1_024)
        }
        "width" | "height" if family == ENDPOINT_OPENAI_VIDEOS => {
            integer_spec(64.0, 16_384.0, 1_024)
        }
        "parameters.width" | "parameters.height" if family == ENDPOINT_HF_TEXT_TO_IMAGE => {
            integer_spec(64.0, 2_048.0, 1_024)
        }
        "parameters.width" | "parameters.height" if family == ENDPOINT_HF_TEXT_TO_VIDEO => {
            integer_spec(64.0, 16_384.0, 1_024)
        }
        "cfg_scale" if family == ENDPOINT_OPENAI_IMAGE_GENERATIONS => number_spec(0.0, 50.0, 1.0),
        "parameters.guidance_scale" if family == ENDPOINT_HF_TEXT_TO_IMAGE => {
            number_spec(0.0, 50.0, 1.0)
        }
        "shift" if family == ENDPOINT_OPENAI_IMAGE_GENERATIONS => number_spec(1.0, 10.0, 3.0),
        "parameters.shift" if family == ENDPOINT_HF_TEXT_TO_IMAGE => number_spec(1.0, 10.0, 3.0),
        _ if boolean_leaf(leaf) => boolean_spec(None),
        _ if integer_leaf(leaf) => integer_spec(0.0, integer_maximum(leaf), integer_baseline(leaf)),
        _ if number_leaf(leaf) => number_spec(
            number_minimum(leaf),
            number_maximum(leaf),
            number_baseline(leaf),
        ),
        _ => return None,
    };
    Some(spec)
}

fn music_request_attribute_spec(path: &str) -> Option<EndpointAttributeSpec> {
    let spec = match path {
        "prompt" | "caption" => {
            string_spec(0, MAX_MUSIC_CAPTION_CHARS, json!("Mayhem calibration"))
        }
        "lyrics" => with_default(
            string_spec(0, MAX_MUSIC_LYRICS_CHARS, json!("Calibration lyrics")),
            json!(""),
        ),
        "style" => with_default(string_spec(0, 505, json!("acoustic")), json!("")),
        "genre" => with_default(string_spec(0, 505, json!("indie folk")), json!("")),
        "tags" => with_default(string_spec(0, 506, json!("intimate, live")), json!("")),
        "sample_query" => with_default(
            string_spec(
                0,
                MAX_MUSIC_CAPTION_CHARS,
                json!("A restrained piano ballad"),
            ),
            json!(""),
        ),
        "description" | "desc" => string_spec(
            0,
            MAX_MUSIC_CAPTION_CHARS,
            json!("A restrained piano ballad"),
        ),
        "audio_codes" => music_audio_codes_spec(Some(json!(""))),
        "audio_code_string" => music_audio_codes_spec(None),
        "language" => music_language_spec(Some(json!("unknown"))),
        "vocal_language" => music_language_spec(None),
        "key" => with_default(string_spec(0, 8, json!("C major")), json!("")),
        "keyscale" | "key_scale" => string_spec(0, 8, json!("C major")),
        "time_signature" => enum_spec(
            Some(json!("")),
            &[
                json!(""),
                json!("2"),
                json!("2/4"),
                json!("3"),
                json!("3/4"),
                json!("4"),
                json!("4/4"),
                json!("6"),
                json!("6/8"),
            ],
        ),
        "timesignature" => enum_spec(
            None,
            &[
                json!(""),
                json!("2"),
                json!("2/4"),
                json!("3"),
                json!("3/4"),
                json!("4"),
                json!("4/4"),
                json!("6"),
                json!("6/8"),
            ],
        ),
        "instruction" => with_default(
            string_spec(
                1,
                1_024,
                json!("Fill the audio semantic mask based on the given conditions:"),
            ),
            json!("Fill the audio semantic mask based on the given conditions:"),
        ),
        "flow_edit_source_caption" => with_default(
            string_spec(
                0,
                MAX_MUSIC_CAPTION_CHARS,
                json!("Original source arrangement"),
            ),
            json!(""),
        ),
        "flow_edit_source_lyrics" => with_default(
            string_spec(0, MAX_MUSIC_LYRICS_CHARS, json!("Original source lyrics")),
            json!(""),
        ),
        "lm_negative_prompt" => with_default(
            string_spec(0, MAX_MUSIC_CAPTION_CHARS, json!("NO USER INPUT")),
            json!("NO USER INPUT"),
        ),
        "negative_prompt" => string_spec(0, MAX_MUSIC_CAPTION_CHARS, json!("NO USER INPUT")),
        "task_type" => enum_spec(
            Some(json!("text2music")),
            &[
                json!("text2music"),
                json!("repaint"),
                json!("cover"),
                json!("cover-nofsq"),
            ],
        ),
        "infer_method" => enum_spec(Some(json!("ode")), &[json!("ode"), json!("sde")]),
        "inference_method" => enum_spec(None, &[json!("ode"), json!("sde")]),
        "sampler" => enum_spec(Some(json!("euler")), &[json!("euler"), json!("heun")]),
        "sampler_mode" => enum_spec(None, &[json!("euler"), json!("heun")]),
        "dcw_mode" => music_dcw_mode_spec(),
        "dcw_wavelet" => enum_spec(
            Some(json!("haar")),
            &[
                json!("haar"),
                json!("db2"),
                json!("db4"),
                json!("sym4"),
                json!("sym8"),
                json!("coif2"),
            ],
        ),
        "chunk_mask_mode" => enum_spec(Some(json!("auto")), &[json!("explicit"), json!("auto")]),
        "repaint_mode" => enum_spec(
            Some(json!("balanced")),
            &[
                json!("conservative"),
                json!("balanced"),
                json!("aggressive"),
            ],
        ),
        "response_format" => enum_spec(
            Some(json!("flac")),
            &[
                json!("flac"),
                json!("opus"),
                json!("aac"),
                json!("wav"),
                json!("wav32"),
                json!("mp3"),
            ],
        ),
        "audio_format" => enum_spec(
            None,
            &[
                json!("flac"),
                json!("mp3"),
                json!("opus"),
                json!("aac"),
                json!("wav"),
                json!("wav32"),
            ],
        ),
        "mp3_bitrate" => enum_spec(
            Some(json!("128k")),
            &[json!("128k"), json!("192k"), json!("256k"), json!("320k")],
        ),
        "mp3_sample_rate" => enum_spec(Some(json!(48_000)), &[json!(44_100), json!(48_000)]),
        "source_audio" | "src_audio" | "ctx_audio" | "reference_audio" | "melody" | "ref_audio" => {
            music_inline_audio_object_spec()
        }
        path if music_inline_audio_child(path, "data") => string_spec(
            MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS,
            MAX_MUSIC_INLINE_AUDIO_BASE64_CHARS,
            json!(endpoint_calibration_wav_base64_fixture(
                MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize
            )
            .expect("minimum music audio fixture is constructible")),
        ),
        path if music_inline_audio_child(path, "encoding") => enum_spec(None, &[json!("base64")]),
        path if music_inline_audio_child(path, "content_type") => {
            music_inline_audio_content_type_spec()
        }
        "instrumental" | "sample_mode" | "use_format" => boolean_spec(Some(false)),
        "format" | "no_fsq" => boolean_spec(None),
        "thinking" => boolean_spec(Some(false)),
        "adg" => boolean_spec(Some(false)),
        "use_adg" => boolean_spec(None),
        "dcw_enabled" => boolean_spec(Some(false)),
        "enable_normalization" => boolean_spec(Some(true)),
        "flow_edit_morph" => boolean_spec(Some(false)),
        "flow_edit" => boolean_spec(None),
        "use_cot_metas" | "use_cot_caption" | "use_cot_language" => boolean_spec(Some(true)),
        "constrained_decoding" => boolean_spec(Some(true)),
        "use_constrained_decoding" => boolean_spec(None),
        "bpm" => music_bpm_spec(),
        "steps" => with_default(integer_spec(1.0, 200.0, 50), json!(50)),
        "inference_steps" => integer_spec(1.0, 200.0, 50),
        "n" => with_default(integer_spec(1.0, 8.0, 2), json!(2)),
        "batch" | "batch_size" => integer_spec(1.0, 8.0, 2),
        "top_k" => integer_spec(0.0, 100.0, 0),
        "lm_top_k" => with_default(integer_spec(0.0, 100.0, 0), json!(0)),
        "seed" => with_default(integer_spec(-1.0, MAX_MUSIC_SEED, 7), json!(-1)),
        "flow_edit_n_avg" => with_default(integer_spec(1.0, 8.0, 1), json!(1)),
        "duration_seconds" => music_duration_spec(Some(Value::Null)),
        "duration" | "audio_duration" => music_duration_spec(None),
        "temperature" => number_spec(0.0, 2.0, 0.85),
        "lm_temperature" => with_default(number_spec(0.0, 2.0, 0.85), json!(0.85)),
        "top_p" => number_spec(0.0, 1.0, 0.9),
        "lm_top_p" => with_default(number_spec(0.0, 1.0, 0.9), json!(0.9)),
        "guidance_scale" => with_default(number_spec(1.0, 15.0, 7.0), json!(7.0)),
        "lm_cfg_scale" => with_default(number_spec(1.0, 3.0, 2.0), json!(2.0)),
        "lm_cfg" => number_spec(1.0, 3.0, 2.0),
        "shift" => with_default(number_spec(1.0, 5.0, 3.0), json!(3.0)),
        "cfg_interval_start" => with_default(number_spec(0.0, 1.0, 0.0), json!(0.0)),
        "cfg_interval_end" => with_default(number_spec(0.0, 1.0, 1.0), json!(1.0)),
        "repaint_start" => with_default(number_spec(0.0, 599.999, 0.0), json!(0.0)),
        "repainting_start" => number_spec(0.0, 599.999, 0.0),
        "repaint_end" => with_default(number_spec(-1.0, 600.0, 30.0), json!(-1.0)),
        "repainting_end" => number_spec(-1.0, 600.0, 30.0),
        "repaint_strength" => with_default(number_spec(0.0, 1.0, 0.5), json!(0.5)),
        "cover_strength" => with_default(number_spec(0.0, 1.0, 1.0), json!(1.0)),
        "audio_cover_strength" => number_spec(0.0, 1.0, 1.0),
        "cover_noise_strength" => with_default(number_spec(0.0, 1.0, 0.0), json!(0.0)),
        "velocity_norm_threshold" => with_default(number_spec(0.0, 5.0, 0.0), json!(0.0)),
        "velocity_ema_factor" => with_default(number_spec(0.0, 0.5, 0.0), json!(0.0)),
        "dcw_scaler" => with_default(number_spec(0.0, 0.1, 0.05), json!(0.05)),
        "dcw_high_scaler" => with_default(number_spec(0.0, 0.1, 0.02), json!(0.02)),
        "normalization_db" => with_default(number_spec(-10.0, 0.0, -1.0), json!(-1.0)),
        "fade_in_duration" | "fade_out_duration" => {
            with_default(number_spec(0.0, 10.0, 0.0), json!(0.0))
        }
        "latent_shift" => with_default(number_spec(-0.2, 0.2, 0.0), json!(0.0)),
        "latent_rescale" => with_default(number_spec(0.5, 1.5, 1.0), json!(1.0)),
        "retake_variance" => with_default(number_spec(0.0, 1.0, 0.5), json!(0.0)),
        "flow_edit_n_min" => with_default(number_spec(0.0, 1.0, 0.0), json!(0.0)),
        "flow_edit_n_max" => with_default(number_spec(0.0, 1.0, 1.0), json!(1.0)),
        "retake_seed" => integer_spec(-1.0, MAX_MUSIC_SEED, 7),
        "custom_timesteps" | "timesteps" => {
            array_spec(2, MAX_MUSIC_CUSTOM_TIMESTEPS, json!([1.0, 0.5, 0.0]))
        }
        "seeds" => array_spec(1, 8, json!([7, 11])),
        _ => return None,
    };
    Some(spec)
}

fn music_inline_audio_object_spec() -> EndpointAttributeSpec {
    object_spec(json!({
        "data": endpoint_calibration_wav_base64_fixture(
            MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize
        ).expect("minimum music audio fixture is constructible"),
        "encoding": "base64",
        "content_type": "audio/wav"
    }))
}

fn music_inline_audio_content_type_spec() -> EndpointAttributeSpec {
    let values = MUSIC_INLINE_AUDIO_CONTENT_TYPES
        .iter()
        .map(|content_type| json!(content_type))
        .collect::<Vec<_>>();
    enum_spec(None, &values)
}

fn tts_reference_audio_object_spec() -> EndpointAttributeSpec {
    object_spec(json!({
        "data": endpoint_calibration_wav_base64_fixture(
            MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize
        ).expect("minimum speech reference-audio fixture is constructible"),
        "encoding": "base64",
        "content_type": "audio/wav"
    }))
}

fn tts_reference_audio_content_type_spec() -> EndpointAttributeSpec {
    enum_spec(None, &[json!("audio/wav")])
}

fn music_language_spec(default: Option<Value>) -> EndpointAttributeSpec {
    let values = MUSIC_VALID_LANGUAGES
        .iter()
        .map(|language| json!(language))
        .collect::<Vec<_>>();
    enum_spec(default, &values)
}

fn music_duration_spec(default: Option<Value>) -> EndpointAttributeSpec {
    let mut spec = union_spec(
        &[
            EndpointValueType::Number,
            EndpointValueType::String,
            EndpointValueType::Null,
        ],
        &[
            Value::Null,
            json!("auto"),
            json!(-1.0),
            json!(10.0),
            json!(600.0),
        ],
    );
    spec.default = default;
    spec.minimum = Some(-1.0);
    spec.maximum = Some(600.0);
    spec
}

fn music_bpm_spec() -> EndpointAttributeSpec {
    let mut spec = union_spec(
        &[EndpointValueType::Integer, EndpointValueType::Null],
        &[Value::Null, json!(30), json!(120), json!(300)],
    );
    spec.default = Some(Value::Null);
    spec.minimum = Some(30.0);
    spec.maximum = Some(300.0);
    spec
}

fn music_audio_codes_spec(default: Option<Value>) -> EndpointAttributeSpec {
    let mut spec = union_spec(
        &[EndpointValueType::String, EndpointValueType::Array],
        &[
            json!("<|audio_code_1|><|audio_code_2|>"),
            json!([
                "<|audio_code_1|><|audio_code_2|>",
                "<|audio_code_3|><|audio_code_4|>"
            ]),
        ],
    );
    spec.default = default;
    spec.min_length = Some(0);
    spec.max_length = Some(MAX_MUSIC_AUDIO_CODES_CHARS);
    spec.min_items = Some(1);
    spec.max_items = Some(8);
    spec
}

fn music_dcw_mode_spec() -> EndpointAttributeSpec {
    let mut spec = enum_spec(
        Some(json!("double")),
        &[json!("low"), json!("high"), json!("double"), json!("pix")],
    );
    spec.calibration_values = vec![json!("low"), json!("high"), json!("pix"), json!("double")];
    spec
}

fn music_inline_audio_child(path: &str, child: &str) -> bool {
    MUSIC_INLINE_AUDIO_ROOTS
        .iter()
        .any(|root| path == format!("{root}.{child}"))
}

fn endpoint_inline_audio_child(family: &str, path: &str, child: &str) -> bool {
    match family {
        ENDPOINT_MAYHEM_MUSIC_GENERATIONS => music_inline_audio_child(path, child),
        ENDPOINT_OPENAI_AUDIO_SPEECH => path == format!("reference_audio.{child}"),
        ENDPOINT_HF_TEXT_TO_SPEECH => path == format!("parameters.reference_audio.{child}"),
        _ => false,
    }
}

fn response_attribute_spec(path: &str) -> Option<EndpointAttributeSpec> {
    let leaf = path.rsplit('.').next().unwrap_or(path);
    let spec = match leaf {
        "id" | "object" | "model" | "status" | "content_type" | "text" | "task" | "language"
        | "audio" | "music" | "image" | "video" | "size" | "seconds" | "quality"
        | "output_format" | "prompt" | "background" => {
            string_spec(0, 512 * 1024 * 1024, json!("$RESPONSE_VALUE"))
        }
        "duration" => number_spec(0.0, 86_400.0, 1.0),
        "sampling_rate" => integer_spec(1.0, 1_000_000.0, 16_000),
        "created" | "created_at" | "completed_at" | "expires_at" | "progress" => {
            integer_spec(0.0, u64::MAX as f64, 1)
        }
        "choices" | "data" | "output" | "embeddings" | "chunks" | "words" | "segments" => {
            array_spec(0, 1_000_000, json!([]))
        }
        "usage" | "mayhem" => object_spec(json!({})),
        "error" => union_spec(
            &[EndpointValueType::Object, EndpointValueType::Null],
            &[json!({}), Value::Null],
        ),
        _ => return None,
    };
    Some(spec)
}

fn boolean_leaf(leaf: &str) -> bool {
    matches!(
        leaf,
        "normalize" | "truncate" | "return_timestamps" | "do_sample" | "use_cache"
    )
}

fn integer_leaf(leaf: &str) -> bool {
    matches!(
        leaf,
        "max_tokens"
            | "max_completion_tokens"
            | "max_output_tokens"
            | "top_k"
            | "seed"
            | "dimensions"
            | "n"
            | "output_compression"
            | "partial_images"
            | "steps"
            | "num_inference_steps"
            | "num_images_per_prompt"
            | "max_length"
            | "max_new_tokens"
            | "min_length"
            | "min_new_tokens"
            | "num_beams"
            | "num_beam_groups"
            | "width"
            | "height"
    )
}

fn number_leaf(leaf: &str) -> bool {
    matches!(
        leaf,
        "temperature"
            | "top_p"
            | "min_p"
            | "typical_p"
            | "frequency_penalty"
            | "presence_penalty"
            | "repeat_penalty"
            | "cfg_scale"
            | "guidance_scale"
            | "shift"
            | "speed"
            | "duration_seconds"
            | "penalty_alpha"
    )
}

fn integer_maximum(leaf: &str) -> f64 {
    match leaf {
        "n" => 10.0,
        "num_images_per_prompt" => 10.0,
        "partial_images" => 3.0,
        "output_compression" => 100.0,
        "steps" | "num_inference_steps" => 1_000.0,
        "width" | "height" => 16_384.0,
        "dimensions" => 1_000_000.0,
        "seed" => u32::MAX as f64,
        "top_k" => 1_000_000.0,
        "num_beams" | "num_beam_groups" => 1_024.0,
        _ => 262_144.0,
    }
}

fn integer_baseline(leaf: &str) -> i64 {
    match leaf {
        "n" => 1,
        "num_images_per_prompt" => 1,
        "partial_images" => 1,
        "output_compression" => 50,
        "steps" | "num_inference_steps" => 4,
        "width" | "height" => 512,
        "dimensions" => 32,
        "top_k" => 20,
        "seed" => 7,
        "num_beams" | "num_beam_groups" => 1,
        _ => 8,
    }
}

fn number_minimum(leaf: &str) -> f64 {
    match leaf {
        "frequency_penalty" | "presence_penalty" => -2.0,
        "repeat_penalty" | "speed" => 0.01,
        "top_p" => 0.000_001,
        _ => 0.0,
    }
}

fn number_maximum(leaf: &str) -> f64 {
    match leaf {
        "temperature" => 2.0,
        "top_p" | "min_p" | "typical_p" => 1.0,
        "frequency_penalty" | "presence_penalty" => 2.0,
        "repeat_penalty" => 10.0,
        "cfg_scale" | "guidance_scale" => 100.0,
        "shift" => 10.0,
        "speed" => 4.0,
        "duration_seconds" => 86_400.0,
        "penalty_alpha" => 1.0,
        _ => 1_000_000.0,
    }
}

fn number_baseline(leaf: &str) -> f64 {
    match leaf {
        "temperature" => 0.6,
        "top_p" => 0.95,
        "min_p" => 0.05,
        "typical_p" => 0.95,
        "repeat_penalty" | "speed" => 1.0,
        "cfg_scale" | "guidance_scale" => 1.0,
        "shift" => 3.0,
        "duration_seconds" => 1.0,
        _ => 0.0,
    }
}

fn string_spec(min_length: u64, max_length: u64, calibration: Value) -> EndpointAttributeSpec {
    let mut spec = EndpointAttributeSpec::new(EndpointValueType::String);
    spec.min_length = Some(min_length);
    spec.max_length = Some(max_length);
    spec.calibration_values.push(calibration);
    spec
}

fn marker_string_spec(calibration: Value) -> EndpointAttributeSpec {
    let mut spec = EndpointAttributeSpec::new(EndpointValueType::String);
    spec.calibration_values.push(calibration);
    spec
}

fn boolean_spec(default: Option<bool>) -> EndpointAttributeSpec {
    let mut spec = EndpointAttributeSpec::new(EndpointValueType::Boolean);
    spec.default = default.map(Value::Bool);
    spec.calibration_values = vec![Value::Bool(false), Value::Bool(true)];
    spec
}

fn integer_spec(minimum: f64, maximum: f64, calibration: i64) -> EndpointAttributeSpec {
    let mut spec = EndpointAttributeSpec::new(EndpointValueType::Integer);
    spec.minimum = Some(minimum);
    spec.maximum = Some(maximum);
    spec.calibration_values = vec![json!(calibration)];
    spec
}

fn number_spec(minimum: f64, maximum: f64, calibration: f64) -> EndpointAttributeSpec {
    let mut spec = EndpointAttributeSpec::new(EndpointValueType::Number);
    spec.minimum = Some(minimum);
    spec.maximum = Some(maximum);
    spec.calibration_values = vec![json!(calibration)];
    spec
}

fn with_default(mut spec: EndpointAttributeSpec, default: Value) -> EndpointAttributeSpec {
    spec.default = Some(default);
    spec
}

fn object_spec(calibration: Value) -> EndpointAttributeSpec {
    let mut spec = EndpointAttributeSpec::new(EndpointValueType::Object);
    spec.calibration_values.push(calibration);
    spec
}

fn object_spec_with_default(default: Value) -> EndpointAttributeSpec {
    let mut spec = object_spec(default.clone());
    spec.default = Some(default);
    spec
}

fn array_spec(min_items: u64, max_items: u64, calibration: Value) -> EndpointAttributeSpec {
    let mut spec = EndpointAttributeSpec::new(EndpointValueType::Array);
    spec.min_items = Some(min_items);
    spec.max_items = Some(max_items);
    spec.calibration_values.push(calibration);
    spec
}

fn union_spec(
    value_types: &[EndpointValueType],
    calibration_values: &[Value],
) -> EndpointAttributeSpec {
    EndpointAttributeSpec {
        value_types: value_types.to_vec(),
        default: None,
        enum_values: Vec::new(),
        minimum: None,
        maximum: None,
        multiple_of: None,
        min_length: None,
        max_length: None,
        min_items: None,
        max_items: None,
        calibration_values: calibration_values.to_vec(),
    }
}

fn enum_spec(default: Option<Value>, values: &[Value]) -> EndpointAttributeSpec {
    let value_types = values
        .iter()
        .map(value_type)
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default();
    let mut value_types = value_types
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let mut spec =
        EndpointAttributeSpec::new(value_types.pop_first().unwrap_or(EndpointValueType::String));
    spec.value_types.extend(value_types);
    spec.default = default;
    spec.enum_values = values.to_vec();
    spec.calibration_values = values.to_vec();
    spec
}

fn array_enum_spec(
    default: Option<Value>,
    values: &[Value],
    min_items: u64,
    max_items: u64,
) -> EndpointAttributeSpec {
    let mut spec = EndpointAttributeSpec::new(EndpointValueType::Array);
    spec.default = default;
    spec.enum_values = values.to_vec();
    spec.min_items = Some(min_items);
    spec.max_items = Some(max_items);
    spec.calibration_values = values.iter().map(|value| json!([value])).collect();
    spec
}

fn value_type(value: &Value) -> Option<EndpointValueType> {
    match value {
        Value::Null => Some(EndpointValueType::Null),
        Value::Bool(_) => Some(EndpointValueType::Boolean),
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            Some(EndpointValueType::Integer)
        }
        Value::Number(_) => Some(EndpointValueType::Number),
        Value::String(_) => Some(EndpointValueType::String),
        Value::Array(_) => Some(EndpointValueType::Array),
        Value::Object(_) => Some(EndpointValueType::Object),
    }
}

#[must_use]
pub fn endpoint_attribute_value_matches(spec: &EndpointAttributeSpec, value: &Value) -> bool {
    validate_endpoint_attribute_value(spec, value).is_ok()
}

pub fn validate_endpoint_attribute_value(
    spec: &EndpointAttributeSpec,
    value: &Value,
) -> Result<(), String> {
    let actual = value_type(value).ok_or_else(|| "unsupported JSON value type".to_owned())?;
    let type_allowed = spec.value_types.contains(&actual)
        || (actual == EndpointValueType::Integer
            && spec.value_types.contains(&EndpointValueType::Number));
    if !type_allowed {
        return Err(format!(
            "expected one of {:?}, got {:?}",
            spec.value_types, actual
        ));
    }
    if !spec.enum_values.is_empty() {
        let enum_match = if actual == EndpointValueType::Array {
            value.as_array().is_some_and(|items| {
                items
                    .iter()
                    .all(|item| spec.enum_values.iter().any(|allowed| allowed == item))
            })
        } else {
            spec.enum_values.iter().any(|allowed| allowed == value)
        };
        if !enum_match {
            return Err(format!("value {value} is not in the declared enum"));
        }
    }
    if let Some(number) = value.as_f64() {
        if !number.is_finite() {
            return Err("number must be finite".to_owned());
        }
        if spec.minimum.is_some_and(|minimum| number < minimum) {
            return Err(format!("number {number} is below the minimum"));
        }
        if spec.maximum.is_some_and(|maximum| number > maximum) {
            return Err(format!("number {number} exceeds the maximum"));
        }
        if let Some(multiple) = spec.multiple_of {
            if !multiple.is_finite() || multiple <= 0.0 {
                return Err("multiple_of must be finite and greater than zero".to_owned());
            }
            let quotient = number / multiple;
            let tolerance = f64::EPSILON * quotient.abs().max(1.0) * 8.0;
            if (quotient - quotient.round()).abs() > tolerance {
                return Err(format!("number {number} is not a multiple of {multiple}"));
            }
        }
    }
    if let Some(text) = value.as_str() {
        let length = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
        if spec.min_length.is_some_and(|minimum| length < minimum) {
            return Err(format!("string length {length} is below the minimum"));
        }
        if spec.max_length.is_some_and(|maximum| length > maximum) {
            return Err(format!("string length {length} exceeds the maximum"));
        }
    }
    if let Some(items) = value.as_array() {
        let length = u64::try_from(items.len()).unwrap_or(u64::MAX);
        if spec.min_items.is_some_and(|minimum| length < minimum) {
            return Err(format!("array length {length} is below the minimum"));
        }
        if spec.max_items.is_some_and(|maximum| length > maximum) {
            return Err(format!("array length {length} exceeds the maximum"));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EndpointCalibrationValue {
    Literal { value: Value },
    Omitted,
    StringLength { length: u64 },
    ArrayLength { length: u64, item: Value },
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EndpointCalibrationMutation {
    pub path: String,
    pub value: EndpointCalibrationValue,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EndpointCalibrationCase {
    pub case_id: String,
    pub endpoint_family: String,
    pub case_kind: String,
    pub attributes: Vec<String>,
    pub expect_accept: bool,
    pub base_request: Value,
    pub mutations: Vec<EndpointCalibrationMutation>,
    pub expected_response_attributes: Vec<String>,
    pub contract_fingerprint: String,
}

pub fn generate_endpoint_calibration_cases(
    contract: &EndpointFamilyContract,
) -> Result<Vec<EndpointCalibrationCase>, String> {
    let mut base_request = json!({});
    for path in &contract.required_request_attributes {
        let spec = contract
            .request_attribute_specs
            .get(path)
            .ok_or_else(|| format!("required attribute {path} has no signed spec"))?;
        let value = endpoint_calibration_baseline(spec)
            .ok_or_else(|| format!("required attribute {path} has no calibration baseline"))?;
        set_endpoint_path(&mut base_request, path, value)?;
    }
    if contract.family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS {
        for path in ["prompt", "lyrics"] {
            let Some(spec) = contract.request_attribute_specs.get(path) else {
                continue;
            };
            let value = endpoint_calibration_nonempty_string(spec)
                .or_else(|| endpoint_calibration_baseline(spec))
                .ok_or_else(|| format!("music attribute {path} has no calibration baseline"))?;
            set_endpoint_path(&mut base_request, path, value)?;
        }
    }
    validate_endpoint_request(contract, &base_request).map_err(|violations| {
        format!(
            "generated baseline request is invalid: {}",
            violations
                .iter()
                .map(|violation| format!("{}: {}", violation.path, violation.reason))
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;

    let contract_fingerprint = endpoint_contract_fingerprint(contract);
    let mut cases = Vec::new();
    for path in &contract.request_attributes {
        let spec = contract
            .request_attribute_specs
            .get(path)
            .ok_or_else(|| format!("request attribute {path} has no signed spec"))?;
        let required = contract.required_request_attributes.contains(path);
        if required {
            push_calibration_case(
                &mut cases,
                contract,
                &contract_fingerprint,
                &base_request,
                "required_present",
                vec![literal_mutation(
                    path,
                    endpoint_calibration_baseline(spec).ok_or_else(|| {
                        format!("required attribute {path} has no calibration baseline")
                    })?,
                )],
                true,
                Vec::new(),
            );
            push_calibration_case(
                &mut cases,
                contract,
                &contract_fingerprint,
                &base_request,
                "required_missing",
                vec![EndpointCalibrationMutation {
                    path: path.clone(),
                    value: EndpointCalibrationValue::Omitted,
                }],
                false,
                Vec::new(),
            );
        } else {
            push_calibration_case(
                &mut cases,
                contract,
                &contract_fingerprint,
                &base_request,
                if spec.default.is_some() {
                    "omitted_default"
                } else {
                    "omitted_optional"
                },
                calibration_mutations_for_omission(contract, path),
                true,
                Vec::new(),
            );
        }

        let mut accepted_values = spec.calibration_values.clone();
        if let Some(default) = &spec.default {
            accepted_values.push(default.clone());
        }
        if spec.value_types == [EndpointValueType::Array] {
            accepted_values.extend(spec.enum_values.iter().cloned().map(|value| json!([value])));
        } else {
            accepted_values.extend(spec.enum_values.iter().cloned());
        }
        deduplicate_values(&mut accepted_values);
        for (index, value) in accepted_values.into_iter().enumerate() {
            push_calibration_case(
                &mut cases,
                contract,
                &contract_fingerprint,
                &base_request,
                &format!("accepted_value_{index}"),
                calibration_mutations_for_literal(contract, path, value),
                true,
                Vec::new(),
            );
        }

        add_boundary_calibration_cases(
            &mut cases,
            contract,
            &contract_fingerprint,
            &base_request,
            path,
            spec,
        );
        push_calibration_case(
            &mut cases,
            contract,
            &contract_fingerprint,
            &base_request,
            "wrong_type",
            vec![literal_mutation(path, wrong_type_value(spec))],
            false,
            Vec::new(),
        );
        if !spec.enum_values.is_empty() {
            let invalid_enum = if spec.value_types.contains(&EndpointValueType::Array) {
                json!(["__mayhem_invalid_enum__"])
            } else {
                json!("__mayhem_invalid_enum__")
            };
            push_calibration_case(
                &mut cases,
                contract,
                &contract_fingerprint,
                &base_request,
                "invalid_enum",
                vec![literal_mutation(path, invalid_enum)],
                false,
                Vec::new(),
            );
        }
    }

    for path in &contract.response_attributes {
        push_calibration_case(
            &mut cases,
            contract,
            &contract_fingerprint,
            &base_request,
            "response_attribute",
            response_attribute_calibration_mutations(contract, path),
            true,
            vec![path.clone()],
        );
    }

    let mut covered_interaction_pairs = std::collections::BTreeSet::new();
    for group in &contract.interaction_groups {
        for left in 0..group.len() {
            for right in (left + 1)..group.len() {
                let left_path = &group[left];
                let right_path = &group[right];
                let pair = if left_path <= right_path {
                    (left_path.clone(), right_path.clone())
                } else {
                    (right_path.clone(), left_path.clone())
                };
                if !covered_interaction_pairs.insert(pair) {
                    continue;
                }
                let left_value = endpoint_calibration_interaction_value(
                    contract
                        .request_attribute_specs
                        .get(left_path)
                        .ok_or_else(|| format!("interaction attribute {left_path} has no spec"))?,
                )
                .ok_or_else(|| format!("interaction attribute {left_path} has no test value"))?;
                let right_value = endpoint_calibration_interaction_value(
                    contract
                        .request_attribute_specs
                        .get(right_path)
                        .ok_or_else(|| format!("interaction attribute {right_path} has no spec"))?,
                )
                .ok_or_else(|| format!("interaction attribute {right_path} has no test value"))?;
                push_calibration_case(
                    &mut cases,
                    contract,
                    &contract_fingerprint,
                    &base_request,
                    "pairwise_interaction",
                    {
                        let mut mutations = vec![
                            literal_mutation(left_path, left_value.clone()),
                            literal_mutation(right_path, right_value.clone()),
                        ];
                        add_calibration_companion_mutations(
                            contract,
                            left_path,
                            Some(&left_value),
                            &mut mutations,
                        );
                        add_calibration_companion_mutations(
                            contract,
                            right_path,
                            Some(&right_value),
                            &mut mutations,
                        );
                        mutations
                    },
                    true,
                    Vec::new(),
                );
            }
        }
    }

    let mut ids = std::collections::BTreeMap::new();
    for case in &cases {
        if let Some(previous) = ids.insert(case.case_id.clone(), case) {
            return Err(format!(
                "generated calibration matrix contains duplicate case ID {} ({} {:?} and {} {:?})",
                case.case_id,
                previous.case_kind,
                previous.attributes,
                case.case_kind,
                case.attributes,
            ));
        }
    }
    Ok(cases)
}

fn response_attribute_calibration_mutations(
    contract: &EndpointFamilyContract,
    response_path: &str,
) -> Vec<EndpointCalibrationMutation> {
    let request_path_is_signed = |path: &str| {
        contract
            .request_attributes
            .iter()
            .any(|value| value == path)
    };
    let mut mutations = Vec::new();

    match (contract.family.as_str(), response_path) {
        (ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION, "chunks")
            if request_path_is_signed("parameters.return_timestamps") =>
        {
            mutations.push(literal_mutation(
                "parameters.return_timestamps",
                json!("word"),
            ));
        }
        (
            ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS,
            "task" | "language" | "duration" | "words" | "segments",
        ) if request_path_is_signed("response_format") => {
            mutations.push(literal_mutation("response_format", json!("verbose_json")));
            match response_path {
                "words" if request_path_is_signed("timestamp_granularities") => {
                    mutations.push(literal_mutation("timestamp_granularities", json!(["word"])));
                }
                "segments" if request_path_is_signed("timestamp_granularities") => {
                    mutations.push(literal_mutation(
                        "timestamp_granularities",
                        json!(["segment"]),
                    ));
                }
                _ => {}
            }
        }
        _ => {}
    }
    mutations
}

pub fn materialize_endpoint_calibration_request(
    case: &EndpointCalibrationCase,
    substitutions: &BTreeMap<String, Value>,
) -> Result<Value, String> {
    let mut request = substitute_calibration_markers(case.base_request.clone(), substitutions);
    for mutation in &case.mutations {
        match &mutation.value {
            EndpointCalibrationValue::Literal { value } => set_endpoint_path(
                &mut request,
                &mutation.path,
                substitute_calibration_markers(value.clone(), substitutions),
            )?,
            EndpointCalibrationValue::Omitted => {
                remove_endpoint_path(&mut request, &mutation.path)?;
            }
            EndpointCalibrationValue::StringLength { length } => {
                let length = usize::try_from(*length).map_err(|_| {
                    format!("string fixture for {} exceeds this platform", mutation.path)
                })?;
                let fixture = endpoint_calibration_string_fixture(case, &mutation.path, length);
                set_endpoint_path(&mut request, &mutation.path, json!(fixture))?;
            }
            EndpointCalibrationValue::ArrayLength { length, item } => {
                let length = usize::try_from(*length).map_err(|_| {
                    format!("array fixture for {} exceeds this platform", mutation.path)
                })?;
                let item = substitute_calibration_markers(item.clone(), substitutions);
                let values = if case.endpoint_family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS
                    && matches!(mutation.path.as_str(), "custom_timesteps" | "timesteps")
                    && case.expect_accept
                    && length >= 2
                {
                    let denominator = (length - 1) as f64;
                    (0..length)
                        .map(|index| json!(1.0 - index as f64 / denominator))
                        .collect()
                } else {
                    vec![item; length]
                };
                set_endpoint_path(&mut request, &mutation.path, Value::Array(values))?;
            }
        }
    }
    Ok(request)
}

pub fn materialize_endpoint_request_defaults(
    contract: &EndpointFamilyContract,
    request: &Value,
) -> Result<Value, String> {
    let mut normalized = if contract.family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS {
        normalize_music_request_aliases(contract, request)
            .map_err(|violation| format!("{}: {}", violation.path, violation.reason))?
    } else {
        request.clone()
    };
    let dimension_aliases = if matches!(
        contract.family.as_str(),
        ENDPOINT_OPENAI_IMAGE_GENERATIONS | ENDPOINT_OPENAI_VIDEOS
    ) {
        let object = normalized
            .as_object()
            .ok_or_else(|| "endpoint request must be an object".to_owned())?;
        let has_size = object.contains_key("size");
        let has_width = object.contains_key("width");
        let has_height = object.contains_key("height");
        if has_size && (has_width || has_height) {
            return Err("request cannot combine size with width/height".to_owned());
        }
        if has_width != has_height {
            return Err("request width and height must be supplied together".to_owned());
        }
        Some((
            has_size,
            has_width,
            contract.family == ENDPOINT_OPENAI_IMAGE_GENERATIONS,
        ))
    } else {
        None
    };
    for path in &contract.request_attributes {
        if dimension_aliases.is_some_and(|(has_size, has_dimensions, _)| {
            (has_size && matches!(path.as_str(), "width" | "height"))
                || (has_dimensions && path == "size")
        }) {
            continue;
        }
        if contract.family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS
            && music_alias_canonical(contract, path).is_some()
        {
            continue;
        }
        if contract.family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS
            && !music_request_default_applies(path, &normalized)
        {
            continue;
        }
        if !endpoint_values_at_path(&normalized, path).is_empty() {
            continue;
        }
        let Some(mut default) = contract
            .request_attribute_specs
            .get(path)
            .and_then(|spec| spec.default.clone())
        else {
            continue;
        };
        if contract.family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS
            && normalized
                .get("thinking")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            if path == "dcw_scaler" {
                default = json!(0.02);
            } else if path == "dcw_high_scaler" {
                default = json!(0.06);
            }
        }
        set_endpoint_path(&mut normalized, path, default)?;
    }
    if let Some((_, _, require_dimensions)) = dimension_aliases {
        let object = normalized
            .as_object()
            .ok_or_else(|| "normalized endpoint request must be an object".to_owned())?;
        let has_size = object.contains_key("size");
        let has_width = object.contains_key("width");
        let has_height = object.contains_key("height");
        if has_width != has_height || (has_size && has_width) {
            return Err(
                "signed endpoint contract cannot combine size with width/height".to_owned(),
            );
        }
        if require_dimensions && !has_size && !has_width {
            return Err(
                "signed image endpoint contract must resolve exactly one of size or width/height"
                    .to_owned(),
            );
        }
    }
    Ok(normalized)
}

pub fn artifact_generation_input_characters(family: &str, request: &Value) -> u64 {
    let paths: &[&str] = match family {
        ENDPOINT_MAYHEM_MUSIC_GENERATIONS => &[
            "prompt",
            "global_caption",
            "lyrics",
            "style",
            "genre",
            "tags",
            "sample_query",
            "audio_codes",
            "negative_prompt",
            "lm_negative_prompt",
            "instruction",
            "flow_edit_source_caption",
            "flow_edit_source_lyrics",
        ],
        ENDPOINT_MAYHEM_AUDIO_GENERATIONS => &["prompt", "negative_prompt"],
        ENDPOINT_HF_TEXT_TO_AUDIO => &["inputs", "parameters.negative_prompt"],
        _ => &["prompt", "inputs", "negative_prompt"],
    };
    paths
        .iter()
        .flat_map(|path| endpoint_values_at_path(request, path))
        .fold(0_u64, |total, value| {
            total.saturating_add(json_text_characters(value))
        })
}

fn json_text_characters(value: &Value) -> u64 {
    match value {
        Value::String(value) => u64::try_from(value.chars().count()).unwrap_or(u64::MAX),
        Value::Array(values) => values.iter().fold(0_u64, |total, value| {
            total.saturating_add(json_text_characters(value))
        }),
        _ => 0,
    }
}

fn normalize_music_request_aliases(
    contract: &EndpointFamilyContract,
    request: &Value,
) -> Result<Value, EndpointContractViolation> {
    let Some(request_object) = request.as_object() else {
        return Ok(request.clone());
    };
    let mut normalized = request_object.clone();
    for (canonical, aliases) in MUSIC_REQUEST_ALIAS_GROUPS {
        let canonical_is_signed = contract
            .request_attributes
            .iter()
            .any(|path| path == canonical);
        if !canonical_is_signed {
            continue;
        }
        let signed_aliases = aliases
            .iter()
            .filter(|alias| {
                contract
                    .request_attributes
                    .iter()
                    .any(|path| path == **alias)
            })
            .copied()
            .collect::<Vec<_>>();
        let supplied = std::iter::once(*canonical)
            .chain(signed_aliases.iter().copied())
            .filter(|path| request_object.contains_key(*path))
            .collect::<Vec<_>>();
        if supplied.len() > 1 {
            return Err(EndpointContractViolation {
                path: (*canonical).to_owned(),
                reason: format!(
                    "ambiguous aliases cannot be combined: {}",
                    supplied.join(", ")
                ),
            });
        }
        let Some(alias) = signed_aliases
            .iter()
            .find(|alias| request_object.contains_key(**alias))
        else {
            continue;
        };
        let value = normalized
            .remove(*alias)
            .expect("supplied music alias exists in normalized request");
        normalized.insert((*canonical).to_owned(), value);
    }
    if contract
        .request_attributes
        .iter()
        .any(|path| path == "no_fsq")
        && request_object.contains_key("no_fsq")
    {
        let no_fsq = normalized
            .remove("no_fsq")
            .and_then(|value| value.as_bool())
            .ok_or_else(|| EndpointContractViolation {
                path: "no_fsq".to_owned(),
                reason: "no_fsq must be a boolean".to_owned(),
            })?;
        if no_fsq {
            match normalized.get("task_type").and_then(Value::as_str) {
                None | Some("cover") => {
                    normalized.insert("task_type".to_owned(), json!("cover-nofsq"));
                }
                Some("cover-nofsq") => {}
                Some(task) => {
                    return Err(EndpointContractViolation {
                        path: "no_fsq".to_owned(),
                        reason: format!(
                            "no_fsq=true conflicts with task_type={task}; use cover or cover-nofsq"
                        ),
                    });
                }
            }
        }
    }
    Ok(Value::Object(normalized))
}

pub fn canonicalize_endpoint_request_aliases(
    contract: &EndpointFamilyContract,
    request: &Value,
) -> Result<Value, String> {
    if contract.family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS {
        normalize_music_request_aliases(contract, request)
            .map_err(|violation| format!("{}: {}", violation.path, violation.reason))
    } else {
        Ok(request.clone())
    }
}

fn music_alias_canonical(contract: &EndpointFamilyContract, path: &str) -> Option<&'static str> {
    MUSIC_REQUEST_ALIAS_GROUPS
        .iter()
        .find(|(canonical, aliases)| {
            contract
                .request_attributes
                .iter()
                .any(|signed| signed == canonical)
                && aliases.contains(&path)
                && contract
                    .request_attributes
                    .iter()
                    .any(|signed| signed == path)
        })
        .map(|(canonical, _)| *canonical)
}

fn music_request_default_applies(path: &str, request: &Value) -> bool {
    let task = request
        .get("task_type")
        .and_then(Value::as_str)
        .unwrap_or("text2music");
    if path == "seed" && request.get("seeds").is_some() {
        return false;
    }
    if path == "instruction" {
        return task == "text2music";
    }
    if music_lm_control(path) {
        if matches!(task, "cover" | "cover-nofsq" | "repaint")
            || request
                .get("flow_edit_morph")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return false;
        }
        if path == "use_cot_language"
            && request
                .get("language")
                .and_then(Value::as_str)
                .is_some_and(|language| language != "unknown")
        {
            return false;
        }
    }
    if matches!(path, "steps" | "shift") && request.get("custom_timesteps").is_some() {
        return false;
    }
    if matches!(path, "mp3_bitrate" | "mp3_sample_rate") {
        return request
            .get("response_format")
            .and_then(Value::as_str)
            .unwrap_or("flac")
            == "mp3";
    }
    if music_dcw_control(path) {
        if !request
            .get("dcw_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return false;
        }
        if path == "dcw_high_scaler" {
            return request
                .get("dcw_mode")
                .and_then(Value::as_str)
                .unwrap_or("double")
                == "double";
        }
    }
    if path == "normalization_db" {
        return request
            .get("enable_normalization")
            .and_then(Value::as_bool)
            .unwrap_or(true);
    }
    if music_repaint_control(path) {
        if path == "repaint_strength" {
            return task == "repaint"
                && request
                    .get("repaint_mode")
                    .and_then(Value::as_str)
                    .unwrap_or("balanced")
                    == "balanced";
        }
        return task == "repaint";
    }
    if music_cover_control(path) {
        return matches!(task, "cover" | "cover-nofsq" | "repaint");
    }
    if music_flow_control(path) {
        return request
            .get("flow_edit_morph")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    }
    true
}

fn music_repaint_control(path: &str) -> bool {
    matches!(
        path,
        "repaint_start" | "repaint_end" | "repaint_mode" | "repaint_strength" | "chunk_mask_mode"
    )
}

fn music_cover_control(path: &str) -> bool {
    matches!(path, "cover_strength" | "cover_noise_strength")
}

fn music_dcw_control(path: &str) -> bool {
    matches!(
        path,
        "dcw_mode" | "dcw_scaler" | "dcw_high_scaler" | "dcw_wavelet"
    )
}

fn music_lm_control(path: &str) -> bool {
    matches!(
        path,
        "lm_temperature"
            | "lm_cfg_scale"
            | "lm_top_k"
            | "lm_top_p"
            | "lm_negative_prompt"
            | "use_cot_metas"
            | "use_cot_caption"
            | "use_cot_language"
            | "constrained_decoding"
    )
}

fn music_flow_control(path: &str) -> bool {
    matches!(
        path,
        "flow_edit_source_caption"
            | "flow_edit_source_lyrics"
            | "flow_edit_n_min"
            | "flow_edit_n_max"
            | "flow_edit_n_avg"
    )
}

fn endpoint_calibration_baseline(spec: &EndpointAttributeSpec) -> Option<Value> {
    spec.default
        .clone()
        .or_else(|| spec.calibration_values.first().cloned())
        .or_else(|| spec.enum_values.first().cloned())
}

fn endpoint_calibration_nonempty_string(spec: &EndpointAttributeSpec) -> Option<Value> {
    spec.calibration_values
        .iter()
        .chain(spec.enum_values.iter())
        .find(|value| value.as_str().is_some_and(|value| !value.trim().is_empty()))
        .cloned()
}

fn endpoint_calibration_interaction_value(spec: &EndpointAttributeSpec) -> Option<Value> {
    spec.calibration_values
        .last()
        .cloned()
        .or_else(|| spec.enum_values.last().cloned())
        .or_else(|| spec.default.clone())
}

fn literal_mutation(path: &str, value: Value) -> EndpointCalibrationMutation {
    EndpointCalibrationMutation {
        path: path.to_owned(),
        value: EndpointCalibrationValue::Literal { value },
    }
}

fn calibration_mutations_for_literal(
    contract: &EndpointFamilyContract,
    path: &str,
    value: Value,
) -> Vec<EndpointCalibrationMutation> {
    let mut mutations = vec![literal_mutation(path, value.clone())];
    add_calibration_companion_mutations(contract, path, Some(&value), &mut mutations);
    mutations
}

fn calibration_mutations_for_omission(
    contract: &EndpointFamilyContract,
    path: &str,
) -> Vec<EndpointCalibrationMutation> {
    let mut mutations = vec![EndpointCalibrationMutation {
        path: path.to_owned(),
        value: EndpointCalibrationValue::Omitted,
    }];
    if contract.family != ENDPOINT_MAYHEM_MUSIC_GENERATIONS {
        return mutations;
    }
    let semantic_path = music_alias_canonical(contract, path).unwrap_or(path);
    if let Some(canonical) = music_alias_canonical(contract, path) {
        mutations.push(EndpointCalibrationMutation {
            path: canonical.to_owned(),
            value: EndpointCalibrationValue::Omitted,
        });
    }
    let companion = match semantic_path {
        "prompt" => "lyrics",
        "lyrics" => "prompt",
        _ => return mutations,
    };
    if let Some(value) = contract
        .request_attribute_specs
        .get(companion)
        .and_then(endpoint_calibration_nonempty_string)
    {
        mutations.push(literal_mutation(companion, value));
    }
    mutations
}

fn endpoint_calibration_companion_baseline(
    contract: &EndpointFamilyContract,
    path: &str,
    fallback: Value,
) -> Value {
    contract
        .request_attribute_specs
        .get(path)
        .and_then(endpoint_calibration_baseline)
        .unwrap_or(fallback)
}

fn endpoint_calibration_companion_maximum(
    contract: &EndpointFamilyContract,
    path: &str,
    fallback: Value,
) -> Value {
    contract
        .request_attribute_specs
        .get(path)
        .and_then(|spec| {
            spec.calibration_values
                .iter()
                .chain(spec.enum_values.iter())
                .chain(spec.default.iter())
                .filter(|value| validate_endpoint_attribute_value(spec, value).is_ok())
                .filter_map(|value| value.as_f64().map(|number| (number, value)))
                .max_by(|(left, _), (right, _)| left.total_cmp(right))
                .map(|(_, value)| value.clone())
        })
        .unwrap_or(fallback)
}

fn endpoint_calibration_companion_minimum(
    contract: &EndpointFamilyContract,
    path: &str,
    fallback: Value,
) -> Value {
    contract
        .request_attribute_specs
        .get(path)
        .and_then(|spec| {
            spec.calibration_values
                .iter()
                .chain(spec.enum_values.iter())
                .chain(spec.default.iter())
                .filter(|value| validate_endpoint_attribute_value(spec, value).is_ok())
                .filter_map(|value| value.as_f64().map(|number| (number, value)))
                .min_by(|(left, _), (right, _)| left.total_cmp(right))
                .map(|(_, value)| value.clone())
        })
        .unwrap_or(fallback)
}

fn endpoint_calibration_video_frame_count(contract: &EndpointFamilyContract) -> usize {
    let numeric_minimum = contract
        .request_attribute_specs
        .get("messages.content.video.num_frames")
        .and_then(|spec| spec.minimum)
        .filter(|minimum| minimum.is_finite() && *minimum >= 1.0)
        .map(|minimum| minimum.ceil() as usize);
    let calibrated_minimum = endpoint_calibration_companion_minimum(
        contract,
        "messages.content.video.num_frames",
        json!(2),
    )
    .as_u64()
    .and_then(|count| usize::try_from(count).ok())
    .filter(|count| *count > 0)
    .unwrap_or(2);
    let decoded_frames_minimum = contract
        .request_attribute_specs
        .get("messages.content.video.frames")
        .and_then(|spec| spec.min_items)
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or(1);
    numeric_minimum
        .unwrap_or(calibrated_minimum)
        .max(decoded_frames_minimum)
        .max(1)
}

fn add_calibration_companion_mutations(
    contract: &EndpointFamilyContract,
    path: &str,
    value: Option<&Value>,
    mutations: &mut Vec<EndpointCalibrationMutation>,
) {
    if contract.family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS {
        if let Some(canonical) = music_alias_canonical(contract, path) {
            mutations.push(EndpointCalibrationMutation {
                path: canonical.to_owned(),
                value: EndpointCalibrationValue::Omitted,
            });
        }
        let semantic_path = music_alias_canonical(contract, path).unwrap_or(path);
        if matches!(semantic_path, "style" | "genre" | "tags")
            && !mutations.iter().any(|mutation| mutation.path == "prompt")
        {
            mutations.push(EndpointCalibrationMutation {
                path: "prompt".to_owned(),
                value: EndpointCalibrationValue::Omitted,
            });
        }
    }
    let mut add = |companion_path: &str, companion_value: Value| {
        if contract
            .request_attributes
            .iter()
            .any(|declared| declared == companion_path)
            && !(contract.family == ENDPOINT_OPENAI_VIDEOS
                && companion_path == "num_frames"
                && mutations.iter().any(|mutation| mutation.path == "seconds"))
            && !mutations
                .iter()
                .any(|mutation| mutation.path == companion_path)
        {
            mutations.push(literal_mutation(companion_path, companion_value));
        }
    };

    if path.starts_with("messages.content.") {
        add("messages.role", json!("user"));
    }
    if let Some(root) = endpoint_inline_audio_root(&contract.family, path) {
        add(&format!("{root}.data"), json!("$AUDIO_BASE64"));
        add(&format!("{root}.encoding"), json!("base64"));
        add(
            &format!("{root}.content_type"),
            json!("$AUDIO_CONTENT_TYPE"),
        );
    }
    if contract.family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS {
        let semantic_path = music_alias_canonical(contract, path).unwrap_or(path);
        let empty_semantic_text = value
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty());
        if semantic_path == "prompt" && empty_semantic_text {
            if let Some(lyrics) = contract
                .request_attribute_specs
                .get("lyrics")
                .and_then(endpoint_calibration_nonempty_string)
            {
                add("lyrics", lyrics);
            }
        } else if semantic_path == "lyrics" && empty_semantic_text {
            if let Some(prompt) = contract
                .request_attribute_specs
                .get("prompt")
                .and_then(endpoint_calibration_nonempty_string)
            {
                add("prompt", prompt);
            }
        }
        if matches!(semantic_path, "style" | "genre" | "tags") {
            if let Some(lyrics) = contract
                .request_attribute_specs
                .get("lyrics")
                .and_then(endpoint_calibration_nonempty_string)
            {
                add("lyrics", lyrics);
            }
        }
        if semantic_path == "instrumental" && value.and_then(Value::as_bool) == Some(true) {
            add("lyrics", json!("[Instrumental]"));
        }
        let task_needs_source = semantic_path == "task_type"
            && matches!(
                value.and_then(Value::as_str),
                Some("cover" | "cover-nofsq" | "repaint")
            );
        let flow_enabled =
            semantic_path == "flow_edit_morph" && value.and_then(Value::as_bool) == Some(true);
        if music_repaint_control(semantic_path) {
            add("task_type", json!("repaint"));
            if semantic_path == "repaint_strength" {
                add("repaint_mode", json!("balanced"));
            }
        } else if music_cover_control(semantic_path) {
            add("task_type", json!("cover"));
        } else if music_flow_control(semantic_path) {
            add("flow_edit_morph", json!(true));
        }
        if music_inline_audio_root(semantic_path)
            .is_some_and(|root| matches!(root, "source_audio" | "src_audio" | "ctx_audio"))
        {
            add("task_type", json!("cover"));
        }
        if semantic_path == "audio_codes" && music_audio_codes_nonempty(value) {
            add("task_type", json!("cover"));
            if let Some(length) = value
                .and_then(Value::as_array)
                .and_then(|items| u64::try_from(items.len()).ok())
            {
                add("n", json!(length));
            }
        }
        if music_dcw_control(semantic_path) {
            add("dcw_enabled", json!(true));
            if semantic_path == "dcw_high_scaler" {
                add("dcw_mode", json!("double"));
            }
        }
        if semantic_path == "normalization_db" {
            add("enable_normalization", json!(true));
        }
        if semantic_path == "no_fsq" && value.and_then(Value::as_bool) == Some(true) {
            add("task_type", json!("cover"));
            add("source_audio.data", json!("$AUDIO_BASE64"));
            add("source_audio.encoding", json!("base64"));
            add("source_audio.content_type", json!("$AUDIO_CONTENT_TYPE"));
        }
        if semantic_path == "retake_seed" {
            add("retake_variance", json!(0.5));
        }
        if matches!(semantic_path, "mp3_bitrate" | "mp3_sample_rate") {
            add("response_format", json!("mp3"));
        }
        if semantic_path == "seeds" {
            if let Some(length) = value
                .and_then(Value::as_array)
                .and_then(|items| u64::try_from(items.len()).ok())
            {
                add("n", json!(length));
            }
        }
        if task_needs_source
            || flow_enabled
            || music_repaint_control(semantic_path)
            || music_cover_control(semantic_path)
            || music_flow_control(semantic_path)
        {
            add("source_audio.data", json!("$AUDIO_BASE64"));
            add("source_audio.encoding", json!("base64"));
            add("source_audio.content_type", json!("$AUDIO_CONTENT_TYPE"));
        }
    }

    match path {
        "messages.role" if value.and_then(Value::as_str) == Some("tool") => {
            add("messages.tool_call_id", json!("call-calibration"));
        }
        "messages.tool_calls" => add("messages.role", json!("assistant")),
        "messages.tool_call_id" => add("messages.role", json!("tool")),
        "messages.content.type" => match value.and_then(Value::as_str) {
            Some("text") => add("messages.content.text", json!("Mayhem calibration")),
            Some("image_url") => {
                add("messages.content.image_url.url", json!("$IMAGE_DATA_URL"));
            }
            Some("input_audio") => {
                add("messages.content.input_audio.data", json!("$AUDIO_BASE64"));
                add("messages.content.input_audio.format", json!("wav"));
            }
            Some("video") => {
                let frame_count = endpoint_calibration_video_frame_count(contract);
                add(
                    "messages.content.video.frames",
                    Value::Array(vec![json!("$IMAGE_DATA_URL"); frame_count]),
                );
                add("messages.content.video.num_frames", json!(frame_count));
                add(
                    "messages.content.video.fps",
                    endpoint_calibration_companion_baseline(
                        contract,
                        "messages.content.video.fps",
                        json!(8.0),
                    ),
                );
            }
            _ => {}
        },
        "messages.content.text" => add("messages.content.type", json!("text")),
        "messages.content.image_url.url" => {
            add("messages.content.type", json!("image_url"));
        }
        "messages.content.input_audio.data" | "messages.content.input_audio.format" => {
            add("messages.content.type", json!("input_audio"));
            add("messages.content.input_audio.data", json!("$AUDIO_BASE64"));
            add("messages.content.input_audio.format", json!("wav"));
        }
        "messages.content.video.data" | "messages.content.video.content_type" => {
            let frame_count = endpoint_calibration_video_frame_count(contract);
            add("messages.content.type", json!("video"));
            add("messages.content.video.data", json!("$VIDEO_BASE64"));
            add("messages.content.video.content_type", json!("video/mp4"));
            add("messages.content.video.num_frames", json!(frame_count));
            add(
                "messages.content.video.fps",
                endpoint_calibration_companion_baseline(
                    contract,
                    "messages.content.video.fps",
                    json!(8.0),
                ),
            );
        }
        "messages.content.video.frames" => {
            let frame_count = value.and_then(Value::as_array).map(Vec::len).unwrap_or(2);
            add("messages.content.type", json!("video"));
            add(
                "messages.content.video.frames",
                Value::Array(vec![json!("$IMAGE_DATA_URL"); frame_count]),
            );
            add("messages.content.video.num_frames", json!(frame_count));
            add(
                "messages.content.video.fps",
                endpoint_calibration_companion_baseline(
                    contract,
                    "messages.content.video.fps",
                    json!(8.0),
                ),
            );
        }
        "messages.content.video.num_frames" => {
            let frame_count = value
                .and_then(Value::as_u64)
                .map(|count| count.min(64))
                .and_then(|count| usize::try_from(count).ok())
                .filter(|count| *count > 0)
                .unwrap_or_else(|| endpoint_calibration_video_frame_count(contract));
            add("messages.content.type", json!("video"));
            add(
                "messages.content.video.frames",
                Value::Array(vec![json!("$IMAGE_DATA_URL"); frame_count]),
            );
            add("messages.content.video.num_frames", json!(frame_count));
            add(
                "messages.content.video.fps",
                endpoint_calibration_companion_baseline(
                    contract,
                    "messages.content.video.fps",
                    json!(8.0),
                ),
            );
        }
        "messages.content.video.fps" => {
            let frame_count = endpoint_calibration_video_frame_count(contract);
            add("messages.content.type", json!("video"));
            add(
                "messages.content.video.frames",
                Value::Array(vec![json!("$IMAGE_DATA_URL"); frame_count]),
            );
            add("messages.content.video.num_frames", json!(frame_count));
            add(
                "messages.content.video.fps",
                endpoint_calibration_companion_baseline(
                    contract,
                    "messages.content.video.fps",
                    json!(8.0),
                ),
            );
        }
        "seconds" if contract.family == ENDPOINT_OPENAI_VIDEOS => add(
            "fps",
            endpoint_calibration_companion_baseline(contract, "fps", json!(8.0)),
        ),
        "fps" if contract.family == ENDPOINT_OPENAI_VIDEOS => add(
            "num_frames",
            endpoint_calibration_companion_baseline(contract, "num_frames", json!(9)),
        ),
        "num_frames" if contract.family == ENDPOINT_OPENAI_VIDEOS => add(
            "fps",
            endpoint_calibration_companion_maximum(contract, "fps", json!(8.0)),
        ),
        "parameters.fps" if contract.family == ENDPOINT_HF_TEXT_TO_VIDEO => add(
            "parameters.num_frames",
            endpoint_calibration_companion_minimum(contract, "parameters.num_frames", json!(9)),
        ),
        "parameters.num_frames" if contract.family == ENDPOINT_HF_TEXT_TO_VIDEO => add(
            "parameters.fps",
            endpoint_calibration_companion_maximum(contract, "parameters.fps", json!(8.0)),
        ),
        "width"
            if matches!(
                contract.family.as_str(),
                ENDPOINT_OPENAI_IMAGE_GENERATIONS | ENDPOINT_OPENAI_VIDEOS
            ) =>
        {
            add(
                "height",
                endpoint_calibration_companion_baseline(contract, "height", json!(1024)),
            )
        }
        "height"
            if matches!(
                contract.family.as_str(),
                ENDPOINT_OPENAI_IMAGE_GENERATIONS | ENDPOINT_OPENAI_VIDEOS
            ) =>
        {
            add(
                "width",
                endpoint_calibration_companion_baseline(contract, "width", json!(1024)),
            )
        }
        "parameters.width" if contract.family == ENDPOINT_HF_TEXT_TO_VIDEO => add(
            "parameters.height",
            endpoint_calibration_companion_baseline(contract, "parameters.height", json!(1024)),
        ),
        "parameters.height" if contract.family == ENDPOINT_HF_TEXT_TO_VIDEO => add(
            "parameters.width",
            endpoint_calibration_companion_baseline(contract, "parameters.width", json!(1024)),
        ),
        _ => {}
    }
}

fn endpoint_calibration_string_fixture(
    case: &EndpointCalibrationCase,
    path: &str,
    length: usize,
) -> String {
    if endpoint_inline_audio_child(&case.endpoint_family, path, "content_type")
        && length >= "audio/x".len()
    {
        return format!("audio/{}", "x".repeat(length - "audio/".len()));
    }
    if endpoint_inline_audio_child(&case.endpoint_family, path, "data")
        && case.expect_accept
        && length >= MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize
        && length % 4 == 0
    {
        if let Some(audio) = endpoint_calibration_wav_base64_fixture(length) {
            return audio;
        }
    }
    if case.endpoint_family != ENDPOINT_MAYHEM_MUSIC_GENERATIONS {
        return "x".repeat(length);
    }
    if matches!(path, "audio_codes" | "audio_code_string") {
        const TOKEN: &str = "<|audio_code_63999|>";
        if length == 0 {
            return String::new();
        }
        if case.expect_accept && length % TOKEN.len() == 0 {
            return TOKEN.repeat(length / TOKEN.len());
        }
    }
    if matches!(path, "key" | "keyscale" | "key_scale") && case.expect_accept {
        return match length {
            0 => String::new(),
            8 => "C# minor".to_owned(),
            _ => "C major".chars().take(length).collect(),
        };
    }
    "x".repeat(length)
}

fn endpoint_calibration_wav_base64_fixture(encoded_length: usize) -> Option<String> {
    let is_tts_max = encoded_length == MAX_TTS_REFERENCE_AUDIO_BASE64_CHARS as usize;
    let decoded_length = if encoded_length == MAX_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize {
        MAX_MUSIC_INLINE_AUDIO_BYTES
    } else if is_tts_max {
        MAX_TTS_REFERENCE_AUDIO_BYTES
    } else {
        encoded_length.checked_div(4)?.checked_mul(3)?
    };
    if decoded_length < MIN_MUSIC_INLINE_AUDIO_BYTES
        || decoded_length > MAX_MUSIC_INLINE_AUDIO_BYTES
    {
        return None;
    }
    let data_length = decoded_length.checked_sub(44)?;
    if data_length % 4 != 0 {
        return None;
    }
    let (channels, sample_rate, block_align) = if is_tts_max {
        // Keep the byte-limit boundary inside the independent ten-second TTS limit.
        (2_u16, 480_000_u32, 4_u16)
    } else if data_length <= 600_usize.checked_mul(8_000)?.checked_mul(2)? {
        (1_u16, 8_000_u32, 2_u16)
    } else {
        (2_u16, 96_000_u32, 4_u16)
    };
    let riff_length = u32::try_from(decoded_length.checked_sub(8)?).ok()?;
    let data_length_u32 = u32::try_from(data_length).ok()?;
    let mut wav = Vec::with_capacity(decoded_length);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_length.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_length_u32.to_le_bytes());
    while wav.len() < decoded_length {
        wav.extend_from_slice(&[0x00, 0x10, 0x00, 0xf0]);
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(wav);
    (encoded.len() == encoded_length).then_some(encoded)
}

fn music_inline_audio_root(path: &str) -> Option<&str> {
    MUSIC_INLINE_AUDIO_ROOTS.iter().copied().find(|root| {
        path == *root
            || path
                .strip_prefix(*root)
                .is_some_and(|suffix| suffix.starts_with('.'))
    })
}

fn endpoint_inline_audio_root(family: &str, path: &str) -> Option<&'static str> {
    match family {
        ENDPOINT_MAYHEM_MUSIC_GENERATIONS => {
            MUSIC_INLINE_AUDIO_ROOTS.iter().copied().find(|root| {
                path == *root
                    || path
                        .strip_prefix(*root)
                        .is_some_and(|suffix| suffix.starts_with('.'))
            })
        }
        ENDPOINT_OPENAI_AUDIO_SPEECH
            if path == "reference_audio" || path.starts_with("reference_audio.") =>
        {
            Some("reference_audio")
        }
        ENDPOINT_HF_TEXT_TO_SPEECH
            if path == "parameters.reference_audio"
                || path.starts_with("parameters.reference_audio.") =>
        {
            Some("parameters.reference_audio")
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn push_calibration_case(
    cases: &mut Vec<EndpointCalibrationCase>,
    contract: &EndpointFamilyContract,
    contract_fingerprint: &str,
    base_request: &Value,
    case_kind: &str,
    mutations: Vec<EndpointCalibrationMutation>,
    expect_accept: bool,
    expected_response_attributes: Vec<String>,
) {
    let mut attributes = mutations
        .iter()
        .map(|mutation| mutation.path.clone())
        .chain(expected_response_attributes.iter().cloned())
        .collect::<Vec<_>>();
    attributes.sort();
    attributes.dedup();
    let identity = json!({
        "family": contract.family,
        "kind": case_kind,
        "attributes": attributes,
        "mutations": mutations,
        "responses": expected_response_attributes,
        "accept": expect_accept,
        "contract": contract_fingerprint,
    });
    let digest = blake3::hash(
        &serde_json::to_vec(&identity).expect("calibration case identity is serializable"),
    )
    .to_hex()
    .to_string();
    cases.push(EndpointCalibrationCase {
        case_id: format!("{}-{case_kind}-{}", contract.family, &digest[..16]),
        endpoint_family: contract.family.clone(),
        case_kind: case_kind.to_owned(),
        attributes,
        expect_accept,
        base_request: base_request.clone(),
        mutations,
        expected_response_attributes,
        contract_fingerprint: contract_fingerprint.to_owned(),
    });
}

fn add_boundary_calibration_cases(
    cases: &mut Vec<EndpointCalibrationCase>,
    contract: &EndpointFamilyContract,
    contract_fingerprint: &str,
    base_request: &Value,
    path: &str,
    spec: &EndpointAttributeSpec,
) {
    for (kind, value, accept) in numeric_boundary_values(spec) {
        push_calibration_case(
            cases,
            contract,
            contract_fingerprint,
            base_request,
            kind,
            calibration_mutations_for_literal(contract, path, value),
            accept,
            Vec::new(),
        );
    }
    for (kind, length, accept) in length_boundary_values(spec.min_length, spec.max_length) {
        let mut mutations = vec![EndpointCalibrationMutation {
            path: path.to_owned(),
            value: EndpointCalibrationValue::StringLength { length },
        }];
        add_calibration_companion_mutations(contract, path, None, &mut mutations);
        if contract.family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS
            && music_alias_canonical(contract, path).unwrap_or(path) == "audio_codes"
            && length > 0
            && !mutations
                .iter()
                .any(|mutation| mutation.path == "task_type")
        {
            mutations.push(literal_mutation("task_type", json!("cover")));
        }
        push_calibration_case(
            cases,
            contract,
            contract_fingerprint,
            base_request,
            kind,
            mutations,
            accept,
            Vec::new(),
        );
    }
    let array_item = spec
        .calibration_values
        .iter()
        .chain(spec.default.iter())
        .find_map(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
        .or_else(|| spec.enum_values.first().cloned())
        .unwrap_or_else(|| json!("item"));
    for (kind, length, accept) in length_boundary_values(spec.min_items, spec.max_items) {
        let mut mutations = vec![EndpointCalibrationMutation {
            path: path.to_owned(),
            value: EndpointCalibrationValue::ArrayLength {
                length,
                item: array_item.clone(),
            },
        }];
        let companion_value = usize::try_from(length)
            .ok()
            .map(|length| Value::Array(vec![array_item.clone(); length]));
        add_calibration_companion_mutations(
            contract,
            path,
            companion_value.as_ref(),
            &mut mutations,
        );
        push_calibration_case(
            cases,
            contract,
            contract_fingerprint,
            base_request,
            kind,
            mutations,
            accept,
            Vec::new(),
        );
    }
}

fn numeric_boundary_values(spec: &EndpointAttributeSpec) -> Vec<(&'static str, Value, bool)> {
    let integer = spec.value_types.contains(&EndpointValueType::Integer)
        && !spec.value_types.contains(&EndpointValueType::Number);
    let mut values = Vec::new();
    if let Some(minimum) = spec.minimum {
        values.push(("minimum_valid", number_value(minimum, integer), true));
        values.push((
            "below_minimum",
            number_value(
                if integer {
                    minimum - 1.0
                } else {
                    minimum - 0.001
                },
                integer,
            ),
            false,
        ));
    }
    if let Some(maximum) = spec.maximum {
        values.push(("maximum_valid", number_value(maximum, integer), true));
        values.push((
            "above_maximum",
            number_value(
                if integer {
                    maximum + 1.0
                } else {
                    maximum + 0.001
                },
                integer,
            ),
            false,
        ));
    }
    if let Some(multiple) = spec
        .multiple_of
        .filter(|value| value.is_finite() && *value > 0.0)
    {
        let minimum = spec.minimum.unwrap_or(0.0);
        let maximum = spec.maximum.unwrap_or(f64::MAX);
        let valid = (minimum / multiple).ceil() * multiple;
        if valid.is_finite() && valid <= maximum {
            values.push(("multiple_of_valid", number_value(valid, integer), true));
            let delta = if integer { 1.0 } else { multiple / 2.0 };
            if delta > 0.0 && (delta / multiple).fract() != 0.0 {
                let above = valid + delta;
                let below = valid - delta;
                let invalid = if above <= maximum {
                    Some(above)
                } else if below >= minimum {
                    Some(below)
                } else {
                    None
                };
                if let Some(invalid) = invalid {
                    values.push(("not_multiple_of", number_value(invalid, integer), false));
                }
            }
        }
    }
    values
}

fn number_value(value: f64, integer: bool) -> Value {
    if integer && value >= 0.0 && value <= u64::MAX as f64 {
        json!(value as u64)
    } else if integer && value >= i64::MIN as f64 && value <= i64::MAX as f64 {
        json!(value as i64)
    } else {
        json!(value)
    }
}

fn length_boundary_values(
    minimum: Option<u64>,
    maximum: Option<u64>,
) -> Vec<(&'static str, u64, bool)> {
    let mut values = Vec::new();
    if let Some(minimum) = minimum {
        values.push(("minimum_length_valid", minimum, true));
        if minimum > 0 {
            values.push(("below_minimum_length", minimum - 1, false));
        }
    }
    if let Some(maximum) = maximum {
        values.push(("maximum_length_valid", maximum, true));
        if let Some(above) = maximum.checked_add(1) {
            values.push(("above_maximum_length", above, false));
        }
    }
    values
}

fn wrong_type_value(spec: &EndpointAttributeSpec) -> Value {
    for candidate in [
        Value::Bool(true),
        json!(1),
        json!(0.5),
        json!("wrong-type"),
        json!([]),
        json!({}),
        Value::Null,
    ] {
        if !endpoint_attribute_value_matches(spec, &candidate) {
            return candidate;
        }
    }
    Value::Null
}

fn deduplicate_values(values: &mut Vec<Value>) {
    let mut seen = std::collections::BTreeSet::new();
    values.retain(|value| {
        seen.insert(serde_json::to_string(value).expect("JSON value is serializable"))
    });
}

fn set_endpoint_path(target: &mut Value, path: &str, value: Value) -> Result<(), String> {
    if let Some(rest) = path.strip_prefix("messages.content.") {
        let object = ensure_first_message_content_part(target)?;
        return set_object_path(
            object,
            rest.split('.').collect::<Vec<_>>().as_slice(),
            value,
        );
    }
    if let Some(rest) = path.strip_prefix("messages.") {
        let message = ensure_first_array_object(target, "messages")?;
        return set_object_path(
            message,
            rest.split('.').collect::<Vec<_>>().as_slice(),
            value,
        );
    }
    let segments = path.split('.').collect::<Vec<_>>();
    let object = target
        .as_object_mut()
        .ok_or_else(|| "calibration request root must be an object".to_owned())?;
    set_object_path(object, &segments, value)
}

fn set_object_path(
    object: &mut serde_json::Map<String, Value>,
    segments: &[&str],
    value: Value,
) -> Result<(), String> {
    let Some((head, tail)) = segments.split_first() else {
        return Err("endpoint attribute path is empty".to_owned());
    };
    if tail.is_empty() {
        object.insert((*head).to_owned(), value);
        return Ok(());
    }
    let child = object
        .entry((*head).to_owned())
        .or_insert_with(|| json!({}));
    if !child.is_object() {
        *child = json!({});
    }
    set_object_path(
        child
            .as_object_mut()
            .expect("child was replaced with an object"),
        tail,
        value,
    )
}

fn ensure_first_array_object<'a>(
    target: &'a mut Value,
    key: &str,
) -> Result<&'a mut serde_json::Map<String, Value>, String> {
    let root = target
        .as_object_mut()
        .ok_or_else(|| "calibration request root must be an object".to_owned())?;
    let array = root.entry(key.to_owned()).or_insert_with(|| json!([{}]));
    if !array.is_array() {
        *array = json!([{}]);
    }
    let items = array
        .as_array_mut()
        .expect("value was replaced with an array");
    if items.is_empty() {
        items.push(json!({}));
    }
    if !items[0].is_object() {
        items[0] = json!({});
    }
    Ok(items[0]
        .as_object_mut()
        .expect("first array item was replaced with an object"))
}

fn ensure_first_message_content_part(
    target: &mut Value,
) -> Result<&mut serde_json::Map<String, Value>, String> {
    let message = ensure_first_array_object(target, "messages")?;
    let content = message
        .entry("content".to_owned())
        .or_insert_with(|| json!([{}]));
    if !content.is_array() {
        *content = json!([{}]);
    }
    let parts = content
        .as_array_mut()
        .expect("content was replaced with an array");
    if parts.is_empty() {
        parts.push(json!({}));
    }
    if !parts[0].is_object() {
        parts[0] = json!({});
    }
    Ok(parts[0]
        .as_object_mut()
        .expect("content part was replaced with an object"))
}

fn remove_endpoint_path(target: &mut Value, path: &str) -> Result<(), String> {
    if !path.contains('.') {
        target
            .as_object_mut()
            .ok_or_else(|| "calibration request root must be an object".to_owned())?
            .remove(path);
        return Ok(());
    }
    let segments = path.split('.').collect::<Vec<_>>();
    remove_nested_endpoint_path(target, &segments);
    Ok(())
}

fn remove_nested_endpoint_path(value: &mut Value, segments: &[&str]) {
    if segments.is_empty() {
        return;
    }
    match value {
        Value::Array(items) => {
            for item in items {
                remove_nested_endpoint_path(item, segments);
            }
        }
        Value::Object(object) => {
            if segments.len() == 1 {
                object.remove(segments[0]);
            } else if let Some(child) = object.get_mut(segments[0]) {
                remove_nested_endpoint_path(child, &segments[1..]);
            }
        }
        _ => {}
    }
}

fn substitute_calibration_markers(value: Value, substitutions: &BTreeMap<String, Value>) -> Value {
    match value {
        Value::String(marker) => substitutions
            .get(&marker)
            .cloned()
            .unwrap_or(Value::String(marker)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| substitute_calibration_markers(item, substitutions))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, substitute_calibration_markers(value, substitutions)))
                .collect(),
        ),
        value => value,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EndpointContractViolation {
    pub path: String,
    pub reason: String,
}

#[must_use]
pub fn endpoint_contract_fingerprint(contract: &EndpointFamilyContract) -> String {
    let encoded = serde_json::to_vec(contract).expect("endpoint contracts are JSON serializable");
    blake3::hash(&encoded).to_hex().to_string()
}

/// Hashes the semantic JSON value while remaining stable across JavaScript relays.
/// JavaScript serializes integral JSON numbers such as `1.0` as `1`; those values
/// must compare equally without making changed integers or non-integral values equal.
#[must_use]
pub fn endpoint_request_fingerprint(request: &Value) -> String {
    let canonical = javascript_roundtrip_stable_value(request);
    blake3::hash(canonical.to_string().as_bytes())
        .to_hex()
        .to_string()
}

fn javascript_roundtrip_stable_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(javascript_roundtrip_stable_value)
                .collect(),
        ),
        Value::Object(object) => {
            let mut stable = serde_json::Map::new();
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (key, value) in entries {
                stable.insert(key.clone(), javascript_roundtrip_stable_value(value));
            }
            Value::Object(stable)
        }
        Value::Number(number) => {
            if number.is_i64() || number.is_u64() {
                return Value::Number(number.clone());
            }
            let Some(float) = number.as_f64() else {
                return Value::Number(number.clone());
            };
            if float == 0.0 {
                Value::Number(0.into())
            } else if float.fract() == 0.0 && float.abs() <= 9_007_199_254_740_991.0 {
                Value::Number((float as i64).into())
            } else {
                Value::Number(number.clone())
            }
        }
        value => value.clone(),
    }
}

pub fn validate_endpoint_request(
    contract: &EndpointFamilyContract,
    request: &Value,
) -> Result<(), Vec<EndpointContractViolation>> {
    let raw_request = request;
    let normalized;
    let request = if contract.family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS {
        match normalize_music_request_aliases(contract, request) {
            Ok(value) => {
                normalized = value;
                &normalized
            }
            Err(violation) => return Err(vec![violation]),
        }
    } else {
        request
    };
    let mut violations = validate_endpoint_value(
        &contract.request_attributes,
        &contract.required_request_attributes,
        &contract.request_attribute_specs,
        request,
        "request",
    )
    .err()
    .unwrap_or_default();
    if matches!(
        contract.family.as_str(),
        ENDPOINT_OPENAI_IMAGE_GENERATIONS | ENDPOINT_OPENAI_VIDEOS
    ) {
        validate_openai_dimension_aliases(contract, request, &mut violations);
    }
    if contract.family == ENDPOINT_OPENAI_VIDEOS {
        validate_openai_video_duration_aliases(request, &mut violations);
    }
    if contract.family == ENDPOINT_MAYHEM_MUSIC_GENERATIONS {
        validate_music_alias_attribute_specs(contract, raw_request, &mut violations);
        validate_music_request(contract, request, &mut violations);
    }
    if matches!(
        contract.family.as_str(),
        ENDPOINT_OPENAI_AUDIO_SPEECH | ENDPOINT_HF_TEXT_TO_SPEECH
    ) {
        validate_tts_reference_audio(contract, request, &mut violations);
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

fn validate_tts_reference_audio(
    contract: &EndpointFamilyContract,
    request: &Value,
    violations: &mut Vec<EndpointContractViolation>,
) {
    let (path, audio) = if contract.family == ENDPOINT_HF_TEXT_TO_SPEECH {
        (
            "parameters.reference_audio",
            request.pointer("/parameters/reference_audio"),
        )
    } else {
        ("reference_audio", request.get("reference_audio"))
    };
    let Some(audio) = audio else {
        return;
    };
    validate_music_inline_audio(contract, path, audio, violations);
    if audio
        .get("encoding")
        .and_then(Value::as_str)
        .is_some_and(|encoding| encoding == "base64")
    {
        if let Err(reason) = validated_tts_reference_audio_item(path, audio) {
            violations.push(EndpointContractViolation {
                path: path.to_owned(),
                reason,
            });
        }
    }
}

fn validate_music_alias_attribute_specs(
    contract: &EndpointFamilyContract,
    raw_request: &Value,
    violations: &mut Vec<EndpointContractViolation>,
) {
    for (_, aliases) in MUSIC_REQUEST_ALIAS_GROUPS {
        for alias in *aliases {
            if raw_request.get(*alias).is_none() {
                continue;
            }
            for (path, spec) in contract.request_attribute_specs.iter().filter(|(path, _)| {
                path.as_str() == *alias || path.starts_with(&format!("{alias}."))
            }) {
                for candidate in endpoint_values_at_path(raw_request, path) {
                    if let Err(reason) = validate_endpoint_attribute_value(spec, candidate) {
                        violations.push(EndpointContractViolation {
                            path: path.clone(),
                            reason,
                        });
                    }
                }
            }
        }
    }
}

fn validate_music_request(
    contract: &EndpointFamilyContract,
    request: &Value,
    violations: &mut Vec<EndpointContractViolation>,
) {
    let task = request
        .get("task_type")
        .and_then(Value::as_str)
        .unwrap_or("text2music");
    let has_prompt = request
        .get("prompt")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty());
    let has_lyrics = request
        .get("lyrics")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty());
    let has_source_audio = request.get("source_audio").is_some();
    let instrumental = request
        .get("instrumental")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if instrumental
        && request
            .get("lyrics")
            .and_then(Value::as_str)
            .is_some_and(|lyrics| {
                let lyrics = lyrics.trim();
                !lyrics.is_empty() && lyrics != "[Instrumental]"
            })
    {
        violations.push(EndpointContractViolation {
            path: "lyrics".to_owned(),
            reason: "instrumental=true cannot be combined with vocal lyrics".to_owned(),
        });
    }
    if task == "text2music" && !has_prompt && !has_lyrics {
        violations.push(EndpointContractViolation {
            path: "prompt".to_owned(),
            reason: "text2music requires a nonempty prompt/caption or nonempty lyrics".to_owned(),
        });
    }

    let sample_mode = request
        .get("sample_mode")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let has_sample_query = request
        .get("sample_query")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty());
    let use_format = request
        .get("use_format")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let sample_controls_enabled = sample_mode || has_sample_query || use_format;
    if sample_controls_enabled && task != "text2music" {
        violations.push(EndpointContractViolation {
            path: "sample_mode".to_owned(),
            reason: "sample_mode, sample_query, and use_format are supported only for text2music"
                .to_owned(),
        });
    }
    if use_format && !has_prompt && !has_lyrics {
        violations.push(EndpointContractViolation {
            path: "use_format".to_owned(),
            reason: "use_format requires a nonempty prompt/caption or nonempty lyrics".to_owned(),
        });
    }
    let flow_enabled = request
        .get("flow_edit_morph")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let has_audio_codes = music_audio_codes_nonempty(request.get("audio_codes"));
    if sample_controls_enabled && has_audio_codes {
        violations.push(EndpointContractViolation {
            path: "audio_codes".to_owned(),
            reason: "audio_codes cannot be combined with sample_mode, sample_query, or use_format"
                .to_owned(),
        });
    }
    if has_audio_codes && !matches!(task, "cover" | "cover-nofsq") {
        violations.push(EndpointContractViolation {
            path: "task_type".to_owned(),
            reason: "audio_codes require an explicit cover or cover-nofsq task".to_owned(),
        });
    }
    if has_audio_codes && has_source_audio {
        violations.push(EndpointContractViolation {
            path: "audio_codes".to_owned(),
            reason: "audio_codes and source_audio cannot be combined because the runtime would ignore source_audio"
                .to_owned(),
        });
    }
    if has_audio_codes && flow_enabled {
        violations.push(EndpointContractViolation {
            path: "audio_codes".to_owned(),
            reason: "audio_codes cannot be combined with flow editing".to_owned(),
        });
    }
    let source_driven_task = matches!(task, "cover" | "cover-nofsq" | "repaint");
    if source_driven_task
        && !has_source_audio
        && !(matches!(task, "cover" | "cover-nofsq") && has_audio_codes)
    {
        violations.push(EndpointContractViolation {
            path: "source_audio".to_owned(),
            reason: format!("{task} requires inline source_audio or validated audio_codes"),
        });
    }
    if flow_enabled && !has_source_audio {
        violations.push(EndpointContractViolation {
            path: "source_audio".to_owned(),
            reason: "flow editing requires inline source_audio".to_owned(),
        });
    }
    if task == "text2music" && has_source_audio && !flow_enabled {
        violations.push(EndpointContractViolation {
            path: "source_audio".to_owned(),
            reason: "plain text2music does not consume source_audio; enable flow editing or choose a source-driven task"
                .to_owned(),
        });
    }
    if flow_enabled && !matches!(task, "text2music" | "cover" | "cover-nofsq") {
        violations.push(EndpointContractViolation {
            path: "flow_edit_morph".to_owned(),
            reason: format!("flow editing is not supported for task_type {task}"),
        });
    }
    if !flow_enabled {
        for path in [
            "flow_edit_source_caption",
            "flow_edit_source_lyrics",
            "flow_edit_n_min",
            "flow_edit_n_max",
            "flow_edit_n_avg",
        ] {
            if request.get(path).is_some() {
                violations.push(EndpointContractViolation {
                    path: path.to_owned(),
                    reason: "flow-edit controls require flow_edit_morph=true".to_owned(),
                });
            }
        }
    } else {
        let n_min = request
            .get("flow_edit_n_min")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let n_max = request
            .get("flow_edit_n_max")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        if n_min > n_max {
            violations.push(EndpointContractViolation {
                path: "flow_edit_n_max".to_owned(),
                reason: "flow_edit_n_max must be greater than or equal to flow_edit_n_min"
                    .to_owned(),
            });
        }
        if request.get("sampler").and_then(Value::as_str) == Some("heun") {
            violations.push(EndpointContractViolation {
                path: "sampler".to_owned(),
                reason: "flow editing v1 bypasses heun; sampler must be euler".to_owned(),
            });
        }
        if request.get("adg").and_then(Value::as_bool).unwrap_or(false) {
            violations.push(EndpointContractViolation {
                path: "adg".to_owned(),
                reason: "flow editing v1 bypasses ADG".to_owned(),
            });
        }
        if request
            .get("dcw_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            violations.push(EndpointContractViolation {
                path: "dcw_enabled".to_owned(),
                reason: "flow editing v1 bypasses DCW".to_owned(),
            });
        }
        if request.get("infer_method").and_then(Value::as_str) == Some("sde") {
            violations.push(EndpointContractViolation {
                path: "infer_method".to_owned(),
                reason: "flow editing v1 does not honor infer_method=sde".to_owned(),
            });
        }
    }

    if request
        .get("thinking")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && source_driven_task
    {
        violations.push(EndpointContractViolation {
            path: "thinking".to_owned(),
            reason: "thinking is not consumed by cover or repaint tasks".to_owned(),
        });
    }
    let lm_path_allowed = !source_driven_task && !flow_enabled;
    if !lm_path_allowed {
        for path in [
            "lm_temperature",
            "lm_cfg_scale",
            "lm_top_k",
            "lm_top_p",
            "lm_negative_prompt",
        ] {
            if request.get(path).is_some() {
                violations.push(EndpointContractViolation {
                    path: path.to_owned(),
                    reason: "LM controls are not consumed by source or flow-edit tasks".to_owned(),
                });
            }
        }
        for path in [
            "use_cot_metas",
            "use_cot_caption",
            "use_cot_language",
            "constrained_decoding",
        ] {
            if request.get(path).and_then(Value::as_bool) == Some(true) {
                violations.push(EndpointContractViolation {
                    path: path.to_owned(),
                    reason: "LM and CoT controls are not consumed by source or flow-edit tasks"
                        .to_owned(),
                });
            }
        }
    }
    if request
        .get("language")
        .and_then(Value::as_str)
        .is_some_and(|language| language != "unknown")
        && request.get("use_cot_language").and_then(Value::as_bool) == Some(true)
    {
        violations.push(EndpointContractViolation {
            path: "use_cot_language".to_owned(),
            reason: "use_cot_language=true would replace the explicit language".to_owned(),
        });
    }

    if task != "repaint" {
        for path in [
            "repaint_start",
            "repaint_end",
            "repaint_mode",
            "repaint_strength",
            "chunk_mask_mode",
        ] {
            if request.get(path).is_some() {
                violations.push(EndpointContractViolation {
                    path: path.to_owned(),
                    reason: format!("repaint control is not supported for task_type {task}"),
                });
            }
        }
    } else {
        let start = request
            .get("repaint_start")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let end = request
            .get("repaint_end")
            .and_then(Value::as_f64)
            .unwrap_or(-1.0);
        if end >= 0.0 && end <= start {
            violations.push(EndpointContractViolation {
                path: "repaint_end".to_owned(),
                reason: "repaint_end must be -1 or greater than repaint_start".to_owned(),
            });
        }
        if request.get("repaint_strength").is_some()
            && request
                .get("repaint_mode")
                .and_then(Value::as_str)
                .unwrap_or("balanced")
                != "balanced"
        {
            violations.push(EndpointContractViolation {
                path: "repaint_strength".to_owned(),
                reason: "repaint_strength is active only when repaint_mode=balanced".to_owned(),
            });
        }
    }

    if !matches!(task, "cover" | "cover-nofsq" | "repaint") {
        for path in ["cover_strength", "cover_noise_strength"] {
            if request.get(path).is_some() {
                violations.push(EndpointContractViolation {
                    path: path.to_owned(),
                    reason: format!("cover control is not supported for task_type {task}"),
                });
            }
        }
    }

    let cfg_interval_start = request
        .get("cfg_interval_start")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let cfg_interval_end = request
        .get("cfg_interval_end")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    if cfg_interval_start > cfg_interval_end {
        violations.push(EndpointContractViolation {
            path: "cfg_interval_end".to_owned(),
            reason: "cfg_interval_end must be greater than or equal to cfg_interval_start"
                .to_owned(),
        });
    }
    if request.get("infer_method").and_then(Value::as_str) == Some("sde")
        && request.get("sampler").and_then(Value::as_str) == Some("heun")
    {
        violations.push(EndpointContractViolation {
            path: "sampler".to_owned(),
            reason: "sampler=heun is not honored with infer_method=sde".to_owned(),
        });
    }

    let dcw_enabled = request
        .get("dcw_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !dcw_enabled {
        for path in ["dcw_mode", "dcw_scaler", "dcw_high_scaler", "dcw_wavelet"] {
            if request.get(path).is_some() {
                violations.push(EndpointContractViolation {
                    path: path.to_owned(),
                    reason: "DCW controls require dcw_enabled=true".to_owned(),
                });
            }
        }
    } else if request.get("dcw_high_scaler").is_some()
        && request
            .get("dcw_mode")
            .and_then(Value::as_str)
            .unwrap_or("double")
            != "double"
    {
        violations.push(EndpointContractViolation {
            path: "dcw_high_scaler".to_owned(),
            reason: "dcw_high_scaler is active only when dcw_mode=double".to_owned(),
        });
    }

    if !request
        .get("enable_normalization")
        .and_then(Value::as_bool)
        .unwrap_or(true)
        && request.get("normalization_db").is_some()
    {
        violations.push(EndpointContractViolation {
            path: "normalization_db".to_owned(),
            reason: "normalization_db requires enable_normalization=true".to_owned(),
        });
    }

    let fade_in = request
        .get("fade_in_duration")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let fade_out = request
        .get("fade_out_duration")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let duration_value = request.get("duration_seconds");
    if let Some(value) = duration_value {
        let valid = value.is_null()
            || value.as_str() == Some("auto")
            || value
                .as_f64()
                .is_some_and(|duration| duration == -1.0 || (10.0..=600.0).contains(&duration));
        if !valid {
            violations.push(EndpointContractViolation {
                path: "duration_seconds".to_owned(),
                reason: "duration must be null, auto, -1, or between 10 and 600 seconds".to_owned(),
            });
        }
    }
    let duration = duration_value
        .and_then(Value::as_f64)
        .or_else(|| {
            contract
                .request_attribute_specs
                .get("duration_seconds")
                .and_then(|spec| spec.default.as_ref())
                .and_then(Value::as_f64)
        })
        .unwrap_or(-1.0);
    if duration > 0.0 && fade_in + fade_out > duration {
        violations.push(EndpointContractViolation {
            path: "fade_out_duration".to_owned(),
            reason: "fade-in and fade-out durations must not exceed duration_seconds".to_owned(),
        });
    }

    if request.get("retake_seed").is_some()
        && request
            .get("retake_variance")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            == 0.0
    {
        violations.push(EndpointContractViolation {
            path: "retake_variance".to_owned(),
            reason: "retake_seed requires retake_variance greater than zero".to_owned(),
        });
    }

    let response_format = request
        .get("response_format")
        .and_then(Value::as_str)
        .unwrap_or("flac");
    if response_format != "mp3" {
        for path in ["mp3_bitrate", "mp3_sample_rate"] {
            if request.get(path).is_some() {
                violations.push(EndpointContractViolation {
                    path: path.to_owned(),
                    reason: "MP3 controls require response_format=mp3".to_owned(),
                });
            }
        }
    }

    let composed_caption_chars = music_composed_caption_chars(contract, request);
    if composed_caption_chars > MAX_MUSIC_CAPTION_CHARS {
        violations.push(EndpointContractViolation {
            path: "prompt".to_owned(),
            reason: format!(
                "composed prompt, style, genre, and tags length {composed_caption_chars} exceeds the signed {MAX_MUSIC_CAPTION_CHARS}-character budget"
            ),
        });
    }
    if let Some(key) = request.get("key").and_then(Value::as_str) {
        if !valid_music_keyscale(key) {
            violations.push(EndpointContractViolation {
                path: "key".to_owned(),
                reason: "key must be empty or a supported note followed by major or minor"
                    .to_owned(),
            });
        }
    }

    if contract
        .request_attributes
        .iter()
        .any(|signed| signed == "audio_codes")
    {
        if let Some(audio_codes) = request.get("audio_codes") {
            if let Err(reason) = validate_music_audio_codes_value(contract, request, audio_codes) {
                violations.push(EndpointContractViolation {
                    path: "audio_codes".to_owned(),
                    reason,
                });
            }
        }
    }

    for path in ["custom_timesteps"] {
        if !contract
            .request_attributes
            .iter()
            .any(|signed| signed == path)
        {
            continue;
        }
        let Some(items) = request.get(path).and_then(Value::as_array) else {
            continue;
        };
        if request.get("steps").is_some() || request.get("shift").is_some() {
            violations.push(EndpointContractViolation {
                path: path.to_owned(),
                reason: "custom_timesteps overrides steps and shift; do not supply them together"
                    .to_owned(),
            });
        }
        let mut previous = None;
        for (index, item) in items.iter().enumerate() {
            let Some(value) = item.as_f64() else {
                violations.push(EndpointContractViolation {
                    path: format!("{path}[{index}]"),
                    reason: "custom timestep must be a number".to_owned(),
                });
                continue;
            };
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                violations.push(EndpointContractViolation {
                    path: format!("{path}[{index}]"),
                    reason: "custom timestep must be in the closed range 0..=1".to_owned(),
                });
            }
            if previous.is_some_and(|previous| previous <= value) {
                violations.push(EndpointContractViolation {
                    path: format!("{path}[{index}]"),
                    reason: "custom timesteps must be in strict descending order".to_owned(),
                });
            }
            previous = Some(value);
        }
    }

    if contract
        .request_attributes
        .iter()
        .any(|signed| signed == "seeds")
    {
        if let Some(seeds) = request.get("seeds").and_then(Value::as_array) {
            if request.get("seed").is_some() {
                violations.push(EndpointContractViolation {
                    path: "seeds".to_owned(),
                    reason: "seed and seeds cannot both be supplied".to_owned(),
                });
            }
            if let Some(batch_size) = request.get("n").and_then(Value::as_u64).or_else(|| {
                contract
                    .request_attribute_specs
                    .get("n")
                    .and_then(|spec| spec.default.as_ref())
                    .and_then(Value::as_u64)
            }) {
                let seed_count = u64::try_from(seeds.len()).unwrap_or(u64::MAX);
                if seed_count != batch_size {
                    violations.push(EndpointContractViolation {
                        path: "seeds".to_owned(),
                        reason: format!(
                            "seeds must contain exactly one value per batch item ({batch_size})"
                        ),
                    });
                }
            }
            for (index, seed) in seeds.iter().enumerate() {
                if value_type(seed) != Some(EndpointValueType::Integer)
                    || seed
                        .as_i64()
                        .is_none_or(|value| !(-1..=i64::from(u32::MAX)).contains(&value))
                {
                    violations.push(EndpointContractViolation {
                        path: format!("seeds[{index}]"),
                        reason: "seed must be -1 or an unsigned 32-bit integer".to_owned(),
                    });
                }
            }
        }
    }

    for root in ["source_audio", "reference_audio"] {
        if !contract
            .request_attributes
            .iter()
            .any(|signed| signed == root)
        {
            continue;
        }
        let Some(audio) = request.get(root) else {
            continue;
        };
        validate_music_inline_audio(contract, root, audio, violations);
    }
}

fn music_composed_caption_chars(contract: &EndpointFamilyContract, request: &Value) -> u64 {
    let signed = |path: &str| {
        contract
            .request_attributes
            .iter()
            .any(|candidate| candidate == path)
    };
    let mut length = request
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|_| signed("prompt"))
        .map(str::trim)
        .map(|text| u64::try_from(text.chars().count()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    for (path, label) in [
        ("style", "Style: "),
        ("genre", "Genre: "),
        ("tags", "Tags: "),
    ] {
        let Some(text) = request
            .get(path)
            .and_then(Value::as_str)
            .filter(|_| signed(path))
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        if length > 0 {
            length = length.saturating_add(1);
        }
        length = length
            .saturating_add(u64::try_from(label.chars().count()).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(text.chars().count()).unwrap_or(u64::MAX));
    }
    length
}

fn valid_music_keyscale(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    let Some((note, mode)) = value.split_once(' ') else {
        return false;
    };
    let mut chars = note.chars();
    matches!(chars.next(), Some('A'..='G'))
        && matches!(chars.as_str(), "" | "#" | "b" | "♯" | "♭")
        && matches!(mode, "major" | "minor")
}

fn validate_music_inline_audio(
    contract: &EndpointFamilyContract,
    root: &str,
    audio: &Value,
    violations: &mut Vec<EndpointContractViolation>,
) {
    let Some(audio) = audio.as_object() else {
        return;
    };
    for child in ["data", "encoding", "content_type"] {
        if !audio.contains_key(child) {
            violations.push(EndpointContractViolation {
                path: format!("{root}.{child}"),
                reason: "inline audio attribute is required when the audio object is supplied"
                    .to_owned(),
            });
        }
    }
    if audio.get("encoding").and_then(Value::as_str) != Some("base64") {
        return;
    }
    if let Some(content_type) = audio.get("content_type").and_then(Value::as_str) {
        if !valid_audio_content_type(content_type) {
            violations.push(EndpointContractViolation {
                path: format!("{root}.content_type"),
                reason: "content type is not in the signed inline-audio MIME allowlist".to_owned(),
            });
        }
    }
    let Some(data) = audio.get("data").and_then(Value::as_str) else {
        return;
    };
    let signed_encoded_limit = contract
        .request_attribute_specs
        .get(&format!("{root}.data"))
        .and_then(|spec| spec.max_length)
        .unwrap_or(MAX_MUSIC_INLINE_AUDIO_BASE64_CHARS);
    let decoded_limit =
        maximum_base64_decoded_size(signed_encoded_limit).min(MAX_MUSIC_INLINE_AUDIO_BYTES as u64);
    if let Err(reason) = validate_base64_decoded_size(data, decoded_limit) {
        violations.push(EndpointContractViolation {
            path: format!("{root}.data"),
            reason,
        });
    }
}

fn valid_audio_content_type(content_type: &str) -> bool {
    MUSIC_INLINE_AUDIO_CONTENT_TYPES.contains(&content_type)
}

fn validated_music_inline_audio_item(root: &str, audio: &Value) -> Result<(u64, u64), String> {
    let audio = audio
        .as_object()
        .ok_or_else(|| format!("{root} must be an inline audio object"))?;
    if audio.get("encoding").and_then(Value::as_str) != Some("base64") {
        return Err(format!("{root}.encoding must be base64"));
    }
    let content_type = audio
        .get("content_type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{root}.content_type must be an audio MIME type"))?;
    let encoded = audio
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{root}.data must contain base64 audio"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| format!("{root}.data contains invalid base64 audio"))?;
    let metadata = validated_audio_metadata(&bytes)
        .ok_or_else(|| format!("{root} must contain valid bounded audio"))?;
    if !music_audio_content_type_matches_format(content_type, metadata.format) {
        return Err(format!(
            "{root}.content_type {content_type} does not match the encoded audio"
        ));
    }
    if metadata.duration_seconds_ceil > MAX_MUSIC_INLINE_AUDIO_SECONDS {
        return Err(format!(
            "{root} exceeds the {MAX_MUSIC_INLINE_AUDIO_SECONDS}-second signed limit"
        ));
    }
    Ok((
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        metadata.duration_seconds_ceil,
    ))
}

fn validated_tts_reference_audio_item(root: &str, audio: &Value) -> Result<(u64, u64), String> {
    let audio = audio
        .as_object()
        .ok_or_else(|| format!("{root} must be an inline audio object"))?;
    if audio.get("encoding").and_then(Value::as_str) != Some("base64") {
        return Err(format!("{root}.encoding must be base64"));
    }
    if audio.get("content_type").and_then(Value::as_str) != Some("audio/wav") {
        return Err(format!("{root}.content_type must be audio/wav"));
    }
    let encoded = audio
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{root}.data must contain base64 audio"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| format!("{root}.data contains invalid base64 audio"))?;
    if bytes.len() > MAX_TTS_REFERENCE_AUDIO_BYTES {
        return Err(format!(
            "{root} exceeds the {MAX_TTS_REFERENCE_AUDIO_BYTES}-byte signed limit"
        ));
    }
    let metadata = validated_audio_metadata(&bytes)
        .ok_or_else(|| format!("{root} must contain valid bounded WAV audio"))?;
    if metadata.format != ValidatedAudioFormat::Wav {
        return Err(format!("{root} must contain WAV audio"));
    }
    if metadata.duration_seconds_ceil > MAX_TTS_REFERENCE_AUDIO_SECONDS {
        return Err(format!(
            "{root} exceeds the {MAX_TTS_REFERENCE_AUDIO_SECONDS}-second signed limit"
        ));
    }
    Ok((
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        metadata.duration_seconds_ceil,
    ))
}

fn music_audio_content_type_matches_format(
    content_type: &str,
    format: ValidatedAudioFormat,
) -> bool {
    match format {
        ValidatedAudioFormat::Wav => matches!(content_type, "audio/wav" | "audio/x-wav"),
        ValidatedAudioFormat::Flac => content_type == "audio/flac",
        ValidatedAudioFormat::Mp3 => matches!(content_type, "audio/mpeg" | "audio/mp3"),
        ValidatedAudioFormat::Opus => matches!(content_type, "audio/ogg" | "audio/opus"),
        ValidatedAudioFormat::Aac => {
            matches!(
                content_type,
                "audio/aac" | "audio/m4a" | "audio/mp4" | "audio/x-m4a"
            )
        }
    }
}

fn validate_music_audio_codes(audio_codes: &str) -> Result<u64, String> {
    if audio_codes.len() > MAX_MUSIC_AUDIO_CODES_CHARS as usize {
        return Err(format!(
            "audio codes exceed the {MAX_MUSIC_AUDIO_CODES_CHARS}-byte encoded maximum"
        ));
    }
    if audio_codes.is_empty() {
        return Ok(0);
    }
    let mut remainder = audio_codes;
    let mut count = 0_u64;
    while !remainder.is_empty() {
        let Some(after_prefix) = remainder.strip_prefix("<|audio_code_") else {
            return Err("audio codes must be concatenated <|audio_code_N|> tokens".to_owned());
        };
        let Some(end) = after_prefix.find("|>") else {
            return Err("audio code token is missing its |>-terminator".to_owned());
        };
        let digits = &after_prefix[..end];
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("audio code token must contain a decimal code value".to_owned());
        }
        let value = digits
            .parse::<u64>()
            .map_err(|_| "audio code value exceeds the parser integer range".to_owned())?;
        if value > MAX_MUSIC_AUDIO_CODE_VALUE {
            return Err(format!(
                "audio code value {value} exceeds the pinned parser maximum {MAX_MUSIC_AUDIO_CODE_VALUE}"
            ));
        }
        count = count.saturating_add(1);
        if count > MAX_MUSIC_AUDIO_CODE_COUNT {
            return Err(format!(
                "audio code count exceeds the 5 Hz, 600-second maximum of {MAX_MUSIC_AUDIO_CODE_COUNT}"
            ));
        }
        remainder = &after_prefix[end + 2..];
    }
    Ok(count)
}

fn music_audio_codes_nonempty(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(codes)) => !codes.is_empty(),
        Some(Value::Array(codes)) => !codes.is_empty(),
        _ => false,
    }
}

fn validate_music_audio_codes_value(
    contract: &EndpointFamilyContract,
    request: &Value,
    audio_codes: &Value,
) -> Result<(), String> {
    match audio_codes {
        Value::String(codes) => {
            validate_music_audio_codes(codes)?;
        }
        Value::Array(codes) => {
            if codes.is_empty() || codes.len() > 8 {
                return Err("audio codes arrays must contain between 1 and 8 strings".to_owned());
            }
            let mut token_count = None;
            for (index, codes) in codes.iter().enumerate() {
                let Some(codes) = codes.as_str() else {
                    return Err(format!("audio codes item {index} must be a string"));
                };
                if codes.is_empty() {
                    return Err(format!("audio codes item {index} cannot be empty"));
                }
                let count = validate_music_audio_codes(codes)?;
                if token_count
                    .replace(count)
                    .is_some_and(|previous| previous != count)
                {
                    return Err(
                        "audio codes arrays must use equal-duration code strings".to_owned()
                    );
                }
            }
            let batch_size = request.get("n").and_then(Value::as_u64).or_else(|| {
                contract
                    .request_attribute_specs
                    .get("n")
                    .and_then(|spec| spec.default.as_ref())
                    .and_then(Value::as_u64)
            });
            if batch_size.is_some_and(|batch_size| batch_size != codes.len() as u64) {
                return Err(
                    "audio codes arrays must contain exactly one code string per batch item"
                        .to_owned(),
                );
            }
        }
        _ => {}
    }
    Ok(())
}

fn maximum_base64_decoded_size(encoded_char_limit: u64) -> u64 {
    encoded_char_limit.saturating_div(4).saturating_mul(3)
}

fn validate_base64_decoded_size(encoded: &str, maximum_decoded_bytes: u64) -> Result<u64, String> {
    let bytes = encoded.as_bytes();
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return Err("inline audio data must be non-empty canonical base64".to_owned());
    }
    let padding = if bytes.ends_with(b"==") {
        2
    } else if bytes.ends_with(b"=") {
        1
    } else {
        0
    };
    let payload_len = bytes.len() - padding;
    if bytes[..payload_len]
        .iter()
        .any(|byte| base64_sextet(*byte).is_none())
        || bytes[payload_len..].iter().any(|byte| *byte != b'=')
    {
        return Err("inline audio data must be canonical base64".to_owned());
    }
    if padding == 2 && base64_sextet(bytes[payload_len - 1]).is_none_or(|value| value & 0x0f != 0) {
        return Err("inline audio data must be canonical base64".to_owned());
    }
    if padding == 1 && base64_sextet(bytes[payload_len - 1]).is_none_or(|value| value & 0x03 != 0) {
        return Err("inline audio data must be canonical base64".to_owned());
    }
    let decoded_bytes = u64::try_from(bytes.len() / 4)
        .unwrap_or(u64::MAX)
        .saturating_mul(3)
        .saturating_sub(padding as u64);
    if decoded_bytes > maximum_decoded_bytes {
        return Err(format!(
            "decoded inline audio size {decoded_bytes} exceeds the signed limit {maximum_decoded_bytes}"
        ));
    }
    Ok(decoded_bytes)
}

fn base64_sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn validate_openai_dimension_aliases(
    contract: &EndpointFamilyContract,
    request: &Value,
    violations: &mut Vec<EndpointContractViolation>,
) {
    let Some(object) = request.as_object() else {
        return;
    };
    let has_size = object.contains_key("size");
    let has_width = object.contains_key("width");
    let has_height = object.contains_key("height");
    if has_size && (has_width || has_height) {
        violations.push(EndpointContractViolation {
            path: "size".to_owned(),
            reason: "size cannot be combined with width/height".to_owned(),
        });
        return;
    }
    if has_width != has_height {
        violations.push(EndpointContractViolation {
            path: if has_width { "height" } else { "width" }.to_owned(),
            reason: "width and height must be supplied together".to_owned(),
        });
    }
    let Some(size) = object.get("size").and_then(Value::as_str) else {
        return;
    };
    let Some((width, height)) = size.split_once('x') else {
        violations.push(EndpointContractViolation {
            path: "size".to_owned(),
            reason: "size must be WIDTHxHEIGHT".to_owned(),
        });
        return;
    };
    if height.contains('x') {
        violations.push(EndpointContractViolation {
            path: "size".to_owned(),
            reason: "size must be WIDTHxHEIGHT".to_owned(),
        });
        return;
    }
    for (path, component) in [("width", width), ("height", height)] {
        let Ok(component) = component.parse::<u64>() else {
            violations.push(EndpointContractViolation {
                path: "size".to_owned(),
                reason: format!("size {path} is not a positive integer"),
            });
            continue;
        };
        let Some(spec) = contract.request_attribute_specs.get(path) else {
            continue;
        };
        if let Err(reason) = validate_endpoint_attribute_value(spec, &json!(component)) {
            violations.push(EndpointContractViolation {
                path: "size".to_owned(),
                reason: format!("size {path} violates the signed {path} constraint: {reason}"),
            });
        }
    }
}

fn validate_openai_video_duration_aliases(
    request: &Value,
    violations: &mut Vec<EndpointContractViolation>,
) {
    let Some(object) = request.as_object() else {
        return;
    };
    if object.contains_key("seconds") && object.contains_key("num_frames") {
        violations.push(EndpointContractViolation {
            path: "seconds".to_owned(),
            reason: "seconds cannot be combined with num_frames".to_owned(),
        });
    }
}

pub fn validate_endpoint_response(
    contract: &EndpointFamilyContract,
    response: &Value,
) -> Result<(), Vec<EndpointContractViolation>> {
    validate_endpoint_value(
        &contract.response_attributes,
        &contract.required_response_attributes,
        &contract.response_attribute_specs,
        response,
        "response",
    )
}

fn validate_endpoint_value(
    attributes: &[String],
    required: &[String],
    specs: &BTreeMap<String, EndpointAttributeSpec>,
    value: &Value,
    direction: &str,
) -> Result<(), Vec<EndpointContractViolation>> {
    let Some(object) = value.as_object() else {
        return Err(vec![EndpointContractViolation {
            path: direction.to_owned(),
            reason: format!("endpoint {direction} must be a JSON object"),
        }]);
    };
    let roots = attributes
        .iter()
        .filter_map(|path| path.split('.').next())
        .collect::<std::collections::BTreeSet<_>>();
    let mut violations = Vec::new();
    for key in object.keys() {
        if !roots.contains(key.as_str()) {
            violations.push(EndpointContractViolation {
                path: key.clone(),
                reason: format!("unsupported {direction} attribute"),
            });
        }
    }
    for path in required {
        if endpoint_values_at_path(value, path).is_empty() {
            violations.push(EndpointContractViolation {
                path: path.clone(),
                reason: format!("required {direction} attribute is missing"),
            });
        }
    }
    for (path, spec) in specs {
        for candidate in endpoint_values_at_path(value, path) {
            if let Err(reason) = validate_endpoint_attribute_value(spec, candidate) {
                violations.push(EndpointContractViolation {
                    path: path.clone(),
                    reason,
                });
            }
        }
    }
    for root in &roots {
        let Some(root_value) = object.get(*root) else {
            continue;
        };
        let has_declared_children = attributes
            .iter()
            .any(|path| path.starts_with(&format!("{root}.")));
        if has_declared_children {
            validate_nested_endpoint_attributes(
                root,
                root_value,
                attributes,
                &mut violations,
                direction,
            );
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

fn endpoint_values_at_path<'a>(value: &'a Value, path: &str) -> Vec<&'a Value> {
    let segments = path.split('.').collect::<Vec<_>>();
    let mut values = Vec::new();
    collect_endpoint_values(value, &segments, &mut values);
    values
}

fn collect_endpoint_values<'a>(value: &'a Value, segments: &[&str], output: &mut Vec<&'a Value>) {
    if segments.is_empty() {
        output.push(value);
        return;
    }
    match value {
        Value::Array(items) => {
            for item in items {
                collect_endpoint_values(item, segments, output);
            }
        }
        Value::Object(object) => {
            if let Some(next) = object.get(segments[0]) {
                collect_endpoint_values(next, &segments[1..], output);
            }
        }
        _ => {}
    }
}

fn validate_nested_endpoint_attributes(
    path: &str,
    value: &Value,
    attributes: &[String],
    violations: &mut Vec<EndpointContractViolation>,
    direction: &str,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                validate_nested_endpoint_attributes(path, item, attributes, violations, direction);
            }
        }
        Value::Object(object) => {
            for (key, child) in object {
                let child_path = format!("{path}.{key}");
                let exact = attributes.iter().any(|declared| declared == &child_path);
                let descendant = attributes
                    .iter()
                    .any(|declared| declared.starts_with(&format!("{child_path}.")));
                if !exact && !descendant {
                    violations.push(EndpointContractViolation {
                        path: child_path,
                        reason: format!("unsupported nested {direction} attribute"),
                    });
                    continue;
                }
                if descendant {
                    validate_nested_endpoint_attributes(
                        &child_path,
                        child,
                        attributes,
                        violations,
                        direction,
                    );
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn endpoint_request_fingerprint_survives_javascript_number_serialization() {
        let rust_shape = serde_json::from_str::<Value>(
            r#"{"cfg_scale":1.0,"shift":3.0,"nested":{"epsilon":1e-6,"zero":-0.0}}"#,
        )
        .unwrap();
        let javascript_shape = serde_json::from_str::<Value>(
            r#"{"nested":{"zero":0,"epsilon":0.000001},"shift":3,"cfg_scale":1}"#,
        )
        .unwrap();

        assert_eq!(
            endpoint_request_fingerprint(&rust_shape),
            endpoint_request_fingerprint(&javascript_shape)
        );
        assert_eq!(
            endpoint_request_fingerprint(&rust_shape),
            blake3::hash(
                javascript_roundtrip_stable_value(&javascript_shape)
                    .to_string()
                    .as_bytes()
            )
            .to_hex()
            .to_string(),
            "the normalized sender hash remains compatible with the existing receiver hash"
        );
    }

    #[test]
    fn endpoint_request_fingerprint_detects_numeric_value_changes() {
        assert_ne!(
            endpoint_request_fingerprint(&json!({"seed": 9_007_199_254_740_993_u64})),
            endpoint_request_fingerprint(&json!({"seed": 9_007_199_254_740_992_u64}))
        );
        assert_ne!(
            endpoint_request_fingerprint(&json!({"cfg_scale": 1.5})),
            endpoint_request_fingerprint(&json!({"cfg_scale": 1.500_000_1}))
        );
    }

    #[test]
    fn every_endpoint_template_has_complete_typed_specs() {
        for family in [
            ENDPOINT_OPENAI_CHAT_COMPLETIONS,
            ENDPOINT_OPENAI_COMPLETIONS,
            ENDPOINT_OPENAI_RESPONSES,
            ENDPOINT_HF_MULTIMODAL_CHAT,
            ENDPOINT_OPENAI_EMBEDDINGS,
            ENDPOINT_HF_FEATURE_EXTRACTION,
            ENDPOINT_OPENAI_IMAGE_GENERATIONS,
            ENDPOINT_HF_TEXT_TO_IMAGE,
            ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS,
            ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION,
            ENDPOINT_OPENAI_AUDIO_SPEECH,
            ENDPOINT_HF_TEXT_TO_SPEECH,
            ENDPOINT_OPENAI_VIDEOS,
            ENDPOINT_HF_TEXT_TO_VIDEO,
            ENDPOINT_MAYHEM_AUDIO_GENERATIONS,
            ENDPOINT_MAYHEM_MUSIC_GENERATIONS,
            ENDPOINT_HF_TEXT_TO_AUDIO,
        ] {
            let contract = endpoint_family_contract_template(family)
                .unwrap_or_else(|| panic!("missing endpoint template for {family}"));
            assert_eq!(
                contract.request_attributes.len(),
                contract.request_attribute_specs.len(),
                "request schema mismatch for {family}"
            );
            assert_eq!(
                contract.response_attributes.len(),
                contract.response_attribute_specs.len(),
                "response schema mismatch for {family}"
            );
            for required in &contract.required_request_attributes {
                assert!(contract.request_attribute_specs.contains_key(required));
            }
            for spec in contract.request_attribute_specs.values() {
                assert!(!spec.value_types.is_empty());
                for value in &spec.calibration_values {
                    validate_endpoint_attribute_value(spec, value).unwrap();
                }
                if let Some(default) = &spec.default {
                    validate_endpoint_attribute_value(spec, default).unwrap();
                }
            }
        }
    }

    #[test]
    fn speech_templates_sign_and_validate_voice_cloning_controls() {
        let reference_audio =
            endpoint_calibration_wav_base64_fixture(MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize)
                .unwrap();
        let descriptor = json!({
            "data": reference_audio,
            "encoding": "base64",
            "content_type": "audio/wav"
        });

        let openai = endpoint_family_contract_template(ENDPOINT_OPENAI_AUDIO_SPEECH).unwrap();
        let openai_request = json!({
            "model": "test/chatterbox",
            "input": "Mayhem voice-cloning calibration.",
            "voice": "default",
            "reference_audio": descriptor,
            "exaggeration": 0.7,
            "cfg_weight": 0.3,
            "temperature": 0.8,
            "min_p": 0.05,
            "top_p": 1.0,
            "repetition_penalty": 1.2,
            "seed": 7
        });
        assert!(validate_endpoint_request(&openai, &openai_request).is_ok());

        let hf = endpoint_family_contract_template(ENDPOINT_HF_TEXT_TO_SPEECH).unwrap();
        let hf_request = json!({
            "inputs": "Mayhem voice-cloning calibration.",
            "parameters": {
                "reference_audio": openai_request["reference_audio"],
                "exaggeration": 0.7,
                "cfg_weight": 0.3,
                "seed": 7,
                "generation_parameters": {
                    "temperature": 0.8,
                    "min_p": 0.05,
                    "top_p": 1.0,
                    "repetition_penalty": 1.2
                }
            }
        });
        assert!(validate_endpoint_request(&hf, &hf_request).is_ok());

        let maximum_reference =
            endpoint_calibration_wav_base64_fixture(MAX_TTS_REFERENCE_AUDIO_BASE64_CHARS as usize)
                .expect("maximum TTS reference fixture is constructible");
        assert_eq!(
            maximum_reference.len(),
            MAX_TTS_REFERENCE_AUDIO_BASE64_CHARS as usize
        );
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&maximum_reference)
                .unwrap()
                .len(),
            MAX_TTS_REFERENCE_AUDIO_BYTES
        );
        let mut maximum_request = openai_request.clone();
        maximum_request["reference_audio"]["data"] = json!(maximum_reference);
        assert!(validate_endpoint_request(&openai, &maximum_request).is_ok());

        let mut invalid = openai_request;
        invalid["reference_audio"]["content_type"] = json!("text/plain");
        assert!(validate_endpoint_request(&openai, &invalid)
            .unwrap_err()
            .iter()
            .any(|violation| violation.path == "reference_audio.content_type"));

        let mut alternate_wav_mime = invalid;
        alternate_wav_mime["reference_audio"]["content_type"] = json!("audio/x-wav");
        assert!(validate_endpoint_request(&openai, &alternate_wav_mime)
            .unwrap_err()
            .iter()
            .any(|violation| violation.path == "reference_audio.content_type"));

        let data_length = 11_u32 * 8_000 * 2;
        let mut long_wav = Vec::with_capacity((44 + data_length) as usize);
        long_wav.extend_from_slice(b"RIFF");
        long_wav.extend_from_slice(&(36 + data_length).to_le_bytes());
        long_wav.extend_from_slice(b"WAVEfmt ");
        long_wav.extend_from_slice(&16_u32.to_le_bytes());
        long_wav.extend_from_slice(&1_u16.to_le_bytes());
        long_wav.extend_from_slice(&1_u16.to_le_bytes());
        long_wav.extend_from_slice(&8_000_u32.to_le_bytes());
        long_wav.extend_from_slice(&16_000_u32.to_le_bytes());
        long_wav.extend_from_slice(&2_u16.to_le_bytes());
        long_wav.extend_from_slice(&16_u16.to_le_bytes());
        long_wav.extend_from_slice(b"data");
        long_wav.extend_from_slice(&data_length.to_le_bytes());
        long_wav.resize((44 + data_length) as usize, 0);
        let mut too_long = alternate_wav_mime;
        too_long["reference_audio"]["content_type"] = json!("audio/wav");
        too_long["reference_audio"]["data"] =
            json!(base64::engine::general_purpose::STANDARD.encode(long_wav));
        assert!(validate_endpoint_request(&openai, &too_long)
            .unwrap_err()
            .iter()
            .any(|violation| violation.reason.contains("10-second")));
    }

    #[test]
    fn attribute_validator_enforces_enum_bounds_and_types() {
        let contract = endpoint_family_contract_template(ENDPOINT_OPENAI_VIDEOS).unwrap();
        let seconds = &contract.request_attribute_specs["seconds"];
        assert!(validate_endpoint_attribute_value(seconds, &json!("8")).is_ok());
        assert!(validate_endpoint_attribute_value(seconds, &json!(8)).is_err());
        assert!(validate_endpoint_attribute_value(seconds, &json!("13")).is_err());

        let prompt = &contract.request_attribute_specs["prompt"];
        assert!(validate_endpoint_attribute_value(prompt, &json!("")).is_err());
        assert!(validate_endpoint_attribute_value(prompt, &json!("valid")).is_ok());

        let error = &contract.response_attribute_specs["error"];
        assert!(validate_endpoint_attribute_value(error, &Value::Null).is_ok());
        assert!(validate_endpoint_attribute_value(error, &json!({"code": "failed"})).is_ok());
        assert!(validate_endpoint_attribute_value(error, &json!("failed")).is_err());
    }

    #[test]
    fn attribute_validator_enforces_numeric_multiple() {
        let mut spec = integer_spec(256.0, 2_048.0, 1_024);
        spec.multiple_of = Some(16.0);
        assert!(validate_endpoint_attribute_value(&spec, &json!(1024)).is_ok());
        assert!(validate_endpoint_attribute_value(&spec, &json!(1025)).is_err());
        assert!(numeric_boundary_values(&spec)
            .iter()
            .any(|(kind, _, accepted)| *kind == "not_multiple_of" && !accepted));
    }

    #[test]
    fn request_defaults_materialize_without_overwriting_client_values() {
        let mut contract = endpoint_family_contract_template(ENDPOINT_HF_TEXT_TO_IMAGE).unwrap();
        contract
            .request_attribute_specs
            .get_mut("parameters.width")
            .unwrap()
            .default = Some(json!(1024));
        contract
            .request_attribute_specs
            .get_mut("parameters.height")
            .unwrap()
            .default = Some(json!(1024));

        let normalized = materialize_endpoint_request_defaults(
            &contract,
            &json!({"inputs":"hello", "parameters":{"width":768}}),
        )
        .unwrap();
        assert_eq!(normalized["parameters"]["width"], json!(768));
        assert_eq!(normalized["parameters"]["height"], json!(1024));
    }

    #[test]
    fn openai_image_aliases_share_signed_dimension_constraints() {
        let mut contract = endpoint_family_contract_template(ENDPOINT_OPENAI_IMAGE_GENERATIONS)
            .expect("OpenAI image endpoint contract");
        for path in ["width", "height"] {
            let spec = contract.request_attribute_specs.get_mut(path).unwrap();
            spec.default = Some(json!(1024));
            spec.multiple_of = Some(16.0);
        }

        let normalized = materialize_endpoint_request_defaults(
            &contract,
            &json!({"model":"test/image", "prompt":"a compass"}),
        )
        .unwrap();
        assert_eq!(normalized["width"], json!(1024));
        assert_eq!(normalized["height"], json!(1024));
        assert!(normalized.get("size").is_none());

        let explicit_size = json!({
            "model":"test/image",
            "prompt":"a compass",
            "size":"768x1024"
        });
        assert!(validate_endpoint_request(&contract, &explicit_size).is_ok());
        let normalized = materialize_endpoint_request_defaults(&contract, &explicit_size).unwrap();
        assert_eq!(normalized["size"], json!("768x1024"));
        assert!(normalized.get("width").is_none());
        assert!(normalized.get("height").is_none());

        let invalid_multiple = validate_endpoint_request(
            &contract,
            &json!({"model":"test/image", "prompt":"a compass", "size":"770x1024"}),
        )
        .unwrap_err();
        assert!(invalid_multiple.iter().any(|violation| {
            violation.path == "size" && violation.reason.contains("multiple of 16")
        }));
        let conflicting = validate_endpoint_request(
            &contract,
            &json!({
                "model":"test/image",
                "prompt":"a compass",
                "size":"1024x1024",
                "width":1024,
                "height":1024
            }),
        )
        .unwrap_err();
        assert!(conflicting
            .iter()
            .any(|violation| violation.reason.contains("cannot be combined")));
    }

    #[test]
    fn openai_video_aliases_share_signed_dimension_constraints() {
        let mut contract = endpoint_family_contract_template(ENDPOINT_OPENAI_VIDEOS)
            .expect("OpenAI video endpoint contract");
        for (path, default) in [("width", 768), ("height", 512)] {
            let spec = contract.request_attribute_specs.get_mut(path).unwrap();
            spec.default = Some(json!(default));
            spec.minimum = Some(256.0);
            spec.maximum = Some(2_048.0);
            spec.multiple_of = Some(64.0);
        }

        let normalized = materialize_endpoint_request_defaults(
            &contract,
            &json!({"model":"test/video", "prompt":"a compass"}),
        )
        .unwrap();
        assert_eq!(normalized["width"], json!(768));
        assert_eq!(normalized["height"], json!(512));
        assert!(normalized.get("size").is_none());

        let explicit_size = json!({
            "model":"test/video",
            "prompt":"a compass",
            "size":"512x256"
        });
        assert!(validate_endpoint_request(&contract, &explicit_size).is_ok());
        let normalized = materialize_endpoint_request_defaults(&contract, &explicit_size).unwrap();
        assert_eq!(normalized["size"], json!("512x256"));
        assert!(normalized.get("width").is_none());
        assert!(normalized.get("height").is_none());

        let malformed = validate_endpoint_request(
            &contract,
            &json!({"model":"test/video", "prompt":"a compass", "size":"512-wide"}),
        )
        .unwrap_err();
        assert!(malformed
            .iter()
            .any(|violation| violation.reason.contains("WIDTHxHEIGHT")));

        let invalid_multiple = validate_endpoint_request(
            &contract,
            &json!({"model":"test/video", "prompt":"a compass", "size":"500x512"}),
        )
        .unwrap_err();
        assert!(invalid_multiple
            .iter()
            .any(|violation| violation.reason.contains("multiple of 64")));
    }

    #[test]
    fn video_calibration_pairs_dimensions_and_duration_controls() {
        let mut contract = endpoint_family_contract_template(ENDPOINT_OPENAI_VIDEOS)
            .expect("OpenAI video endpoint contract");
        contract
            .request_attribute_specs
            .get_mut("fps")
            .unwrap()
            .calibration_values = vec![json!(1), json!(24), json!(50)];
        contract
            .request_attribute_specs
            .get_mut("num_frames")
            .unwrap()
            .calibration_values = vec![json!(9), json!(497)];
        let cases = generate_endpoint_calibration_cases(&contract).expect("video matrix");

        let width_case = cases
            .iter()
            .find(|case| {
                case.expect_accept
                    && case.case_kind.starts_with("accepted_value_")
                    && case
                        .mutations
                        .first()
                        .is_some_and(|mutation| mutation.path == "width")
            })
            .expect("accepted width case");
        assert!(width_case
            .mutations
            .iter()
            .any(|mutation| mutation.path == "height"));

        let seconds_case = cases
            .iter()
            .find(|case| {
                case.expect_accept
                    && case.case_kind.starts_with("accepted_value_")
                    && case
                        .mutations
                        .first()
                        .is_some_and(|mutation| mutation.path == "seconds")
            })
            .expect("accepted seconds case");
        assert!(seconds_case
            .mutations
            .iter()
            .any(|mutation| mutation.path == "fps"));

        let fps_case = cases
            .iter()
            .find(|case| {
                case.expect_accept
                    && case.case_kind.starts_with("accepted_value_")
                    && case
                        .mutations
                        .first()
                        .is_some_and(|mutation| mutation.path == "fps")
            })
            .expect("accepted fps case");
        assert!(fps_case
            .mutations
            .iter()
            .any(|mutation| mutation.path == "num_frames"));

        let frame_case = cases
            .iter()
            .find(|case| {
                case.expect_accept
                    && case.case_kind.starts_with("accepted_value_")
                    && case.mutations.first().is_some_and(|mutation| {
                        mutation.path == "num_frames"
                            && mutation.value
                                == (EndpointCalibrationValue::Literal { value: json!(497) })
                    })
            })
            .expect("accepted maximum frame case");
        assert!(frame_case.mutations.iter().any(|mutation| {
            mutation.path == "fps"
                && mutation.value == (EndpointCalibrationValue::Literal { value: json!(50) })
        }));

        let mut hf_contract = endpoint_family_contract_template(ENDPOINT_HF_TEXT_TO_VIDEO)
            .expect("HF video endpoint contract");
        hf_contract
            .request_attribute_specs
            .get_mut("parameters.fps")
            .unwrap()
            .calibration_values = vec![json!(1), json!(24), json!(50)];
        hf_contract
            .request_attribute_specs
            .get_mut("parameters.num_frames")
            .unwrap()
            .calibration_values = vec![json!(9), json!(497)];
        hf_contract
            .request_attribute_specs
            .get_mut("parameters.num_frames")
            .unwrap()
            .default = Some(json!(121));
        let hf_cases = generate_endpoint_calibration_cases(&hf_contract).expect("HF video matrix");
        let hf_fps_case = hf_cases
            .iter()
            .find(|case| {
                case.expect_accept
                    && case.case_kind.starts_with("accepted_value_")
                    && case
                        .mutations
                        .first()
                        .is_some_and(|mutation| mutation.path == "parameters.fps")
            })
            .expect("accepted HF fps case");
        assert!(hf_fps_case.mutations.iter().any(|mutation| {
            mutation.path == "parameters.num_frames"
                && mutation.value == (EndpointCalibrationValue::Literal { value: json!(9) })
        }));
        let hf_frame_case = hf_cases
            .iter()
            .find(|case| {
                case.expect_accept
                    && case.case_kind.starts_with("accepted_value_")
                    && case.mutations.first().is_some_and(|mutation| {
                        mutation.path == "parameters.num_frames"
                            && mutation.value
                                == (EndpointCalibrationValue::Literal { value: json!(497) })
                    })
            })
            .expect("accepted HF maximum frame case");
        assert!(hf_frame_case.mutations.iter().any(|mutation| {
            mutation.path == "parameters.fps"
                && mutation.value == (EndpointCalibrationValue::Literal { value: json!(50) })
        }));

        assert!(cases.iter().all(|case| {
            let paths = case
                .mutations
                .iter()
                .map(|mutation| mutation.path.as_str())
                .collect::<BTreeSet<_>>();
            !(paths.contains("seconds") && paths.contains("num_frames"))
        }));

        let conflicting_duration = json!({
            "model": "test/video",
            "prompt": "a compass",
            "seconds": "10",
            "num_frames": 9,
            "fps": 8
        });
        let violations = validate_endpoint_request(&contract, &conflicting_duration).unwrap_err();
        assert!(violations.iter().any(|violation| {
            violation.path == "seconds"
                && violation
                    .reason
                    .contains("cannot be combined with num_frames")
        }));
    }

    #[test]
    fn music_template_signs_source_controls_with_finite_closed_specs() {
        let contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        for path in [
            "prompt",
            "caption",
            "lyrics",
            "instrumental",
            "style",
            "genre",
            "tags",
            "sample_mode",
            "sample_query",
            "use_format",
            "audio_codes",
            "language",
            "vocal_language",
            "bpm",
            "key",
            "keyscale",
            "time_signature",
            "timesignature",
            "duration_seconds",
            "duration",
            "steps",
            "inference_steps",
            "guidance_scale",
            "seed",
            "n",
            "batch",
            "batch_size",
            "task_type",
            "thinking",
            "instruction",
            "lm_temperature",
            "lm_cfg_scale",
            "lm_top_k",
            "lm_top_p",
            "lm_negative_prompt",
            "use_cot_metas",
            "use_cot_caption",
            "use_cot_language",
            "constrained_decoding",
            "infer_method",
            "sampler",
            "sampler_mode",
            "velocity_norm_threshold",
            "velocity_ema_factor",
            "dcw_enabled",
            "dcw_mode",
            "dcw_scaler",
            "dcw_high_scaler",
            "dcw_wavelet",
            "shift",
            "custom_timesteps",
            "adg",
            "cfg_interval_start",
            "cfg_interval_end",
            "repaint_start",
            "repaint_end",
            "repaint_mode",
            "repaint_strength",
            "chunk_mask_mode",
            "cover_strength",
            "cover_noise_strength",
            "enable_normalization",
            "normalization_db",
            "fade_in_duration",
            "fade_out_duration",
            "latent_shift",
            "latent_rescale",
            "retake_seed",
            "retake_variance",
            "flow_edit_morph",
            "flow_edit_source_caption",
            "flow_edit_source_lyrics",
            "flow_edit_n_min",
            "flow_edit_n_max",
            "flow_edit_n_avg",
            "seeds",
            "response_format",
            "mp3_bitrate",
            "mp3_sample_rate",
            "source_audio",
            "reference_audio",
        ] {
            assert!(
                contract.request_attribute_specs.contains_key(path),
                "missing signed music attribute {path}"
            );
        }
        assert_eq!(
            contract.request_attribute_specs["lyrics"].max_length,
            Some(4_096)
        );
        for path in ["prompt", "caption"] {
            assert_eq!(
                contract.request_attribute_specs[path].max_length,
                Some(MAX_MUSIC_CAPTION_CHARS),
                "{path}"
            );
        }
        assert_eq!(
            contract.request_attribute_specs["style"].max_length,
            Some(505)
        );
        assert_eq!(
            contract.request_attribute_specs["genre"].max_length,
            Some(505)
        );
        assert_eq!(
            contract.request_attribute_specs["tags"].max_length,
            Some(506)
        );
        assert_eq!(
            contract.request_attribute_specs["audio_codes"].max_length,
            Some(MAX_MUSIC_AUDIO_CODES_CHARS)
        );
        assert_eq!(
            contract.request_attribute_specs["audio_codes"].value_types,
            vec![EndpointValueType::String, EndpointValueType::Array]
        );
        assert_eq!(
            contract.request_attribute_specs["audio_codes"].max_items,
            Some(8)
        );
        assert_eq!(
            contract.request_attribute_specs["custom_timesteps"].max_items,
            Some(200)
        );
        assert_eq!(contract.request_attribute_specs["seeds"].max_items, Some(8));
        for (path, minimum, maximum) in [
            ("bpm", 30.0, 300.0),
            ("duration_seconds", -1.0, 600.0),
            ("steps", 1.0, 200.0),
            ("guidance_scale", 1.0, 15.0),
            ("seed", -1.0, MAX_MUSIC_SEED),
            ("lm_temperature", 0.0, 2.0),
            ("lm_cfg_scale", 1.0, 3.0),
            ("velocity_norm_threshold", 0.0, 5.0),
            ("velocity_ema_factor", 0.0, 0.5),
            ("normalization_db", -10.0, 0.0),
            ("latent_shift", -0.2, 0.2),
            ("latent_rescale", 0.5, 1.5),
        ] {
            let spec = &contract.request_attribute_specs[path];
            assert_eq!(spec.minimum, Some(minimum), "{path}");
            assert_eq!(spec.maximum, Some(maximum), "{path}");
        }
        for unsupported in [
            "global_caption",
            "use_cot_lyrics",
            "lm_repetition_penalty",
            "lm_repeat_penalty",
            "repaint_latent_crossfade_frames",
            "repaint_wav_crossfade_sec",
            "typical_p",
            "do_sample",
            "max_new_tokens",
            "cot_lyrics",
        ] {
            assert!(
                !contract
                    .request_attributes
                    .iter()
                    .any(|path| path == unsupported),
                "{unsupported}"
            );
            let mut request = json!({"model": "test/music", "prompt": "closed contract"});
            request
                .as_object_mut()
                .unwrap()
                .insert(unsupported.to_owned(), json!(true));
            let violations = validate_endpoint_request(&contract, &request).unwrap_err();
            assert!(
                violations
                    .iter()
                    .any(|violation| violation.path == unsupported),
                "{unsupported}: {violations:?}"
            );
        }
        for forbidden in [
            "source_audio_path",
            "src_audio_path",
            "reference_audio_path",
            "melody_path",
            "audio_path",
            "lm_model_path",
        ] {
            assert!(!contract
                .request_attributes
                .iter()
                .any(|path| path == forbidden));
        }
    }

    #[test]
    fn music_defaults_materialize_from_canonical_source_calibration() {
        let contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        let normalized = materialize_endpoint_request_defaults(
            &contract,
            &json!({"model": "test/music", "prompt": "minimal source request"}),
        )
        .unwrap();
        for (path, expected) in [
            ("thinking", json!(false)),
            ("use_cot_caption", json!(true)),
            ("duration_seconds", Value::Null),
            ("steps", json!(50)),
            ("guidance_scale", json!(7.0)),
            ("n", json!(2)),
            ("response_format", json!("flac")),
            ("lm_temperature", json!(0.85)),
            ("lm_cfg_scale", json!(2.0)),
            ("lm_top_k", json!(0)),
            ("lm_top_p", json!(0.9)),
            ("infer_method", json!("ode")),
            ("sampler", json!("euler")),
            ("shift", json!(3.0)),
            ("dcw_enabled", json!(false)),
            ("enable_normalization", json!(true)),
            ("normalization_db", json!(-1.0)),
            ("latent_shift", json!(0.0)),
            ("latent_rescale", json!(1.0)),
        ] {
            assert_eq!(normalized[path], expected, "{path}");
        }
        assert_eq!(normalized["bpm"], Value::Null);
        assert!(normalized.get("seeds").is_none());
        assert!(normalized.get("retake_seed").is_none());
        assert!(normalized.get("repaint_mode").is_none());
        assert!(normalized.get("cover_strength").is_none());
        assert!(normalized.get("flow_edit_n_min").is_none());
        assert!(normalized.get("mp3_bitrate").is_none());
        assert!(normalized.get("mp3_sample_rate").is_none());
        assert!(validate_endpoint_request(&contract, &normalized).is_ok());

        let explicit = materialize_endpoint_request_defaults(
            &contract,
            &json!({
                "model": "test/music",
                "caption": "explicit aliases",
                "batch_size": 4,
                "audio_format": "mp3",
                "thinking": true
            }),
        )
        .unwrap();
        assert_eq!(explicit["prompt"], json!("explicit aliases"));
        assert_eq!(explicit["n"], json!(4));
        assert_eq!(explicit["response_format"], json!("mp3"));
        assert_eq!(explicit["mp3_bitrate"], json!("128k"));
        assert_eq!(explicit["mp3_sample_rate"], json!(48_000));
        assert_eq!(explicit["thinking"], json!(true));
        for alias in ["caption", "batch_size", "audio_format"] {
            assert!(explicit.get(alias).is_none(), "{alias}");
        }
    }

    #[test]
    fn artifact_generation_character_usage_counts_all_consumed_text_deterministically() {
        let contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        let music = materialize_endpoint_request_defaults(
            &contract,
            &json!({"model": "test/music", "prompt": "duet"}),
        )
        .unwrap();
        assert_eq!(
            artifact_generation_input_characters(ENDPOINT_MAYHEM_MUSIC_GENERATIONS, &music),
            76
        );
        assert_eq!(
            artifact_generation_input_characters(
                ENDPOINT_MAYHEM_AUDIO_GENERATIONS,
                &json!({"prompt": "rain", "negative_prompt": "speech"})
            ),
            10
        );
        assert_eq!(
            artifact_generation_input_characters(
                ENDPOINT_HF_TEXT_TO_AUDIO,
                &json!({
                    "inputs": "ocean",
                    "parameters": {"negative_prompt": "voices"}
                })
            ),
            11
        );
    }

    #[test]
    fn music_task_requirements_allow_lyrics_only_and_source_driven_requests() {
        let contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        assert_eq!(contract.required_request_attributes, vec!["model"]);
        assert!(validate_endpoint_request(
            &contract,
            &json!({"model": "test/music", "lyrics": "[Verse]\nSignal in the rain"})
        )
        .is_ok());

        let source_audio =
            endpoint_calibration_wav_base64_fixture(MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize)
                .unwrap();
        let source_driven = json!({
            "model": "test/music",
            "task_type": "cover",
            "source_audio": {
                "data": source_audio,
                "encoding": "base64",
                "content_type": "audio/wav"
            }
        });
        assert!(validate_endpoint_request(&contract, &source_driven).is_ok());
        let normalized = materialize_endpoint_request_defaults(&contract, &source_driven).unwrap();
        assert_eq!(normalized["cover_strength"], json!(1.0));
        assert!(normalized.get("repaint_mode").is_none());
        assert!(normalized.get("instruction").is_none());
        assert!(validate_endpoint_request(&contract, &normalized).is_ok());

        let missing_text =
            validate_endpoint_request(&contract, &json!({"model": "test/music"})).unwrap_err();
        assert!(missing_text.iter().any(|violation| {
            violation.path == "prompt" && violation.reason.contains("prompt/caption")
        }));
        let missing_source = validate_endpoint_request(
            &contract,
            &json!({"model": "test/music", "task_type": "repaint"}),
        )
        .unwrap_err();
        assert!(missing_source
            .iter()
            .any(|violation| violation.path == "source_audio"));
    }

    #[test]
    fn music_flow_and_repaint_controls_are_task_consistent() {
        let contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        let source_audio = json!({
            "data": "AA==",
            "encoding": "base64",
            "content_type": "audio/flac"
        });
        let invalid_repaint_range = validate_endpoint_request(
            &contract,
            &json!({
                "model": "test/music",
                "task_type": "repaint",
                "source_audio": source_audio,
                "repaint_start": 4.0,
                "repaint_end": 2.0
            }),
        )
        .unwrap_err();
        assert!(invalid_repaint_range
            .iter()
            .any(|violation| violation.path == "repaint_end"));

        let text_repaint_control = validate_endpoint_request(
            &contract,
            &json!({
                "model": "test/music",
                "prompt": "not repainting",
                "repaint_strength": 0.5
            }),
        )
        .unwrap_err();
        assert!(text_repaint_control
            .iter()
            .any(|violation| violation.path == "repaint_strength"));

        let invalid_flow_range = validate_endpoint_request(
            &contract,
            &json!({
                "model": "test/music",
                "prompt": "target arrangement",
                "source_audio": {
                    "data": "AA==",
                    "encoding": "base64",
                    "content_type": "audio/flac"
                },
                "flow_edit_morph": true,
                "flow_edit_n_min": 0.8,
                "flow_edit_n_max": 0.2
            }),
        )
        .unwrap_err();
        assert!(invalid_flow_range
            .iter()
            .any(|violation| violation.path == "flow_edit_n_max"));

        let missing_flow_source = validate_endpoint_request(
            &contract,
            &json!({
                "model": "test/music",
                "prompt": "target arrangement",
                "flow_edit_morph": true
            }),
        )
        .unwrap_err();
        assert!(missing_flow_source
            .iter()
            .any(|violation| violation.path == "source_audio"));
    }

    #[test]
    fn music_consumed_cross_field_controls_are_source_consistent() {
        let contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        assert!(validate_endpoint_request(
            &contract,
            &json!({
                "model": "test/music",
                "prompt": "controlled render",
                "duration_seconds": 10,
                "cfg_interval_start": 0.2,
                "cfg_interval_end": 0.8,
                "fade_in_duration": 4,
                "fade_out_duration": 6,
                "retake_seed": 7,
                "retake_variance": 0.2,
                "response_format": "mp3",
                "mp3_bitrate": "256k",
                "mp3_sample_rate": 44_100
            })
        )
        .is_ok());

        let source_audio = json!({
            "data": "AA==",
            "encoding": "base64",
            "content_type": "audio/flac"
        });
        for (request, path) in [
            (
                json!({
                    "model": "test/music",
                    "task_type": "cover",
                    "source_audio": source_audio,
                    "sample_mode": true
                }),
                "sample_mode",
            ),
            (
                json!({
                    "model": "test/music",
                    "prompt": "conflicting preprocess",
                    "sample_query": "a generated sample",
                    "audio_codes": "<|audio_code_1|>"
                }),
                "audio_codes",
            ),
            (
                json!({
                    "model": "test/music",
                    "prompt": "reversed interval",
                    "cfg_interval_start": 0.8,
                    "cfg_interval_end": 0.2
                }),
                "cfg_interval_end",
            ),
            (
                json!({
                    "model": "test/music",
                    "prompt": "overlong fades",
                    "duration_seconds": 10,
                    "fade_in_duration": 6,
                    "fade_out_duration": 5
                }),
                "fade_out_duration",
            ),
            (
                json!({
                    "model": "test/music",
                    "prompt": "missing retake variance",
                    "retake_seed": 7
                }),
                "retake_variance",
            ),
            (
                json!({
                    "model": "test/music",
                    "prompt": "wrong output controls",
                    "response_format": "flac",
                    "mp3_bitrate": "256k"
                }),
                "mp3_bitrate",
            ),
        ] {
            let violations = validate_endpoint_request(&contract, &request).unwrap_err();
            assert!(
                violations.iter().any(|violation| violation.path == path),
                "{path}: {violations:?}"
            );
        }
        assert!(validate_endpoint_attribute_value(
            &contract.request_attribute_specs["retake_seed"],
            &json!("7")
        )
        .is_err());
    }

    #[test]
    fn music_aliases_normalize_without_merging_independent_modifiers() {
        let contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        let reference_audio =
            endpoint_calibration_wav_base64_fixture(MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize)
                .unwrap();
        let request = json!({
            "model": "test/music",
            "caption": "a close-miked prepared piano",
            "style": "acoustic",
            "genre": "minimalism",
            "tags": "close-miked, prepared piano",
            "vocal_language": "en",
            "key_scale": "D minor",
            "timesignature": "6",
            "duration": 12.5,
            "inference_steps": 12,
            "batch_size": 2,
            "use_adg": true,
            "audio_format": "flac",
            "temperature": 0.7,
            "top_k": 20,
            "top_p": 0.8,
            "sampler_mode": "heun",
            "lm_cfg": 2.0,
            "use_cot_metas": true,
            "use_cot_caption": false,
            "use_cot_language": false,
            "use_constrained_decoding": true,
            "melody": {
                "data": reference_audio,
                "encoding": "base64",
                "content_type": "audio/wav"
            }
        });
        assert!(validate_endpoint_request(&contract, &request).is_ok());

        let normalized = materialize_endpoint_request_defaults(&contract, &request).unwrap();
        for (canonical, expected) in [
            ("prompt", json!("a close-miked prepared piano")),
            ("style", json!("acoustic")),
            ("genre", json!("minimalism")),
            ("tags", json!("close-miked, prepared piano")),
            ("language", json!("en")),
            ("key", json!("D minor")),
            ("time_signature", json!("6")),
            ("duration_seconds", json!(12.5)),
            ("steps", json!(12)),
            ("n", json!(2)),
            ("adg", json!(true)),
            ("response_format", json!("flac")),
            ("lm_temperature", json!(0.7)),
            ("lm_top_k", json!(20)),
            ("lm_top_p", json!(0.8)),
            ("sampler", json!("heun")),
            ("lm_cfg_scale", json!(2.0)),
            ("constrained_decoding", json!(true)),
        ] {
            assert_eq!(normalized[canonical], expected, "{canonical}");
        }
        assert_eq!(normalized["reference_audio"]["data"], reference_audio);
        for alias in [
            "caption",
            "vocal_language",
            "key_scale",
            "timesignature",
            "duration",
            "inference_steps",
            "batch_size",
            "use_adg",
            "audio_format",
            "temperature",
            "top_k",
            "top_p",
            "sampler_mode",
            "melody",
        ] {
            assert!(normalized.get(alias).is_none(), "{alias} was not removed");
        }

        let source_audio =
            endpoint_calibration_wav_base64_fixture(MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize)
                .unwrap();
        let source_alias = json!({
            "model": "test/music",
            "prompt": "cover alias",
            "task_type": "cover",
            "src_audio": {
                "data": source_audio,
                "encoding": "base64",
                "content_type": "audio/wav"
            }
        });
        assert!(validate_endpoint_request(&contract, &source_alias).is_ok());
        let normalized_source =
            materialize_endpoint_request_defaults(&contract, &source_alias).unwrap();
        assert_eq!(normalized_source["source_audio"]["data"], source_audio);
        assert!(normalized_source.get("src_audio").is_none());

        let ambiguous = validate_endpoint_request(
            &contract,
            &json!({
                "model": "test/music",
                "prompt": "canonical",
                "caption": "alias"
            }),
        )
        .unwrap_err();
        assert_eq!(ambiguous[0].path, "prompt");
        assert!(ambiguous[0].reason.contains("ambiguous aliases"));

        for (canonical, request) in [
            (
                "sampler",
                json!({
                    "model": "test/music",
                    "prompt": "ambiguous sampler",
                    "sampler": "euler",
                    "sampler_mode": "heun"
                }),
            ),
            (
                "lm_temperature",
                json!({
                    "model": "test/music",
                    "prompt": "ambiguous temperature",
                    "lm_temperature": 0.85,
                    "temperature": 0.7
                }),
            ),
            (
                "lm_top_k",
                json!({
                    "model": "test/music",
                    "prompt": "ambiguous top k",
                    "lm_top_k": 0,
                    "top_k": 20
                }),
            ),
            (
                "lm_top_p",
                json!({
                    "model": "test/music",
                    "prompt": "ambiguous top p",
                    "lm_top_p": 0.9,
                    "top_p": 0.8
                }),
            ),
        ] {
            let violations = validate_endpoint_request(&contract, &request).unwrap_err();
            assert_eq!(violations[0].path, canonical);
            assert!(violations[0].reason.contains("ambiguous aliases"));
        }

        let independent = json!({
            "model": "test/music",
            "prompt": "pulse",
            "style": "dry",
            "genre": "minimal",
            "tags": "analog"
        });
        assert!(validate_endpoint_request(&contract, &independent).is_ok());
    }

    #[test]
    fn music_catalog_narrowing_applies_after_alias_normalization() {
        let mut contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        contract
            .request_attribute_specs
            .get_mut("prompt")
            .unwrap()
            .max_length = Some(8);
        contract
            .request_attribute_specs
            .get_mut("caption")
            .unwrap()
            .max_length = Some(6);
        contract
            .request_attribute_specs
            .get_mut("bpm")
            .unwrap()
            .maximum = Some(300.0);
        contract.request_attributes.retain(|path| path != "tags");
        contract.request_attribute_specs.remove("tags");

        let narrowed_alias = validate_endpoint_request(
            &contract,
            &json!({"model": "test/music", "caption": "longer than eight"}),
        )
        .unwrap_err();
        assert!(narrowed_alias
            .iter()
            .any(|violation| violation.path == "prompt"));
        let alias_signed_bound = validate_endpoint_request(
            &contract,
            &json!({"model": "test/music", "caption": "1234567"}),
        )
        .unwrap_err();
        assert!(alias_signed_bound
            .iter()
            .any(|violation| violation.path == "caption"));
        let narrowed_bpm = validate_endpoint_request(
            &contract,
            &json!({"model": "test/music", "prompt": "valid", "bpm": 301}),
        )
        .unwrap_err();
        assert!(narrowed_bpm.iter().any(|violation| violation.path == "bpm"));
        let unsupported_alias = validate_endpoint_request(
            &contract,
            &json!({"model": "test/music", "prompt": "valid", "tags": "ambient"}),
        )
        .unwrap_err();
        assert!(unsupported_alias
            .iter()
            .any(|violation| violation.path == "tags"));
    }

    #[test]
    fn music_inline_audio_is_closed_typed_and_bounded_after_decoding() {
        let contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        let arbitrary_audio = base64::engine::general_purpose::STANDARD
            .encode(vec![0x5a_u8; MIN_MUSIC_INLINE_AUDIO_BYTES]);
        let valid = json!({
            "model": "test/music",
            "prompt": "transform arbitrary bytes",
            "task_type": "cover",
            "source_audio": {
                "data": arbitrary_audio,
                "encoding": "base64",
                "content_type": "audio/wav"
            }
        });
        assert!(validate_endpoint_request(&contract, &valid).is_ok());
        assert_eq!(
            validate_base64_decoded_size(
                valid["source_audio"]["data"].as_str().unwrap(),
                MIN_MUSIC_INLINE_AUDIO_BYTES as u64
            ),
            Ok(MIN_MUSIC_INLINE_AUDIO_BYTES as u64),
            "audio validation must not require a codec magic header"
        );
        assert!(valid_audio_content_type("audio/mp3"));
        assert!(valid_audio_content_type("audio/m4a"));
        assert!(!valid_audio_content_type("audio/x-flac"));
        assert!(!valid_audio_content_type("audio/x-m4a"));
        assert!(validate_base64_decoded_size("AAECAw==", 3)
            .unwrap_err()
            .contains("decoded inline audio size 4"));

        for (request, path) in [
            (
                json!({
                    "model": "test/music",
                    "prompt": "missing metadata",
                    "source_audio": {"data": "AA==", "encoding": "base64"}
                }),
                "source_audio.content_type",
            ),
            (
                json!({
                    "model": "test/music",
                    "prompt": "malformed",
                    "source_audio": {
                        "data": "A===",
                        "encoding": "base64",
                        "content_type": "audio/wav"
                    }
                }),
                "source_audio.data",
            ),
            (
                json!({
                    "model": "test/music",
                    "prompt": "unsupported audio subtype",
                    "source_audio": {
                        "data": "AA==",
                        "encoding": "base64",
                        "content_type": "audio/x-arbitrary"
                    }
                }),
                "source_audio.content_type",
            ),
            (
                json!({
                    "model": "test/music",
                    "prompt": "closed object",
                    "source_audio": {
                        "data": "AA==",
                        "encoding": "base64",
                        "content_type": "audio/wav",
                        "path": "/tmp/input.wav"
                    }
                }),
                "source_audio.path",
            ),
            (
                json!({
                    "model": "test/music",
                    "prompt": "no string paths",
                    "source_audio": "/tmp/input.wav"
                }),
                "source_audio",
            ),
            (
                json!({
                    "model": "test/music",
                    "prompt": "no path fields",
                    "source_audio_path": "/tmp/input.wav"
                }),
                "source_audio_path",
            ),
        ] {
            let violations = validate_endpoint_request(&contract, &request).unwrap_err();
            assert!(
                violations.iter().any(|violation| violation.path == path),
                "{path}: {violations:?}"
            );
        }
    }

    #[test]
    fn music_inline_audio_load_measures_every_validated_input() {
        let encoded =
            endpoint_calibration_wav_base64_fixture(MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize)
                .unwrap();
        let request = json!({
            "source_audio": {
                "data": encoded,
                "encoding": "base64",
                "content_type": "audio/wav"
            },
            "reference_audio": {
                "data": encoded,
                "encoding": "base64",
                "content_type": "audio/wav"
            }
        });
        let load = artifact_generation_inline_audio_load(&request).unwrap();
        assert_eq!(load.item_count, 2);
        assert_eq!(
            load.max_item_bytes,
            u64::try_from(MIN_MUSIC_INLINE_AUDIO_BYTES).unwrap()
        );
        assert_eq!(load.max_item_seconds, 1);

        let mut wrong_mime = request.clone();
        wrong_mime["source_audio"]["content_type"] = json!("audio/flac");
        assert!(artifact_generation_inline_audio_load(&wrong_mime)
            .unwrap_err()
            .contains("does not match"));

        let mut garbage = request;
        garbage["source_audio"]["data"] = json!(base64::engine::general_purpose::STANDARD
            .encode(vec![0x5a_u8; MIN_MUSIC_INLINE_AUDIO_BYTES]));
        assert!(artifact_generation_inline_audio_load(&garbage)
            .unwrap_err()
            .contains("valid bounded audio"));
    }

    #[test]
    fn music_custom_timesteps_require_bounded_numeric_items() {
        let contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        assert!(validate_endpoint_request(
            &contract,
            &json!({
                "model": "test/music",
                "prompt": "valid schedule",
                "custom_timesteps": [1, 0.5, 0]
            })
        )
        .is_ok());
        let wrong_item = validate_endpoint_request(
            &contract,
            &json!({
                "model": "test/music",
                "prompt": "invalid schedule",
                "timesteps": [1.0, "0.5", 0.0]
            }),
        )
        .unwrap_err();
        assert!(wrong_item
            .iter()
            .any(|violation| violation.path == "custom_timesteps[1]"));
        let unsafe_item = validate_endpoint_request(
            &contract,
            &json!({
                "model": "test/music",
                "prompt": "invalid schedule",
                "custom_timesteps": [1.01]
            }),
        )
        .unwrap_err();
        assert!(unsafe_item
            .iter()
            .any(|violation| violation.path == "custom_timesteps[0]"));
    }

    #[test]
    fn music_enums_ranges_and_composed_caption_budget_are_enforced() {
        let contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        for (path, accepted, rejected) in [
            ("task_type", json!("cover"), json!("flow-edit")),
            ("time_signature", json!("4/4"), json!("7/8")),
            ("infer_method", json!("sde"), json!("deterministic")),
            ("sampler", json!("heun"), json!("rk4")),
            ("dcw_mode", json!("double"), json!("triple")),
            ("dcw_wavelet", json!("coif2"), json!("coif3")),
            ("repaint_mode", json!("aggressive"), json!("maximum")),
            ("chunk_mask_mode", json!("explicit"), json!("masked")),
            ("response_format", json!("wav32"), json!("pcm")),
            ("mp3_bitrate", json!("320k"), json!("96k")),
            ("mp3_sample_rate", json!(44_100), json!(96_000)),
        ] {
            let spec = &contract.request_attribute_specs[path];
            assert!(
                validate_endpoint_attribute_value(spec, &accepted).is_ok(),
                "{path}"
            );
            assert!(
                validate_endpoint_attribute_value(spec, &rejected).is_err(),
                "{path}"
            );
        }
        for (path, minimum, maximum) in [
            ("bpm", json!(30), json!(300)),
            ("duration_seconds", json!(10), json!(600)),
            ("steps", json!(1), json!(200)),
            ("guidance_scale", json!(1), json!(15)),
            ("lm_temperature", json!(0), json!(2)),
            ("lm_cfg_scale", json!(1), json!(3)),
            ("velocity_norm_threshold", json!(0), json!(5)),
            ("velocity_ema_factor", json!(0), json!(0.5)),
            ("dcw_scaler", json!(0), json!(0.1)),
            ("normalization_db", json!(-10), json!(0)),
            ("fade_in_duration", json!(0), json!(10)),
            ("latent_shift", json!(-0.2), json!(0.2)),
            ("latent_rescale", json!(0.5), json!(1.5)),
            ("retake_variance", json!(0), json!(1)),
            ("flow_edit_n_avg", json!(1), json!(8)),
        ] {
            let spec = &contract.request_attribute_specs[path];
            assert!(
                validate_endpoint_attribute_value(spec, &minimum).is_ok(),
                "{path} minimum"
            );
            assert!(
                validate_endpoint_attribute_value(spec, &maximum).is_ok(),
                "{path} maximum"
            );
        }

        let within_budget = json!({
            "model": "test/music",
            "prompt": "p".repeat(480),
            "style": "s".repeat(4),
            "genre": "g".repeat(4),
            "tags": "t"
        });
        assert!(validate_endpoint_request(&contract, &within_budget).is_ok());
        let over_budget = json!({
            "model": "test/music",
            "prompt": "p".repeat(480),
            "style": "s".repeat(4),
            "genre": "g".repeat(4),
            "tags": "tt"
        });
        let violations = validate_endpoint_request(&contract, &over_budget).unwrap_err();
        assert!(violations.iter().any(|violation| {
            violation.path == "prompt" && violation.reason.contains("composed")
        }));
    }

    #[test]
    fn music_audio_codes_and_seed_arrays_follow_pinned_source_limits() {
        let contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        assert_eq!(
            validate_music_audio_codes("<|audio_code_0|><|audio_code_63999|>"),
            Ok(2)
        );
        assert!(validate_music_audio_codes("<|audio_code_64000|>")
            .unwrap_err()
            .contains("pinned parser maximum"));
        assert!(validate_music_audio_codes("prefix<|audio_code_1|>").is_err());
        let maximum_codes = "<|audio_code_63999|>".repeat(MAX_MUSIC_AUDIO_CODE_COUNT as usize);
        assert!(u64::try_from(maximum_codes.len()).unwrap() <= MAX_MUSIC_AUDIO_CODES_CHARS);
        assert_eq!(
            validate_music_audio_codes(&maximum_codes),
            Ok(MAX_MUSIC_AUDIO_CODE_COUNT)
        );
        assert!(
            validate_music_audio_codes(&format!("{maximum_codes}<|audio_code_0|>"))
                .unwrap_err()
                .contains("encoded maximum")
        );

        assert!(validate_endpoint_request(
            &contract,
            &json!({
                "model": "test/music",
                "prompt": "seeded",
                "task_type": "cover",
                "audio_codes": "<|audio_code_1|>",
                "n": 3,
                "seeds": [-1, 0, 7]
            })
        )
        .is_ok());
        assert!(validate_endpoint_request(
            &contract,
            &json!({
                "model": "test/music",
                "prompt": "per-item code hints",
                "task_type": "cover",
                "audio_codes": [
                    "<|audio_code_1|><|audio_code_2|>",
                    "<|audio_code_3|><|audio_code_4|>"
                ],
                "n": 2
            })
        )
        .is_ok());
        for invalid in [
            json!({
                "model": "test/music",
                "prompt": "wrong code batch",
                "task_type": "cover",
                "audio_codes": ["<|audio_code_1|>"],
                "n": 2
            }),
            json!({
                "model": "test/music",
                "prompt": "unequal code durations",
                "task_type": "cover",
                "audio_codes": [
                    "<|audio_code_1|>",
                    "<|audio_code_2|><|audio_code_3|>"
                ],
                "n": 2
            }),
            json!({
                "model": "test/music",
                "prompt": "wrong code type",
                "task_type": "cover",
                "audio_codes": [7, 8],
                "n": 2
            }),
        ] {
            let violations = validate_endpoint_request(&contract, &invalid).unwrap_err();
            assert!(violations
                .iter()
                .any(|violation| violation.path == "audio_codes"));
        }
        for invalid in [
            json!([1.5]),
            json!(["7"]),
            json!([-2]),
            json!([4_294_967_296_u64]),
        ] {
            let violations = validate_endpoint_request(
                &contract,
                &json!({"model": "test/music", "prompt": "seeded", "n": 1, "seeds": invalid}),
            )
            .unwrap_err();
            assert!(violations
                .iter()
                .any(|violation| violation.path == "seeds[0]"));
        }

        for request in [
            json!({
                "model": "test/music",
                "prompt": "ambiguous seed",
                "n": 1,
                "seed": 7,
                "seeds": [7]
            }),
            json!({
                "model": "test/music",
                "prompt": "wrong seed count",
                "n": 2,
                "seeds": [7]
            }),
        ] {
            let violations = validate_endpoint_request(&contract, &request).unwrap_err();
            assert!(violations.iter().any(|violation| violation.path == "seeds"));
        }

        let normalized = materialize_endpoint_request_defaults(
            &contract,
            &json!({
                "model": "test/music",
                "prompt": "per-item seeds",
                "n": 2,
                "seeds": [7, 11]
            }),
        )
        .unwrap();
        assert!(normalized.get("seed").is_none());
        assert!(validate_endpoint_request(&contract, &normalized).is_ok());
    }

    #[test]
    fn music_extensions_do_not_change_other_endpoint_families() {
        let audio = endpoint_family_contract_template(ENDPOINT_MAYHEM_AUDIO_GENERATIONS)
            .expect("audio endpoint contract");
        assert!(!audio.request_attributes.iter().any(|path| path == "lyrics"));
        assert!(validate_endpoint_request(
            &audio,
            &json!({
                "model": "test/audio",
                "prompt": "rain",
                "input_audio": {"data": "not-decoded-here", "format": "wav"}
            })
        )
        .is_ok());

        let chat = endpoint_family_contract_template(ENDPOINT_OPENAI_CHAT_COMPLETIONS)
            .expect("chat endpoint contract");
        assert!(validate_endpoint_request(
            &chat,
            &json!({
                "model": "test/chat",
                "messages": [{"role": "user", "content": "hello"}]
            })
        )
        .is_ok());
    }

    #[test]
    fn selected_model_contract_rejects_unknown_and_unsupported_attributes() {
        let mut contract = endpoint_family_contract_template(ENDPOINT_OPENAI_CHAT_COMPLETIONS)
            .expect("chat contract");
        contract
            .request_attributes
            .retain(|attribute| attribute != "top_k");
        contract.request_attribute_specs.remove("top_k");
        assert!(validate_endpoint_request(
            &contract,
            &json!({"model":"test", "messages":[{"role":"user","content":"hello"}]})
        )
        .is_ok());
        let unsupported = validate_endpoint_request(
            &contract,
            &json!({
                "model":"test",
                "messages":[{"role":"user","content":"hello"}],
                "top_k":20
            }),
        )
        .unwrap_err();
        assert_eq!(unsupported[0].path, "top_k");
        let unknown = validate_endpoint_request(
            &contract,
            &json!({
                "model":"test",
                "messages":[{"role":"user","content":"hello"}],
                "provider_native":true
            }),
        )
        .unwrap_err();
        assert_eq!(unknown[0].path, "provider_native");
    }

    #[test]
    fn nested_hf_parameters_are_closed_and_typed() {
        let contract = endpoint_family_contract_template(ENDPOINT_HF_TEXT_TO_IMAGE)
            .expect("HF image contract");
        assert!(validate_endpoint_request(
            &contract,
            &json!({
                "inputs":"draw a square",
                "parameters":{"width":512,"height":512,"guidance_scale":1.0}
            })
        )
        .is_ok());
        let unknown = validate_endpoint_request(
            &contract,
            &json!({"inputs":"draw a square", "parameters":{"magic":true}}),
        )
        .unwrap_err();
        assert_eq!(unknown[0].path, "parameters.magic");
        let wrong_type = validate_endpoint_request(
            &contract,
            &json!({"inputs":"draw a square", "parameters":{"width":"512"}}),
        )
        .unwrap_err();
        assert_eq!(wrong_type[0].path, "parameters.width");
    }

    #[test]
    fn audio_file_calibration_descriptor_uses_fixture_metadata() {
        let contract = endpoint_family_contract_template(ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS)
            .expect("audio transcription contract");
        let descriptor = contract.request_attribute_specs["file"].calibration_values[0].clone();
        let substitutions = BTreeMap::from([
            ("$AUDIO_BYTES".to_owned(), json!(12_345)),
            ("$AUDIO_BLAKE3".to_owned(), json!("11".repeat(32))),
            ("$AUDIO_CONTENT_TYPE".to_owned(), json!("audio/flac")),
            ("$AUDIO_FILENAME".to_owned(), json!("calibration.flac")),
        ]);
        let descriptor = substitute_calibration_markers(descriptor, &substitutions);

        assert_eq!(descriptor["bytes"], json!(12_345));
        assert_eq!(descriptor["blake3"], json!("11".repeat(32)));
        assert_eq!(descriptor["content_type"], "audio/flac");
        assert_eq!(descriptor["filename"], "calibration.flac");
    }

    #[test]
    fn array_enum_calibration_cases_remain_arrays() {
        let contract = endpoint_family_contract_template(ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS)
            .expect("audio transcription contract");
        let substitutions = BTreeMap::from([
            ("$MODEL".to_owned(), json!("test/model")),
            ("$AUDIO_BYTES".to_owned(), json!(44)),
            ("$AUDIO_BLAKE3".to_owned(), json!("11".repeat(32))),
            ("$AUDIO_CONTENT_TYPE".to_owned(), json!("audio/wav")),
            ("$AUDIO_FILENAME".to_owned(), json!("calibration.wav")),
        ]);
        let cases = generate_endpoint_calibration_cases(&contract).expect("calibration matrix");
        let accepted = cases
            .iter()
            .filter(|case| {
                case.expect_accept
                    && case
                        .attributes
                        .contains(&"timestamp_granularities".to_owned())
            })
            .collect::<Vec<_>>();
        assert!(!accepted.is_empty());
        for case in accepted {
            let request = materialize_endpoint_calibration_request(case, &substitutions)
                .expect("materialized request");
            assert!(
                validate_endpoint_request(&contract, &request).is_ok(),
                "{} produced {}",
                case.case_id,
                request["timestamp_granularities"]
            );
        }
    }

    #[test]
    fn conditional_stt_response_cases_enable_their_request_controls() {
        let substitutions = BTreeMap::from([
            ("$MODEL".to_owned(), json!("test/model")),
            ("$AUDIO_BYTES".to_owned(), json!(44)),
            ("$AUDIO_BLAKE3".to_owned(), json!("11".repeat(32))),
            ("$AUDIO_CONTENT_TYPE".to_owned(), json!("audio/wav")),
            ("$AUDIO_FILENAME".to_owned(), json!("calibration.wav")),
            ("$AUDIO_BASE64".to_owned(), json!("UklGRg==")),
        ]);

        let openai = endpoint_family_contract_template(ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS)
            .expect("audio transcription contract");
        let openai_cases = generate_endpoint_calibration_cases(&openai).expect("OpenAI STT matrix");
        for (response_attribute, expected_granularity) in [
            ("task", None),
            ("language", None),
            ("duration", None),
            ("words", Some("word")),
            ("segments", Some("segment")),
        ] {
            let case = openai_cases
                .iter()
                .find(|case| {
                    case.case_kind == "response_attribute"
                        && case
                            .expected_response_attributes
                            .contains(&response_attribute.to_owned())
                })
                .expect("conditional OpenAI response row");
            let request = materialize_endpoint_calibration_request(case, &substitutions)
                .expect("materialized OpenAI response row");
            assert_eq!(request["response_format"], "verbose_json");
            if let Some(granularity) = expected_granularity {
                assert_eq!(request["timestamp_granularities"], json!([granularity]));
            }
            assert!(validate_endpoint_request(&openai, &request).is_ok());
        }

        let hf = endpoint_family_contract_template(ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION)
            .expect("HF ASR contract");
        let hf_case = generate_endpoint_calibration_cases(&hf)
            .expect("HF ASR matrix")
            .into_iter()
            .find(|case| {
                case.case_kind == "response_attribute"
                    && case
                        .expected_response_attributes
                        .contains(&"chunks".to_owned())
            })
            .expect("HF chunks response row");
        let request = materialize_endpoint_calibration_request(&hf_case, &substitutions)
            .expect("materialized HF chunks row");
        assert_eq!(request["parameters"]["return_timestamps"], json!("word"));
        assert!(validate_endpoint_request(&hf, &request).is_ok());
    }

    #[test]
    fn generated_matrix_covers_every_declared_attribute_and_interaction() {
        for family in [
            ENDPOINT_OPENAI_CHAT_COMPLETIONS,
            ENDPOINT_OPENAI_COMPLETIONS,
            ENDPOINT_OPENAI_RESPONSES,
            ENDPOINT_HF_MULTIMODAL_CHAT,
            ENDPOINT_OPENAI_EMBEDDINGS,
            ENDPOINT_HF_FEATURE_EXTRACTION,
            ENDPOINT_OPENAI_IMAGE_GENERATIONS,
            ENDPOINT_HF_TEXT_TO_IMAGE,
            ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS,
            ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION,
            ENDPOINT_OPENAI_AUDIO_SPEECH,
            ENDPOINT_HF_TEXT_TO_SPEECH,
            ENDPOINT_OPENAI_VIDEOS,
            ENDPOINT_HF_TEXT_TO_VIDEO,
            ENDPOINT_MAYHEM_AUDIO_GENERATIONS,
            ENDPOINT_MAYHEM_MUSIC_GENERATIONS,
            ENDPOINT_HF_TEXT_TO_AUDIO,
        ] {
            let contract = endpoint_family_contract_template(family).unwrap();
            let cases = generate_endpoint_calibration_cases(&contract)
                .unwrap_or_else(|err| panic!("matrix generation failed for {family}: {err}"));
            assert!(!cases.is_empty());
            for attribute in &contract.request_attributes {
                assert!(
                    cases.iter().any(|case| {
                        case.attributes.contains(attribute) && case.case_kind == "wrong_type"
                    }),
                    "{family}/{attribute} has no wrong-type row"
                );
                assert!(
                    cases.iter().any(|case| {
                        case.attributes.contains(attribute)
                            && matches!(
                                case.case_kind.as_str(),
                                "required_present" | "omitted_default" | "omitted_optional"
                            )
                    }),
                    "{family}/{attribute} has no presence/default row"
                );
            }
            for attribute in &contract.response_attributes {
                assert!(
                    cases
                        .iter()
                        .any(|case| { case.expected_response_attributes.contains(attribute) }),
                    "{family}/{attribute} has no response row"
                );
            }
            for group in &contract.interaction_groups {
                for left in 0..group.len() {
                    for right in (left + 1)..group.len() {
                        assert!(
                            cases.iter().any(|case| {
                                case.case_kind == "pairwise_interaction"
                                    && case.attributes.contains(&group[left])
                                    && case.attributes.contains(&group[right])
                            }),
                            "{family} missing pairwise row for {} + {}",
                            group[left],
                            group[right]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn chat_matrix_requires_roles_and_materializes_media_companions() {
        let contract = endpoint_family_contract_template(ENDPOINT_HF_MULTIMODAL_CHAT)
            .expect("HF multimodal chat contract");
        assert!(contract
            .required_request_attributes
            .contains(&"messages.role".to_owned()));
        assert!(contract
            .request_attributes
            .contains(&"parallel_tool_calls".to_owned()));
        assert!(validate_endpoint_request(
            &contract,
            &json!({
                "model": "test/model",
                "messages": [{ "role": "user", "content": "use both tools" }],
                "parallel_tool_calls": true
            }),
        )
        .is_ok());
        let cases = generate_endpoint_calibration_cases(&contract).expect("calibration matrix");
        let substitutions = BTreeMap::from([
            ("$MODEL".to_owned(), json!("test/model")),
            (
                "$IMAGE_DATA_URL".to_owned(),
                json!("data:image/png;base64,aGVsbG8="),
            ),
            ("$AUDIO_BASE64".to_owned(), json!("aGVsbG8=")),
            ("$VIDEO_BASE64".to_owned(), json!("aGVsbG8=")),
        ]);

        let missing_role = cases
            .iter()
            .find(|case| {
                case.case_kind == "required_missing"
                    && case.attributes == vec!["messages.role".to_owned()]
            })
            .expect("missing-role rejection row");
        let request = materialize_endpoint_calibration_request(missing_role, &substitutions)
            .expect("missing-role request");
        assert!(validate_endpoint_request(&contract, &request).is_err());

        let video = cases
            .iter()
            .find(|case| {
                case.case_kind.starts_with("accepted_value_")
                    && case.mutations.iter().any(|mutation| {
                        mutation.path == "messages.content.type"
                            && mutation.value
                                == (EndpointCalibrationValue::Literal {
                                    value: json!("video"),
                                })
                    })
            })
            .expect("video type row");
        let request =
            materialize_endpoint_calibration_request(video, &substitutions).expect("video request");
        assert_eq!(
            request["messages"][0]["content"][0]["video"]["frames"]
                .as_array()
                .expect("decoded video frames")
                .len(),
            1
        );
        assert_eq!(
            request["messages"][0]["content"][0]["video"]["num_frames"],
            1
        );
        assert_eq!(request["messages"][0]["role"], "user");
        assert!(validate_endpoint_request(&contract, &request).is_ok());
    }

    #[test]
    fn chat_matrix_uses_model_contract_video_fps_for_companions() {
        let mut contract = endpoint_family_contract_template(ENDPOINT_HF_MULTIMODAL_CHAT)
            .expect("HF multimodal chat contract");
        let fps = contract
            .request_attribute_specs
            .get_mut("messages.content.video.fps")
            .expect("video fps spec");
        fps.minimum = Some(1.0);
        fps.maximum = Some(1.0);
        fps.calibration_values = vec![json!(1.0)];
        let num_frames = contract
            .request_attribute_specs
            .get_mut("messages.content.video.num_frames")
            .expect("video frame-count spec");
        num_frames.minimum = Some(4.0);
        num_frames.maximum = Some(64.0);
        num_frames.calibration_values = vec![json!(4)];
        let frames = contract
            .request_attribute_specs
            .get_mut("messages.content.video.frames")
            .expect("decoded video-frames spec");
        frames.min_items = Some(4);
        frames.max_items = Some(64);
        frames.calibration_values = vec![Value::Array(vec![json!("$IMAGE_DATA_URL"); 4])];

        let cases = generate_endpoint_calibration_cases(&contract).expect("calibration matrix");
        let substitutions = BTreeMap::from([
            ("$MODEL".to_owned(), json!("test/model")),
            (
                "$IMAGE_DATA_URL".to_owned(),
                json!("data:image/png;base64,aGVsbG8="),
            ),
            ("$AUDIO_BASE64".to_owned(), json!("aGVsbG8=")),
            ("$VIDEO_BASE64".to_owned(), json!("aGVsbG8=")),
        ]);
        let video = cases
            .iter()
            .find(|case| {
                case.case_kind.starts_with("accepted_value_")
                    && case.mutations.iter().any(|mutation| {
                        mutation.path == "messages.content.type"
                            && mutation.value
                                == (EndpointCalibrationValue::Literal {
                                    value: json!("video"),
                                })
                    })
            })
            .expect("video type row");
        let request =
            materialize_endpoint_calibration_request(video, &substitutions).expect("video request");

        assert_eq!(
            request["messages"][0]["content"][0]["video"]["fps"],
            json!(1.0)
        );
        assert_eq!(
            request["messages"][0]["content"][0]["video"]["frames"]
                .as_array()
                .expect("video frames")
                .len(),
            4
        );
        assert_eq!(
            request["messages"][0]["content"][0]["video"]["num_frames"],
            json!(4)
        );
        assert!(validate_endpoint_request(&contract, &request).is_ok());

        let frame_boundaries = cases
            .iter()
            .filter(|case| {
                matches!(
                    case.case_kind.as_str(),
                    "minimum_length_valid" | "maximum_length_valid"
                ) && case
                    .mutations
                    .iter()
                    .any(|mutation| mutation.path == "messages.content.video.frames")
            })
            .collect::<Vec<_>>();
        assert_eq!(frame_boundaries.len(), 2);
        for case in frame_boundaries {
            let request = materialize_endpoint_calibration_request(case, &substitutions)
                .expect("video frame boundary request");
            let video = &request["messages"][0]["content"][0]["video"];
            let frame_count = video["frames"].as_array().expect("video frames").len();
            assert_eq!(request["messages"][0]["content"][0]["type"], "video");
            assert_eq!(video["num_frames"], json!(frame_count));
            assert_eq!(video["fps"], json!(1.0));
            assert!(validate_endpoint_request(&contract, &request).is_ok());
        }
    }

    #[test]
    fn chat_video_companion_count_honors_signed_decoded_frame_minimum() {
        let mut contract = endpoint_family_contract_template(ENDPOINT_HF_MULTIMODAL_CHAT)
            .expect("HF multimodal chat contract");
        let num_frames = contract
            .request_attribute_specs
            .get_mut("messages.content.video.num_frames")
            .expect("video frame-count spec");
        num_frames.minimum = Some(4.0);
        num_frames.maximum = Some(64.0);
        num_frames.calibration_values = vec![json!(4)];
        let frames = contract
            .request_attribute_specs
            .get_mut("messages.content.video.frames")
            .expect("decoded video-frames spec");
        frames.min_items = Some(8);
        frames.max_items = Some(64);
        frames.calibration_values = vec![Value::Array(vec![json!("$IMAGE_DATA_URL"); 8])];

        assert_eq!(
            endpoint_calibration_video_frame_count(&contract),
            8,
            "decoded-frame min_items must remain authoritative when it is stricter"
        );
    }

    #[test]
    fn music_matrix_materializes_requests_matching_each_expected_verdict() {
        let contract = endpoint_family_contract_template(ENDPOINT_MAYHEM_MUSIC_GENERATIONS)
            .expect("music endpoint contract");
        let cases = generate_endpoint_calibration_cases(&contract).expect("calibration matrix");
        let substitutions = BTreeMap::from([
            ("$MODEL".to_owned(), json!("test/music")),
            (
                "$AUDIO_BASE64".to_owned(),
                json!(endpoint_calibration_wav_base64_fixture(
                    MIN_MUSIC_INLINE_AUDIO_BASE64_CHARS as usize
                )
                .unwrap()),
            ),
            ("$AUDIO_CONTENT_TYPE".to_owned(), json!("audio/wav")),
        ]);

        for case in cases {
            let request = materialize_endpoint_calibration_request(&case, &substitutions)
                .unwrap_or_else(|error| panic!("{} materialization failed: {error}", case.case_id));
            let verdict = validate_endpoint_request(&contract, &request);
            let accepted = verdict.is_ok();
            let detail = verdict
                .err()
                .map(|violations| {
                    violations
                        .into_iter()
                        .map(|violation| format!("{}: {}", violation.path, violation.reason))
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_default();
            assert_eq!(
                accepted, case.expect_accept,
                "{} ({}) produced the wrong verdict: {detail}",
                case.case_id, case.case_kind,
            );
        }
    }
}
