#!/bin/sh
# 打一个不签名的 IPA（只有真机的 arm64），给自己签名后装到 iPhone 上试。上架走 Xcode 的 Archive，不用这个。
# 产物：build/P2PCLI-<版本>-unsigned.ipa
set -eu

here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"

sh build-rust.sh device
xcodegen generate --quiet
xcodebuild -project P2PCLI.xcodeproj -scheme P2PCLI -configuration Release \
    -sdk iphoneos -destination 'generic/platform=iOS' \
    -derivedDataPath build/dd-device -skipPackagePluginValidation -skipMacroValidation \
    CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY="" \
    build > build/ipa-build.log 2>&1 || { grep -E "error:" build/ipa-build.log | sort -u | head -20; exit 1; }

app=build/dd-device/Build/Products/Release-iphoneos/P2PCLI.app
version=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$app/Info.plist")
out="$here/build/P2PCLI-$version-unsigned.ipa"
rm -rf build/ipa "$out"
mkdir -p build/ipa/Payload
cp -R "$app" build/ipa/Payload/
(cd build/ipa && zip -qry "$out" Payload)
rm -rf build/ipa
echo "已生成：$out"
