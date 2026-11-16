#!/usr/bin/env bash
# Build cost per engine: a clean release build of the trial with only that
# engine's feature (and one with none), timed, and the stripped binary's
# size. Writes out/build-cost.jsonl, which `report` shows.
#
#   ./measure-build.sh litehtml blitz        # engines to measure
#
# Each build gets its own empty target folder (under out/), so nothing is
# shared between them; the folders are removed afterwards. Set
# CARGO_BUILD_JOBS=2 on a small machine, and SKIP_BASELINE=1 to skip the
# build with no engine when it's already been measured on this machine.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p out
result=out/build-cost.jsonl
host=$(uname -sm)

measure() {
  local feature=$1 dir="$PWD/out/build-$1"
  rm -rf "$dir"
  # Servo builds from its own package (see servo-build/Cargo.toml).
  local pkg=.
  [ "$feature" = servo ] && pkg=servo-build
  local args=(build --release --target-dir "$dir" --manifest-path "$pkg/Cargo.toml")
  [ "$feature" != none ] && args+=(--features "$feature")
  local start end
  start=$(date +%s)
  if cargo "${args[@]}" > "out/build-$feature.log" 2>&1; then
    end=$(date +%s)
    local size
    size=$(wc -c < "$dir/release/email-engines" | tr -d ' ')
    printf '{"engine":"%s","host":"%s","seconds":%d,"bytes":%d}\n' "$feature" "$host" $((end - start)) "$size" >> "$result"
    echo "$feature: $((end - start)) s, $size bytes"
  else
    printf '{"engine":"%s","host":"%s","error":"build failed: see out/build-%s.log"}\n' "$feature" "$host" "$feature" >> "$result"
    echo "$feature: build failed (out/build-$feature.log)"
  fi
  rm -rf "$dir"
}

[ "${SKIP_BASELINE:-}" = 1 ] || measure none
for e in "$@"; do measure "$e"; done
