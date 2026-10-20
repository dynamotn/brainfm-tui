#!/usr/bin/env bash
# @file screenshots.sh
# @brief Regenerate the README screenshots in docs/screenshots.
# @description
#   Renders the TUI from sample data (an ignored test in src/ui/screenshots.rs),
#   rasterizes each SVG at 2x with resvg, then drops the SVGs since they
#   depend on locally installed fonts. Needs resvg; looks best with Iosevka.
set -euo pipefail
shopt -s nullglob

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${root}/docs/screenshots"

cargo test --manifest-path "${root}/Cargo.toml" --lib screenshots -- --ignored --quiet

count=0
for svg in "${out}"/*.svg; do
  resvg --zoom 2 "${svg}" "${svg%.svg}.png"
  rm -- "${svg}"
  count=$((count + 1))
done
echo "Wrote ${count} screenshots to ${out}"
