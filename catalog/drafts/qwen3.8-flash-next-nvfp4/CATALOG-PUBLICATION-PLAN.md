# Qwen3.8 Flash-Next NVFP4 catalog publication plan

This directory is preparation material. It is not a signed catalog release and
must not be used to mutate the live ledger.

## Canonical identity

- Public model ID: `Qwen/Qwen3.8-Flash-Next`.
- Official model revision used for model-card, configuration, and license
  research: `de4b8e4d43b917e7706784d8bb445c9af86a3540`.
- Calibrated upstream artifact:
  `RadixArk/Qwen3.8-Flash-Next-NVFP4@7b719225242aacd3dbd3f9407468c2ee9a9d2594`.
- Immutable catalog mirror:
  `TracNetwork/mayhem-catalog-Qwen-Qwen3-8-Flash-Next-NVFP4@5cc87d1eceade9cef91f143a184dee7e6475bc8d`.
  Independent post-upload verification found exactly 425 files and
  135,318,119,812 bytes, with no path, size, or content-identity mismatch.
- Calibrated snapshot: 419 files and 135,253,622,894 bytes. Its unchanged
  `download-manifest.json` has SHA-256
  `5995053edba6d997fa0b4e45363293c0bd4d670f5824c2b6b7f71a7dccc54374`.
  It is the sole file list for materialization and verification and is also
  stored unchanged as the mirror's 111,025-byte `download-manifest.json`
  sidecar (Git blob `09ebe2b9b2d9bc6f6870d37eb246a739891164cf`).
- Mirror compliance file: the 3,235-byte `LICENSE` from the official initial
  revision `34567a4712bc9766c4449e2e98e4468bfa24d915`, SHA-256
  `a0dc422560841fd68e06d974907f8b4c709bca44a67daad2b528437bdf676c08`.
  It is present in the mirror but excluded from the calibrated snapshot,
  directory artifact root, and runtime materialization.

The catalog artifact source must point at the final mirror revision. Its
`upstream_source` must retain the RadixArk repository and pinned revision. The
unchanged snapshot manifest retains its original ModelScope acquisition fields
and canonical Hugging Face revision. Core validates those as upstream identity,
not as the mirror download location.

## License disposition

The exact identifier is `qwen-community-1.0`. Clause 1 permits use,
modification, publishing, distribution, deployment, and hosting, provided that
the copyright and permission notice accompany copies or substantial portions.
It also requires prominent model naming for a commercial product or service
above either 100 million monthly active users or USD 20 million monthly
revenue. Clause 2 requires a separate Qwen license before commercial use in a
Model-as-a-Service or AI Work Assistant business; its internal-use exception
does not expose the model, its outputs, or its capabilities to third parties.

The immutable mirror is permitted when it includes the notice. Paid third-party
inference activation remains gated on recording the applicable separate
commercial hosted-use license. This preparation neither claims nor denies that
the operator already has such a license.

## Catalog values

- `model_class`: `text-generation`
- `family`: `qwen3.8`
- `params_b`: `125`, matching the official base model comparison table. The
  auxiliary n-gram embeddings and MTP component must be described in notes
  rather than added to this field.
- `tier`: `launch`
- `min_app_version`: `0.2.205`, the first release that preserves the signed PLE
  reader wheel filename during offline installation, maps managed preparation
  containers to the provider uid/gid, accepts the signed source Git identity,
  validates all signed reasoning preflight controls together, observes the
  signed concurrency gauge while bounded probe workers remain live, derives
  boundary-independent OpenAI-compatible calibration units, accepts signed
  runtime KV-cache metadata, and separates video input from video output caps.
- `provenance.license`: `qwen-community-1.0`
- `provenance.license_sha256`:
  `a0dc422560841fd68e06d974907f8b4c709bca44a67daad2b528437bdf676c08`
- `caps.ctx_max`: `262144`, the native model context.
- Served runtime context: `524288`, bound only in
  `artifact.openai_compatible.served_context`; it is a YaRN factor-2 runtime
  configuration and must not replace native `caps.ctx_max`.
- Input modalities: text, image, video. Output modality: text.
- Features: streaming, reasoning, tools, structured JSON, prefix cache,
  cancellation, image, and video.
- Thinking-on sampling: temperature 1.0, top-p 0.95, top-k 20, min-p 0,
  presence penalty 0, repeat penalty 1.
- Thinking-off sampling: temperature 0.7, top-p 0.8, top-k 20, min-p 0,
  presence penalty 1.5, repeat penalty 1.
- Thinking controls: `enable_thinking=true` and `preserve_thinking=true` by
  default; `reasoning_effort` is `low|medium|xhigh` with `xhigh` as the catalog
  default.
- Reuse the Qwen3.8-27B adapter schemas for OpenAI chat completions,
  completions, responses, and HF multimodal chat, substituting this exact model
  ID. Do not reuse fingerprints or resource measurements.

The required provenance conversion row describes the byte mirror, not the
upstream quantization: tool `huggingface_hub`, method
`immutable-byte-mirror:pinned-radixark-nvfp4-snapshot-manifest`, with input and
output SHA-256 both equal to `5995053edba6d997fa0b4e45363293c0bd4d670f5824c2b6b7f71a7dccc54374`.
The artifact notes bind `hf_quant_config.json` SHA-256
`7e69ef4b94302ae5b6f453b913621f698d5631a1d023d8b3e9e3b829721b98e8`
and `conversion_environment.json` SHA-256
`10dadc9b3421b669e533ffcdb3ffb1bf56f42d35936fc24aa2020db0df7480ac`.
The upstream card does not pin every base checkpoint input used by RadixArk;
record that limitation without claiming OpenMayhem performed the NVFP4
quantization.

The primary artifact uses engine `openai-compatible`. Its signed runtime
binding is prepared in `runtime-binding.template.json`. It pins Pennyroyal
2.5.0 at revision `2c675da096939cb01102f8f4871bda3db55f7f28`, SGLang
`0.0.0.dev1+gd91c3682b`, and container
`lmsysorg/sglang@sha256:12d3392bdc8be8d35e9a95f191df6aef99c5114bdbefd41bfdc7e760e6d25ec1`.
The binding uses lifecycle `managed_or_verified_attach`, advertises maximum
concurrency 2, and contains no endpoint or host location.

The retained direct runtime advertised backend model name `pennyroyal` and set
the startup `reasoning_effort` default to `medium`. Both conflict with the
public contract. The managed recipe must use served model name
`Qwen/Qwen3.8-Flash-Next` and startup default `xhigh`; `/v1/models` and signed
`/server_info` checks must agree. This semantic-only recipe correction requires
a focused identity and functional canary rerun before activation.

The final typed recipe is 3,204 bytes, SHA-256
`6c4e24903656268db5385e5e225d11e0666d3573719cf99cb6f468ef29749c67`,
and Merkle root
`a41292018608f3f50dac61e20220e91b1ce0c9f92d0fd60842f85dbbaa801576`.
Its plugin source inventory contains 23 files, 3 directories, and 135,708
bytes, with inventory SHA-256
`b5bd6371a3974e2fa0e21ea37537a15a4262d8cc01d3315a8fac551d72cce259`.
The earlier inventory included a generated `Cargo.lock` that is absent from the
signed clean source bundle and is superseded.
Its Pennyroyal source bundle is 64,070,181 bytes, SHA-256
`776f6d4b1883c78c7d1eaafa23a936fe82d6cc2220bfb9631ec3598b5e342df4`,
and Merkle root
`72c6b143926e6d669f474b52e9ddfecd4b25dbc328edb817ee9600bb0e8dbe23`.
The bundle includes the shallow Git identity required by the qualified launcher;
the earlier rootless git archive identity is invalid and must never be used.
The 16,286-byte Docker 29.1.3 seccomp profile has SHA-256
`c7a33fb8ae1f8346356a61ce833d579c45acf2bc94967c6763634e81010ff816`
and Merkle root
`f3ad6f80ad1ef9702ba87171fd18890d655cfd0df21553b3827c5846dec5a915`.

The PLE reader is supplied as the 292,987-byte platform wheel
`sglang_ssd_stream-0.2.0+pennyroyal2-cp312-cp312-linux_x86_64.whl`, SHA-256
`5fd3bf79524aec7068729e99823a8278e4f3bd7dcacd222f2c55f4395454112f`,
and Merkle root
`c53a31e21cd48728e6a6d71b9c7a63927f062efe77c2661dee4d37d4803321ac`.
The recipe restricts it to CPython 3.12, the CPython 3.12 ABI, and
`linux_x86_64`, and installs it offline without dependencies. An empty-cache
offline install, import, and full PLE checker passed. The earlier source-build
recipe depended on build tooling acquired during qualification and cannot
satisfy clean-provider materialization; its recipe identity is superseded.

The recipe deterministically derives, rather than downloads, its PLE table from
the signed model snapshot. The expected result is 51,200,245,760 bytes,
SHA-256 `b070f9644adf93794d8a1030584ab705809387e64396a9327a68fa3a3a6666b3`,
320,001,536 rows by 160 columns in `float8_e4m3fn`. The derived portable
manifest is 2,563 bytes with SHA-256
`f3a5a692d577457a1b6166ec17b3006ebc55804812400ac3a7bfaa3ce5af75bd`.
The PLE table is not a catalog sidecar.

## Pricing and activity calibration

Catalog `price_ref_au` is denominated in atto-USD per 1,000 tokens. The draft
Tier 1 rates are:

| Dimension | USD per million | Atto-USD per 1,000 |
| --- | ---: | ---: |
| input | 0.059628 | 59,628,000,000,000 |
| cached input | 0.014907 | 14,907,000,000,000 |
| output | 0.201245 | 201,245,000,000,000 |

The Tier 2 draft is an exact 1.5 multiplier: input
89,442,000,000,000, cached input 22,360,500,000,000, and output
301,867,500,000,000 atto-USD per 1,000. The catalog compatibility fields carry
input and output. Each ledger price entry must carry the sorted three-dimension
rate map, including cached input.

`activity-calibration-evidence.json` has SHA-256
`45911631224860d4bb273278ec4cf2b062e48bd061deffa69fe7de9ece93c3d2`.
Its reference work is derived from the retained concurrent prefill/decode run:
520,000 input tokens over 61,440,000 microseconds and 2,046 post-first-token
output tokens over 32,016,500 microseconds. Cached input uses a conservative
upper bound of 25,472 tokens over 232,011 microseconds. Ledger publication uses
that file hash as the activity calibration source hash.

These prices were selected from a provisional 50% undercut of the lower
ordinary competitor observed on 2026-09-13. Refresh competitor prices and get
economic-owner acceptance immediately before proposal; do not silently carry
the provisional values into a signed release.

## Resource admission

The retained concurrency-two run used two simultaneous requests, each with
260,000 prompt tokens and 1,024 requested output tokens. It observed 2 running
requests, 824,384 configured total tokens, 96,462,700,544 bytes peak GPU memory
used, 5,486,149,632 bytes minimum free GPU memory, 603.87 W peak board power,
and 519.81 W mean board power. The two observed time-to-first-token values were
20.9161 s and 40.5239 s; the requests overlapped for 5.9372 s.

A 15% F13 budget derived from that peak is 113,485,530,052 bytes
(105.69 GiB). The qualified host did not prove that portable floor on a nominal
96 GiB device. Do not invent `requirements.min_vram_gb_full_offload` or a
portable F13 admission envelope from this run. Measure the final portable
hardware class or obtain an explicitly reviewed admission policy.

Image and video passed functional probes, but neither has a maximum-size media
working-set profile. Launch admission requires separate image and video
resource profiles with measured item bytes/units, baseline/peak memory, F13
budget, and default inflight limits.

## Canary and reuse policy

Use the three-prompt `canary-qwen3.8-flash-next-speciality-v1` input at source
SHA-256 `fcbfe62df2f134d2d32413769a7a716e928b0a5ca5382efbbbb8ddb98b92785b`.
Its deterministic text prompt assesses all seven signed speciality levels, and
its isolated image and video prompts measure the missing resource profiles and
new response fingerprints. Retain the direct 9-of-9 functional evidence for
unchanged capabilities. No Qwen3.8-27B fingerprint is copied.

The video prompt uses the OpenAI-compatible `video_url.url` data URL accepted by
the pinned SGLang schema. Its nested object also retains the same base64 payload,
MIME type, frame rate, and frame count so Core can attribute the signed resource
profile: 17,042 bytes, SHA-256
`365754adff9583755e7c2bbd7f5cc0a0c614902c0ffd2ddb57ef4395b2238581`,
8 fps, and 16 frames.

The long 260k concurrency-two qualification need not be repeated when the final
419-file snapshot, directory artifact root, container, runtime revision,
executable plugin source, capacity, and scheduling settings remain unchanged.
The switch from an on-provider source build to the signed wheel is acceptable
for reuse only after its empty-cache install, import, and PLE checker evidence
is retained and the final native-provider path passes a cheap two-request
overlap probe. Repeat the long proof if any weight, loader behavior, executable
plugin source, capacity, token pool, scheduling, cache, or context setting
changes.

The signed overlap probe retains `concurrency_max_tokens: 4096` with
`ignore_eos` enforced by Core. Core v0.2.203 polls the signed active gauge while
the first-token barriers are held, requires an observed value of at least two
and first content from both requests, then cancels the bounded probes.

Core v0.2.205 reconstructs reasoning as `<think>…</think>` followed by visible
content, then derives canonical Unicode-scalar units for calibration and live
catalog verification. The fingerprints remain stable across upstream stream
segmentation while normal delivery and billing remain unchanged.

## Exact semantic catalog diff

The baseline is `origin/main` commit
`ea38f49fcd941b3e4e12546095220c53ccbedab0`. Its catalog SHA-256 is
`2112bce6b7f8fa5660c81a81b22f9d4c4f19c263972939601ff4512e5cb2c069`;
the detached signature SHA-256 is
`8245d5aa93930036277ff6cee0ad27b10a277b74486b23ccba10a23dd85a9c33`.
It contains 22 models, 1 generation execution profile, 1 vLLM execution mode,
and 6 enclave attestation bindings.

The publication diff must append exactly one model entry and one
`generation_execution_profiles` entry keyed by the final directory artifact
root. The profile engine is `openai-compatible`, permits only the proved two
independently dispatched text requests, and binds the retained concurrency
evidence hash. No vLLM execution mode is added. All pre-existing entries and
their order remain byte-for-byte semantically unchanged. The old signature is
not edited during preparation; release tooling replaces it only after the
complete catalog validates and an authorized signer signs it.

The model artifact has exactly five catalog sidecars: the unchanged snapshot
manifest, the typed runtime recipe, the Pennyroyal source bundle, the signed
PLE reader wheel, and the recipe-referenced seccomp profile. The 419 model
files are not represented as individual catalog sidecars.

## Ledger and release gates

1. Preserve the verified mirror revision. Its exact 425-object set has all 419
   manifest rows matching path, size, and content identity, with the manifest
   sidecar and `LICENSE` also matching their official bytes. Repeat this
   verification immediately before catalog signing to detect remote drift.
2. Use the verified 419-file Core directory artifact root
   `78191c28fa91602e536baa8ede39891d6d500615086ce2ba42cb920c3b46eabe`
   in the catalog, generation profile, model reference, and enclave
   registration. The framed directory stream SHA-256 is
   `e59e92ab3ce981a018c412134707d12bd293bd54d8ab42ba30c5f1037d0031ae`
   over 135,253,645,960 stream bytes.
3. Bind the five exact sidecars: `snapshot_manifest`, `runtime_recipe`,
   `pennyroyal_source`, `ple_plugin_wheel`, and `seccomp_profile`. All are
   already present at the final mirror revision.
4. Complete portable text F13 and image/video resource profiles.
5. Install the generalized `openai-compatible` Core implementation and validate
   every signed `server_info_checks` pointer against the attached runtime.
6. Run native-provider discovery, model listing, all endpoint families,
   streaming, reasoning, tools, JSON, media, prefix cache, cancellation, and
   independent concurrency-two dispatch. Run the same canonical canary through
   paid gateway routes and retain billing/settlement evidence.
7. Confirm the applicable Qwen commercial hosted-use license, refresh pricing,
   and record economic-owner acceptance.
8. Validate and sign the complete catalog offline. Propose the exact catalog
   anchor, then simulate the model reference, Tier 1 enclave and price, required
   Tier 2 enclave-market registration (which may be empty of providers), and
   paid rooms. The calibration host's missing endorsement-key certificate is a
   host qualification limitation, not permission to omit the Tier 2 market.
9. Commit authorized ledger operations, publish the signed catalog pointer,
    wait for readers to converge, and rerun independent model listing, paid
    routes, canaries, accounting, and settlement verification.

No version, tag, signature, catalog pointer, ledger operation, or live provider
deployment belongs in this preparation branch.
