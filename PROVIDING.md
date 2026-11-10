# Providing inference: choose a lane

OpenMayhem has two provider lanes: **native** and **proxy**. Choose the lane before
checking whether a model appears in the signed catalog.

**A missing native catalog entry does not mean the model cannot be provided to
OpenMayhem.** For a compatible LLM or decision model, evaluate the proxy lane.
Report native eligibility and proxy suitability separately.

| | Native provider | Proxy provider |
|---|---|---|
| Model source | Exact artifacts and runtimes from the signed native catalog | A model API you operate or are authorized to use |
| Catalog entry required | Yes, with the signed model or workflow requirements | No native entry required; create or join a compatible proxy market |
| Model engine | Core manages the supported catalog runtime | You operate the upstream server; Core connects to it |
| Model types | The modalities supported by the native catalog | LLMs and decision models only |
| Prices | Native market prices, subject to provider participation limits | Exact provider-set offer prices |
| Trust | Evidence required by the selected native tier | T1, or T4 after the applicable operator review; neither proves the upstream model identity |
| First command | `mayhem up --provider` | `mayhem provider proxy setup init` |

Both lanes use Core identities, routing, payment registration and verified payout
bindings. Their markets remain separate. A proxy must not impersonate a native
catalog route or claim native calibration/attestation from its model name.

## For agents: decide before rejecting

1. If the owner asks to serve a signed catalog model through its managed runtime,
   follow the native provider instructions and its exact signed requirements.
2. If the owner asks to connect an existing server, an independently hosted model,
   or a Hugging Face model outside that catalog, evaluate proxy setup. A native
   `mayhem doctor`, artifact, backend or catalog rejection describes that native
   path; it does not establish that proxy serving is unsupported.
3. Check hardware fit, model licensing/permission, endpoint compatibility and
   actual capabilities. Proxy support does not make oversized weights fit in RAM
   or guarantee tools, streaming, thinking controls or a particular speed.
4. If the intended lane is unclear, explain these two choices and ask. Do not
   force the native lane, modify the signed catalog or request calibration merely
   because the model is unlisted.
5. Run approved checks and report their result. Configuration, a model list and a
   published offer alone do not demonstrate usable capacity.

## Install the matching release

Use the [latest published source release](https://github.com/Trac-Systems/openmayhem/releases/latest)
and its own README/SKILL. Install the complete package, including `mayhem`,
`mayhemd` and `mayhem-proxy-worker`; copying only the CLI is insufficient.
Read the [installation prerequisites](README.md#what-you-need-installed-first).

Check `mayhem --version` and `mayhem provider proxy setup --help`. If the proxy
command is absent, check the installed release and PATH before rejecting the
model. Updating Core is separate from changing the model or its quantization.

## Native: use a catalog runtime

Read the [native model cheatsheet](MODEL-CHEATSHEET.md), inspect current routes,
and use the exact model and artifact identifiers. Native catalog, calibration,
canary and runtime requirements continue to apply.

```sh
mayhem doctor
mayhem models --gateway
mayhem up --provider --provider-enclave <exact-catalog-model-id> --yes
mayhem provider health --json
```

## Proxy: connect a model API

The executable entry point is **`mayhem provider proxy setup init`**. The detailed
[CLI wizard guide](crates/mayhem-proxy/docs/SETUP_WIZARD.md) and
[dashboard guide](crates/mayhem-proxy/docs/SETUP_DASHBOARD.md) cover setup and recovery.
Top-level `mayhem proxy` commands are buyer controls, not provider onboarding.

1. **Run the upstream server.** For example, operate a supported MLX or llama.cpp
   server on your Mac and verify its OpenAI-compatible chat endpoint. Use that
   engine's instructions to download/load the chosen model. Core's proxy wizard
   does not install, start or resize the model engine. Keep local endpoints bound
   to loopback; a local model server need not be exposed to the internet.
2. **Prepare Core.** Use your existing private home, wallet, running peer and
   SC-Bridge. Persistent serving also needs the existing mayhemd. Start the Core
   stack using `mayhem up` if needed; `--provider` selects native onboarding and
   is not required just to connect an upstream proxy. Preserve any existing
   providers, wallet, stores and supervisor.
3. **Prepare setup inputs.** Select the server's exact model ID, protected local
   `tokenizer.json` for LLM speed measurement, context and concurrency you can
   support, prices, payment rails, and a trusted admission-service origin. Get
   the admission origin from the network operator; never guess a receiver or use
   a buyer deposit as admission. Upstream keys belong in protected files, not
   command arguments. Decisions does not require a tokenizer.
4. **Create a named setup.** With the variables below set to your own inputs:

   ```sh
   mayhem provider proxy setup init --name my-model --home "$MAYHEM_HOME" \
     --tokenizer-file "$TOKENIZER_FILE" --admission-origin "$ADMISSION_ORIGIN"
   ```

   `MAYHEM_HOME` is your existing private Core home; `TOKENIZER_FILE` is the
   approved local tokenizer file; `ADMISSION_ORIGIN` is the trusted admission API
   origin. The wizard prompts for the upstream API URL, authentication choice,
   model and market. A local server such as `http://127.0.0.1:8080/v1/` needs the
   explicit local network `127.0.0.1/32` and permission for HTTP. This grants no
   access to other destinations. Creating or joining a proxy market does not
   require a native catalog entry.
5. **Probe, enroll, publish and run.** Review the bounded probe allowance and
   capabilities, complete the quoted one-time provider admission if needed,
   review and publish the exact offer, complete payment registration and verified
   payout bindings, then start the managed controller. Follow the wizard's
   current status/recovery actions; each external action remains explicit.
   Admission is per canonical provider identity, not a new fee for every model.
6. **Verify serving.** Check the managed controller's current health and run a
   permitted small paid request. Advertise only tested context, concurrency and
   capabilities. Core must keep running to meter requests and report availability
   when the upstream becomes busy or unreachable.

Resume an existing setup instead of recreating it:

```sh
mayhem provider proxy setup list --home "$MAYHEM_HOME"
mayhem provider proxy setup wizard --name my-model --home "$MAYHEM_HOME"
```

The dashboard wizard appears only when the local gateway has explicitly enabled
`--proxy-setup` (first creation) or `--proxy-setup-config` (saved setup). Follow the
[dashboard launch instructions](crates/mayhem-proxy/docs/SETUP_DASHBOARD.md) and
open Core's authenticated provider link. An absent button in the ordinary
dashboard does not mean the proxy lane is unsupported. Reuse the existing gateway
configuration; do not start another listener on an occupied port.

If an upstream is incompatible or too large for the machine, explain that exact
limit. Do not bypass checks or promise readiness based only on a model name.
