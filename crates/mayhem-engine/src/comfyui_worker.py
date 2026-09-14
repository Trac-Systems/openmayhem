import asyncio
import base64
import contextlib
import errno
import json
import math
import os
import sys
import tempfile
import time
import traceback
import uuid
from pathlib import Path

PROTOCOL_PREFIX = "__mayhem_comfyui_worker_v1__"
MAX_RESPONSE_ARTIFACT_BYTES = 128 * 1024 * 1024
MAX_PROGRESS_EVENTS = 4096
POLL_SECONDS = 0.05

protocol_stdout = sys.stdout
sys.stdout = sys.stderr
sys.__stdout__ = sys.stderr

loop = asyncio.new_event_loop()
asyncio.set_event_loop(loop)
prompt_server = None
runner = None
session = None
base_dir = None
control_mode = None
model_path_aliases = {}


def prefer_internal_queue_control():
    return os.name == "nt"


def comfy_path(path):
    text = str(path)
    if os.name == "nt":
        if text.startswith("\\\\?\\UNC\\"):
            return "\\\\" + text[8:]
        if text.startswith("\\\\?\\"):
            return text[4:]
    return text


def normalize_workflow_model_paths(workflow):
    if not model_path_aliases:
        return
    import nodes

    for node in workflow.values():
        if not isinstance(node, dict):
            continue
        class_type = node.get("class_type")
        inputs = node.get("inputs")
        node_class = nodes.NODE_CLASS_MAPPINGS.get(class_type)
        if node_class is None or not isinstance(inputs, dict):
            continue
        try:
            input_types = node_class.INPUT_TYPES()
        except Exception:
            continue
        if not isinstance(input_types, dict):
            continue
        specs = {}
        for group in ("required", "optional", "hidden"):
            values = input_types.get(group)
            if isinstance(values, dict):
                specs.update(values)
        for name, value in inputs.items():
            native = model_path_aliases.get(value) if isinstance(value, str) else None
            spec = specs.get(name)
            if native is None or not isinstance(spec, (list, tuple)) or not spec:
                continue
            choices = spec[0]
            if isinstance(choices, (list, tuple, set)) and native in choices:
                inputs[name] = native


def reply(message_id, ok, result=None, error=None):
    response = {"id": message_id, "ok": ok}
    if result is not None:
        response["result"] = result
    if error is not None:
        response["error"] = str(error)
    protocol_stdout.write(PROTOCOL_PREFIX + json.dumps(response, separators=(",", ":")) + "\n")
    protocol_stdout.flush()


def content_type(path):
    suffix = path.suffix.lower()
    if suffix == ".png":
        return "image/png"
    if suffix in (".jpg", ".jpeg"):
        return "image/jpeg"
    if suffix == ".webp":
        return "image/webp"
    if suffix == ".mp4":
        return "video/mp4"
    if suffix == ".wav":
        return "audio/wav"
    if suffix == ".mp3":
        return "audio/mpeg"
    return "application/octet-stream"


def resolve_output_file(item):
    filename = item.get("filename")
    if not isinstance(filename, str) or not filename:
        return None
    subfolder = item.get("subfolder")
    if not isinstance(subfolder, str):
        subfolder = ""
    kind = item.get("type")
    root = {
        "input": base_dir / "input",
        "temp": base_dir / "temp",
    }.get(kind, base_dir / "output")
    candidate = (root / subfolder / filename).resolve()
    if not candidate.is_relative_to(root.resolve()):
        raise RuntimeError("ComfyUI output path escaped its output root")
    if candidate.is_file():
        return candidate
    return None


def safe_input_file_path(filename):
    if not isinstance(filename, str) or not filename:
        raise ValueError("workflow input file filename is required")
    if filename.startswith(("/", "\\")) or "\\" in filename or len(filename) > 240:
        raise ValueError("workflow input file filename must be a safe relative path")
    parts = filename.split("/")
    if any(part in ("", ".", "..") for part in parts):
        raise ValueError("workflow input file filename contains an unsafe path component")
    for part in parts:
        if not all(ch.isalnum() or ch in "._-" for ch in part):
            raise ValueError("workflow input file filename contains unsupported characters")
    root = (base_dir / "input").resolve()
    candidate = (root / filename).resolve()
    if not candidate.is_relative_to(root):
        raise ValueError("workflow input file escaped the input root")
    return candidate


def input_transfer_root():
    root = (base_dir / "input").resolve()
    root.mkdir(parents=True, exist_ok=True)
    # A sibling keeps atomic replacement on the input drive, while callers
    # cannot address journals or backups through workflow input filenames.
    transfers = root.parent / ".mayhem-input-transfers"
    transfers.mkdir(exist_ok=True)
    return transfers


def restore_input_transfer(backup_root):
    manifest = backup_root / "manifest.json"
    records = []
    if manifest.exists():
        if manifest.stat().st_size > 256 * 1024:
            raise RuntimeError("workflow input recovery journal exceeds its bound")
        records = json.loads(manifest.read_text(encoding="utf-8"))
        if not isinstance(records, list) or len(records) > 1024:
            raise RuntimeError("invalid workflow input recovery journal")
    errors = []
    for index in reversed(range(len(records))):
        try:
            record = records[index]
            path = safe_input_file_path(record["filename"])
            backup = backup_root / str(index)
            if record["original"] is True:
                if backup.exists():
                    backup.replace(path)
                # No backup means either the original was not yet moved, or
                # restoration completed before a previous recovery stopped.
            elif record["original"] is False:
                path.unlink(missing_ok=True)
            else:
                raise ValueError("invalid workflow input recovery record")
        except (OSError, ValueError, KeyError, TypeError) as error:
            errors.append(error)
    if errors:
        raise RuntimeError(f"workflow input cleanup failed; backups retained at {backup_root}") from errors[0]
    manifest.unlink(missing_ok=True)
    (backup_root / "manifest.next").unlink(missing_ok=True)
    backup_root.rmdir()


def recover_input_transfers():
    transfers = input_transfer_root()
    for pending in sorted(transfers.iterdir()):
        if pending.name.startswith("request-") and pending.is_dir():
            restore_input_transfer(pending)


@contextlib.contextmanager
def input_transfer_lock():
    # Two workers can share a runtime cache. Hold an OS lock through generation
    # so one worker cannot recover or overwrite another worker's active inputs.
    with (input_transfer_root() / ".lock").open("a+b") as lock:
        if lock.seek(0, os.SEEK_END) == 0:
            lock.write(b"\0")
            lock.flush()
        lock.seek(0)
        if os.name == "nt":
            import msvcrt
            while True:
                try:
                    msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
                    break
                except OSError as error:
                    if error.errno not in (errno.EACCES, errno.EAGAIN, errno.EDEADLK):
                        raise
                    time.sleep(0.1)
        else:
            import fcntl
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        try:
            yield
        finally:
            lock.seek(0)
            if os.name == "nt":
                msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(lock.fileno(), fcntl.LOCK_UN)


@contextlib.contextmanager
def materialized_input_files(payload):
    with input_transfer_lock():
        with staged_input_files(payload):
            yield


@contextlib.contextmanager
def staged_input_files(payload):
    # Cancellation can terminate this worker, bypassing finally. Recover its
    # journals before another graph can read any input left by that request.
    recover_input_transfers()
    backup_root = Path(tempfile.mkdtemp(prefix="request-", dir=input_transfer_root()))
    written = []
    seen = set()
    try:
        for item in payload.get("input_files") or []:
            if not isinstance(item, dict):
                raise ValueError("workflow input_files entries must be objects")
            filename = item.get("filename")
            path = safe_input_file_path(filename)
            if path in seen:
                raise ValueError(f"duplicate workflow input file {filename}")
            seen.add(path)
            encoded = item.get("data_base64")
            if not isinstance(encoded, str) or not encoded:
                raise ValueError(f"workflow input file {filename} is missing data_base64")
            data = base64.b64decode(encoded, validate=True)
            if not data:
                raise ValueError(f"workflow input file {filename} is empty")
            path.parent.mkdir(parents=True, exist_ok=True)
            original = path.exists()
            if original:
                if not path.is_file():
                    raise ValueError(f"workflow input file {filename} is not a file")
            written.append({"filename": filename, "original": original})
            # Commit the recovery record before changing any caller/provider
            # bytes. Atomic replacement makes interrupted journals readable.
            journal = backup_root / "manifest.next"
            journal.write_text(json.dumps(written), encoding="utf-8")
            journal.replace(backup_root / "manifest.json")
            if original:
                path.replace(backup_root / str(len(written) - 1))
            path.write_bytes(data)
        yield
    finally:
        restore_input_transfer(backup_root)


def collect_artifacts(prompt_id, history):
    artifacts = []
    total_bytes = 0
    for node_id, node_output in (history.get("outputs") or {}).items():
        for key in ("images", "gifs", "audio", "videos"):
            for item in node_output.get(key) or []:
                path = resolve_output_file(item)
                if path is None:
                    continue
                data = path.read_bytes()
                total_bytes += len(data)
                if total_bytes > MAX_RESPONSE_ARTIFACT_BYTES:
                    raise RuntimeError("ComfyUI workflow artifacts exceed response bound")
                artifacts.append({
                    "artifact_id": f"{prompt_id}:{node_id}:{len(artifacts)}",
                    "content_type": content_type(path),
                    "data_base64": base64.b64encode(data).decode("ascii"),
                })
    return artifacts


def append_progress(progress, event):
    if len(progress) < MAX_PROGRESS_EVENTS:
        progress.append(event)
    elif len(progress) == MAX_PROGRESS_EVENTS:
        progress.append({"kind": "truncated", "node": None, "value": {"limit": MAX_PROGRESS_EVENTS}})


def progress_node(data):
    if not isinstance(data, dict):
        return None
    node = data.get("node") or data.get("node_id")
    if node is None:
        return None
    return str(node)


def progress_value(data):
    try:
        json.dumps(data, separators=(",", ":"))
        return data
    except TypeError:
        return {"non_json": True}


def progress_matches_prompt(data, prompt_id):
    if not isinstance(data, dict):
        return False
    if data.get("prompt_id") == prompt_id:
        return True
    for value in data.values():
        if isinstance(value, dict) and value.get("prompt_id") == prompt_id:
            return True
    return False


def append_history_progress(progress, history, prompt_id):
    status = history.get("status") if isinstance(history, dict) else None
    messages = status.get("messages") if isinstance(status, dict) else None
    if not isinstance(messages, list):
        return
    for message in messages:
        if not isinstance(message, (list, tuple)) or len(message) < 2:
            continue
        event = message[0]
        data = message[1]
        if not isinstance(event, str) or not progress_matches_prompt(data, prompt_id):
            continue
        append_progress(progress, {
            "kind": event,
            "node": progress_node(data),
            "value": progress_value(data),
        })


@contextlib.contextmanager
def capture_prompt_server_progress(prompt_id, progress):
    original_send_sync = prompt_server.send_sync

    def send_sync_with_capture(event, data, sid=None):
        if isinstance(event, str) and progress_matches_prompt(data, prompt_id):
            append_progress(progress, {
                "kind": event,
                "node": progress_node(data),
                "value": progress_value(data),
            })
        return original_send_sync(event, data, sid)

    prompt_server.send_sync = send_sync_with_capture
    try:
        yield
    finally:
        prompt_server.send_sync = original_send_sync


def patch_comfy_quantized_offload_probe():
    import comfy.model_patcher

    original = comfy.model_patcher.get_key_weight
    if getattr(original, "_mayhem_quantized_guard", False):
        return

    def guarded_get_key_weight(model, key):
        try:
            return original(model, key)
        except AttributeError as error:
            # Mayhem patch: some Comfy FP4/NVFP4 modules expose custom packed
            # parameters without normal weight/bias attributes; treat those as
            # zero extra offload-estimate bytes instead of aborting the workflow.
            if key.endswith((".weight", ".bias")) and "has no attribute" in str(error):
                return None, None, None
            raise

    guarded_get_key_weight._mayhem_quantized_guard = True
    comfy.model_patcher.get_key_weight = guarded_get_key_weight


def load(payload):
    global prompt_server, base_dir, control_mode, model_path_aliases
    runtime_root = Path(comfy_path(Path(payload["runtime_root"]).resolve()))
    base_dir = Path(comfy_path(Path(payload["base_dir"]).resolve()))
    socket_path = Path(comfy_path(Path(payload["socket_path"]).resolve()))
    device = payload.get("device", "auto")
    vram_reserve_gb = payload.get("vram_reserve_gb")
    if vram_reserve_gb is not None:
        if isinstance(vram_reserve_gb, bool) or not isinstance(
            vram_reserve_gb, (int, float)
        ):
            raise ValueError(
                "ComfyUI vram_reserve_gb must be a finite number between 0 and 1024"
            )
        vram_reserve_gb = float(vram_reserve_gb)
        if not math.isfinite(vram_reserve_gb) or not 0 <= vram_reserve_gb <= 1024:
            raise ValueError(
                "ComfyUI vram_reserve_gb must be a finite number between 0 and 1024"
            )
    custom_node_whitelist = payload.get("custom_node_whitelist", [])
    aliases = payload.get("model_path_aliases", {})
    if not isinstance(aliases, dict) or any(
        not isinstance(key, str) or not isinstance(value, str)
        for key, value in aliases.items()
    ):
        raise ValueError("ComfyUI model_path_aliases must be a string map")
    model_path_aliases = aliases
    for path in (base_dir / "input", base_dir / "output", base_dir / "temp", base_dir / "user"):
        path.mkdir(parents=True, exist_ok=True)
    socket_path.parent.mkdir(parents=True, exist_ok=True)
    with contextlib.suppress(FileNotFoundError):
        socket_path.unlink()

    sys.path.insert(0, comfy_path(runtime_root))
    argv = [
        "main.py",
        "--disable-auto-launch",
        "--disable-all-custom-nodes",
        "--disable-api-nodes",
        "--dont-print-server",
        "--base-directory",
        comfy_path(base_dir),
    ]
    if custom_node_whitelist:
        argv.append("--whitelist-custom-nodes")
        argv.extend(str(name) for name in custom_node_whitelist)
    if device == "cpu":
        argv.append("--cpu")
    if vram_reserve_gb is not None:
        argv.extend(("--reserve-vram", format(vram_reserve_gb, "g")))
    sys.argv = argv
    os.chdir(comfy_path(runtime_root))

    import main
    patch_comfy_quantized_offload_probe()

    if prefer_internal_queue_control():
        main.server.PromptServer.add_routes = lambda self: None

    _, prompt_server, _ = main.start_comfyui(loop)
    if prefer_internal_queue_control():
        loop.run_until_complete(prompt_server.setup())
        control_mode = "internal_queue"
        import nodes
        return {
            "object_info_classes": len(nodes.NODE_CLASS_MAPPINGS),
            "node_classes": sorted(nodes.NODE_CLASS_MAPPINGS.keys()),
            "socket_path": None,
            "control_mode": "internal_queue",
        }
    try:
        result = loop.run_until_complete(load_socket_async(socket_path))
        control_mode = "unix_socket"
        return result
    except PermissionError:
        control_mode = "internal_queue"
        import nodes
        return {
            "object_info_classes": len(nodes.NODE_CLASS_MAPPINGS),
            "node_classes": sorted(nodes.NODE_CLASS_MAPPINGS.keys()),
            "socket_path": None,
            "control_mode": "internal_queue",
        }


async def load_socket_async(socket_path):
    global runner, session
    import aiohttp
    from aiohttp import web

    await prompt_server.setup()
    runner = web.AppRunner(prompt_server.app, access_log=None)
    await runner.setup()
    await web.UnixSite(runner, str(socket_path)).start()
    session = aiohttp.ClientSession(connector=aiohttp.UnixConnector(path=str(socket_path)))
    async with session.get("http://mayhem/object_info") as response:
        object_info = await response.json()
    return {
        "object_info_classes": len(object_info),
        "node_classes": sorted(object_info.keys()),
        "socket_path": str(socket_path),
        "control_mode": "unix_socket",
    }


async def run_workflow(payload):
    if control_mode == "internal_queue":
        return await run_workflow_internal(payload)
    with materialized_input_files(payload):
        workflow = payload["workflow"]
        if not isinstance(workflow, dict):
            raise ValueError("workflow must be an object")
        normalize_workflow_model_paths(workflow)
        client_id = payload.get("client_id") or "mayhem-comfyui"
        timeout_ms = int(payload.get("timeout_ms") or 300000)
        async with session.post(
            "http://mayhem/prompt",
            json={"prompt": workflow, "client_id": client_id},
        ) as response:
            submitted = await response.json()
        if "prompt_id" not in submitted:
            detail = json.dumps(submitted, sort_keys=True, separators=(",", ":"))[:4096]
            raise RuntimeError(f"ComfyUI prompt submission failed: {detail}")
        prompt_id = submitted["prompt_id"]
        deadline = asyncio.get_event_loop().time() + timeout_ms / 1000.0
        progress = []
        history = None
        while asyncio.get_event_loop().time() < deadline:
            await asyncio.sleep(POLL_SECONDS)
            async with session.get(f"http://mayhem/history/{prompt_id}") as response:
                history_map = await response.json()
            if prompt_id in history_map:
                history = history_map[prompt_id]
                break
            append_progress(progress, {"kind": "poll", "node": None, "value": {"prompt_id": prompt_id}})
        if history is None:
            raise TimeoutError("ComfyUI workflow did not complete before timeout")
        status = history.get("status", {})
        if status.get("status_str") != "success":
            raise RuntimeError(f"ComfyUI workflow failed: {status}")
        append_history_progress(progress, history, prompt_id)
        artifacts = collect_artifacts(prompt_id, history)
        return {
            "prompt_id": prompt_id,
            "artifacts": artifacts,
            "progress_events": progress,
        }


async def run_workflow_internal(payload):
    import execution

    with materialized_input_files(payload):
        workflow = payload["workflow"]
        if not isinstance(workflow, dict):
            raise ValueError("workflow must be an object")
        normalize_workflow_model_paths(workflow)
        client_id = payload.get("client_id") or "mayhem-comfyui"
        timeout_ms = int(payload.get("timeout_ms") or 300000)
        prompt_id = str(uuid.uuid4())
        number = prompt_server.number
        prompt_server.number += 1
        prompt = prompt_server.trigger_on_prompt({"prompt": workflow, "client_id": client_id})["prompt"]
        prompt_server.node_replace_manager.apply_replacements(prompt)
        valid = await execution.validate_prompt(prompt_id, prompt, None)
        if not valid[0]:
            detail = {"error": valid[1]}
            if len(valid) > 3:
                detail["node_errors"] = valid[3]
            raise RuntimeError(f"ComfyUI workflow failed validation: {detail}")
        extra_data = {"client_id": client_id, "create_time": int(time.time() * 1000)}
        outputs_to_execute = valid[2]
        sensitive = {}
        for sensitive_val in execution.SENSITIVE_EXTRA_DATA_KEYS:
            if sensitive_val in extra_data:
                sensitive[sensitive_val] = extra_data.pop(sensitive_val)
        deadline = loop.time() + timeout_ms / 1000.0
        progress = []
        history = None
        with capture_prompt_server_progress(prompt_id, progress):
            prompt_server.prompt_queue.put((number, prompt_id, prompt, extra_data, outputs_to_execute, sensitive))
            while loop.time() < deadline:
                await asyncio.sleep(POLL_SECONDS)
                history_map = prompt_server.prompt_queue.get_history(prompt_id=prompt_id)
                if prompt_id in history_map:
                    history = history_map[prompt_id]
                    break
                append_progress(progress, {"kind": "poll", "node": None, "value": {"prompt_id": prompt_id}})
        if history is None:
            raise TimeoutError("ComfyUI workflow did not complete before timeout")
        status = history.get("status", {})
        if status.get("status_str") != "success":
            raise RuntimeError(f"ComfyUI workflow failed: {status}")
        append_history_progress(progress, history, prompt_id)
        artifacts = collect_artifacts(prompt_id, history)
        return {
            "prompt_id": prompt_id,
            "artifacts": artifacts,
            "progress_events": progress,
        }


async def shutdown():
    global runner, session
    if session is not None:
        await session.close()
    if prompt_server is not None:
        client_session = getattr(prompt_server, "client_session", None)
        if client_session is not None:
            await client_session.close()
    if runner is not None:
        await runner.cleanup()


def dispatch(message):
    op = message.get("op")
    payload = message.get("payload")
    if op == "load":
        return load(payload)
    if op == "run_workflow":
        return loop.run_until_complete(run_workflow(payload))
    if op == "shutdown":
        loop.run_until_complete(shutdown())
        return {"shutdown": True}
    raise ValueError(f"unknown operation {op!r}")


def main_loop():
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            break
        message = json.loads(line)
        message_id = message["id"]
        try:
            result = dispatch(message)
            reply(message_id, True, result=result)
            if message.get("op") == "shutdown":
                break
        except Exception as error:
            reply(message_id, False, error="".join(traceback.format_exception(error)))


main_loop()
loop.close()
