# P2PCLI

点对点远程终端。在一台电脑上输入另一台的设备码和临时密码，对方点同意，就能在软件窗口里使用对方电脑的终端。claude、codex 这类命令行 AI 会直接用对方电脑上的账号和环境运行。

- 直连优先：IPv6 直连 > IPv4 打洞 > 中继兜底，全程端到端加密，中继只转发看不到内容
- 像远程桌面软件一样配对：9 位设备码 + 6 位临时密码，密码对了还要被连方在弹窗里点允许
- 终端嵌在软件里：多标签，显示当前线路（IPv6 直连 / IPv4 直连 / 中继）和延迟
- 很小：Tauri 2 + React + xterm.js，macOS 安装包约 8 MB，Windows 安装包约 6 MB

## 下载和安装

到 [Releases](../../releases) 下载：

- macOS（Apple 芯片）：`P2PCLI_x.y.z_aarch64.dmg`
- Windows：`P2PCLI_x.y.z_x64-setup.exe`

安装包没有付费签名，第一次打开会被系统拦一下：

- macOS：打开时提示无法验证开发者，到 系统设置 → 隐私与安全性，点 仍要打开
- Windows：提示 Windows 已保护你的电脑，点 更多信息 → 仍要运行

软件默认用 `wss://p2pcli.yscopy.cn` 牵线，装好就能用。想用自己部署的牵线服务器（自建或 Cloudflare，见下文），在软件标题栏的设置里换掉。

## 怎么用

1. 两台电脑都装好、打开 P2PCLI
2. 被连的一方把左上角的设备码和临时密码发给对方
3. 连接方填好点连接，被连方弹窗里点允许
4. 连上后就像在对方电脑上开了个终端；可以点 + 再开一个标签

被连方顶部会一直显示谁正在用本机终端，随时可以断开。弹窗里勾选以后不用确认的设备，下次连过来免密码免弹窗，只建议给自己的电脑勾。关掉窗口软件会留在托盘，别人照样能连进来。

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
3. 连接方发开场消息带临时密码；被连方校验密码（同一设备连错 5 次锁 10 分钟，所有设备加起来错太多也暂停），再弹窗问主人
4. 主人同意后，连接方每开一个标签就开一条 QUIC 流，被连方起一个真终端接上去（macOS 是用户的登录 shell，Windows 是 PowerShell）

IPv6 直连（`crates/core/src/v6.rs`）：公网 IPv6 没有 NAT，网卡地址就是对外地址，用不着打洞，只是两边路由器有状态防火墙。两边都有公网 IPv6 时，连接方通过牵线服务器通知被连方同时往自己的 IPv6 发包，把被连方的防火墙打开；连接方自己直接连对方的 IPv6，第一个包就走直连，不经过中继。1.5 秒还没通就并行走常规流程（先经中继连上，再打洞），哪条先通用哪条。

选路（`crates/core/src/path.rs`）：IPv6 只要不比 IPv4 慢 50 毫秒以上就用 IPv6，其次 IPv4，中继只在都不通时用。iroh 自带的规则只给 IPv6 让 3 毫秒、又要求快 5 毫秒才换路，做不到 IPv6 优先，所以用了它的选路接口（这个接口标着不稳定，iroh 版本已锁死）。

## 牵线服务器

牵线服务器只负责发设备码、交换地址、转达 IPv6 发包通知，终端数据不经过它。有两种接法，客户端按地址写法自动区分：

| 写法 | 通道 | 适合 |
| --- | --- | --- |
| `公钥@IP:端口` | UDP（iroh，加密并认证服务器身份） | 自己有公网服务器 |
| `wss://域名` | WebSocket（TLS） | 放在 Cloudflare Workers 这类只支持 HTTP 的平台，不用自己维护服务器 |

两种接法连的是不同的服务器时，设备之间互相查不到；同一台自建服务器同时开两个入口时，走 UDP 和走 WebSocket 的设备能互相查到。

### 自建服务器

```sh
p2pcli-server --data-dir /var/lib/p2pcli --port 7788 --ws-port 7789 --public-ip <公网IP>
```

放行 UDP 7788。启动时会打印 UDP 写法要填的地址（`公钥@IP:端口`）。`--ws-port` 开的是明文 WebSocket，对外用 `wss://` 时在前面加反向代理（例如 Caddy：`rdv.example.com { reverse_proxy 127.0.0.1:7789 }`），或者把域名放在 Cloudflare 代理后面。

### Cloudflare Workers

`cloudflare/` 文件夹是同一套协议的 Workers 版，单独一个文件夹，一条命令就能部署：

```sh
cd cloudflare
./deploy.sh 你的域名        # Windows 用 .\deploy.ps1 你的域名
```

需要一个托管在 Cloudflare 的域名，部署后客户端填 `wss://你的域名`。详细说明、本机试跑和费用见 [cloudflare/README.md](cloudflare/README.md)。

## 中继

打不通时转发终端数据的中继要另外准备，iroh 官方公共中继在国内很慢甚至连不上。中继是 [iroh-relay](https://github.com/n0-computer/iroh)，需要一台有公网 IP 的服务器：

```sh
iroh-relay --config-path relay.toml   # 用 Let's Encrypt 自动签证书
```

放行 TCP 80/443（中继）、UDP 7842（中继的地址探测）。客户端在设置里填中继地址，或用 `P2PCLI_RELAY`。

## 开发

需要 Rust 1.88 以上、Node 24、pnpm。

以下命令都在项目根目录执行，不需要进入 `app`：

```sh
cargo test --workspace              # 单元测试、同进程两台设备完整走一遍、IPv6 直连流程
pnpm install                       # 安装应用依赖
pnpm run dev                       # 启动桌面应用开发模式
pnpm run build --bundles app,dmg    # macOS 正式包
pnpm run build --bundles nsis       # Windows 正式包
```

界面预览：在根目录执行 `pnpm run dev:web`，然后打开 `http://localhost:1420/preview.html?s=empty`（s 还可以是 request、busy、relay），用假数据渲染界面。仅构建前端可执行 `pnpm run build:web`。

界面使用 [HeroUI v3](https://heroui.com/)、Tailwind CSS v4 和 [Lucide](https://lucide.dev/) 图标，采用深色桌面工作区布局。按钮、输入框、卡片、弹窗、标签页、复选框和通知使用 HeroUI；应用尺寸与语义样式封装位于 `app/src/components/ui`，主题与布局样式位于 `app/src/styles.css`。聚焦时不显示额外光圈，保留键盘交互和表单错误提示。

基础控件使用 HeroUI 默认深色主题、圆角、尺寸和交互样式；封装只适配业务属性和布局，不重新绘制控件。设置弹窗内的输入框使用库自带的 `secondary` 变体，与弹窗底色区分。

Windows 和 macOS 都使用单行自定义标题栏，网络诊断和设置保留在标题栏内。Windows 接入 [tauri-plugin-window-controls](https://github.com/insd47/tauri-plugin-window-controls) 0.2.1：控件由 Win32 子窗口承载，使用系统图标字体绘制并发送系统窗口命令，支持 Windows 11 贴靠布局；它并非直接复用 DWM 默认标题栏按钮。macOS 使用覆盖式标题栏及系统红黄绿按钮。关闭窗口仍隐藏到托盘。

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

环境变量：

| 变量 | 作用 |
| --- | --- |
| `P2PCLI_SERVER` | 牵线服务器，`公钥@IP:端口` 或 `wss://域名`，默认 `wss://p2pcli.yscopy.cn` |
| `P2PCLI_RELAY` | 自建中继地址，逗号分隔 |
| `P2PCLI_DATA_DIR` | 数据目录，一台机器开两份测试时用 |
| `RUST_LOG` | 日志级别，日志在数据目录的 `p2pcli.log` |
| `P2PCLI_SELFTEST` | 仅调试版：自测脚本，见 `app/src/selftest.ts` |
| `P2PCLI_IPV6_ULA` | 测试用：把局域网 fd 开头的 IPv6 也当公网地址 |

打包时设置 `P2PCLI_DEFAULT_SERVER`、`P2PCLI_DEFAULT_RELAY`，可以把别的默认服务器编进安装包。

跨机测试命令行：

```sh
# 被连方
p2pcli-dev --server <公钥@IP:端口> host --password abc234 --approve yes
# 连接方：跑一条命令，打印输出和线路
p2pcli-dev --server <公钥@IP:端口> connect <设备码> --password abc234 --run 'echo hi' --watch 5
```

`--relay-only` 强制走中继，`--bind-ip` 只绑某个网卡。

## 已验证

- macOS ↔ macOS 局域网直连；macOS ↔ Windows 隔两层 NAT 的 IPv4 打洞直连；强制中继
- 桌面软件三个方向（macOS→macOS、macOS→Windows、Windows→macOS）真界面走通：输设备码、弹窗同意、标签页里跑命令、中文显示
- IPv6 直连流程：同进程测试和两台 Mac 跨机（用局域网 IPv6 代替公网 IPv6），第一个包就走 IPv6

还没验：真公网 IPv6 隔着两边路由器防火墙的直连、自建中继走 HTTPS 的正式部署。

## 许可证

MIT，见 [LICENSE](LICENSE)。
