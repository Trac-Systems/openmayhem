import asyncio
import base64
import copy
import inspect
import io
import json
import math
import os
import queue
import sys
import threading
import wave


protocol_stdout = os.fdopen(os.dup(sys.stdout.fileno()), "w", buffering=1)
os.dup2(sys.stderr.fileno(), sys.stdout.fileno())
sys.stdout = sys.stderr
protocol_stdout_lock = threading.Lock()


engine = None
tokenizer = None
processor = None
ctx_size = 2048
batch_invariant = False
kernel_policy = "auto"
execution_properties = None
event_loop = asyncio.new_event_loop()
asyncio.set_event_loop(event_loop)
request_queue = queue.Queue()
cancelled_requests = set()
active_request_ids = set()
cancelled_requests_lock = threading.Lock()
completed_request_id = 0
generation_multiplexer = None


MAX_KERNEL_BACKEND_LENGTH = 64
MAX_MTP_SPECULATIVE_TOKENS = 32
MAX_EXECUTION_PROBE_SECONDS = 10.0


class RequestCancelled(Exception):
    pass


def register_request(request_id):
    with cancelled_requests_lock:
        active_request_ids.add(int(request_id))


def mark_cancelled(request_id):
    with cancelled_requests_lock:
        request_id = int(request_id)
        if request_id <= completed_request_id and request_id not in active_request_ids:
            return False
        cancelled_requests.add(request_id)
        return True


def finish_request(request_id):
    global completed_request_id
    with cancelled_requests_lock:
        request_id = int(request_id)
        active_request_ids.discard(request_id)
        completed_request_id = max(completed_request_id, request_id)
        completed_cancellations = {
            cancelled_id
            for cancelled_id in cancelled_requests
            if cancelled_id <= completed_request_id
            and cancelled_id not in active_request_ids
        }
        cancelled_requests.difference_update(completed_cancellations)


def request_cancelled(request_id):
    with cancelled_requests_lock:
        return int(request_id) in cancelled_requests


def check_cancelled(request_id):
    if request_cancelled(request_id):
        raise RequestCancelled("engine request cancelled")


async def abort_engine_request(request_id):
    if engine is None:
        return
    abort = getattr(engine, "abort", None)
    if abort is None:
        return
    result = abort(f"mayhem-{int(request_id)}")
    if inspect.isawaitable(result):
        await result


class GenerationMultiplexer:
    def __init__(self, capacity, generate, abort, emit, complete, abort_timeout=2.0):
        capacity = int(capacity)
        if capacity < 1:
            raise ValueError("vLLM generation capacity must be positive")
        self.capacity = capacity
        self._generate = generate
        self._abort = abort
        self._emit = emit
        self._complete = complete
        self._abort_timeout = float(abort_timeout)
        self._semaphore = asyncio.Semaphore(capacity)
        self._tasks = {}
        self._running = set()
        self._engine_running = set()
        self._abort_failures = set()

    def submit(self, request_id, payload):
        request_id = int(request_id)
        existing = self._tasks.get(request_id)
        if existing is not None and not existing.done():
            raise ValueError(f"duplicate active vLLM request id {request_id}")
        register_request(request_id)
        task = asyncio.create_task(
            self._run(request_id, payload), name=f"mayhem-vllm-{request_id}"
        )
        self._tasks[request_id] = task
        return task

    async def cancel(self, request_id):
        request_id = int(request_id)
        task = self._tasks.get(request_id)
        if task is None or task.done():
            return False
        try:
            if request_id in self._engine_running:
                await asyncio.wait_for(
                    self._abort(request_id), timeout=self._abort_timeout
                )
        except Exception:
            self._abort_failures.add(request_id)
        return True

    def engine_started(self, request_id):
        self._engine_running.add(int(request_id))

    def engine_stopped(self, request_id):
        self._engine_running.discard(int(request_id))

    async def drain(self):
        tasks = list(self._tasks.values())
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)

    async def _run(self, request_id, payload):
        try:
            async with self._semaphore:
                check_cancelled(request_id)
                self._running.add(request_id)
                try:
                    # Cancellation is cooperative: the request marker and the
                    # engine abort stop work without abandoning to_thread
                    # preprocessing or releasing this capacity slot early.
                    result = await self._generate(request_id, payload)
                finally:
                    self._running.discard(request_id)
                check_cancelled(request_id)
            self._emit(
                {"id": request_id, "type": "response", "ok": True, "result": result}
            )
        except (RequestCancelled, asyncio.CancelledError) as exc:
            error = str(exc) or "engine request cancelled"
            abort_failed = request_id in self._abort_failures
            message = {
                "id": request_id,
                "type": "response",
                "ok": False,
                "cancelled": True,
                "error": error,
            }
            if abort_failed:
                message["abort_failed"] = True
            self._emit(message)
        except Exception as exc:
            error = str(exc) or repr(exc) or type(exc).__name__
            message = {
                "id": request_id,
                "type": "response",
                "ok": False,
                "error": error,
            }
            if request_id in self._abort_failures:
                message["cancelled"] = True
                message["abort_failed"] = True
            self._emit(message)
        finally:
            self._complete(request_id)
            self._engine_running.discard(request_id)
            self._abort_failures.discard(request_id)
            current = asyncio.current_task()
            if self._tasks.get(request_id) is current:
                self._tasks.pop(request_id, None)


async def cancel_generation_request(request_id):
    multiplexer = generation_multiplexer
    if multiplexer is not None:
        await multiplexer.cancel(request_id)


def schedule_abort(request_id):
    if mark_cancelled(request_id) and event_loop.is_running():
        asyncio.run_coroutine_threadsafe(cancel_generation_request(request_id), event_loop)


def send(message):
    encoded = json.dumps(message, separators=(",", ":")) + "\n"
    with protocol_stdout_lock:
        protocol_stdout.write(encoded)
        protocol_stdout.flush()


def accepted_kwargs(callable_obj, kwargs):
    try:
        signature = inspect.signature(callable_obj)
    except (TypeError, ValueError):
        return kwargs
    parameters = signature.parameters
    if any(param.kind == inspect.Parameter.VAR_KEYWORD for param in parameters.values()):
        return kwargs
    return {key: value for key, value in kwargs.items() if key in parameters}


def required_sampling_kwargs(callable_obj, kwargs, requested):
    accepted = accepted_kwargs(callable_obj, kwargs)
    missing = sorted(name for name in requested if name not in accepted)
    if missing:
        raise ValueError(
            "vLLM backend does not support requested sampling parameter(s): "
            + ", ".join(missing)
        )
    return accepted


def required_engine_kwargs(callable_obj, kwargs, required):
    accepted = accepted_kwargs(callable_obj, kwargs)
    missing = sorted(name for name in required if name not in accepted)
    if missing:
        raise ValueError(
            "vLLM backend lacks required deterministic engine option(s): "
            + ", ".join(missing)
        )
    return accepted


def model_uses_hybrid_attention(path):
    from transformers import AutoConfig

    config = AutoConfig.from_pretrained(path, trust_remote_code=False)
    configs = [config, getattr(config, "text_config", None)]
    hybrid_markers = ("linear", "mamba", "ssm", "gdn", "recurrent", "rwkv")
    for candidate in configs:
        if candidate is None:
            continue
        layer_types = getattr(candidate, "layer_types", None) or []
        if any(
            any(marker in str(layer_type).lower() for marker in hybrid_markers)
            for layer_type in layer_types
        ):
            return True
        model_type = str(getattr(candidate, "model_type", "")).lower()
        if any(marker in model_type for marker in ("mamba", "jamba", "rwkv")):
            return True
    return False


def model_uses_nvfp4(path):
    from transformers import AutoConfig

    config = AutoConfig.from_pretrained(path, trust_remote_code=False)
    quantization = getattr(config, "quantization_config", None) or {}
    return "nvfp4" in json.dumps(quantization, sort_keys=True).lower()


def configure_deterministic_runtime(path):
    global batch_invariant

    # Seeded canaries must survive batching and scheduler changes. vLLM's
    # batch-invariant kernels require NVIDIA compute capability 9.0 or newer,
    # and vLLM currently refuses them for hybrid GDN/Mamba attention stacks.
    required_environment = {
        "VLLM_ENABLE_V1_MULTIPROCESSING": "0",
        "PYTHONHASHSEED": "0",
        "CUBLAS_WORKSPACE_CONFIG": ":4096:8",
    }
    for name, expected in required_environment.items():
        if os.environ.get(name) != expected:
            raise RuntimeError(
                f"deterministic vLLM worker requires {name}={expected} before Python starts"
            )
    import torch

    cuda_supported = bool(torch.version.cuda) and torch.cuda.is_available()
    capability = torch.cuda.get_device_capability() if cuda_supported else (0, 0)
    batch_invariant = (
        cuda_supported and capability >= (9, 0) and not model_uses_hybrid_attention(path)
    )
    os.environ["VLLM_BATCH_INVARIANT"] = "1" if batch_invariant else "0"
    return batch_invariant


def speciality_maps(payload):
    template_kwargs = {}
    sampling_kwargs = {}
    prompt_suffixes = []
    seen_names = set()
    for item in payload.get("speciality_parameters") or []:
        if not isinstance(item, dict):
            raise ValueError("vLLM speciality parameter must be an object")
        name = str(item.get("name") or "")
        target = str(item.get("target") or "")
        native_path = str(item.get("native_path") or "")
        if not name or name in seen_names or not native_path:
            raise ValueError("vLLM speciality parameters require unique names and native paths")
        seen_names.add(name)
        value = item.get("value")
        if target == "chat_template_kwarg":
            key = native_kwarg_name(native_path, "chat_template_kwargs.")
            if key in template_kwargs:
                raise ValueError(f"duplicate vLLM chat-template speciality mapping {key!r}")
            template_kwargs[key] = value
        elif target == "sampling_parameter":
            key = native_kwarg_name(native_path, "sampling_params.")
            if key in sampling_kwargs:
                raise ValueError(f"duplicate vLLM sampling speciality mapping {key!r}")
            sampling_kwargs[key] = value
        elif target == "prompt_suffix":
            if not isinstance(value, str):
                raise ValueError(f"vLLM prompt-suffix speciality {name!r} must map to a string")
            prompt_suffixes.append(value)
        else:
            raise ValueError(f"unsupported vLLM speciality target {target!r}")
    return template_kwargs, sampling_kwargs, prompt_suffixes


def native_kwarg_name(native_path, prefix):
    key = str(native_path)
    if key.startswith(prefix):
        key = key[len(prefix) :]
    if not key or not key.isidentifier():
        raise ValueError(f"native speciality path {native_path!r} is not a callable keyword")
    return key


def required_call_kwargs(callable_obj, kwargs, required, label):
    accepted = accepted_kwargs(callable_obj, kwargs)
    missing = sorted(name for name in required if name not in accepted)
    if missing:
        raise ValueError(
            f"vLLM {label} does not support requested speciality parameter(s): "
            + ", ".join(missing)
        )
    return accepted


def reasoning_enabled(payload):
    for item in payload.get("speciality_parameters") or []:
        name = str(item.get("name") or "").lower()
        native_path = str(item.get("native_path") or "").lower()
        haystack = f"{name} {native_path}"
        if "reason" not in haystack and "think" not in haystack:
            continue
        if any(marker in haystack for marker in ("preserve", "history", "retain")):
            continue
        value = item.get("value")
        level = str(item.get("level") or "").lower()
        if value is False or value == 0 or str(value).lower() in ("false", "off", "none", "disabled"):
            return False
        if level in ("none", "off", "disabled"):
            return False
        return True
    return False


def import_attr(candidates):
    last_error = None
    for module_name, attr_name in candidates:
        try:
            module = __import__(module_name, fromlist=[attr_name])
            return getattr(module, attr_name)
        except Exception as exc:
            last_error = exc
    raise RuntimeError(f"could not import vLLM API: {last_error}")


def encode_text(text):
    if tokenizer is None:
        return []
    try:
        return [int(token) for token in tokenizer.encode(text, add_special_tokens=True)]
    except TypeError:
        return [int(token) for token in tokenizer.encode(text)]


def vocab_size():
    value = getattr(tokenizer, "vocab_size", None)
    if isinstance(value, int):
        return value
    try:
        return len(tokenizer)
    except TypeError:
        return 0


def model_ctx(default):
    for source in (tokenizer, getattr(engine, "model_config", None), getattr(engine, "config", None)):
        if source is None:
            continue
        for name in (
            "max_position_embeddings",
            "max_sequence_length",
            "max_seq_len",
            "context_length",
            "model_max_length",
        ):
            value = getattr(source, name, None)
            if isinstance(value, int) and 0 < value < 1_000_000_000:
                return value
    return int(default)


def positive_int(value, default):
    try:
        parsed = int(value)
        return parsed if parsed > 0 else int(default)
    except Exception:
        return int(default)


def optional_bool(payload, name):
    if name not in payload or payload[name] is None:
        return None
    value = payload[name]
    if type(value) is not bool:
        raise ValueError(f"{name} must be a boolean")
    return value


def kernel_backend(value, name):
    if not isinstance(value, str):
        raise ValueError(f"{name} must be a string")
    if (
        not value
        or len(value.encode("ascii", errors="ignore")) != len(value)
        or len(value) > MAX_KERNEL_BACKEND_LENGTH
        or not ("a" <= value[0] <= "z")
        or any(not (character.islower() or character.isdigit() or character == "_") for character in value)
    ):
        raise ValueError(
            f"{name} must be a lowercase vLLM backend identifier of at most "
            f"{MAX_KERNEL_BACKEND_LENGTH} bytes"
        )
    return value


def optional_kernel_backend(payload, name):
    if name not in payload or payload[name] is None:
        return None
    return kernel_backend(payload[name], name)


def optional_mtp_num_speculative_tokens(payload):
    name = "vllm_mtp_num_speculative_tokens"
    if name not in payload or payload[name] is None:
        return None
    value = payload[name]
    if type(value) is not int or not (1 <= value <= MAX_MTP_SPECULATIVE_TOKENS):
        raise ValueError(
            f"{name} must be between 1 and {MAX_MTP_SPECULATIVE_TOKENS}"
        )
    return value


def optional_compilation_config(payload):
    config = {}
    mode = payload.get("vllm_compilation_mode")
    if mode is not None:
        if type(mode) is not int or not 0 <= mode <= 3:
            raise ValueError("vllm_compilation_mode must be an integer between 0 and 3")
        config["mode"] = mode
    cudagraph_mode = payload.get("vllm_cudagraph_mode")
    if cudagraph_mode is not None:
        supported = ("NONE", "FULL_DECODE_ONLY", "FULL", "PIECEWISE", "FULL_AND_PIECEWISE")
        if type(cudagraph_mode) is not str or cudagraph_mode.upper() not in supported:
            raise ValueError("vllm_cudagraph_mode must be one of " + ", ".join(supported))
        config["cudagraph_mode"] = cudagraph_mode.upper()
    return config


def enum_value(value):
    return getattr(value, "value", value)


def config_value(config, name):
    if isinstance(config, dict):
        return config.get(name)
    return getattr(config, name, None)


def effective_kernel_backend(config, name):
    value = config_value(config, name)
    if value is None:
        return None
    return kernel_backend(enum_value(value), name)


def effective_mtp_num_speculative_tokens(config):
    speculative = config_value(config, "speculative_config")
    if speculative is None:
        return None
    method = enum_value(config_value(speculative, "method"))
    tokens = config_value(speculative, "num_speculative_tokens")
    if method != "mtp":
        raise ValueError("vLLM speculative execution method must be 'mtp'")
    if tokens is None:
        raise ValueError("vLLM did not expose effective MTP num_speculative_tokens")
    return optional_mtp_num_speculative_tokens(
        {"vllm_mtp_num_speculative_tokens": tokens}
    )


def effective_execution_properties(initialized_engine, required_kwargs):
    config = getattr(initialized_engine, "vllm_config", None)
    if config is None:
        raise ValueError("vLLM did not expose initialized vllm_config")
    model_config = config_value(config, "model_config")
    kernel_config = config_value(config, "kernel_config")
    effective = {
        "enforce_eager": config_value(model_config, "enforce_eager"),
        "linear_backend": effective_kernel_backend(kernel_config, "linear_backend"),
        "moe_backend": effective_kernel_backend(kernel_config, "moe_backend"),
        "seed": config_value(model_config, "seed"),
        "use_fp64_gumbel": config_value(model_config, "use_fp64_gumbel"),
        "async_scheduling": config_value(
            config_value(config, "scheduler_config"), "async_scheduling"
        ),
        "kv_cache_dtype": enum_value(
            config_value(config_value(config, "cache_config"), "cache_dtype")
        ),
    }
    if "compilation_config" in required_kwargs:
        compilation = config_value(config, "compilation_config")
        cudagraph_mode = config_value(compilation, "cudagraph_mode")
        effective["compilation_config"] = optional_compilation_config({
            "vllm_compilation_mode": enum_value(config_value(compilation, "mode")),
            # Composite CUDA graph enums have tuple values; their names are canonical.
            "vllm_cudagraph_mode": getattr(cudagraph_mode, "name", cudagraph_mode),
        })
    for name, expected in required_kwargs.items():
        if name == "speculative_config":
            continue
        if name == "compilation_config":
            fields = [
                (f"{name}.{field}", config_value(effective[name], field), value)
                for field, value in expected.items()
            ]
        else:
            fields = [(name, effective[name], expected)]
        for field, actual, requested in fields:
            if type(actual) is not type(requested) or actual != requested:
                raise ValueError(
                    f"vLLM initialized execution mismatch for {field}: "
                    f"expected {requested!r}, got {actual!r}"
                )
    mtp_tokens = effective_mtp_num_speculative_tokens(config)
    expected_mtp_tokens = config_value(
        required_kwargs.get("speculative_config"), "num_speculative_tokens"
    )
    if mtp_tokens != expected_mtp_tokens:
        raise ValueError(
            "vLLM initialized execution mismatch for num_speculative_tokens: "
            f"expected {expected_mtp_tokens!r}, got {mtp_tokens!r}"
        )
    properties = {
        "vllm_enforce_eager": effective["enforce_eager"],
        "vllm_linear_backend": effective["linear_backend"],
        "vllm_moe_backend": effective["moe_backend"],
        "vllm_mtp_num_speculative_tokens": mtp_tokens,
    }
    if "compilation_config" in required_kwargs:
        properties.update({
            "vllm_compilation_mode": effective["compilation_config"].get("mode"),
            "vllm_cudagraph_mode": effective["compilation_config"].get("cudagraph_mode"),
        })
    return properties


async def observe_worker_execution(initialized_engine, payload):
    requested = optional_compilation_config(payload)
    if not requested:
        return {}
    rpc = getattr(initialized_engine, "collective_rpc", None)
    if not callable(rpc):
        raise ValueError("vLLM does not support the named execution observation RPC")
    ranks = await asyncio.wait_for(
        rpc("mayhem_execution_snapshot", timeout=MAX_EXECUTION_PROBE_SECONDS),
        timeout=MAX_EXECUTION_PROBE_SECONDS,
    )
    parallel = config_value(getattr(initialized_engine, "vllm_config", None), "parallel_config")
    count = config_value(parallel, "world_size")
    tp_size = config_value(parallel, "tensor_parallel_size")
    if (type(count) is not int or count < 1 or type(tp_size) is not int
            or tp_size != positive_int(payload.get("tensor_parallel"), 1)
            or count != tp_size):
        raise ValueError("vLLM execution observation lacks initialized rank geometry")
    for name in ("pipeline_parallel_size", "data_parallel_size", "prefill_context_parallel_size"):
        value = config_value(parallel, name)
        if type(value) is not int or value != 1:
            raise ValueError(f"vLLM explicit execution observation requires {name}=1")
    if type(ranks) is not list or len(ranks) != count:
        raise ValueError(f"vLLM execution observation requires exactly {count} worker ranks")
    seen_ranks = set()
    resolved = None
    for rank in ranks:
        if type(rank) is not dict or set(rank) != {
            "rank", "local_rank", "world_size", "pid", "compilation_mode", "cudagraph_mode"
        }:
            raise ValueError("vLLM execution observation has missing/unknown rank fields")
        for field in ("rank", "local_rank", "pid", "world_size"):
            if type(rank[field]) is not int or rank[field] < (1 if field == "pid" else 0):
                raise ValueError(f"vLLM execution observation has invalid {field}")
        if rank["world_size"] != count:
            raise ValueError("vLLM execution observation has inconsistent world_size")
        if rank["rank"] in seen_ranks:
            raise ValueError("vLLM execution observation has duplicate ranks")
        seen_ranks.add(rank["rank"])
        actual = optional_compilation_config({
            "vllm_compilation_mode": rank["compilation_mode"],
            "vllm_cudagraph_mode": rank["cudagraph_mode"],
        })
        if (set(actual) != {"mode", "cudagraph_mode"}
                or actual["cudagraph_mode"] != rank["cudagraph_mode"]):
            raise ValueError("vLLM execution observation lacks resolved compilation fields")
        if resolved is not None and actual != resolved:
            raise ValueError("vLLM execution observation is inconsistent across TP ranks")
        resolved = actual
        for field, expected in requested.items():
            if type(actual[field]) is not type(expected) or actual[field] != expected:
                raise ValueError(
                    f"vLLM worker execution mismatch at rank {rank['rank']} for {field}: "
                    f"expected {expected!r}, got {actual[field]!r}"
                )
    if seen_ranks != set(range(count)):
        raise ValueError("vLLM execution observation has missing/out-of-range worker ranks")
    return {
        "vllm_compilation_mode": resolved["mode"],
        "vllm_cudagraph_mode": resolved["cudagraph_mode"],
        "worker_execution_observation": {
            "source": "worker_extension_cls.collective_rpc",
            "rank_count": len(ranks),
            "world_size": count,
            "ranks": sorted(ranks, key=lambda rank: rank["rank"]),
        },
    }


async def shutdown_rejected_engine(initialized_engine):
    shutdown = getattr(initialized_engine, "shutdown", None)
    if not callable(shutdown):
        raise ValueError("vLLM rejected engine has no shutdown method")
    kwargs = accepted_kwargs(shutdown, {"timeout": MAX_EXECUTION_PROBE_SECONDS})
    result = await asyncio.wait_for(
        asyncio.to_thread(shutdown, **kwargs), timeout=MAX_EXECUTION_PROBE_SECONDS
    )
    if inspect.isawaitable(result):
        await asyncio.wait_for(result, timeout=MAX_EXECUTION_PROBE_SECONDS)


def utilization_float(value):
    if value is None:
        return None
    try:
        parsed = float(value)
    except Exception as exc:
        raise ValueError("gpu_memory_utilization must be a number between 0 and 1") from exc
    if parsed <= 0.0 or parsed > 1.0:
        raise ValueError("gpu_memory_utilization must be between 0 and 1")
    return parsed


def make_structured_outputs_params(grammar):
    if grammar is None:
        return None
    try:
        StructuredOutputsParams = import_attr(
            (
                ("vllm.sampling_params", "StructuredOutputsParams"),
                ("vllm", "StructuredOutputsParams"),
            )
        )
    except Exception:
        GuidedDecodingParams = import_attr(
            (
                ("vllm.sampling_params", "GuidedDecodingParams"),
                ("vllm", "GuidedDecodingParams"),
            )
        )
        return make_guided_params(GuidedDecodingParams, grammar)
    return make_guided_params(StructuredOutputsParams, grammar)


def make_guided_params(cls, grammar):
    kind = grammar.get("kind")
    if kind == "json_schema":
        field = "json"
        value = grammar.get("schema") or {}
    if kind == "gbnf":
        field = "grammar"
        value = grammar.get("grammar") or ""
    if kind == "tool_call":
        field = "json"
        value = tool_call_schema(grammar.get("tools") or [])
    if kind not in ("json_schema", "gbnf", "tool_call"):
        raise ValueError(f"unsupported vLLM grammar kind {kind!r}")
    kwargs = accepted_kwargs(cls, {field: value})
    if field not in kwargs:
        raise ValueError(f"vLLM structured-output API does not accept {field!r}")
    return cls(**kwargs)


def tool_call_schema(tools):
    branches = []
    definitions = {}
    names = set()
    for index, tool in enumerate(tools):
        name = str(tool.get("name", ""))
        if not name:
            continue
        if name in names:
            raise ValueError(f"duplicate tool name {name!r}")
        names.add(name)
        definition = f"tool_{index}_parameters"
        reference = f"#/$defs/{definition}"
        parameters = copy.deepcopy(
            tool.get(
                "parameters",
                {"type": "object", "additionalProperties": True},
            )
        )
        rebase_local_json_schema_refs(parameters, reference)
        definitions[definition] = parameters
        branches.append(
            {
                "type": "object",
                "additionalProperties": False,
                "required": ["tool", "arguments"],
                "properties": {
                    "tool": {"const": name},
                    "arguments": {"$ref": reference},
                },
            }
        )
    if not branches:
        raise ValueError("tool-call grammar requires at least one tool")
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "MayhemToolCall",
        "$defs": definitions,
        "oneOf": branches,
    }


def rebase_local_json_schema_refs(value, new_root):
    if isinstance(value, list):
        for item in value:
            rebase_local_json_schema_refs(item, new_root)
    elif isinstance(value, dict):
        reference = value.get("$ref")
        if reference == "#":
            value["$ref"] = new_root
        elif isinstance(reference, str) and reference.startswith("#/"):
            value["$ref"] = f"{new_root}/{reference[2:]}"
        for item in value.values():
            rebase_local_json_schema_refs(item, new_root)


def make_sampling_params(payload, speciality_sampling_kwargs=None):
    SamplingParams = import_attr((("vllm", "SamplingParams"),))
    kwargs = {
        "max_tokens": int(payload.get("max_new_tokens") or 64),
        "temperature": float(payload.get("temperature") or 0.0),
    }
    top_p = payload.get("top_p")
    if top_p is not None and float(top_p) > 0.0:
        kwargs["top_p"] = float(top_p)
    requested = set()
    top_k = payload.get("top_k")
    if top_k is not None:
        kwargs["top_k"] = int(top_k)
        requested.add("top_k")
    min_p = payload.get("min_p")
    if min_p is not None:
        kwargs["min_p"] = float(min_p)
        requested.add("min_p")
    repeat_penalty = payload.get("repeat_penalty")
    if repeat_penalty is not None:
        kwargs["repetition_penalty"] = float(repeat_penalty)
        requested.add("repetition_penalty")
    frequency_penalty = payload.get("frequency_penalty")
    if frequency_penalty is not None:
        kwargs["frequency_penalty"] = float(frequency_penalty)
        requested.add("frequency_penalty")
    presence_penalty = payload.get("presence_penalty")
    if presence_penalty is not None:
        kwargs["presence_penalty"] = float(presence_penalty)
        requested.add("presence_penalty")
    stop = payload.get("stop") or []
    if stop:
        kwargs["stop"] = [str(value) for value in stop]
        requested.add("stop")
    if payload.get("seed") is not None:
        kwargs["seed"] = int(payload.get("seed"))
    if bool(payload.get("ignore_eos")):
        kwargs["ignore_eos"] = True
        kwargs["min_tokens"] = kwargs["max_tokens"]
    for name, value in (speciality_sampling_kwargs or {}).items():
        if name in kwargs:
            raise ValueError(
                f"vLLM speciality sampling parameter {name!r} conflicts with a standard request field"
            )
        kwargs[name] = value
        requested.add(name)
    structured = make_structured_outputs_params(payload.get("grammar"))
    if structured is not None:
        structured_kwargs = accepted_kwargs(
            SamplingParams,
            {
                "structured_outputs": structured,
                "guided_decoding": structured,
            },
        )
        if not structured_kwargs:
            raise ValueError("vLLM sampling API does not accept structured decoding")
        kwargs.update(structured_kwargs)
    try:
        RequestOutputKind = import_attr((("vllm.sampling_params", "RequestOutputKind"),))
        kwargs["output_kind"] = getattr(RequestOutputKind, "DELTA")
    except Exception:
        pass
    return SamplingParams(**required_sampling_kwargs(SamplingParams, kwargs, requested))


def create_engine(payload):
    global execution_properties, kernel_policy

    execution_properties = None
    path = str(payload["path"])
    configure_deterministic_runtime(path)
    AsyncEngineArgs = import_attr(
        (
            ("vllm.engine.arg_utils", "AsyncEngineArgs"),
            ("vllm", "AsyncEngineArgs"),
        )
    )
    AsyncLLM = import_attr(
        (
            ("vllm.v1.engine.async_llm", "AsyncLLM"),
            ("vllm", "AsyncLLM"),
        )
    )
    tensor_parallel = positive_int(payload.get("tensor_parallel"), 1)
    requested_enforce_eager = optional_bool(payload, "vllm_enforce_eager")
    requested_linear_backend = optional_kernel_backend(payload, "vllm_linear_backend")
    requested_moe_backend = optional_kernel_backend(payload, "vllm_moe_backend")
    requested_mtp_tokens = optional_mtp_num_speculative_tokens(payload)
    requested_compilation_config = optional_compilation_config(payload)
    kwargs = {
        "model": path,
        "tokenizer": path,
        "trust_remote_code": False,
        "max_model_len": positive_int(payload.get("ctx_size"), 2048),
        "max_num_seqs": load_generation_capacity(payload),
        "max_num_batched_tokens": positive_int(
            payload.get("max_num_tokens"), max(256, positive_int(payload.get("ctx_size"), 2048))
        ),
        "tensor_parallel_size": tensor_parallel,
        "enforce_eager": (
            True if requested_enforce_eager is None else requested_enforce_eager
        ),
        "seed": 0,
        "use_fp64_gumbel": True,
        "async_scheduling": False,
        "limit_mm_per_prompt": {"image": 1, "audio": 1, "video": 1},
        "mm_processor_cache_gb": 0,
    }
    required_options = {
        "enforce_eager",
        "seed",
        "use_fp64_gumbel",
        "async_scheduling",
    }
    if requested_compilation_config:
        kwargs["compilation_config"] = requested_compilation_config
        required_options.add("compilation_config")
        kwargs["worker_extension_cls"] = (
            "mayhem_vllm_execution_probe.MayhemExecutionProbe"
        )
        required_options.add("worker_extension_cls")
    uses_nvfp4 = model_uses_nvfp4(path)
    if requested_linear_backend is not None:
        kwargs["linear_backend"] = requested_linear_backend
        required_options.add("linear_backend")
    elif uses_nvfp4:
        # vLLM's batch-invariant path selects CUTLASS for deterministic NVFP4
        # execution. Hybrid GDN models cannot enable that global mode, so pin
        # the same kernel family explicitly instead of using auto-selected
        # FlashInfer linear/MoE kernels.
        kwargs["linear_backend"] = "cutlass"
        required_options.add("linear_backend")
    if requested_moe_backend is not None:
        kwargs["moe_backend"] = requested_moe_backend
        required_options.add("moe_backend")
    elif uses_nvfp4:
        kwargs["moe_backend"] = "cutlass"
        required_options.add("moe_backend")
    if uses_nvfp4 and requested_linear_backend is None and requested_moe_backend is None:
        kernel_policy = "nvfp4-cutlass"
    elif requested_linear_backend is not None or requested_moe_backend is not None:
        kernel_policy = "explicit"
    else:
        kernel_policy = "auto"
    if requested_mtp_tokens is not None:
        kwargs["speculative_config"] = {
            "method": "mtp",
            "num_speculative_tokens": requested_mtp_tokens,
        }
        required_options.add("speculative_config")
    dtype = payload.get("dtype")
    if dtype:
        kwargs["dtype"] = str(dtype)
    kv_cache_dtype = payload.get("kv_cache_dtype")
    if kv_cache_dtype:
        kwargs["kv_cache_dtype"] = str(kv_cache_dtype)
        required_options.add("kv_cache_dtype")
    gpu_memory_utilization = utilization_float(payload.get("gpu_memory_utilization"))
    if gpu_memory_utilization is not None:
        kwargs["gpu_memory_utilization"] = gpu_memory_utilization
    accepted_engine_kwargs = required_engine_kwargs(
        AsyncEngineArgs,
        kwargs,
        required_options,
    )
    # vLLM can mutate nested arguments while building its initialized config.
    required_kwargs = copy.deepcopy({
        name: kwargs[name] for name in sorted(required_options)
        if name != "worker_extension_cls"
    })
    args = AsyncEngineArgs(**accepted_engine_kwargs)
    if hasattr(AsyncLLM, "from_engine_args"):
        initialized_engine = AsyncLLM.from_engine_args(args)
    else:
        initialized_engine = AsyncLLM(args)
    try:
        execution_properties = effective_execution_properties(
            initialized_engine, required_kwargs
        )
    except Exception:
        shutdown = getattr(initialized_engine, "shutdown", None)
        if callable(shutdown):
            shutdown()
        raise
    return initialized_engine


def load_generation_capacity(payload):
    return positive_int(payload.get("max_batch_size"), 1)


def runtime_kv_cache_info():
    config = getattr(getattr(engine, "vllm_config", None), "cache_config", None)
    size_tokens = getattr(config, "kv_cache_size_tokens", None)
    max_concurrency = getattr(config, "kv_cache_max_concurrency", None)
    try:
        size_tokens = int(size_tokens)
    except (TypeError, ValueError, OverflowError):
        size_tokens = 0
    try:
        max_concurrency = float(max_concurrency)
    except (TypeError, ValueError, OverflowError):
        max_concurrency = 0.0
    return {
        "size_tokens": size_tokens if size_tokens > 0 else None,
        "max_concurrency": (
            max_concurrency
            if math.isfinite(max_concurrency) and max_concurrency > 0.0
            else None
        ),
    }


def get_tokenizer():
    global tokenizer
    if tokenizer is not None:
        return tokenizer
    if hasattr(engine, "get_tokenizer"):
        tokenizer = engine.get_tokenizer()
        return tokenizer
    try:
        from transformers import AutoTokenizer

        tokenizer = AutoTokenizer.from_pretrained(model_path, trust_remote_code=False)
        return tokenizer
    except Exception:
        tokenizer = None
        return None


def request_max_tokens(payload):
    value = payload.get("max_new_tokens")
    return 64 if value is None else int(value)


def inline_media_bytes(item):
    data = item.get("data")
    if data:
        try:
            return base64.b64decode(str(data), validate=True)
        except Exception as exc:
            raise ValueError("multimodal data is not valid base64") from exc
    url = item.get("url")
    if not url:
        raise ValueError("multimodal input is missing inline data")
    url = str(url)
    if not url.startswith("data:"):
        raise ValueError("remote media URLs are forbidden; use inline base64 data")
    header, separator, encoded = url.partition(",")
    if not separator or ";base64" not in header:
        raise ValueError("multimodal data URL must be base64 encoded")
    try:
        return base64.b64decode(encoded, validate=True)
    except Exception as exc:
        raise ValueError("multimodal data URL contains invalid base64") from exc


def decode_image(item):
    from PIL import Image

    image = Image.open(io.BytesIO(inline_media_bytes(item)))
    image.load()
    return image.convert("RGB")


def decode_audio(item):
    import numpy as np

    try:
        with wave.open(io.BytesIO(inline_media_bytes(item)), "rb") as wav:
            channels = int(wav.getnchannels())
            sample_width = int(wav.getsampwidth())
            sample_rate = int(wav.getframerate())
            frames = wav.readframes(wav.getnframes())
    except Exception as exc:
        raise ValueError("multimodal audio must be a bounded PCM WAV") from exc
    dtype_by_width = {1: np.uint8, 2: np.int16, 4: np.int32}
    dtype = dtype_by_width.get(sample_width)
    if dtype is None or channels <= 0 or sample_rate <= 0:
        raise ValueError("multimodal WAV sample format is unsupported")
    samples = np.frombuffer(frames, dtype=dtype).astype(np.float32)
    if sample_width == 1:
        samples = (samples - 128.0) / 128.0
    else:
        samples /= float(1 << (sample_width * 8 - 1))
    if channels > 1:
        if samples.size % channels:
            raise ValueError("multimodal WAV channel data is malformed")
        samples = samples.reshape(-1, channels).mean(axis=1)
    return samples, sample_rate


def decode_video(item):
    import av
    import numpy as np

    requested = int(item.get("num_frames") or 8)
    if requested <= 0 or requested > 64:
        raise ValueError("multimodal video num_frames must be between 1 and 64")
    inline_frames = item.get("frames") or []
    if inline_frames:
        if item.get("data") or item.get("url"):
            raise ValueError("multimodal video must use decoded frames or a container, not both")
        if len(inline_frames) != requested:
            raise ValueError("multimodal video num_frames does not match decoded frames")
        decoded = []
        for frame in inline_frames:
            decoded.append(np.asarray(decode_image({"url": frame}), dtype=np.uint8))
        shape = decoded[0].shape
        if any(frame.shape != shape for frame in decoded):
            raise ValueError("multimodal video decoded frames must have matching dimensions")
        source_fps = float(item.get("fps") or 1.0)
        if not math.isfinite(source_fps) or source_fps <= 0:
            raise ValueError("multimodal video fps must be a positive finite number")
        metadata = {
            "fps": source_fps,
            "duration": len(decoded) / source_fps,
            "total_num_frames": len(decoded),
            "frames_indices": list(range(len(decoded))),
            "video_backend": "inline_frames",
            "do_sample_frames": False,
        }
        return np.stack(decoded), metadata
    try:
        container = av.open(io.BytesIO(inline_media_bytes(item)))
        stream = container.streams.video[0]
    except Exception as exc:
        raise ValueError("multimodal video container cannot be decoded") from exc
    total = int(getattr(stream, "frames", 0) or 0)
    average_rate = getattr(stream, "average_rate", None)
    source_fps = float(average_rate) if average_rate is not None else 0.0
    if source_fps <= 0:
        source_fps = float(item.get("fps") or 1.0)
    stream_duration = getattr(stream, "duration", None)
    time_base = getattr(stream, "time_base", None)
    duration = (
        float(stream_duration * time_base)
        if stream_duration is not None and time_base is not None
        else 0.0
    )
    wanted = None
    if total > 0:
        wanted = set(np.linspace(0, total - 1, min(requested, total), dtype=int).tolist())
    frames = []
    frame_indices = []
    try:
        for index, frame in enumerate(container.decode(stream)):
            if wanted is None:
                if len(frames) < requested:
                    frames.append(frame.to_ndarray(format="rgb24"))
                    frame_indices.append(index)
                else:
                    break
            elif index in wanted:
                frames.append(frame.to_ndarray(format="rgb24"))
                frame_indices.append(index)
                if len(frames) == len(wanted):
                    break
    finally:
        container.close()
    if not frames:
        raise ValueError("multimodal video contains no decodable frames")
    total_num_frames = max(total, frame_indices[-1] + 1, len(frames))
    duration = max(duration, total_num_frames / source_fps)
    metadata = {
        "fps": source_fps,
        "duration": duration,
        "total_num_frames": total_num_frames,
        "frames_indices": frame_indices,
        "video_backend": "pyav",
        "do_sample_frames": False,
    }
    return np.stack(frames), metadata


def decode_multimodal_data(payload):
    grouped = {}
    for item in payload.get("media") or []:
        kind = str(item.get("kind") or "")
        if kind == "image":
            value = decode_image(item)
        elif kind == "audio":
            value = decode_audio(item)
        elif kind == "video":
            value = decode_video(item)
        else:
            raise ValueError(f"unsupported multimodal input kind {kind!r}")
        grouped.setdefault(kind, []).append(value)
    return {
        kind: values[0] if len(values) == 1 else values
        for kind, values in grouped.items()
    }


def processor_chat_messages(messages):
    prepared = copy.deepcopy(messages)
    for message in prepared:
        content = message.get("content")
        if not isinstance(content, list):
            continue
        normalized = []
        for part in content:
            kind = part.get("type") if isinstance(part, dict) else None
            if kind == "image_url":
                normalized.append({"type": "image"})
            elif kind == "input_audio":
                normalized.append({"type": "audio"})
            elif kind == "video":
                normalized.append({"type": "video"})
            else:
                normalized.append(part)
        message["content"] = normalized
    return prepared


def render_chat_template(renderer, messages, template_kwargs, label):
    if renderer is None or not hasattr(renderer, "apply_chat_template"):
        raise RuntimeError(f"requested speciality requires the model {label}/chat template")
    kwargs = {
        "tokenize": False,
        "add_generation_prompt": True,
        **template_kwargs,
    }
    try:
        return str(
            renderer.apply_chat_template(
                messages,
                **required_call_kwargs(
                    renderer.apply_chat_template,
                    kwargs,
                    set(template_kwargs),
                    f"{label} chat template",
                ),
            )
        )
    except Exception as exc:
        raise ValueError(f"model {label} chat template failed: {exc}") from exc


def multimodal_engine_prompt(payload, mm_data, template_kwargs, prompt_suffixes):
    prompt = str(payload.get("prompt", ""))
    messages = payload.get("messages") or []
    if mm_data:
        if not messages:
            raise ValueError("multimodal request is missing structured chat messages")
        rendered = render_chat_template(
            processor,
            processor_chat_messages(messages),
            template_kwargs,
            "multimodal processor",
        )
        rendered += "".join(prompt_suffixes)
        return rendered, {"prompt": rendered, "multi_modal_data": mm_data}
    if template_kwargs:
        if not messages:
            raise ValueError("chat-template request is missing structured chat messages")
        prompt = render_chat_template(tokenizer, messages, template_kwargs, "tokenizer")
    prompt += "".join(prompt_suffixes)
    return prompt, prompt


def prepare_generation_request(request_id, payload):
    check_cancelled(request_id)
    if engine is None:
        raise RuntimeError("model has not been loaded")

    max_tokens = request_max_tokens(payload)
    template_kwargs, sampling_kwargs, prompt_suffixes = speciality_maps(payload)
    template_tools = payload.get("tools") or []
    if not isinstance(template_tools, list):
        raise ValueError("vLLM chat-template tools must be an array")
    if template_tools:
        if "tools" in template_kwargs:
            raise ValueError("vLLM chat-template tools conflict with a speciality mapping")
        template_kwargs["tools"] = template_tools
    mm_data = decode_multimodal_data(payload)
    prompt, engine_prompt = multimodal_engine_prompt(
        payload, mm_data, template_kwargs, prompt_suffixes
    )
    check_cancelled(request_id)
    prompt_tokens = encode_text(prompt)
    check_cancelled(request_id)
    if prompt_tokens and len(prompt_tokens) >= ctx_size:
        raise ValueError(
            f"prompt has {len(prompt_tokens)} tokens, leaving no room in ctx_size={ctx_size}"
        )
    if max_tokens <= 0:
        return {"empty": True}

    return {
        "empty": False,
        "engine_prompt": engine_prompt,
        "prompt_tokens": prompt_tokens,
        "sampling_params": make_sampling_params(payload, sampling_kwargs),
        "mm_data": mm_data,
        "reasoning_active": reasoning_enabled(payload),
    }


async def async_handle_generate(request_id, payload):
    prepared = await asyncio.to_thread(prepare_generation_request, request_id, payload)
    check_cancelled(request_id)
    if prepared["empty"]:
        return {
            "text": "",
            "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
            "finish_reason": "length",
        }

    engine_prompt = prepared["engine_prompt"]
    prompt_tokens = prepared["prompt_tokens"]
    sampling_params = prepared["sampling_params"]
    mm_data = prepared["mm_data"]
    text = ""
    completion_tokens = 0
    token_ids = []
    finish_reason = "length"
    reasoning_tokens = 0
    reasoning_active = prepared["reasoning_active"]
    actual_prompt_tokens = len(prompt_tokens)
    multiplexer = generation_multiplexer
    if multiplexer is not None:
        multiplexer.engine_started(request_id)
    try:
        async for output in engine.generate(
            request_id=f"mayhem-{request_id}",
            prompt=engine_prompt,
            sampling_params=sampling_params,
        ):
            if request_cancelled(request_id):
                await abort_engine_request(request_id)
                raise RequestCancelled("engine request cancelled")
            output_prompt_ids = getattr(output, "prompt_token_ids", None)
            if output_prompt_ids is not None:
                actual_prompt_tokens = max(actual_prompt_tokens, len(output_prompt_ids))
            for completion in getattr(output, "outputs", []) or []:
                chunk_text = str(getattr(completion, "text", "") or "")
                ids = getattr(completion, "token_ids", None) or []
                ids = [int(token) for token in ids]
                if chunk_text or ids:
                    if not ids:
                        ids = [-1]
                    for position, token in enumerate(ids):
                        send(
                            {
                                "id": request_id,
                                "type": "token",
                                "chunk": {
                                    "index": completion_tokens,
                                    "token_id": int(token),
                                    "text": chunk_text if position == 0 else "",
                                },
                            }
                        )
                        completion_tokens += 1
                        token_ids.append(int(token))
                        if reasoning_active:
                            reasoning_tokens += 1
                    text += chunk_text
                    if reasoning_active and "</think>" in text:
                        reasoning_active = False
                reason = getattr(completion, "finish_reason", None)
                if reason is not None:
                    finish_reason = "stop" if str(reason) == "stop" else "length"
            if getattr(output, "finished", False):
                break
    finally:
        if multiplexer is not None:
            multiplexer.engine_stopped(request_id)

    check_cancelled(request_id)

    if completion_tokens == 0 and text:
        completion_tokens = 1
    media_token_delta = max(0, actual_prompt_tokens - len(prompt_tokens))
    has_visual_media = "image" in mm_data or "video" in mm_data
    has_audio_media = "audio" in mm_data
    return {
        "text": text,
        "usage": {
            "prompt_tokens": actual_prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": actual_prompt_tokens + completion_tokens,
            "reasoning_tokens": min(reasoning_tokens, completion_tokens),
            "vision_tokens": media_token_delta if has_visual_media else 0,
            "audio_tokens": (
                media_token_delta if has_audio_media and not has_visual_media else 0
            ),
        },
        "finish_reason": finish_reason,
    }


async def handle_load(payload):
    global engine, tokenizer, processor, ctx_size, model_path, generation_multiplexer
    global execution_properties
    model_path = str(payload["path"])
    ctx_size = positive_int(payload.get("ctx_size"), 2048)
    engine = create_engine(payload)
    if optional_compilation_config(payload):
        initialized_engine = engine
        frontend_properties = execution_properties
        execution_properties = None
        try:
            observed = await observe_worker_execution(initialized_engine, payload)
            execution_properties = {**frontend_properties, **observed}
        except BaseException as error:
            engine = tokenizer = processor = generation_multiplexer = None
            try:
                await shutdown_rejected_engine(initialized_engine)
            except Exception as shutdown_error:
                raise RuntimeError(
                    f"vLLM execution observation failed: {error!r}; "
                    f"shutdown failed: {shutdown_error!r}"
                ) from error
            raise
    try:
        from transformers import AutoProcessor, AutoTokenizer

        tokenizer = AutoTokenizer.from_pretrained(model_path, trust_remote_code=False)
        try:
            processor = AutoProcessor.from_pretrained(model_path, trust_remote_code=False)
        except Exception:
            processor = None
    except Exception:
        tokenizer = None
        processor = None
    get_tokenizer()
    kv_cache = runtime_kv_cache_info()
    generation_multiplexer = GenerationMultiplexer(
        load_generation_capacity(payload),
        async_handle_generate,
        abort_engine_request,
        send,
        finish_request,
    )
    return {
        "n_ctx_train": model_ctx(ctx_size),
        "n_vocab": int(vocab_size()),
        "kv_cache_size_tokens": kv_cache["size_tokens"],
        "kv_cache_max_concurrency": kv_cache["max_concurrency"],
        "execution": execution_properties,
        "determinism": {
            "async_scheduling": False,
            "batch_invariant": batch_invariant,
            "v1_multiprocessing": False,
            "python_hash_seed": 0,
            "fp64_gumbel": True,
            "kernel_policy": kernel_policy,
            "seed": 0,
        },
    }


def handle_tokenize(payload):
    if engine is None:
        raise RuntimeError("model has not been loaded")
    return {"token_ids": encode_text(str(payload.get("text", "")))}


def handle(request_id, op, payload):
    if op == "load":
        return handle_load(payload or {})
    if op == "tokenize":
        return handle_tokenize(payload or {})
    if op == "shutdown":
        raise SystemExit(0)
    raise ValueError(f"unknown vLLM worker op {op!r}")


async def emit_control_response(request):
    request_id = int(request.get("id", 0))
    try:
        if "parse_error" in request:
            raise ValueError(request["parse_error"])
        result = handle(request_id, str(request.get("op", "")), request.get("payload"))
        if inspect.isawaitable(result):
            result = await result
        check_cancelled(request_id)
        send({"id": request_id, "type": "response", "ok": True, "result": result})
    except SystemExit:
        raise
    except RequestCancelled as exc:
        send(
            {
                "id": request_id,
                "type": "response",
                "ok": False,
                "cancelled": True,
                "error": str(exc),
            }
        )
    except Exception as exc:
        error = str(exc) or repr(exc) or type(exc).__name__
        send({"id": request_id, "type": "response", "ok": False, "error": error})
    finally:
        finish_request(request_id)


def read_requests():
    for line in sys.stdin:
        if not line.strip():
            continue
        try:
            request = json.loads(line)
        except Exception as exc:
            request_queue.put({"id": 0, "op": "invalid", "parse_error": str(exc)})
            continue
        request_id = int(request.get("id", 0))
        if str(request.get("op", "")) == "cancel":
            payload = request.get("payload") or {}
            schedule_abort(int(payload.get("request_id", request_id)))
            continue
        register_request(request_id)
        request_queue.put(request)
    request_queue.put(None)


async def run_worker():
    global generation_multiplexer

    while True:
        request = await asyncio.to_thread(request_queue.get)
        if request is None:
            break
        request_id = int(request.get("id", 0))
        op = str(request.get("op", ""))
        if op == "generate":
            if generation_multiplexer is None:
                await emit_control_response(request)
                continue
            try:
                generation_multiplexer.submit(request_id, request.get("payload") or {})
            except Exception as exc:
                error = str(exc) or repr(exc) or type(exc).__name__
                send({"id": request_id, "type": "response", "ok": False, "error": error})
                finish_request(request_id)
            continue

        if generation_multiplexer is not None:
            await generation_multiplexer.drain()
        await emit_control_response(request)

    if generation_multiplexer is not None:
        await generation_multiplexer.drain()


threading.Thread(target=read_requests, name="mayhem-vllm-control", daemon=True).start()
event_loop.run_until_complete(run_worker())
