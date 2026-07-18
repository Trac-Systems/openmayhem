import contextlib
import pathlib
import sys
import types
import unittest


def load_worker():
    numpy = types.ModuleType("numpy")
    numpy.float32 = object()
    soundfile = types.ModuleType("soundfile")
    soxr = types.ModuleType("soxr")

    torch = types.ModuleType("torch")
    torch.float32 = object()
    torch.is_tensor = lambda value: False
    torch.inference_mode = contextlib.nullcontext
    torch.cuda = types.SimpleNamespace(is_available=lambda: False)
    torch.backends = types.SimpleNamespace(
        mps=types.SimpleNamespace(is_available=lambda: False),
        cuda=types.SimpleNamespace(matmul=types.SimpleNamespace(allow_tf32=False)),
        cudnn=types.SimpleNamespace(allow_tf32=False),
    )
    torch.device = lambda kind: types.SimpleNamespace(type=kind)

    transformers = types.ModuleType("transformers")
    transformers.AutoModelForTDT = object()
    transformers.AutoProcessor = object()
    transformers_utils = types.ModuleType("transformers.utils")
    transformers_utils.logging = types.SimpleNamespace(
        set_verbosity_error=lambda: None
    )

    for name, module in [
        ("numpy", numpy),
        ("soundfile", soundfile),
        ("soxr", soxr),
        ("torch", torch),
        ("transformers", transformers),
        ("transformers.utils", transformers_utils),
    ]:
        sys.modules[name] = module

    worker_path = pathlib.Path(__file__).parents[1] / "src" / "transformers_asr_worker.py"
    worker = types.ModuleType("transformers_asr_worker_under_test")
    worker.__file__ = str(worker_path)
    exec(compile(worker_path.read_bytes(), str(worker_path), "exec"), worker.__dict__)
    return worker


class FakeSamples:
    def __init__(self, origin, length):
        self.origin = origin
        self.shape = (length,)

    def __getitem__(self, item):
        if not isinstance(item, slice) or item.step is not None:
            raise AssertionError(f"unexpected sample index {item!r}")
        start = 0 if item.start is None else item.start
        stop = self.shape[0] if item.stop is None else item.stop
        return FakeSamples(self.origin + start, stop - start)


class TransformersAsrWorkerTest(unittest.TestCase):
    def setUp(self):
        self.worker = load_worker()

    def test_transcribe_window_uses_canonical_processor_timestamps(self):
        sequences = object()
        durations = object()

        class Processor:
            def __init__(self):
                self.decode_call = None

            def __call__(self, samples, **kwargs):
                self.process_call = (samples, kwargs)
                return {}

            def decode(self, decoded_sequences, **kwargs):
                self.decode_call = (decoded_sequences, kwargs)
                return (
                    ["Transformers timestamps."],
                    [
                        [
                            {"token": "Trans", "start": 0.123, "end": 0.234},
                            {"token": "formers", "start": 0.234, "end": 0.987},
                            {"token": " time", "start": 1.234, "end": 1.345},
                            {"token": "stamps", "start": 1.345, "end": 1.999},
                            {"token": ".", "start": 1.999, "end": 1.999},
                        ]
                    ],
                )

        class Model:
            def generate(self, **kwargs):
                self.generate_kwargs = kwargs
                return types.SimpleNamespace(
                    sequences=sequences,
                    durations=durations,
                )

        samples = object()
        processor = Processor()
        model = Model()
        self.worker.processor = processor
        self.worker.model = model
        self.worker.device = types.SimpleNamespace(type="cpu")
        self.worker.sample_rate = 12345
        self.worker.subsampling_factor = 777

        text, words = self.worker.transcribe_window(samples)

        self.assertEqual(text, "Transformers timestamps.")
        self.assertEqual(
            words,
            [
                {"text": "Transformers", "start": 0.123, "end": 0.987},
                {"text": "timestamps.", "start": 1.234, "end": 1.999},
            ],
        )
        self.assertEqual(
            processor.process_call,
            (samples, {"sampling_rate": 12345, "return_tensors": "pt"}),
        )
        self.assertIs(processor.decode_call[0], sequences)
        self.assertIs(processor.decode_call[1]["durations"], durations)
        self.assertTrue(processor.decode_call[1]["skip_special_tokens"])
        self.assertEqual(model.generate_kwargs, {"return_dict_in_generate": True})

    def test_canonical_token_chunks_form_punctuated_words(self):
        words = self.worker.words_from_timestamps(
            [
                {"token": "Qu", "start": 0.08, "end": 0.16},
                {"token": "ilter", "start": 0.16, "end": 0.40},
                {"token": "'", "start": 0.40, "end": 0.40},
                {"token": "s", "start": 0.48, "end": 0.56},
                {"token": " well", "start": 0.72, "end": 0.96},
                {"token": "-", "start": 0.96, "end": 0.96},
                {"token": "being", "start": 1.04, "end": 1.36},
                {"token": "!", "start": 1.36, "end": 1.36},
            ]
        )

        self.assertEqual(
            words,
            [
                {"text": "Quilter's", "start": 0.08, "end": 0.56},
                {"text": "well-being!", "start": 0.72, "end": 1.36},
            ],
        )

    def test_transcribe_window_rejects_decode_without_canonical_timestamps(self):
        class Processor:
            def __call__(self, samples, **kwargs):
                return {}

            def decode(self, sequences, **kwargs):
                return ["timestamp payload missing"]

        class Model:
            def generate(self, **kwargs):
                return types.SimpleNamespace(
                    sequences=object(),
                    durations=object(),
                )

        self.worker.processor = Processor()
        self.worker.model = Model()
        self.worker.device = types.SimpleNamespace(type="cpu")

        with self.assertRaisesRegex(RuntimeError, "canonical timestamps"):
            self.worker.transcribe_window(object())

    def test_long_audio_overlap_stitching_is_repeatable(self):
        windows = {
            0: [
                {"text": "Alpha", "start": 0.2, "end": 0.8},
                {"text": "boundary.", "start": 9.2, "end": 9.8},
                {"text": "Next", "start": 10.2, "end": 10.6},
            ],
            8: [
                {"text": "boundary.", "start": 1.2, "end": 1.8},
                {"text": "Next", "start": 2.2, "end": 2.6},
                {"text": "middle.", "start": 11.2, "end": 11.8},
                {"text": "Final", "start": 12.2, "end": 12.6},
            ],
            18: [
                {"text": "middle.", "start": 1.2, "end": 1.8},
                {"text": "Final", "start": 2.2, "end": 2.6},
                {"text": "tail.", "start": 6.5, "end": 7.0},
            ],
        }
        calls = []

        def transcribe_window(samples):
            calls.append(samples.origin)
            return "unused context text", [dict(word) for word in windows[samples.origin]]

        self.worker.model = object()
        self.worker.processor = object()
        self.worker.sample_rate = 1
        self.worker.CORE_CHUNK_SECONDS = 10.0
        self.worker.LEFT_CONTEXT_SECONDS = 2.0
        self.worker.RIGHT_CONTEXT_SECONDS = 1.0
        self.worker.decode_audio = lambda encoded: (FakeSamples(0, 25), 25.0)
        self.worker.transcribe_window = transcribe_window

        first = self.worker.transcribe({"audio_base64": "ignored"})
        second = self.worker.transcribe({"audio_base64": "ignored"})

        self.assertEqual(first, second)
        self.assertEqual(calls, [0, 8, 18, 0, 8, 18])
        self.assertEqual(first["text"], "Alpha boundary. Next middle. Final tail.")
        self.assertEqual(
            first["words"],
            [
                {"text": "Alpha", "start": 0.2, "end": 0.8},
                {"text": "boundary.", "start": 9.2, "end": 9.8},
                {"text": "Next", "start": 10.2, "end": 10.6},
                {"text": "middle.", "start": 19.2, "end": 19.8},
                {"text": "Final", "start": 20.2, "end": 20.6},
                {"text": "tail.", "start": 24.5, "end": 25.0},
            ],
        )

    def test_overlap_boundaries_are_partitioned_without_dropping_words(self):
        words = self.worker.reconcile_word_timestamps(
            [
                {"text": "before", "start": 9.6, "end": 10.2},
                {"text": "after", "start": 9.9, "end": 10.5},
            ],
            11.0,
        )

        self.assertEqual([word["text"] for word in words], ["before", "after"])
        self.assertGreater(words[0]["end"], words[0]["start"])
        self.assertGreater(words[1]["end"], words[1]["start"])
        self.assertLessEqual(words[0]["end"], words[1]["start"])


if __name__ == "__main__":
    unittest.main()
