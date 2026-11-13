#!/bin/sh
# 编手机要用的 Rust 库（crates/mobile-ffi），从编好的库生成 Kotlin 绑定。Gradle 打包前自动跑。
# 产物放在 app/build/rust：jniLibs/arm64-v8a/libp2pcli_mobile.so 和 uniffi/ 下的 Kotlin 绑定。
#
# 要 Android NDK：按 ANDROID_NDK_HOME、NDK_HOME、$ANDROID_HOME/ndk/最新版 的顺序找。
# 要 Rust 的 aarch64-linux-android 编译目标：rustup target add aarch64-linux-android
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
out="$here/app/build/rust"

ndk="${ANDROID_NDK_HOME:-${NDK_HOME:-}}"
if [ -z "$ndk" ] && [ -n "${ANDROID_HOME:-}" ] && [ -d "$ANDROID_HOME/ndk" ]; then
    ndk="$ANDROID_HOME/ndk/$(ls "$ANDROID_HOME/ndk" | sort -V | tail -1)"
fi
[ -n "$ndk" ] && [ -d "$ndk" ] || { echo "找不到 Android NDK：设 ANDROID_NDK_HOME 或 NDK_HOME" >&2; exit 1; }
case "$(uname -s)" in
    Darwin) host=darwin-x86_64 ;;
    Linux) host=linux-x86_64 ;;
    *) echo "不支持在这个系统上编：$(uname -s)" >&2; exit 1 ;;
esac
bin="$ndk/toolchains/llvm/prebuilt/$host/bin"
# API 24：和 core、App 的最低版本对得上
export CC_aarch64_linux_android="$bin/aarch64-linux-android24-clang"
export AR_aarch64_linux_android="$bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$bin/aarch64-linux-android24-clang"

cd "$repo"
cargo build --locked --release -p p2pcli-mobile-ffi --target aarch64-linux-android
lib="$repo/target/aarch64-linux-android/release/libp2pcli_mobile.so"

mkdir -p "$out/jniLibs/arm64-v8a"
cp "$lib" "$out/jniLibs/arm64-v8a/"
rm -rf "$out/uniffi"
# 正式版的库去掉了符号（工作区 release 配置 strip = true），UniFFI 写在库里的接口说明也跟着没了。
# 接口在各平台都一样：从本机不去符号的调试版读，生成绑定的命令行也在本机跑
cargo build --locked -p p2pcli-mobile-ffi --features bindgen
case "$(uname -s)" in Darwin) host_lib="$repo/target/debug/libp2pcli_mobile.dylib" ;; *) host_lib="$repo/target/debug/libp2pcli_mobile.so" ;; esac
"$repo/target/debug/uniffi-bindgen" generate --library "$host_lib" --language kotlin --out-dir "$out/uniffi" --no-format
echo "Rust 库和 Kotlin 绑定已生成：$out"
