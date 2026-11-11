#!/usr/bin/env bash
# Build the Rust core for the iPhone app (see docs/ios.md).
#
#   scripts/ios-rust.sh            release build (what goes on the phone)
#   scripts/ios-rust.sh --debug    quicker, unoptimised
#
# Makes:
#   ios/build/OwlPostCore.xcframework   the library, for the iPhone and the
#                                       simulator (Apple silicon)
#   ios/OwlPost/Generated/              the Swift bindings, compiled into the app
# Neither is committed. Run it after changing crates/owlpost-core or
# crates/owlpost-ffi, then build in Xcode.
#
# Uses Xcode even when xcode-select points at the Command Line Tools.
set -euo pipefail
cd "$(dirname "$0")/.."

export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
# Match the app (IPHONEOS_DEPLOYMENT_TARGET in ios/OwlPost.xcodeproj), so the
# C parts (SQLite, ring) don't use anything newer than the app allows.
export IPHONEOS_DEPLOYMENT_TARGET=17.0
profile=release
flag=--release
if [[ "${1:-}" == "--debug" ]]; then
    profile=debug
    flag=
fi

for target in aarch64-apple-ios aarch64-apple-ios-sim; do
    echo "== $target"
    cargo build $flag --target "$target" -p owlpost-ffi --lib
done

# The bindings come from a Mac build of the same code.
echo "== Swift bindings"
cargo build -p owlpost-ffi
gen=$(mktemp -d)
cargo run -q -p owlpost-ffi --bin uniffi-bindgen -- generate \
    --library target/debug/libowlpost_ffi.dylib --language swift --out-dir "$gen"

rm -rf ios/OwlPost/Generated ios/build/OwlPostCore.xcframework
mkdir -p ios/OwlPost/Generated ios/build/headers
cp "$gen/owlpost_ffi.swift" ios/OwlPost/Generated/
cp "$gen/owlpost_ffiFFI.h" ios/build/headers/
cp "$gen/owlpost_ffiFFI.modulemap" ios/build/headers/module.modulemap
rm -rf "$gen"

echo "== XCFramework"
xcodebuild -create-xcframework \
    -library "target/aarch64-apple-ios/$profile/libowlpost_ffi.a" -headers ios/build/headers \
    -library "target/aarch64-apple-ios-sim/$profile/libowlpost_ffi.a" -headers ios/build/headers \
    -output ios/build/OwlPostCore.xcframework
echo "Done: ios/build/OwlPostCore.xcframework and ios/OwlPost/Generated/"
