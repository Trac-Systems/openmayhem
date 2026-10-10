# Proxy acceptance from an installed bundle

`scripts/tests/proxy-installed-runtime.test.sh` uses the actual candidate
executables, the packager's tracked source allowlists and manifest/checksum
functions, and the signed `install.sh` path. It generates a disposable signing
key; no real release signing material is used or installed globally.

Build the current eight release executables first. Set `CARGO_TARGET_DIR` to the
same target directory used for those builds, `MAYHEM_TEST_BINARY_DIR` to its
absolute binary directory, and `MAYHEM_TEST_OUTPUT_DIR` to a new absolute evidence
directory. Then run:

```sh
bash scripts/tests/proxy-installed-runtime.test.sh
```

The script refuses an existing output directory. It neither modifies `HOME` nor
bootstraps Node/Pear/opencode, updates shell profiles, starts existing services,
downloads models or connects to a live peer. It requires existing Node
dependencies and materializes them into its disposable archive. Linux hosts
requiring the installer's AppArmor setup are refused before installation; use a
disposable host prepared for the normal sandbox separately.

The installed CLI, supervisor and decoder must pass the existing guided-create
and supervised-Run acceptance cases. Their working directory is outside the
checkout; `MAYHEM_ASSET_DIR` is removed. Wallet-helper imports come from the
installed assets and the managed probe uses the installed worker. Missing package
files cannot be concealed by development asset overrides. The canonical peer,
upstream, bridge, wallet and payment authorization in these cases are fixtures.

The checks verify that recovery restores one original child/configuration, does
not refill its spent probe allowance, and does not create native children. They
also compare every installed runtime asset with its signed digest after the
cases. The evidence retains binary hashes, manifest and small logs. Successful
runs remove their large archive/staging/installation trees by default;
`MAYHEM_TEST_KEEP_PACKAGE=1` retains them for inspection. Failed runs retain their
new disposable trees for diagnosis.

This is installed-runtime proof on the host running the script. Cached dependency
snapshots and debug builds do not establish fresh production dependency hydration,
release-profile qualification, other architectures, Windows installer execution,
live backend capacity or external payment acceptance. Those remain separate
release gates; do not promote this fixture signature to a production release.
