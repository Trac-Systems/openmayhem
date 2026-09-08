# Image references

An image-generation catalog contract can expose `input_reference` (a PNG or
JPEG data URL) and `strength` (0–1, default 0.6). The fields are optional for
text-to-image requests. Adding them to an endpoint template does not advertise
them for an existing model; the model requires measured, signed catalog evidence.

The gateway validates decoded image content before opening a provider session.
The reference and strength remain part of the normalized request fingerprint,
and source bytes/pixels participate in route selection and provider admission.
A single measured image capacity profile must cover the source and requested
output; calibration must measure their combined memory working set.

The stable-diffusion.cpp adapter sends exactly one `init_images` entry and an
explicit `denoising_strength` to `/sdapi/v1/img2img`. Without a reference it
continues to use `/sdapi/v1/txt2img`. A confirmed worker-exit retry retains the
same endpoint and request bytes. Cancellation uses the existing request and
upload cancellation path. The signed model's steps/guidance offsets are retained.

Only inline PNG/JPEG content is accepted. The decoder verifies MIME, base64,
dimensions and pixels, with limits of 20 MiB decoded and 40 million pixels.
Provider capacities may be smaller. Remote URLs and local paths are rejected.
Malformed references do not silently fall back to text-only generation.

Strength controls how much the result changes from the source. At 1 the native
engine discards the starting image by design. Reference-effect calibration must
therefore compare fixed prompt/seed requests below 1 with different source images.
Output resizing follows the explicit image-generation dimensions.

The calibration matrix uses a real image fixture for strength boundary cases;
it must not substitute padded text for an encoded image. Functional image
canaries preserve reference, strength and negative prompt, and account for
source as well as output bytes/pixels.
Image boundary cases marked `calibration_only` remain in signed calibration
evidence, while runtime probes use the remaining functional cases. A set must
retain at least one runtime probe.

## Workflow reference files

ComfyUI consumes request-carried files through its input directory for the
duration of the graph. Uploads replace matching provider fixtures temporarily;
the original bytes are restored after success, validation failure or execution
failure. Invalid later attachments cannot strand earlier uploads.

A journal outside the public input directory permits the next worker to restore
files after process termination. An OS lock prevents workers sharing a runtime
cache from recovering or overwriting another worker's active inputs. Recovery
errors stop the request and retain backups. Input and output path checks use
path components, including resolved symlinks, rather than string prefixes.
