# P2PCLI iOS 原生版

SwiftUI 写的界面；连接、协议、会话、传文件用 Rust 的 `crates/mobile-ffi`（和安卓同一份），UniFFI 生成 Swift 绑定。

## 编译

要 Xcode 27.1 以上（iPhone Duo 要用 iOS 27.1 SDK 编）、XcodeGen、Rust：

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
brew install xcodegen
xcodebuild -downloadComponent MetalToolchain   # SwiftTerm 带 Metal 着色器
cd mobile/ios
sh build-rust.sh          # 编 Rust 库，生成 build/rust/P2pcliMobile.xcframework 和 Swift 绑定；只在模拟器上试可以加 sim
xcodegen                  # 按 project.yml 生成 P2PCLI.xcodeproj
open P2PCLI.xcodeproj
```

打一个不签名的 IPA（自己签名后装到真机上试）：`sh build-ipa.sh`，产物在 `build/P2PCLI-<版本>-unsigned.ipa`。

改了 `crates/mobile-ffi` 的接口要重跑 `build-rust.sh`；加了文件、改了工程设置要重跑 `xcodegen`。
图标和 App 图标由 `python3 mobile/ios/tools/make_assets.py`（在仓库根目录跑）生成。

界面测试会把主要页面走一遍并截图，见 `P2PCLIUITests`。连一台测试主机（`p2pcli-dev host`）时把设备码、密码传进去：

```sh
TEST_RUNNER_P2PCLI_CODE=123456789 TEST_RUNNER_P2PCLI_PASSWORD=xxxx TEST_RUNNER_P2PCLI_SHOTS=/tmp/shots \
  xcodebuild test -project P2PCLI.xcodeproj -scheme P2PCLI -destination 'platform=iOS Simulator,name=iPhone Duo' \
  -skipPackagePluginValidation
```

## iPhone Duo

照苹果的 [Preparing your app for iPhone Duo](https://developer.apple.com/documentation/technologyoverviews/preparing-your-app-for-iphone-duo)
和 [Designing for iPhone Duo](https://developer.apple.com/design/human-interface-guidelines/designing-for-iphone-duo) 做：

- 用 iOS 27.1 以上的 SDK 编：旧 SDK 编的 App 在 iPhone Duo 上不会铺到状态栏和摄像头下面。
- 布局按容器大小算，不用屏幕尺寸、设备类型、横竖屏做判断；支持横竖屏和分屏（`UIRequiresFullScreen` 关着）。
- 设备列表和设备、终端用系统的 `NavigationSplitView`：合上（外屏、窄）时一层层推进去，展开（内屏、宽）时两栏并排，
  半折时系统让两栏避开折痕。
- 导航栏、工具栏都用系统的 `.toolbar`，每一项都给文字和图标（`Label`），系统在外屏把它们竖着放在侧边、
  放不下的收进溢出菜单；不常用的项放 `.secondaryAction`，直接进系统的溢出菜单，没有自己做“⋯”菜单。
- 终端和按键条是自己画的，按文档用 `GeometryProxy.reservedRegions(kind: .division)` 查折痕（`Fold.swift`）：
  像笔记本那样立着时终端在折痕上面、按键在下面；像书一样拿着时按键条分成两组放在折痕两边。
- 弹出的小面板（更多按键、开着的终端）用 `toolbarVerticalBehavior(.disabled)`，工具栏横着放。
- 扫码用系统的 `DataScannerViewController`，开合换屏时它自己换摄像头。
- 上架：2027 年 4 月起新 App 和更新要附 iPhone Duo 的截图（在模拟器里截）。

折叠姿态（半折、立着）要在 Xcode 的 Device Hub 里拨，命令行和界面测试拨不了。

## 上架前还要做的

- 在 Apple Developer 里建 App ID（`io.github.ilovebinglu.p2pcli`），在 Xcode 里选开发团队签名。
- 局域网发现用组播：真机上要向苹果申请 `com.apple.developer.networking.multicast` 权限，批下来之前只能用设备码连。
- App Store Connect 里的出口合规：只用了系统和 Rust 里标准的加密（QUIC/TLS），按“豁免”填。
