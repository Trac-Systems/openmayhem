[![release](https://img.shields.io/github/v/release/Trac-Systems/main_settlement_bus)](https://github.com/Trac-Systems/main_settlement_bus/releases/latest)
[![tag](https://img.shields.io/github/v/tag/Trac-Systems/main_settlement_bus?sort=semver)](https://github.com/Trac-Systems/main_settlement_bus/tags)
[![npm](https://img.shields.io/npm/v/trac-msb)](https://www.npmjs.com/package/trac-msb)
[![license](https://img.shields.io/github/license/Trac-Systems/main_settlement_bus)](https://github.com/Trac-Systems/main_settlement_bus/blob/main/LICENSE)
[![node](https://img.shields.io/badge/node-v22.22.0-brightgreen)](https://www.npmjs.com/package/trac-msb)
[![dependabot](https://img.shields.io/badge/dependabot-enabled-brightgreen)](https://github.com/Trac-Systems/main_settlement_bus/security/dependabot)
[![MSB-Unit-Tests](https://github.com/Trac-Systems/main_settlement_bus/actions/workflows/unit-tests.yml/badge.svg?branch=main)](https://github.com/Trac-Systems/main_settlement_bus/actions/workflows/unit-tests.yml)
[![Acceptance Tests](https://github.com/Trac-Systems/main_settlement_bus/actions/workflows/acceptance-tests.yml/badge.svg?branch=main)](https://github.com/Trac-Systems/main_settlement_bus/actions/workflows/acceptance-tests.yml)
# Main Settlement Bus (MSB)

A peer-to-peer crypto validator network to verify and append transactions.

Always follow the guidance in the [Security Policy](SECURITY.md) for release compatibility, upgrade steps, and required follow-up actions.

The MSB leverages the [Pear Runtime and Holepunch](https://pears.com/).

## Prerequisites

Node.js is required to run the application. Before installing Node.js, refer to the official [Node.js documentation](https://nodejs.org) for the latest recommended version and installation instructions. For this project, Node.js v22.22.0 (LTS) and npm 11.6.1 or newer are compatible.

MSB supports both Pear v2 and Pear v3. Its Pear runner uses `pear run` with Pear v2 and the embedded [`pear-runtime`](https://docs.pears.com/reference/pear/runtime/) module with Pear v3.

The Pear CLI is required for Pear v2 and for Pear deployment commands. Pear v3 can run MSB through the embedded module without a globally installed CLI. Install the CLI when it is needed:

```sh
npm install -g pear
which pear
```

Docker is optional and only needed for running the containerized RPC node. Before installing Docker, refer to the official [Docker documentation](https://www.docker.com) for the latest recommended version and installation instructions. For running the containerized RPC node, the latest Docker is recommended. Tested with Docker version 28.3.2, build 578ccf6.

## Install

```shell
git clone -b <tag> --single-branch git@github.com:Trac-Systems/main_settlement_bus.git
cd main_settlement_bus
npm install
```

## Post-install checklist

Before running tests, install bare globally:

```sh
npm install -g bare
```

- ✅ `npm run test:unit:all` – confirms the codebase builds and runs under both supported runtimes.
- 📋 `npm run test:acceptance` – optional but recommended before upgrades. This suite spins up in-process nodes and may take a few minutes.
- 🌐 RPC smoke test – start `STORES_DIRECTORY=smoke-store MSB_HOST=127.0.0.1 MSB_PORT=5000 NETWORK=mainnet npm run env-rpc` in one terminal, then execute `curl -s http://127.0.0.1:5000/v1/fee` from another terminal to verify `/v1` routes respond. Stop the node with `Ctrl+C` once finished.

## Usage

Runtime entry points cover CLI-driven runs (`start`, `rpc`) and `.env`-aware runs (`env`, `env-rpc`). Each section below lists the accepted configuration inputs.

### Startup input validation

Startup input is validated before MSB finishes booting. This applies to direct CLI flags and to the `.env` / inline environment-variable entry points, because those scripts pass the same flags through the Pear v2/v3 runner.

- `--network` / `NETWORK` must be one of `mainnet`, `development`, `testnet1`, or `testnet` (`testnet` is treated as an alias for `testnet1`).
- `--stores-directory` / `STORES_DIRECTORY` must be a non-empty string.
- `--host` / `MSB_HOST` must be a non-empty string when RPC mode is enabled.
- `--port` / `MSB_PORT` must be an integer in range `1-65535` when RPC mode is enabled.

MSB also validates the high-risk overrideable config values that are normalized into shared runtime state before startup:

- `bootstrap` must be a 32-byte hex string or `Buffer`.
- `channel` must be a string or `Buffer` with length `1-32` bytes.
- `storesDirectory`, `host`, `port`, and `dhtBootstrap` overrides are validated before the node starts.

When one of these values is invalid, startup fails immediately with a field-specific error instead of silently falling back.

### Interactive regular node

#### Regular node with .env file

This variant reads configuration from `.env`:

```
# .env
STORES_DIRECTORY=<stores_directory>
NETWORK=<network>
```

then

```
npm run env
```

The script sources `.env` before invoking program and falls back to `stores` for `STORES_DIRECTORY` and `mainnet` for `NETWORK` when unset.

#### Inline environment variables

```sh
STORES_DIRECTORY=<stores_directory> NETWORK=testnet npm run env
```

This run persists data under `${STORES_DIRECTORY}` (defaults to `stores` under the project root), connects to testnet (defaults to `mainnet`) and is intended for inline or CLI-supplied configuration. Each network will have its own store subfolder to avoid collision

#### CLI flags

```sh
npm run start -- --stores-directory <stores_directory> --network testnet
```

Supported network values are `mainnet`, `development`, `testnet1`, and `testnet` (`testnet` maps to `testnet1`).

### RPC-enabled node

#### RPC with .env file

```
# .env
STORES_DIRECTORY=<stores_directory>
MSB_HOST=127.0.0.1
MSB_PORT=5000
NETWORK=mainnet
```

```
npm run env-rpc
```

This entry point sources `.env` automatically and defaults to `stores`, `127.0.0.1`, `5000`, and `mainnet` when variables are not present. Supported `NETWORK` values are `mainnet`, `development`, `testnet`, and `testnet1`.

#### Inline environment variables

```sh
STORES_DIRECTORY=<stores_directory> MSB_HOST=<host> MSB_PORT=<port> NETWORK=<network> npm run env-rpc
```

Override any combination of `STORES_DIRECTORY`, `MSB_HOST`, `MSB_PORT`, or `NETWORK`. Data is persisted under `<stores_directory>/<store_name>` (default `stores/mainnet` for this script).

#### CLI flags

```sh
npm run rpc --host=<host> --port=<port> -- --stores-directory <stores_directory> --network <network>
```

Supported network values are `mainnet`, `development`, `testnet1`, and `testnet` (`testnet` maps to `testnet1`). Invalid `--host`, `--port`, `--stores-directory`, or `--network` values fail before the RPC node starts.

## Docker usage

You can run the RPC node in a containerized environment using the provided `docker-compose.yml` file. The `msb-rpc` service is already wired up. You usually only need to tweak these variables:

- `MSB_STORE`: name of the store directory under `./stores`.
- `MSB_HOST`: host interface to bind (defaults to `127.0.0.1` to avoid exposing everything).
- `MSB_PORT`: port the RPC server listens on **inside** the container (defaults to `5000`).
- `MSB_PUBLISH_PORT`: host port to expose (defaults to `MSB_PORT`, so set it only when the host port should differ).
- `NETWORK`: network environment for the RPC process (defaults to `mainnet`). Supported values are `mainnet`, `development`, `testnet`, and `testnet1`.

Leave `MSB_PORT=5000` if you just want to publish the default RPC port and only bump `MSB_PUBLISH_PORT` when the host side must change. Set both to the same value if you want the RPC server itself to listen on another port.

Example (keep container port 5000, expose host port 6000):

```sh
MSB_STORE=rpc-node-store \
MSB_HOST=127.0.0.1 \
MSB_PORT=5000 \
NETWORK=mainnet \
MSB_PUBLISH_PORT=6000 \
docker compose up -d msb-rpc
```

### Running `msb-rpc` with Docker Compose

Any of the following launch methods can be applied:

1. **Using a `.env` file** – populate `.env`, then start the service:

   ```sh
   docker compose up -d msb-rpc
   ```

   or

   ```sh
   docker compose --env-file .env up -d msb-rpc
   ```

   Add any of the variables listed above to `.env`. When the host port needs to differ from the container port, set `MSB_PUBLISH_PORT` without touching `MSB_PORT`.

   Example `.env` (publishes host port 1337, keeps the container on 5000):

   ```dotenv
   MSB_STORE=rpc-node-store
   MSB_HOST=127.0.0.1
   MSB_PORT=5000
   NETWORK=mainnet
   MSB_PUBLISH_PORT=1337
   ```

2. **Passing variables inline** – use this method when environment variables should be provided directly in the command line, without modifying the `.env` file:

   ```sh
   MSB_STORE=<store_name> MSB_HOST=<host> MSB_PORT=<container_port> NETWORK=<network> MSB_PUBLISH_PORT=<host_port> docker compose up -d msb-rpc
   ```

   Skip `MSB_PORT` when you just want to keep the container on `5000` and expose a different host port.

3. **Reusing an existing store directory** – mount the path that already holds your store and pin the host binding you need:

   ```sh
   docker compose run -d --name msb-rpc \
      -e MSB_STORE=<store_name> \
      -e MSB_HOST=<host> \
      -e MSB_PORT=<container_port> \
      -e NETWORK=<network> \
      -e MSB_PUBLISH_PORT=<host_port> \
      -p <host_address>:<host_port>:<container_port> \
      -v /absolute/path/to/your/store_directory:/msb/stores \
      msb-rpc
   ```

   Adjust `/absolute/path/to/your/store_directory` to the directory that already contains the persisted store. Once the container exists, bring it back with `docker compose start msb-rpc`. If the container should stay on `5000`, omit `-e MSB_PORT=<container_port>` and just set `MSB_PUBLISH_PORT` plus the matching `-p` flag.

   Example with specific values:

   ```sh
   docker compose run -d --name msb-rpc \
       -e MSB_STORE=rpc-node-store \
       -e MSB_HOST=127.0.0.1 \
       -e MSB_PORT=5000 \
       -e NETWORK=mainnet \
       -e MSB_PUBLISH_PORT=6000 \
       -p 127.0.0.1:6000:5000 \
       -v /absolute/path/to/your/store_directory:/msb/stores \
       msb-rpc
   ```

Stop the service with `docker compose stop msb-rpc`, remove the stack entirely with `docker compose down` when you are finished.

> Note: The RPC instance must synchronize with the network after startup, so full readiness may take some time.

## Troubleshooting

- **Dependency install failures** – confirm you are on Node.js v22.22.0 (LTS) and npm ≥ 11.6.1. If packages still fail to build, clear artifacts (`rm -rf node_modules package-lock.json && npm install`) and rerun `npm run test:unit:all`.
- **Unit tests fail only in one runtime** – run the targeted commands (`npm run test:unit:node` or `npm run test:unit:bare`) to isolate regressions, then inspect `tests/unit/unit.test.js` for the failing cases.

## Development

### VS Code

Open the repository root in VS Code so the editor can resolve the local `eslint` dependency, the root `eslint.config.js`, and the existing debugger launch configurations in `.vscode/launch.json`.

Install these extensions:

- `dbaeumer.vscode-eslint` (microsoft.com)

For linting, add the following to your workspace or user `.vscode/settings.json`:

```json
{
  "editor.formatOnSave": false,
  "editor.codeActionsOnSave": {
    "source.fixAll.eslint": "explicit"
  },
  "eslint.useFlatConfig": true,
  "eslint.workingDirectories": [
    {
      "mode": "auto"
    }
  ],
  "[javascript]": {
    "editor.defaultFormatter": "dbaeumer.vscode-eslint"
  }
}
```

### Linting and tests

Use these commands during development:

- `npm run lint` checks the full repository with ESLint.
- `npm run lint:fix` applies ESLint autofixes where possible.
- `npm run test:unit:all` runs both unit suites and is the main pre-commit test command.
- `npm run test:acceptance` runs the RPC acceptance test suite.
