# 0.2.199

Fixes offline installation of signed managed-runtime wheels. Core now preserves
the recipe-validated wheel filename when mounting it into the runtime installer,
allowing package tooling to validate the wheel name, version, ABI, and platform
tags before installation.

The managed runtime remains fail closed: artifact hashes, the signed filename,
offline installation, and the installed package identity must all match before a
provider can advertise the route. Contract version remains 25 and this release
requires no pricing migration.
