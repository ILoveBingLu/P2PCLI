**简体中文** | [English](development.en.md)

# 开发

## 原理

```
crates/core     核心库：身份、连接、配对校验、远程终端、牵线客户端、选路
crates/server   牵线服务器：发设备码、记在线状态和地址（终端数据不经过它），UDP 和 WebSocket 两个入口
cloudflare/     牵线服务器的 Cloudflare Workers 版（WebSocket）
crates/dev      开发测试用命令行
app/            桌面软件（src-tauri 是 Rust 端，src 是界面）
```

连接层是 [iroh](https://github.com/n0-computer/iroh)（QUIC）。每台设备有一把长期私钥，公钥就是设备身份，连接两端互相认证并加密。

1. 每台设备启动时连牵线服务器（UDP 或 WebSocket），登记拿到固定的设备码，之后报告当前地址
2. 连接方按设备码查到对方公钥和地址，连过去
3. 连接方发开场消息带配对密码；被连方校验密码（同一设备连错 5 次锁 10 分钟，所有设备加起来错太多也暂停），再弹窗问主人
4. 主人同意后，连接方每开一个标签就开一条 QUIC 流，被连方起一个真终端接上去（macOS 是用户的登录 shell，Windows 是 PowerShell）

IPv6 直连（`crates/core/src/v6.rs`）：公网 IPv6 没有 NAT，网卡地址就是对外地址，用不着打洞，只是两边路由器有状态防火墙。两边都有公网 IPv6 时，连接方通过牵线服务器通知被连方同时往自己的 IPv6 发包，把被连方的防火墙打开；连接方自己直接连对方的 IPv6，第一个包就走直连，不经过中继。1.5 秒还没通就并行走常规流程（先经中继连上，再打洞），哪条先通用哪条。

局域网发现（`crates/core/src/lan.rs`）：用 iroh 官方的 [iroh-mdns-address-lookup](https://docs.rs/iroh-mdns-address-lookup) 在局域网里广播和发现，服务名用 `p2pcli`，只认 P2PCLI 设备。设备名和系统放在广播的附加数据里，只发到局域网：iroh 自带的附加数据接口会连带发到公共 DNS，所以包了一层，只在交给 mDNS 的那份里带上。局域网连接按公钥直连，不经过牵线服务器；广播内容核实不了，所以不拿它新建连接历史，只更新已有记录。

多网卡发现通过 `vendor/iroh-mdns-address-lookup` 补丁显式指定 IPv4 组播接口，覆盖所有已启用、支持组播的非回环、非 TUN 网卡，不依赖可能指向 VMware 的默认组播接口。常驻发现和刷新扫描使用同样的接口选择；每次扫描更新常驻实例的接口列表，桌面端每 30 秒自动扫描，也可手动刷新。IPv6 保持依赖原有行为。补丁来源和范围见该目录的 `PATCHES.md`，无需修改系统路由或停用虚拟网卡。

拥塞控制（`crates/core/src/quic.rs`）：默认用 Cubic。以前往返 20 毫秒以上自动换成 BBR3：模拟的均匀随机丢包链路上它快得多（往返 106 毫秒、丢包 1.3% 时约 40 Mbps，Cubic 只有约 1 Mbps），但真实的跨境链路（往返约 80 毫秒）上，noq 的 BBR3 碰到零星丢包后带宽估计一路掉到 0.5 Mbps 左右、恢复不了，同一条线 Cubic 一直 30~80 Mbps，所以改回 Cubic。环境变量 `P2PCLI_CC=bbr3`（一直用 BBR3）或 `auto`（按延迟选）能换回来对比。单条流的窗口不要调大：接收缓冲最多容纳 1024 个不连续的数据块，窗口放大后丢包交错时会被对方直接断开连接；要提速靠并发分段（`crates/core/src/files/lanes.rs`），多条流各算各的这个限制。用模拟链路实测真实的传文件代码（`cargo test -p p2pcli-core --test lossy_link -- --ignored --nocapture --test-threads=1 file_transfer_over`）：往返 106 毫秒、丢包 1.3% 时一条流 38 Mbps，并发约 73 Mbps；往返 106 毫秒、300 Mbps、丢包 0.1% 时 58 对 208 Mbps；局域网和近距离持平。

选路（`crates/core/src/path.rs`）：IPv6 只要不比 IPv4 慢 50 毫秒以上就用 IPv6，其次 IPv4，中继只在都不通时用。iroh 自带的规则只给 IPv6 让 3 毫秒、又要求快 5 毫秒才换路，做不到 IPv6 优先，所以用了它的选路接口（这个接口标着不稳定，iroh 版本已锁死）。

## 开发环境

需要 Rust 1.88 以上、Node 24、pnpm。

以下命令都在项目根目录执行，不需要进入 `app`：

```sh
cargo test --workspace              # 单元测试、同进程两台设备完整走一遍、IPv6 直连流程
pnpm install                       # 安装应用依赖
pnpm run dev                       # 启动桌面应用开发模式
pnpm run build --bundles app,dmg    # macOS 正式包
pnpm run build --bundles nsis       # Windows 正式包
pnpm run build --bundles appimage,deb,rpm  # Linux 本机架构正式包
```

## 发布

发布软件：改好 `app/src-tauri/tauri.conf.json` 的版本号，推一个同版本的 `vX.Y.Z` 标签（标签附带的说明会成为更新说明），GitHub Actions 会同时编译 Mac（Apple 芯片、Intel）、Windows（x64、ARM64、32 位）和 Linux（x64、ARM64）七个目标。Linux 各输出 AppImage、DEB、RPM，所有安装包、更新包、签名、`latest.json` 和 `SHA256SUMS` 汇总到发布页（`.github/workflows/app-release.yml`）。Linux 更新按架构及安装格式匹配，DEB/RPM 更新需要管理员授权。改了打包配置可以推 `ci/` 分支，或在 Actions 手动运行“安装包发布”，只编不发，产物在 Artifacts 中；只有推版本标签才发布。普通 `main` 推送不会打包。

Linux 开发依赖、原生 runner 和验收步骤见 [Linux 桌面支持](linux-desktop.md)。dmg 窗口背景和 Windows 安装界面的图在 `app/src-tauri/installer`，Mac 菜单栏图标是 `app/src-tauri/icons/tray.svg`，改了 SVG 在 Mac 上跑 `CHROME=<Chrome 可执行文件> node scripts/render-art.mjs` 重新出图。在线更新包要签名：仓库 Secrets 里放 `TAURI_SIGNING_PRIVATE_KEY`（私钥文件内容，没有密码），公钥写在 `tauri.conf.json` 的 `plugins.updater.pubkey`。私钥丢了，已经装上的软件就收不到更新了。本地打正式包不需要私钥，只有加上 `--config '{"bundle":{"createUpdaterArtifacts":true}}'` 才会生成更新包并要求签名。

Mac 安装包用一张自签名的代码签名证书签（证书名 `P2PCLI Self-Signed Code Signing`）：仓库 Secrets 里放 `MACOS_CERTIFICATE`（p12 文件转 base64）和 `MACOS_CERTIFICATE_PASSWORD`，发版流程导入到临时钥匙串，用 `APPLE_SIGNING_IDENTITY` 盖过配置里的 `-`。临时签名的认人规则是整个程序的指纹，每个版本都不同，macOS 会把新版本当成另一个程序，用户每次更新都得重新给录屏、辅助功能权限；用同一张证书签，规则变成“这张证书签的”，更新后权限还在。证书不是苹果发的，第一次打开照样要在隐私与安全性里放行。每个版本都要用同一张证书，丢了或换了，所有用户都要重新授权一次。正式发布时缺这两项就不发；本地打包照旧临时签名。

发布牵线服务器：改好根目录 `Cargo.toml` 的版本号，推一个同版本的 `server-vX.Y.Z` 标签，GitHub Actions 会编译 Linux x86_64、aarch64 两个版本连同校验和放到发布页（`.github/workflows/server-release.yml`），一键安装脚本就从那里下载。

## 界面

界面预览：在根目录执行 `pnpm run dev:web`，然后打开 `http://localhost:1420/preview.html?s=empty`（s 还可以是 request、busy、relay），用假数据渲染界面。仅构建前端可执行 `pnpm run build:web`。

界面使用 [HeroUI v3](https://heroui.com/)、Tailwind CSS v4 和 [Lucide](https://lucide.dev/) 图标，采用深色桌面工作区布局。按钮、输入框、卡片、弹窗、标签页、复选框、通知和悬停提示使用 HeroUI；应用尺寸与语义样式封装位于 `app/src/components/ui`，主题与布局样式位于 `app/src/styles.css`。聚焦时不显示额外光圈，保留键盘交互和表单错误提示。

基础控件使用 HeroUI 默认深色主题、圆角、尺寸和交互样式；封装只适配业务属性和布局，不重新绘制控件。设置页内的输入框使用库自带的 `secondary` 变体，与弹窗底色区分。

Windows 和 macOS 都使用单行自定义标题栏，网络诊断和设置保留在标题栏内。Windows 接入 [tauri-plugin-window-controls](https://github.com/insd47/tauri-plugin-window-controls) 0.2.1（通过仓库内 `vendor/tauri-plugin-window-controls` 补丁修正图标灰度并取消失焦淡化，保留原 MIT 许可）：控件由 Win32 子窗口承载，使用系统图标字体绘制并发送系统窗口命令，支持 Windows 11 贴靠布局；它并非直接复用 DWM 默认标题栏按钮。macOS 使用覆盖式标题栏及系统红黄绿按钮，`trafficLightPosition` 设为 `{ x: 14, y: 24 }`，针对 44 逻辑像素标题栏修正按钮偏高的问题。Tao/Wry 的 `y` 用于调整原生标题栏容器高度，并非按钮中心坐标，不能直接取网页标题栏高度的一半；不同 macOS 版本的最终对齐需实机核对。关闭窗口仍隐藏到托盘。

平台窗口配置分别位于 `app/src-tauri/tauri.windows.conf.json` 和 `app/src-tauri/tauri.macos.conf.json`；窗口数组会整体覆盖主配置，调整窗口尺寸时需同步。Windows 主窗口由 Rust setup 通过插件 builder 创建，标题栏高度为 44 逻辑像素，前端右侧预留 140 像素容纳三个按钮和 DPI 舍入。`windows-app-manifest.xml` 提供分层子窗口需要的系统兼容声明。

`pnpm run check` 只做 TypeScript 类型检查，不生成构建产物。预览页额外支持 `s=unconfigured`（未配置服务器）、`s=offline`（离线）、`s=error`（连接与诊断失败），用于检查异常状态。

新增、移除应用依赖同样直接在根目录执行：

```sh
pnpm add <包名>       # 添加运行依赖
pnpm add -D <包名>    # 添加开发依赖
pnpm remove <包名>    # 移除依赖
```

根目录的 `package.json` 保存应用脚本和依赖，`pnpm-lock.yaml` 锁定依赖版本；界面和 Tauri 源码仍放在 `app` 目录。

桌面端使用自定义标题栏，整合品牌、当前设备、终端数量和窗口控制。拖动标题栏空白或文字区域可移动窗口，双击可最大化 / 还原；关闭按钮隐藏到托盘，退出应用仍使用托盘菜单。修改窗口配置或权限后需要重新启动开发进程；纯浏览器预览中窗口控制按钮禁用。

## 图标

- macOS：源文件是 `app/src-tauri/icons/AppIcon.icon`（Icon Composer 的分层液态玻璃图标，可以直接用 Icon Composer 打开改）。改完在 `app/src-tauri/icons` 里运行 `./build-macos-icons.sh`，生成 `Assets.car`（macOS 26 起用，跟着浅色、深色、着色、透明外观变）和 `icon.icns`（老系统和开发模式的 Dock 图标），两份都提交，打包时不需要 Xcode。脚本需要 Xcode 26 以上。
- Windows 和界面里的 PNG：源文件是 `app/icon-source.svg`，用 `pnpm exec tauri icon app/icon-source.svg -o app/src-tauri/icons` 生成，生成后把 `icon.icns` 换回上面脚本生成的那份。

## 环境变量

| 变量 | 作用 |
| --- | --- |
| `P2PCLI_SERVER` | 牵线服务器，`公钥@IP或域名:端口` 或 `wss://域名`，默认 `wss://p2pcli.aiqji.cn` |
| `P2PCLI_RELAY` | 自建中继地址，逗号分隔 |
| `P2PCLI_DATA_DIR` | 数据目录，一台机器开两份测试时用 |
| `RUST_LOG` | 日志级别，日志在数据目录的 `p2pcli.log` |
| `P2PCLI_SELFTEST` | 仅调试版：自测脚本，见 `app/src/selftest.ts` |
| `P2PCLI_IPV6_ULA` | 测试用：把局域网 fd 开头的 IPv6 也当公网地址 |

打包时设置 `P2PCLI_DEFAULT_SERVER`、`P2PCLI_DEFAULT_RELAY`，可以把别的默认服务器编进安装包。

## 跨机测试

```sh
# 被连方
p2pcli-dev --server <公钥@IP:端口> host --password abc234 --approve yes
# 连接方：跑一条命令，打印输出和线路
p2pcli-dev --server <公钥@IP:端口> connect <设备码> --password abc234 --run 'echo hi' --watch 5
# 局域网：列出附近设备，再按公钥连，不用牵线服务器
p2pcli-dev --server "" nearby
p2pcli-dev --server "" connect <公钥> --password abc234 --run 'echo hi'
```

`--relay-only` 强制走中继，`--bind-ip` 只绑某个网卡，`--no-lan` 关掉局域网发现，`--lan-hidden` 只看别人不广播本机。

## 已验证

以下是既有连接和终端功能的验证记录，不代表新增协作协议、managed worker 或两设备任务闭环已完成实机验收。

- macOS ↔ macOS 局域网直连；macOS ↔ Windows 隔两层 NAT 的 IPv4 打洞直连；强制中继
- 桌面软件三个方向（macOS→macOS、macOS→Windows、Windows→macOS）真界面走通：输设备码、弹窗同意、标签页里跑命令、中文显示
- IPv6 直连流程：同进程测试和两台 Mac 跨机（用局域网 IPv6 代替公网 IPv6），第一个包就走 IPv6

还没验：真公网 IPv6 隔着两边路由器防火墙的直连、自建中继走 HTTPS 的正式部署。

## 协作（实验性）

跨设备、跨会话协作已接入核心存储、网络协议、`collab` CLI 和桌面入口，目前作为实验性功能使用。两设备手动创建、领取、回报和验收流程见 [协作手动闭环](collaboration-manual.md)，设计边界和验收条件见 [协作设计文档](cross-device-collaboration.md)。managed worker 目前仅有 mock 子进程/JSON 测试，尚未完成真实 AI 长任务或物理两设备协作 E2E 验收。

## 终端会话保持

桌面端终端由被连设备上的独立后台进程托管：macOS/Linux 使用 tmux，Windows 使用原生 ConPTY 的 [psmux](https://github.com/psmux/psmux)，不需要 WSL。

- Windows、macOS 与 Linux 发行配置均内置会话管理器，用户无需安装 tmux、psmux、Homebrew 或 WSL，首次启动也不联网下载组件。
- 按打包目标系统和 CPU 架构选择组件：Windows 只带对应架构的 psmux 3.4.0；Mac、Linux 只带对应架构的 tmux 3.7c。Linux 文件名为 `p2pcli-tmux`，不占用系统 `/usr/bin/tmux`。不会把另一系统的二进制放进安装包。Apple Silicon 和 Intel 分包；仅显式选择 universal 时使用已提交的双架构 Mac 文件。
- 桌面端通过应用目录中的绝对路径调用组件，不依赖用户 PATH。若安装内容损坏则提示重新安装，不会悄悄调用系统中其他版本。独立 core/CLI 仍可使用自行安装的工具。
- 连接设备后显示后台会话列表，可新建、恢复或明确结束会话；终端工具栏的会话按钮可再次打开列表。
- 断线、关闭标签和退出 P2PCLI 只断开附着客户端。再次连接同一设备，可恢复原进程、工作目录、变量和终端画面。
- macOS、Linux 上用 tmux 的控制模式（`tmux -C`）接会话，学 iTerm2 的 tmux 集成（`crates/core/src/tmux_control.rs`）：tmux 不画屏幕，只把程序的原始输出交出来，原样转给连接方，历史攒在连接方的终端里，往上翻、选字都在本机，不走网络。接上时用 `capture-pane` 补最近 5000 行历史和当前画面，再按 `display` 查到的光标、键盘鼠标模式还原；连接方收得慢、积压超过 2 秒时 tmux 暂停这个窗格，被连方跳过积压的输出、直接重画最新画面。对方是旧版本、或者被连方是 Windows（psmux 的控制模式没验证过）时，照旧用普通接法，滚轮转给 tmux 进复制模式翻历史。真 tmux 的回归用例是 `control_mode_keeps_history_on_viewer` 和 `control_mode_skips_backlog_for_slow_viewer`，跑法同下。
- 点击“结束会话”并确认，或者在 shell 内执行 `exit`，才会结束该会话。系统重启、注销或后台 tmux/psmux 服务被结束仍会使进程退出。
- 会话按双方设备公钥隔离；重连仍需要正常认证。不要删除设备密钥，否则新身份不能恢复旧身份的会话。
- 协议已升级到 v2，连接双方需要一起更新。开发用一次性 CLI 命令使用临时终端并保留真实退出码，不创建后台会话。

预览页 `preview.html?s=sessions` 提供后台会话模拟数据。真实会话恢复回归用例为 `persistent_session_survives_disconnect_and_host_restart`，默认忽略，开发者可用 `P2PCLI_TEST_SESSION_PROGRAM` 指定准备好的组件绝对路径后显式运行，无需全局安装；测试会起独立的测试命名空间。

## 会话异常日志

在**被控端**打开标题栏的「网络诊断 → 查看日志」，会在文件管理器中选中 `p2pcli.log`。默认级别即可记录，无需运行诊断命令或设置 `RUST_LOG`。

- 记录创建、恢复、主动结束、网络流断开，以及连接客户端的 PID、退出码和信号；客户端退出不等于后台会话退出。
- 应用运行期间每 10 秒查询后台会话，包括已脱离的会话。发现未通过应用主动结束的会话消失时，记录上次后台 PID、socket 和当时的组件进程快照，不自动重建或终止会话。shell 内执行 `exit` 也会表现为会话消失，不能仅凭这条记录判定崩溃。
- Mac 还会提取最近三天、最多五份 tmux 系统崩溃报告中的时间、PID、异常和终止原因，需与故障时间关联；没有系统报告时不能凭空还原后台服务的退出原因。不记录终端输入或输出内容。
- 日志重启后追加保留，每份约 5 MiB，最多保留当前文件和四份历史文件（`p2pcli.log.1` 至 `.4`）。应用启动会留下新进程标记，可用于核对开发模式重启；强制结束进程可能没有正常退出记录。


## 仓库内置的预编译会话组件

Windows 的 psmux 3.4.0、macOS 和 Linux 的 tmux 3.7c 已作为真实二进制文件纳入 Git，许可证一并提交。拉取完整仓库后即可使用，无需另装 tmux、psmux 或 Homebrew；开发启动、打包和应用首次启动都不会下载或编译这些组件。项目本身仍需正常安装 Node/Rust 开发依赖。

- Windows 提供官方 x64、ARM64、x86 便携包中的 `psmux.exe`。
- Mac 提供官方 Apple Silicon、Intel 预编译版，最低支持 **macOS 15**。只依赖系统动态库，不需要额外安装 libevent 等库。
- Mac 通用版也已提交；它是两份未修改的官方二进制切片组成的 FAT 文件，没有重新编译或修改最低系统版本。
- Linux 提供官方 x86_64 / ARM64 静态 musl ELF，组件本身不依赖发行版的 tmux/libevent/ncurses 动态库；桌面程序仍依赖 GTK/WebKitGTK。
- `pnpm run dev` 启动前、`pnpm run build` 的前置步骤检查组件文件的大小、SHA-256、CPU 架构、Mac 最低系统版本和动态库路径、Linux ELF 静态链接。缺失或损坏时中止并提示恢复仓库文件。
- `app/src-tauri/binaries` 保存可执行文件；`app/src-tauri/resources/session-tools` 保存对应平台许可和来源记录；`scripts/session-tools.json` 锁定官方来源、归档及文件 SHA-256。
- Tauri 的平台配置只引用该平台的 `externalBin` 和许可目录，再按目标架构选择单份二进制。仓库包含所有版本，安装包不会携带另一平台或无关架构的组件。Mac 通用包只在显式指定 `universal-apple-darwin` 时使用。
- Tauri 在开发时将组件复制到可执行文件旁，Mac 发行包里的组件和 app 一起做 ad-hoc 签名（`tauri.macos.conf.json` 里 `signingIdentity` 是 `-`），没有公证。Git 已保留 Mac 文件的可执行权限。

可单独校验或查看目标，不构建应用：

```sh
pnpm run check:session-tools
pnpm run check:session-tools --all
pnpm run check:session-tools --target aarch64-pc-windows-msvc
pnpm run check:session-tools --target aarch64-apple-darwin --print-plan
node --test scripts/check-session-tools.test.mjs
```

默认使用 Tauri 传入的 `TAURI_ENV_TARGET_TRIPLE`，因此 `pnpm run build --target ...` 自动适配；手动校验可指定 `--target`，否则读取 Rust 主机目标。旧命令 `pnpm run prepare:session-tools` 保留为本地校验兼容入口，已移除下载和源码编译流程。历史 `.cache/session-tools` 缓存不再使用，也不参与打包。

维护者升级组件时应从清单中的官方项目获取发布文件，核对发布归档哈希，将二进制、许可证与清单一并更新，并运行全平台校验；不要在开发启动脚本中添加下载或编译逻辑。
