use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::{
    EndpointAttributeSpec, EndpointFamilyContract, EndpointValueType,
    ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION, ENDPOINT_HF_FEATURE_EXTRACTION,
    ENDPOINT_HF_MULTIMODAL_CHAT, ENDPOINT_HF_TEXT_TO_AUDIO, ENDPOINT_HF_TEXT_TO_IMAGE,
    ENDPOINT_HF_TEXT_TO_SPEECH, ENDPOINT_HF_TEXT_TO_VIDEO, ENDPOINT_MAYHEM_AUDIO_GENERATIONS,
    ENDPOINT_MAYHEM_MUSIC_GENERATIONS, ENDPOINT_OPENAI_AUDIO_SPEECH,
    ENDPOINT_OPENAI_AUDIO_TRANSCRIPTIONS, ENDPOINT_OPENAI_CHAT_COMPLETIONS,
    ENDPOINT_OPENAI_COMPLETIONS, ENDPOINT_OPENAI_EMBEDDINGS, ENDPOINT_OPENAI_IMAGE_GENERATIONS,
    ENDPOINT_OPENAI_RESPONSES, ENDPOINT_OPENAI_VIDEOS,
};

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
            ],
            &["inputs"],
            &["audio", "content_type", "usage", "mayhem"],
        ),
        ENDPOINT_OPENAI_VIDEOS => (
            &["model", "prompt", "input_reference", "seconds", "size"],
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
                "parameters.guidance_scale",
                "parameters.negative_prompt",
                "parameters.num_inference_steps",
                "parameters.seed",
                "parameters.width",
                "parameters.height",
                "parameters.fps",
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
                "melody",
                "duration_seconds",
                "response_format",
                "temperature",
                "top_k",
                "top_p",
                "typical_p",
                "guidance_scale",
                "seed",
                "do_sample",
                "max_new_tokens",
            ],
            &["model", "prompt"],
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
        ENDPOINT_OPENAI_AUDIO_SPEECH => &[&[
            "voice",
            "response_format",
            "speed",
            "instructions",
            "stream_format",
        ]],
        ENDPOINT_OPENAI_VIDEOS => &[&["input_reference", "seconds", "size"]],
        ENDPOINT_HF_TEXT_TO_VIDEO => &[
            &["parameters.num_frames", "parameters.num_inference_steps"],
            &[
                "parameters.guidance_scale",
                "parameters.negative_prompt",
                "parameters.seed",
            ],
        ],
        ENDPOINT_MAYHEM_AUDIO_GENERATIONS | ENDPOINT_MAYHEM_MUSIC_GENERATIONS => &[&[
            "temperature",
            "top_k",
            "top_p",
            "typical_p",
            "do_sample",
            "max_new_tokens",
        ]],
        ENDPOINT_HF_TEXT_TO_AUDIO
        | ENDPOINT_HF_TEXT_TO_SPEECH
        | ENDPOINT_HF_AUTOMATIC_SPEECH_RECOGNITION => &[&[
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
                array_spec(1, 32, json!(["blur"]))
            } else {
                string_spec(0, 32_000, json!("blur"))
            }
        }
        "input_audio" | "melody" | "parameters.audio" => object_spec(json!({
            "data": "$AUDIO_BASE64",
            "format": "wav"
        })),
        "input_reference" => union_spec(
            &[EndpointValueType::String, EndpointValueType::Object],
            &[json!("$IMAGE_FILE"), json!({"image_url":"$IMAGE_DATA_URL"})],
        ),
        "voice" => union_spec(
            &[EndpointValueType::String, EndpointValueType::Object],
            &[json!("$VOICE"), json!({"id":"$VOICE"})],
        ),
        "parameters.voice" | "parameters.speaker_id" | "scheduler" | "parameters.scheduler" => {
            string_spec(1, 256, json!("$MODEL_VALUE"))
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
        "messages.content.video.num_frames" | "parameters.num_frames" => {
            integer_spec(1.0, 4096.0, 16)
        }
        "messages.content.video.fps" | "parameters.fps" => number_spec(0.01, 240.0, 8.0),
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
        "size" if family == ENDPOINT_OPENAI_VIDEOS => enum_spec(
            Some(json!("720x1280")),
            &[
                json!("720x1280"),
                json!("1280x720"),
                json!("1024x1792"),
                json!("1792x1024"),
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
        "parameters.width" | "parameters.height" if family == ENDPOINT_HF_TEXT_TO_IMAGE => {
            integer_spec(64.0, 2_048.0, 1_024)
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
                vec![EndpointCalibrationMutation {
                    path: path.clone(),
                    value: EndpointCalibrationValue::Omitted,
                }],
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
                set_endpoint_path(&mut request, &mutation.path, json!("x".repeat(length)))?;
            }
            EndpointCalibrationValue::ArrayLength { length, item } => {
                let length = usize::try_from(*length).map_err(|_| {
                    format!("array fixture for {} exceeds this platform", mutation.path)
                })?;
                let item = substitute_calibration_markers(item.clone(), substitutions);
                set_endpoint_path(
                    &mut request,
                    &mutation.path,
                    Value::Array(vec![item; length]),
                )?;
            }
        }
    }
    Ok(request)
}

pub fn materialize_endpoint_request_defaults(
    contract: &EndpointFamilyContract,
    request: &Value,
) -> Result<Value, String> {
    let image_aliases = if contract.family == ENDPOINT_OPENAI_IMAGE_GENERATIONS {
        let object = request
            .as_object()
            .ok_or_else(|| "endpoint request must be an object".to_owned())?;
        let has_size = object.contains_key("size");
        let has_width = object.contains_key("width");
        let has_height = object.contains_key("height");
        if has_size && (has_width || has_height) {
            return Err("image request cannot combine size with width/height".to_owned());
        }
        if has_width != has_height {
            return Err("image request width and height must be supplied together".to_owned());
        }
        Some((has_size, has_width))
    } else {
        None
    };
    let mut normalized = request.clone();
    for path in &contract.request_attributes {
        if image_aliases.is_some_and(|(has_size, has_dimensions)| {
            (has_size && matches!(path.as_str(), "width" | "height"))
                || (has_dimensions && path == "size")
        }) {
            continue;
        }
        if !endpoint_values_at_path(&normalized, path).is_empty() {
            continue;
        }
        let Some(default) = contract
            .request_attribute_specs
            .get(path)
            .and_then(|spec| spec.default.clone())
        else {
            continue;
        };
        set_endpoint_path(&mut normalized, path, default)?;
    }
    if image_aliases.is_some() {
        let object = normalized
            .as_object()
            .ok_or_else(|| "normalized endpoint request must be an object".to_owned())?;
        let has_size = object.contains_key("size");
        let has_width = object.contains_key("width");
        let has_height = object.contains_key("height");
        if has_width != has_height || has_size == has_width {
            return Err(
                "signed image endpoint contract must resolve exactly one of size or width/height"
                    .to_owned(),
            );
        }
    }
    Ok(normalized)
}

fn endpoint_calibration_baseline(spec: &EndpointAttributeSpec) -> Option<Value> {
    spec.default
        .clone()
        .or_else(|| spec.calibration_values.first().cloned())
        .or_else(|| spec.enum_values.first().cloned())
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

fn add_calibration_companion_mutations(
    contract: &EndpointFamilyContract,
    path: &str,
    value: Option<&Value>,
    mutations: &mut Vec<EndpointCalibrationMutation>,
) {
    let mut add = |companion_path: &str, companion_value: Value| {
        if contract
            .request_attributes
            .iter()
            .any(|declared| declared == companion_path)
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
                add(
                    "messages.content.video.frames",
                    json!(["$IMAGE_DATA_URL", "$IMAGE_DATA_URL"]),
                );
                add("messages.content.video.num_frames", json!(2));
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
            add("messages.content.type", json!("video"));
            add("messages.content.video.data", json!("$VIDEO_BASE64"));
            add("messages.content.video.content_type", json!("video/mp4"));
            add("messages.content.video.num_frames", json!(16));
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
            let frame_count = value.and_then(Value::as_u64).unwrap_or(2).min(64) as usize;
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
            add("messages.content.type", json!("video"));
            add(
                "messages.content.video.frames",
                json!(["$IMAGE_DATA_URL", "$IMAGE_DATA_URL"]),
            );
            add("messages.content.video.num_frames", json!(2));
            add(
                "messages.content.video.fps",
                endpoint_calibration_companion_baseline(
                    contract,
                    "messages.content.video.fps",
                    json!(8.0),
                ),
            );
        }
        "width" if contract.family == ENDPOINT_OPENAI_IMAGE_GENERATIONS => add(
            "height",
            endpoint_calibration_companion_baseline(contract, "height", json!(1024)),
        ),
        "height" if contract.family == ENDPOINT_OPENAI_IMAGE_GENERATIONS => add(
            "width",
            endpoint_calibration_companion_baseline(contract, "width", json!(1024)),
        ),
        _ => {}
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
    let mut violations = validate_endpoint_value(
        &contract.request_attributes,
        &contract.required_request_attributes,
        &contract.request_attribute_specs,
        request,
        "request",
    )
    .err()
    .unwrap_or_default();
    if contract.family == ENDPOINT_OPENAI_IMAGE_GENERATIONS {
        validate_openai_image_dimension_aliases(contract, request, &mut violations);
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

fn validate_openai_image_dimension_aliases(
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
    fn attribute_validator_enforces_enum_bounds_and_types() {
        let contract = endpoint_family_contract_template(ENDPOINT_OPENAI_VIDEOS).unwrap();
        let seconds = &contract.request_attribute_specs["seconds"];
        assert!(validate_endpoint_attribute_value(seconds, &json!("8")).is_ok());
        assert!(validate_endpoint_attribute_value(seconds, &json!(8)).is_err());
        assert!(validate_endpoint_attribute_value(seconds, &json!("5")).is_err());

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
            2
        );
        assert_eq!(
            request["messages"][0]["content"][0]["video"]["num_frames"],
            2
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
}
