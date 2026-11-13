# 第三方素材和库

| 用在哪 | 来源 | 许可 |
|---|---|---|
| 终端模拟（解析输出、画字、选字复制） | [SwiftTerm](https://github.com/migueldeicaza/SwiftTerm) 1.19.0（Swift Package） | MIT |
| 界面图标 `Assets.xcassets/lucide.*` | [lucide](https://lucide.dev)，由 `tools/make_assets.py` 从 lucide-react 生成 | ISC |
| 系统图标 `Assets.xcassets/platform.*` | [Simple Icons](https://simpleicons.org)，和桌面端 `app/src/assets/platforms` 同一份 | CC0 1.0 |
| AI 图标 `Assets.xcassets/agent.*` | [Lobe Icons](https://github.com/lobehub/lobe-icons)，由 `tools/make_assets.py` 从桌面端 `app/src/assets/agents` 渲染 | MIT，见 `app/src/assets/agents/LICENSE` |

扫码用系统自带的 VisionKit（`DataScannerViewController`），从相册认二维码用 Vision，都不是第三方库。
