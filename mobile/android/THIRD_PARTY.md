# 第三方素材和库

| 用在哪 | 来源 | 许可 |
|---|---|---|
| 终端模拟（解析输出、画字） | [termux-app](https://github.com/termux/termux-app) 的 terminal-emulator、terminal-view v0.118.3（JitPack） | Apache 2.0（termux-app 的 LICENSE.md 里列为 GPLv3 的例外） |
| 界面图标 `ui/Lucide.kt` | [lucide](https://lucide.dev)，由 `tools/lucide_to_kotlin.py` 从 lucide-react 生成 | ISC |
| 系统图标 `res/drawable/ic_platform_*.xml` | [Simple Icons](https://simpleicons.org)，和桌面端 `app/src/assets/platforms` 同一份 | CC0 1.0 |
| AI 图标 `res/drawable-nodpi/agent_*.png` | [Lobe Icons](https://github.com/lobehub/lobe-icons)，由 `tools/render_assets.sh` 从桌面端 `app/src/assets/agents` 渲染 | MIT，见 `app/src/assets/agents/LICENSE` |
| 调用 Rust 库 | [JNA](https://github.com/java-native-access/jna) | Apache 2.0 / LGPL 2.1 |
| 扫码取景 | [CameraX](https://developer.android.com/media/camera/camerax) 1.5.1 | Apache 2.0 |
| 认二维码 | [ML Kit 条码识别](https://developers.google.com/ml-kit/vision/barcode-scanning)（自带模型的版本，不依赖谷歌服务）17.3.0 | [ML Kit 条款](https://developers.google.com/ml-kit/terms) |
