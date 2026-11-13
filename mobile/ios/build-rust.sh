#!/bin/sh
# 编 iPhone 要用的 Rust 库（crates/mobile-ffi），打成 XCFramework，再从编好的库生成 Swift 绑定。
# 产物放在 build/rust：P2pcliMobile.xcframework（真机 + Apple 芯片 Mac 上的模拟器）和 swift/p2pcli_mobile.swift。
#
# 要 Xcode，要 Rust 的两个编译目标：rustup target add aarch64-apple-ios aarch64-apple-ios-sim
# 用法：sh build-rust.sh            真机和模拟器都编
#       sh build-rust.sh sim        只编模拟器（在模拟器上试的时候快一些）
#       sh build-rust.sh device     只编真机（打安装包）
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
out="$here/build/rust"
only="${1:-all}"

# 用 Xcode 自带的 clang：别的 clang（比如 swiftly 装的）编 ring 会报 KernelKit 的错
export IPHONEOS_DEPLOYMENT_TARGET=17.0
export CC_aarch64_apple_ios="$(xcrun --sdk iphoneos -f clang)"
export CC_aarch64_apple_ios_sim="$(xcrun --sdk iphonesimulator -f clang)"
export AR_aarch64_apple_ios="$(xcrun --sdk iphoneos -f ar)"
export AR_aarch64_apple_ios_sim="$(xcrun --sdk iphonesimulator -f ar)"

cd "$repo"
case "$only" in
    sim) targets="aarch64-apple-ios-sim" ;;
    device) targets="aarch64-apple-ios" ;;
    *) targets="aarch64-apple-ios aarch64-apple-ios-sim" ;;
esac
for t in $targets; do
    cargo build --locked --release -p p2pcli-mobile-ffi --target "$t"
done

rm -rf "$out"
mkdir -p "$out/swift" "$out/headers"
# 正式版的库去掉了符号，UniFFI 写在库里的接口说明也跟着没了：从本机的调试版读（接口各平台一样）
cargo build --locked -p p2pcli-mobile-ffi --features bindgen
"$repo/target/debug/uniffi-bindgen" generate --library "$repo/target/debug/libp2pcli_mobile.dylib" \
    --language swift --out-dir "$out/gen" --no-format
mv "$out/gen/p2pcli_mobile.swift" "$out/swift/"
mv "$out/gen/p2pcli_mobileFFI.h" "$out/headers/"
# XCFramework 里的模块描述必须叫 module.modulemap
mv "$out/gen/p2pcli_mobileFFI.modulemap" "$out/headers/module.modulemap"
rm -rf "$out/gen"

set --
for t in $targets; do
    set -- "$@" -library "$repo/target/$t/release/libp2pcli_mobile.a" -headers "$out/headers"
done
xcodebuild -create-xcframework "$@" -output "$out/P2pcliMobile.xcframework" >/dev/null
echo "Rust 库和 Swift 绑定已生成：$out"
