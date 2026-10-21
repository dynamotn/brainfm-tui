#!/usr/bin/env bash
# @file pinned-cargo.sh
# @brief Run cargo with the toolchain pinned in rust-toolchain.toml.
# @description
#   Version managers such as mise export RUSTUP_TOOLCHAIN, which overrides
#   rust-toolchain.toml; `rustup run` puts the pinned compiler back, so the
#   pre-commit hooks see the same lints as CI.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
channel="$(sed -n 's/^channel = "\(.*\)"$/\1/p' "${root}/rust-toolchain.toml")"
if [[ -z "${channel}" ]]; then
  echo "pinned-cargo: no channel in ${root}/rust-toolchain.toml" >&2
  exit 1
fi
exec rustup run "${channel}" cargo "$@"
