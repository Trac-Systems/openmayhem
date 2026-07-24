import base64
import hashlib
import importlib.util
import io
import json
import os
import pathlib
import struct
import sys
import tempfile
import unittest
import wave
from unittest import mock


WORKER_PATH = (
    pathlib.Path(__file__).resolve().parents[1] / "src" / "chatterbox_worker.py"
)
SPEC = importlib.util.spec_from_file_location("mayhem_chatterbox_worker", WORKER_PATH)
worker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(worker)


class _Cuda:
    seeds = []

    @classmethod
    def is_available(cls):
        return False

    @classmethod
    def manual_seed(cls, seed):
        cls.seeds.append(("one", seed))

    @classmethod
    def manual_seed_all(cls, seed):
        cls.seeds.append(("all", seed))


class _Mps:
    @staticmethod
    def is_available():
        return False


class _Torch:
    cuda = _Cuda
    backends = type("Backends", (), {"mps": _Mps})
    seeds = []

    @classmethod
    def manual_seed(cls, seed):
        cls.seeds.append(seed)


class _NumpyRandom:
    seeds = []

    @classmethod
    def seed(cls, seed):
        cls.seeds.append(seed)


class _Numpy:
    random = _NumpyRandom


class _FakeTokens:
    def __init__(self, count):
        self.shape = (1, count)


class _FakeTokenizer:
    def __init__(self):
        self.calls = []
        self.forced_token_count = None

    def text_to_tokens(self, text):
        self.calls.append(text)
        count = self.forced_token_count
        return _FakeTokens(len(text) if count is None else count)


class _FakeT3:
    hp = type("Hp", (), {"max_text_tokens": 2_048})()


class _FakeConditionals(dict):
    def __deepcopy__(self, _memo):
        raise RuntimeError("non-leaf tensor conditionals cannot be deep-copied")


class _FakeModel:
    sr = 24_000

    def __init__(self):
        self.conds = _FakeConditionals(voice="builtin")
        self.calls = []
        self.t3 = _FakeT3()
        self.tokenizer = _FakeTokenizer()

    def generate(
        self,
        text,
        repetition_penalty=1.2,
        min_p=0.05,
        top_p=1.0,
        audio_prompt_path=None,
        exaggeration=0.5,
        cfg_weight=0.5,
        temperature=0.8,
    ):
        reference = None
        if audio_prompt_path is not None:
            path = pathlib.Path(audio_prompt_path)
            reference = path.read_bytes()
            self.conds = {"voice": "clone"}
        self.calls.append(
            {
                "audio_prompt_path": audio_prompt_path,
                "cfg_weight": cfg_weight,
                "conds": self.conds.copy(),
                "exaggeration": exaggeration,
                "min_p": min_p,
                "reference": reference,
                "repetition_penalty": repetition_penalty,
                "temperature": temperature,
                "text": text,
                "top_p": top_p,
            }
        )
        return object()


class _FakeChatterboxTTS:
    model = None
    generate = _FakeModel.generate

    @classmethod
    def from_local(cls, model_root, device):
        cls.model = _FakeModel()
        return cls.model


class _DirectUrlDistribution:
    def __init__(self, commit):
        self.commit = commit

    def read_text(self, name):
        if name != "direct_url.json":
            return None
        return json.dumps(
            {
                "url": "https://github.com/resemble-ai/Perth.git",
                "vcs_info": {
                    "commit_id": self.commit,
                    "requested_revision": self.commit,
                    "vcs": "git",
                },
            }
        )


def _fake_punc_norm(text):
    text = " ".join(text.split())
    if text and text[0].islower():
        text = text[0].upper() + text[1:]
    if text and text[-1] not in ".!?-,":
        text += "."
    return text


class ChatterboxWorkerTests(unittest.TestCase):
    def setUp(self):
        self.environment = mock.patch.dict(
            os.environ,
            {
                "HF_HUB_OFFLINE": "1",
                "HF_DATASETS_OFFLINE": "1",
                "TRANSFORMERS_OFFLINE": "1",
                "DIFFUSERS_OFFLINE": "1",
                "PIP_NO_INDEX": "1",
                "UV_OFFLINE": "1",
                "MAYHEM_CHATTERBOX_DEVICE": "cpu",
            },
            clear=False,
        )
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.model_root = self.root / "model"
        self.cache_root = self.root / "cache"
        self.model_root.mkdir()
        self.cache_root.mkdir()
        for name in worker.REQUIRED_MODEL_FILES:
            (self.model_root / name).write_bytes(b"fixture")
        worker._runtime = None
        _Torch.seeds.clear()
        _Cuda.seeds.clear()
        _NumpyRandom.seeds.clear()

    def load(self, input_character_limit=2_048):
        dependencies = mock.patch.object(
            worker,
            "_load_runtime_dependencies",
            return_value=(
                _Numpy,
                _Torch,
                _FakeChatterboxTTS,
                _fake_punc_norm,
                "0.1.7",
            ),
        )
        source_check = mock.patch.object(
            worker,
            "_verify_runtime_sources",
            return_value=worker.TTS_SOURCE_SHA256,
        )
        dependencies.start()
        source_check.start()
        self.addCleanup(dependencies.stop)
        self.addCleanup(source_check.stop)
        return worker._handle_load(
            {
                "cache_root": str(self.cache_root),
                "input_character_limit": input_character_limit,
                "model_root": str(self.model_root),
            }
        )

    def test_load_is_original_offline_and_reports_enforced_pins(self):
        with mock.patch.object(
            pathlib.Path,
            "resolve",
            side_effect=AssertionError("sandbox worker must not re-resolve trusted roots"),
        ):
            result = self.load(4_096)
        config = result["execution_config"]
        self.assertEqual(config["model_family"], "original_english")
        self.assertEqual(
            config["source_commit"],
            "59bc590b3cad826e5d5987745bf6844627a21ad5",
        )
        self.assertEqual(
            config["perth_commit"],
            "ce86c49d029f42272c1902eccb675556b9ed2330",
        )
        self.assertEqual(config["runtime_source_sha256"], worker.TTS_SOURCE_SHA256)
        self.assertEqual(config["input_character_limit"], 4_096)
        self.assertEqual(config["max_text_tokens"], 2_048)
        self.assertEqual(config["reference_audio_limit_seconds"], 10)
        self.assertEqual(config["t3_reference_seconds"], 6)
        self.assertEqual(config["s3gen_reference_seconds"], 10)
        self.assertEqual(result["n_ctx_train"], 2_048)

    def test_controls_and_inline_clone_reach_exact_generate_call(self):
        self.load()
        reference = pcm_wav(24_000, 24_000)
        with mock.patch.object(
            worker,
            "_waveform_to_pcm16",
            return_value=(b"\x00\x00" * 24_000, 24_000),
        ):
            result = worker._handle_synthesize(
                request_payload(
                    reference=reference,
                    exaggeration=0.7,
                    cfg_weight=0.3,
                    temperature=1.1,
                    seed=7,
                    min_p=0.08,
                    top_p=0.9,
                    repetition_penalty=1.4,
                )
            )
        call = _FakeChatterboxTTS.model.calls[-1]
        self.assertEqual(call["text"], "Clone this voice")
        self.assertEqual(call["reference"], reference)
        self.assertEqual(call["exaggeration"], 0.7)
        self.assertEqual(call["cfg_weight"], 0.3)
        self.assertEqual(call["temperature"], 1.1)
        self.assertEqual(call["min_p"], 0.08)
        self.assertEqual(call["top_p"], 0.9)
        self.assertEqual(call["repetition_penalty"], 1.4)
        self.assertEqual(_Torch.seeds, [7])
        self.assertEqual(_NumpyRandom.seeds, [7])
        self.assertEqual(result["reference_audio_used"], True)
        self.assertEqual(result["seed_applied"], True)
        self.assertEqual(result["sample_rate"], 24_000)
        self.assertEqual(result["sample_count"], 24_000)
        self.assertIsNone(_FakeChatterboxTTS.model.conds)
        self.assertFalse(
            any((self.cache_root / "inputs").glob("reference-*.wav")),
            "inline voice reference must be deleted after generation",
        )

    def test_clone_conditionals_do_not_bleed_into_the_next_default_voice(self):
        self.load()
        reference = pcm_wav(24_000, 24_000)
        with mock.patch.object(
            worker,
            "_waveform_to_pcm16",
            return_value=(b"\x00\x00" * 24_000, 24_000),
        ):
            worker._handle_synthesize(request_payload(reference=reference))
            worker._handle_synthesize(request_payload(reference=None))
        clone_call, default_call = _FakeChatterboxTTS.model.calls
        self.assertEqual(clone_call["conds"], {"voice": "clone"})
        self.assertEqual(default_call["conds"], {"voice": "builtin"})
        self.assertIsNone(_FakeChatterboxTTS.model.conds)

    def test_reference_file_permission_failure_is_immediate_not_retried(self):
        inputs = self.cache_root / "inputs"
        inputs.mkdir()
        with mock.patch.object(
            pathlib.Path,
            "open",
            side_effect=PermissionError("sandbox denied reference input"),
        ) as open_file:
            with self.assertRaisesRegex(PermissionError, "sandbox denied"):
                worker._write_reference_audio(inputs, b"reference")
        self.assertEqual(open_file.call_count, 1)

    def test_reference_file_name_collisions_are_bounded(self):
        inputs = self.cache_root / "inputs"
        inputs.mkdir()
        with mock.patch.object(
            pathlib.Path,
            "open",
            side_effect=FileExistsError("collision"),
        ) as open_file:
            with self.assertRaisesRegex(worker.ProtocolError, "unique Chatterbox"):
                worker._write_reference_audio(inputs, b"reference")
        self.assertEqual(open_file.call_count, worker.MAX_REFERENCE_FILE_ATTEMPTS)

    def test_reference_longer_than_ten_seconds_is_rejected_not_clipped(self):
        self.load()
        reference = pcm_wav(24_000, 240_001)
        with self.assertRaisesRegex(worker.ProtocolError, "10-second"):
            worker._handle_synthesize(request_payload(reference=reference))
        self.assertEqual(_FakeChatterboxTTS.model.calls, [])

    def test_signed_character_limit_is_enforced_without_truncation(self):
        self.load(input_character_limit=8)
        with self.assertRaisesRegex(worker.ProtocolError, "signed Chatterbox"):
            worker._handle_synthesize(request_payload(text="123456789"))
        self.assertEqual(_FakeChatterboxTTS.model.calls, [])

    def test_source_native_text_token_limit_rejects_without_truncation(self):
        self.load(input_character_limit=4_096)
        tokenizer = _FakeChatterboxTTS.model.tokenizer
        tokenizer.forced_token_count = 2_049
        with self.assertRaisesRegex(
            worker.ProtocolError,
            "exceeding pinned T3 max_text_tokens=2048.*not truncated",
        ):
            worker._handle_synthesize(request_payload(text="hello"))
        self.assertEqual(tokenizer.calls, ["Hello."])
        self.assertEqual(_FakeChatterboxTTS.model.calls, [])

    def test_runtime_source_and_perth_commit_are_both_verified(self):
        module_path = self.root / "release_tts.py"
        module_path.write_text("class ReleaseTTS:\n    pass\n", encoding="utf-8")
        spec = importlib.util.spec_from_file_location("release_tts", module_path)
        module = importlib.util.module_from_spec(spec)
        sys.modules["release_tts"] = module
        self.addCleanup(sys.modules.pop, "release_tts", None)
        spec.loader.exec_module(module)
        source_hash = hashlib.sha256(module_path.read_bytes()).hexdigest()
        with mock.patch.object(worker, "TTS_SOURCE_SHA256", source_hash), mock.patch(
            "importlib.metadata.distribution",
            return_value=_DirectUrlDistribution(worker.PERTH_COMMIT),
        ), mock.patch.object(
            pathlib.Path,
            "resolve",
            side_effect=AssertionError("sandbox worker must not re-resolve trusted source"),
        ):
            self.assertEqual(
                worker._verify_runtime_sources(module.ReleaseTTS),
                source_hash,
            )
        with mock.patch.object(worker, "TTS_SOURCE_SHA256", source_hash), mock.patch(
            "importlib.metadata.distribution",
            return_value=_DirectUrlDistribution("0" * 40),
        ):
            with self.assertRaisesRegex(worker.ProtocolError, "must resolve to commit"):
                worker._verify_runtime_sources(module.ReleaseTTS)

    def test_protocol_rejects_unknown_fields_and_emits_canonical_json(self):
        with self.assertRaisesRegex(worker.ProtocolError, "fields must be exactly"):
            worker._handle_message(
                {"id": 1, "op": "shutdown", "payload": None, "extra": True}
            )
        output = io.StringIO()
        with mock.patch.object(worker.sys, "stdout", output):
            worker._emit({"z": 1, "a": 2})
        self.assertEqual(output.getvalue(), '{"a":2,"z":1}\n')


def request_payload(
    *,
    text="Clone this voice",
    reference=None,
    exaggeration=0.5,
    cfg_weight=0.5,
    temperature=0.8,
    seed=None,
    min_p=0.05,
    top_p=1.0,
    repetition_penalty=1.2,
):
    encoded_reference = (
        None
        if reference is None
        else {
            "content_type": "audio/wav",
            "data_base64": base64.b64encode(reference).decode("ascii"),
        }
    )
    return {
        "cfg_weight": cfg_weight,
        "exaggeration": exaggeration,
        "input": text,
        "min_p": min_p,
        "reference_audio": encoded_reference,
        "repetition_penalty": repetition_penalty,
        "seed": seed,
        "temperature": temperature,
        "top_p": top_p,
    }


def pcm_wav(sample_rate, sample_count):
    output = io.BytesIO()
    with wave.open(output, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(sample_rate)
        wav.writeframes(struct.pack("<h", 0) * sample_count)
    return output.getvalue()


if __name__ == "__main__":
    unittest.main()
