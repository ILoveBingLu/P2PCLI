# P2PCLI

点对点远程终端。在一台电脑上输入另一台的设备码和配对密码，对方点同意，就能在软件窗口里使用对方电脑的终端。claude、codex 这类命令行 AI 会直接用对方电脑上的账号和环境运行。

- 直连优先：IPv6 直连 > IPv4 打洞 > 中继兜底，全程端到端加密，中继只转发看不到内容
- 像远程桌面软件一样配对：9 位设备码 + 6 位配对密码（可以固定，重启也不变），密码对了还要被连方在弹窗里点允许
- 终端嵌在软件里：支持多标签切换，终端占满主工作区
- 设备之间直接传文件：拖进终端、右键发送，或者在终端里用 `p2pcli send`、`p2pcli pull`；`p2pcli exec` 能在对方电脑上跑命令，命令行 AI 也能用
- 认得会话里跑的命令行 AI：显示是哪个 AI、在干活还是在等你确认，等你确认或干完活时提醒你，终端下面有输入框，打完一整句再发给它
- Tauri 2 + React + xterm.js，安装包仅携带当前平台和架构的会话组件

## 下载和安装

到 [Releases](../../releases) 下载：

- macOS 15 及以上，Apple 芯片：`P2PCLI_x.y.z_aarch64.dmg`；Intel 芯片：`P2PCLI_x.y.z_x64.dmg`（不知道是哪种：苹果菜单 → 关于本机，看芯片一栏）
- Windows 10 及以上：大多数电脑 `P2PCLI_x.y.z_x64-setup.exe`，骁龙等 ARM 芯片 `P2PCLI_x.y.z_arm64-setup.exe`，32 位系统 `P2PCLI_x.y.z_x86-setup.exe`（不知道是哪种：设置 → 系统 → 系统信息，看系统类型一栏）

装好以后软件会自己检查更新（启动后一次，之后每 6 小时一次，设置 → 关于 里也能手动查），有新版本在主界面点“更新并重启”就行。更新包用发布者的私钥签过名，软件只装签名对得上的包。设置里配了 HTTP 代理时，检查和下载更新也走代理。

安装包没有付费签名，第一次打开会被系统拦一下：

- macOS：打开时提示 Apple 无法验证 P2PCLI，点 完成，再到 系统设置 → 隐私与安全性，在下面点 仍要打开
- Windows：提示 Windows 已保护你的电脑，点 更多信息 → 仍要运行

软件默认用 `wss://p2pcli.aiqji.cn` 牵线，装好就能用。想用自己部署的牵线服务器（自建或 Cloudflare，见下文），在软件标题栏的设置里换掉。

## 怎么用

1. 两台电脑都装好、打开 P2PCLI
2. 被连的一方点击标题栏右侧的“本机信息”图标，在弹出面板中查看、复制设备码和配对密码，发给对方。配对密码默认每次打开软件都换新的；点旁边的锁可以固定下来，重启也不变，对方以后用同一个密码就能连
3. 连接方点击标题栏右侧的加号，填写设备码和配对密码后点连接；也可在左侧连接历史中选择设备。被连方弹窗里点允许。连上过一次，软件会记住这台的配对密码，以后从连接历史或附近设备点一下就自动带上；对方换了密码会提示重新输入，从连接历史里删掉这台，记住的密码也一起删
4. 连上后就像在对方电脑上开了个终端；可以点 + 再开一个标签。左侧的设备和会话都能右键：新建会话、刷新会话、结束会话、复制设备码、从连接历史移除

左侧最上面的“本机”是这台电脑自己：点它展开就能在本机开终端、接回后台会话，多标签、后台会话、命令行 AI 的状态识别都和连远程设备一样，不经过网络，也不用配对和同意。本机的终端标签底部显示“本机”，关掉标签不影响它。本机没有 tmux（Windows 是 psmux）时，新开的是普通终端，能用，只是关了软件就没了。

打开软件后，左侧连接历史里的设备会自动显示在不在线、延迟多少（不弹窗、不算一次连接；延迟每 2 秒刷新，对方断网或关机几秒内显示离线），对方同意过一次的设备还会直接列出你开在它上面的会话（只有编号和状态），点会话就连上并打开；离线时显示上次看到的会话。被连方可以在 设置 → 受信设备 里管理这些配对过的设备。对方的 P2PCLI 也要是新版本才有延迟和会话列表，旧版本显示“需更新”。

同一个局域网里的电脑不用设备码：打开软件后会自动出现在左侧“附近设备”里，点一下就连（还是要配对密码和对方同意，受信设备直接连上）。连接历史里的设备在局域网里在线时会标“局域网”，从那里连也不经过牵线服务器，外网断了也能用。访客 Wi-Fi、开了设备隔离的网络互相看不到，照常用设备码；Windows 第一次打开时防火墙询问要允许，网络被设成“公用网络”时可能也看不到。不想让同一网络里的人看到本机，在设置里关掉“在局域网中显示本机”。

互相信任：连接时勾选“互相信任”，连上的同时把对方记成本机的受信设备，对方以后可以直接控制本机，不用密码也不用点同意（它上面的命令行 AI 就能直接操作本机），可以在 设置 → 受信设备 里随时移除。没勾的话，只要本机成功连过对方，对方反过来连本机时就不用输配对密码，只在本机弹窗问你；窗口没在前台时还会发系统通知。弹窗里点拒绝或没人点，30 秒内同一台设备再来也直接被拒，不会反复弹窗。没互相连过的设备照旧要配对密码。

被连方标题栏右边会一直显示谁正在用本机终端，点开能看是谁、开了几个终端、连了多久，随时可以断开。弹窗里勾选以后不用确认的设备，下次连过来免密码免弹窗，只建议给自己的电脑勾。关掉窗口软件会留在托盘，别人照样能连进来。

终端底部栏右边是这条连接的线路和延迟，点开是连接详情：网络延迟、按键回显（敲一个键到屏幕上出现字的时间，对方程序在刷屏时不测）、上下行带宽、丢包率；走的是 IPv6 直连、IPv4 直连还是中继，两头的地址；对方的软件版本、系统和会话管理器（tmux、psmux）。打开时每秒刷新一次，关着不统计。对方的版本和系统要两边都是 0.1.5 以上才看得到。

## 传文件

- 在左侧设备上右键选“发送文件”或“发送文件夹”；或者把文件拖进某台设备的终端窗口，发完会把文件在对方电脑上的路径填进终端，在那边跑的 claude、codex 直接就能读
- 收到的文件放在 `下载/P2PCLI/<发送设备名>/` 里，重名的自动加编号，不会覆盖。配对过的设备、自己连过的设备发来的直接收，别的设备发来要点同意
- 并发分段：文件切成 8 MiB 一段，最多开 8 条流一起发，每段各带 BLAKE3 校验，校验通过才算收好，断了再发会跳过已经收好的段。对方是旧版本时自动退回一条流。跨境这类高延迟、丢包的链路上能快一倍到几倍，传输面板里会显示实时速度和几条流并发
- 每个文件传完用 BLAKE3 核对；传到一半断了，再发同一批会从断点接着传
- 两边都要是 0.1.3 以上的版本

装好软件会带一个命令行工具 `p2pcli`（Mac 放在 `~/.local/bin`，Windows 自动加进 PATH；设置 → 文件传输 里能看状态、一键加 PATH），P2PCLI 开着就能用：

```sh
p2pcli devices                       # 列出设备、在线和延迟
p2pcli send 设备名 报告.pdf 项目目录   # 发文件或文件夹；设备名可以只写一部分，也可以写设备码
p2pcli send 设备名 a.txt --to ~/work  # 放到对方电脑的这个目录，同名文件盖掉
p2pcli pull 设备名 ~/work/out.log     # 把对方电脑上的文件拉到当前目录，--to 换本机目录
p2pcli exec 设备名 -- ls -la ~/work   # 在对方电脑上跑一条命令，输出和退出码带回来
p2pcli inbox                         # 看最近收到的文件
p2pcli send 设备名 a.txt --json       # 输出 JSON 给程序和 AI 读；失败时退出码不是 0
```

`send` 不带 `--to` 时只往对方的收件目录放，不盖已有的文件，对方认识这台设备就直接收。`send --to`、`pull`、`exec` 能碰对方电脑上任意位置，所以走和开终端一样的认证：配对密码对、对方点过同意（或勾了以后不用确认）才行，连上后复用同一条连接。两台设备之间如果谁都没连过谁，第一次要先在界面里连一次、输对方的配对密码（连的时候可以勾“互相信任”）；对方连过本机，或者对方勾了受信，就不用密码。对方没勾受信时每次新连接都要它点同意。被拉走、被写入文件时，对方的传输记录和提醒里看得到，跑命令时标题栏会算一个正在用的终端。两边都要是 0.1.5 以上，对方版本太旧会提示更新。

多机互动：软件启动时会自动把一份使用说明（Agent Skills 格式的 `SKILL.md`）放进本机已安装的 AI 的技能目录，AI 新开的会话就知道有 `p2pcli`、什么时候该用、出错怎么办，不用用户教：

- Claude Code：`~/.claude/skills/p2pcli/`（`CLAUDE_CONFIG_DIR` 换了位置就跟着换）
- Codex、Gemini CLI、Cursor、OpenCode、Copilot CLI、Kimi CLI：通用的 `~/.agents/skills/p2pcli/`
- Qwen Code：`~/.qwen/skills/p2pcli/`；CodeBuddy：`~/.codebuddy/skills/p2pcli/`

只往已经存在的 AI 配置目录里写，不替没装的 AI 建目录；只写一个叫 `p2pcli` 的文件夹，不改 `CLAUDE.md`、`AGENTS.md`。文件里带 `managed-by: p2pcli` 标记，软件升级时只覆盖带标记的，别人放的同名技能不动。设置 → 多机互动 里能看到检测到哪些 AI、各自装没装，可以单个或全部安装、更新、删除，也能关掉“启动时自动安装和更新”（关掉只是不再自动动，已装的不会被删；开发版也能手动装，只是不自动装）。已经开着的 AI 窗口要重开才会读到。说明的原文在 `app/src-tauri/src/p2pcli-skill.md`，加了命令或改了用法要同步改它。

`exec` 细节：

- 命令交给对方的 shell 解析（macOS、Linux 用登录 shell，Windows 用 PowerShell），引号自己写好，用法同 ssh
- 没有伪终端，命令的输入是空的，不能交互，程序也不会因为连着终端而换样子；标准输出和标准错误分开回来，字节原样不乱码
- 退出码就是命令的退出码；超时（`--timeout 秒`）是 124，被信号结束是 128 加信号编号，P2PCLI 自己出错（找不到设备、连不上）是 255
- 默认在对方的用户目录跑，`--cwd` 换目录；`--json` 把标准输出、标准错误、退出码打包成 JSON
- 命令行被 Ctrl+C 或管道关了，对方会把整组进程一起结束；命令自己放到后台的进程不管，要长期跑的用 `nohup` 并重定向输出

## 认会话里的 AI

普通终端停在空提示符时，底部会浮出这台被控电脑已安装 AI 的启动胶囊，点击就在当前目录启动；输入命令或进入其他程序后隐藏，回到空提示符再显示。检测结果会缓存，旁边的刷新按钮可以重新检测。检测和启动都在被控端完成，双方需更新到支持启动器的版本；未知的自定义提示符会保守隐藏胶囊。

默认按 AI 自身的权限设置启动。右键 Claude Code 胶囊可以选择“启动并允许之后切换全部放行”（实际追加 `--allow-dangerously-skip-permissions`），只增加该模式的可选入口，不自动进入全部放行。

设置 → AI 启动 里可以给本机检测到的每种 AI 单独配置默认启动命令，支持完整路径、环境变量和参数；留空恢复自动检测到的命令。配置只保存在被控端本机，连接方不能上传命令，保存后立即用于之后的启动。若明确需要 Claude Code 启动后跳过工具确认，可以把它的命令设为 `claude --dangerously-skip-permissions`，之后普通点击胶囊也会全部放行；只应在可信项目中使用，它不改变操作系统权限，也不等于 `sudo`。自定义命令会交给本机 Shell 执行，不要在命令里保存令牌或密码。

远程会话里跑着命令行 AI 时，侧边栏、终端标签和底部状态栏会显示它的图标和状态：干活中、等你确认、空闲。忙碌画面连续 10 分钟没有变化会标成“可能卡住”，但不会被误判成空闲；长工具调用也可能暂时没有输出，需要到终端里检查。它停下来等你批准操作，或者干完活时，P2PCLI 会提醒你：窗口在后台发系统通知，在前台但没在看那个会话就弹提示，点“查看”切过去；正看着那个会话就不打扰。

- 认得：Claude Code、Codex、Gemini CLI、OpenCode、Cursor Agent、Copilot CLI、Qwen Code、Kimi CLI、CodeBuddy、Qoder CLI、iFlow CLI、Kiro CLI、Trae CLI、Amp、Aider、Crush、Droid、Goose、Auggie、ZCode（Z.ai 的 zcode-app-cli）。被连的电脑顺着会话的进程往下找，看程序名和完整命令行（Node、Python 写的 AI 程序名是 node、python，要看脚本路径），新出的在 `crates/core/src/agents.rs` 的表里加一行
- 状态：读会话标题和屏幕最后几十行判断，比如屏幕底部有编号选项和“Enter to confirm”就是在等你，有“✻ Cogitating…”或“esc to interrupt”就是在干活。整屏内容只在被连的电脑上解析；对方收到 AI 类型、状态，以及能明确识别的确认菜单和请求摘要
- 各家改了界面文字时状态可能判断不准，规则也在 `agents.rs`，测试里放着真实截下来的屏幕
- 额度：终端底部栏显示那台电脑上 AI 账号的额度，比如“5 小时 35% · 7 天 75%”，用到 90%、用满各提醒一次。Claude Code 读被连电脑上状态栏脚本存下的 `~/.claude/.statusline-ratelimits.json`（Claude Code 只把额度交给状态栏脚本，没配这样的状态栏脚本就读不到）；Codex 读 `~/.codex/sessions` 里最新会话日志最后一次记下的额度。只读本机文件，不联网、不碰登录凭据
- 输入框：看着跑 AI 的会话时，终端下面多一个输入框，在本机打完一整句再发过去，中文输入法选字不卡，长段落好改。回车发送，Shift+回车换行，输入框空着时上下箭头翻发过的话；文件拖到输入框上，传完把对方电脑上的路径填进来。⌘L（Windows 是 Ctrl+Shift+L）在终端和输入框之间切换，Esc 回终端，收起后按同一个键或输入框右上角的突出开关再打开。发送时按整段粘贴发过去，AI 不会把中间的换行当成回车；AI 在等你确认时不让发，免得一句话被当成选项
- 确认列表：Claude Code、Codex、Gemini CLI 的常见编号单选确认框，以及 OpenCode 的标准三项权限菜单，会浮在输入框上方，显示请求摘要和原始选项。被控端点击前重新核对屏幕、AI 进程和确认编号，再发送方向键与回车；同一确认只能提交一次。多选、自由输入、截断菜单、无法确定高亮项等情况保留终端操作，双方需更新。只传识别出的菜单及附近请求摘要，不上传整屏。
- 切模型、思考强度、权限模式：Claude Code 的会话在输入框右边有三个按钮。模型和强度照你在终端里打 `/model`、`/effort` 那样切，Claude Code 会顺带把它存成那台电脑上新会话的默认值（它自己的规矩，正在跑的其他会话不受影响）；权限模式只能 Shift+Tab 轮换，每按一下核对一次，到了就停，只改这个会话，“全部放行”要启动时加了 `--dangerously-skip-permissions` 才在轮换里。等你确认时都不能切，模型和强度要等它这一轮干完。当前值的来源：模型和强度读被连电脑上这个会话的对话记录（按 `~/.claude/sessions/进程号.json` 找到 `~/.claude/projects` 下的记录，设了 `CLAUDE_CONFIG_DIR` 就在那里找），刚开还没记录时读开头的横幅；权限模式读屏幕最底下那行。Codex 的模型列表随账号变，两个按钮打开它自己的 `/model` 和 `/permissions` 选择框。OpenCode 提供模型、思考强度、权限三个入口：模型打开 `/models`，思考强度打开 `/variants`（档位由模型和供应商配置决定），权限通过默认 Ctrl+P 打开命令面板并搜索 `permissions`，由用户在终端里确认开启或关闭自动批准，不会直接放行。按 OpenCode v1.19.4 核对；旧版若没有 variants 或自动批准入口需更新 OpenCode，自定义过 Ctrl+P 的用户仍可在终端自行打开命令面板。OpenCode 正在工作或等待确认时禁用这三个入口；当前选项在 OpenCode 自己的选择框中显示，不会把 Claude 的设置当成 OpenCode 的当前值。ZCode 也有这三个按钮，三样都从它最底下的状态栏读（`◈ 模型 ─ ◉ 模式 ─ ⚡ 强度`），能选的模型和每个模型的强度档位读被连电脑上的 `~/.zcode/cli/config.json`，只取模型名和档位，不碰里面的 key；切模型、强度发 `/model 提供方/模型`、`/effort 档位`，权限模式同样 Shift+Tab 轮换（build → edit → yolo → plan）。ZCode 自己的规矩：模型只改这个会话，强度会成为那台电脑上新开 ZCode 的默认值，权限模式会成为那个目录下的默认值
- 两边都要是 0.1.4 以上的版本；连着的设备每 4 秒更新一次状态
- AI 品牌图标是 [Lobe Icons](https://github.com/lobehub/lobe-icons)（MIT）的静态 SVG，放在 `app/src/assets/agents`，按图片显示，见那里的 LICENSE。不引 `@lobehub/icons`：它会带进整套 Lobe UI，新版本又常常刚发布、过不了 pnpm 默认的发布时间检查（24 小时内发布的包不装）。新增 CLI 时按 `agents.rs` 里的 kind 放一个同名 SVG；没收录的显示通用机器人图标

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

拥塞控制（`crates/core/src/quic.rs`）：iroh 默认的 Cubic 靠丢包判断拥堵，跨境这类往返一百多毫秒、丢包 1% 上下的链路上，传文件只剩 1 Mbps 左右。软件按链路延迟自动选算法：最小往返延迟 20 毫秒以上用 BBR3，局域网和近距离保持 Cubic（BBR3 在低延迟链路上反而更慢）。用模拟链路实测（`cargo test -p p2pcli-core --test lossy_link -- --ignored --nocapture --test-threads=1`）：往返 106 毫秒、丢包 1.3% 时从 0.9 Mbps 提到约 40 Mbps，局域网速度不变。发送数据的一头用什么算法由它自己决定，所以两边都升级到 0.1.5 以上才能两个方向都提速。单条流的窗口不要调大：接收缓冲最多容纳 1024 个不连续的数据块，窗口放大后丢包交错时会被对方直接断开连接；要提速靠并发分段（`crates/core/src/files/lanes.rs`），多条流各算各的这个限制。用模拟链路实测真实的传文件代码（`cargo test -p p2pcli-core --test lossy_link -- --ignored --nocapture --test-threads=1 file_transfer_over`）：往返 106 毫秒、丢包 1.3% 时一条流 38 Mbps，并发约 73 Mbps；往返 106 毫秒、300 Mbps、丢包 0.1% 时 58 对 208 Mbps；局域网和近距离持平。

选路（`crates/core/src/path.rs`）：IPv6 只要不比 IPv4 慢 50 毫秒以上就用 IPv6，其次 IPv4，中继只在都不通时用。iroh 自带的规则只给 IPv6 让 3 毫秒、又要求快 5 毫秒才换路，做不到 IPv6 优先，所以用了它的选路接口（这个接口标着不稳定，iroh 版本已锁死）。

## 牵线服务器

牵线服务器只负责发设备码、交换地址、转达 IPv6 发包通知，终端数据不经过它。有两种接法，客户端按地址写法自动区分：

| 写法 | 通道 | 适合 |
| --- | --- | --- |
| `公钥@IP:端口` 或 `公钥@域名:端口` | UDP（iroh，加密并认证服务器身份） | 自己有公网服务器 |
| `wss://域名` | WebSocket（TLS） | 放在 Cloudflare Workers 这类只支持 HTTP 的平台，不用自己维护服务器 |

两种接法连的是不同的服务器时，设备之间互相查不到；同一台自建服务器同时开两个入口时，走 UDP 和走 WebSocket 的设备能互相查到。

### 自建服务器

Linux 服务器（带 systemd，x86_64 或 aarch64）上一条命令装好：

```sh
curl -fsSL https://raw.githubusercontent.com/ILoveBingLu/P2PCLI/main/crates/server/install.sh | sudo bash
```

脚本会下载发布页上编译好的程序、核对校验和、装成开机自启的服务，最后打印软件里要填的地址（`公钥@IP:端口`）。装完在云服务器控制台的安全组里放行 UDP 7788。再跑一次就是升级（只换程序，原来的服务配置不动），参数写在 `bash -s --` 后面，例如 `| sudo bash -s -- --port 9000`：`--ws-port` 另开 WebSocket 入口，`--mirror` 给国内服务器下载 GitHub 慢时加代理前缀，`--uninstall` 卸载，`--help` 看全部。

不用脚本也可以直接跑：

```sh
p2pcli-server --data-dir /var/lib/p2pcli --port 7788 --ws-port 7789 --public-ip <公网IP>
```

启动时会打印 UDP 写法要填的地址（`公钥@IP:端口`），IP 也可以换成直接解析到服务器的域名，每次连接时现解析，服务器换 IP 改解析就行。`--ws-port` 开的是明文 WebSocket，对外用 `wss://` 时在前面加反向代理（例如 Caddy：`rdv.example.com { reverse_proxy 127.0.0.1:7789 }`），或者把域名放在 Cloudflare、阿里云 ESA 这类 CDN 后面。CDN 只转 HTTP，域名走了 CDN 就只能用 `wss://` 写法，好处是服务器 IP 不公开，CDN 帮着挡攻击。

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

发布软件：改好 `app/src-tauri/tauri.conf.json` 的版本号，推一个同版本的 `vX.Y.Z` 标签（标签附带的说明会成为更新说明），GitHub Actions 会同时编译 Mac（Apple 芯片、Intel）和 Windows（x64、ARM64、32 位）五个安装包，连同在线更新用的 `latest.json` 发到发布页并标成最新版本（`.github/workflows/app-release.yml`）。改了打包配置可以先推一个 `ci/` 开头的分支试编，只编不发，编出来的包在那次运行的 Artifacts 里。dmg 窗口背景和 Windows 安装界面的图在 `app/src-tauri/installer`，Mac 菜单栏图标是 `app/src-tauri/icons/tray.svg`，改了 SVG 在 Mac 上跑 `CHROME=<Chrome 可执行文件> node scripts/render-art.mjs` 重新出图。在线更新包要签名：仓库 Secrets 里放 `TAURI_SIGNING_PRIVATE_KEY`（私钥文件内容，没有密码），公钥写在 `tauri.conf.json` 的 `plugins.updater.pubkey`。私钥丢了，已经装上的软件就收不到更新了。本地打正式包不需要私钥，只有加上 `--config '{"bundle":{"createUpdaterArtifacts":true}}'` 才会生成更新包并要求签名。

发布牵线服务器：改好根目录 `Cargo.toml` 的版本号，推一个同版本的 `server-vX.Y.Z` 标签，GitHub Actions 会编译 Linux x86_64、aarch64 两个版本连同校验和放到发布页（`.github/workflows/server-release.yml`），一键安装脚本就从那里下载。

界面预览：在根目录执行 `pnpm run dev:web`，然后打开 `http://localhost:1420/preview.html?s=empty`（s 还可以是 request、busy、relay），用假数据渲染界面。仅构建前端可执行 `pnpm run build:web`。

界面使用 [HeroUI v3](https://heroui.com/)、Tailwind CSS v4 和 [Lucide](https://lucide.dev/) 图标，采用深色桌面工作区布局。按钮、输入框、卡片、弹窗、标签页、复选框、通知和悬停提示使用 HeroUI；应用尺寸与语义样式封装位于 `app/src/components/ui`，主题与布局样式位于 `app/src/styles.css`。聚焦时不显示额外光圈，保留键盘交互和表单错误提示。

基础控件使用 HeroUI 默认深色主题、圆角、尺寸和交互样式；封装只适配业务属性和布局，不重新绘制控件。设置页内的输入框使用库自带的 `secondary` 变体，与弹窗底色区分。

Windows 和 macOS 都使用单行自定义标题栏，网络诊断和设置保留在标题栏内。Windows 接入 [tauri-plugin-window-controls](https://github.com/insd47/tauri-plugin-window-controls) 0.2.1（通过仓库内 `vendor/tauri-plugin-window-controls` 补丁修正图标灰度并取消失焦淡化，保留原 MIT 许可）：控件由 Win32 子窗口承载，使用系统图标字体绘制并发送系统窗口命令，支持 Windows 11 贴靠布局；它并非直接复用 DWM 默认标题栏按钮。macOS 使用覆盖式标题栏及系统红黄绿按钮，`trafficLightPosition` 设为 `{ x: 14, y: 24 }`，针对 44 逻辑像素标题栏修正按钮偏高的问题。Tao/Wry 的 `y` 用于调整原生标题栏容器高度，并非按钮中心坐标，不能直接取网页标题栏高度的一半；不同 macOS 版本的最终对齐需实机核对。关闭窗口仍隐藏到托盘。

点击标题栏齿轮进入主区域的设置页，返回工作区时已有终端会话保持运行。“代理设置”分类中可启用 HTTP 代理，例如 `http://127.0.0.1:7890`（使用代理软件的 HTTP 或混合端口）。代理用于 WebSocket 牵线和 HTTP(S) 中继；UDP 牵线、点对点直连不经过该代理。暂不支持 SOCKS5、HTTPS 代理或账号密码认证，也不会自动读取系统代理。保存后即时应用，不必重启应用。设备名、牵线地址变更保留当前终端连接；代理或中继变更会重建网络端点，需重新连接远端，但后台终端会话继续运行。环境变量指定的服务器/中继仍优先于设置页。

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

图标：

- macOS：源文件是 `app/src-tauri/icons/AppIcon.icon`（Icon Composer 的分层液态玻璃图标，可以直接用 Icon Composer 打开改）。改完在 `app/src-tauri/icons` 里运行 `./build-macos-icons.sh`，生成 `Assets.car`（macOS 26 起用，跟着浅色、深色、着色、透明外观变）和 `icon.icns`（老系统和开发模式的 Dock 图标），两份都提交，打包时不需要 Xcode。脚本需要 Xcode 26 以上。
- Windows 和界面里的 PNG：源文件是 `app/icon-source.svg`，用 `pnpm exec tauri icon app/icon-source.svg -o app/src-tauri/icons` 生成，生成后把 `icon.icns` 换回上面脚本生成的那份。

环境变量：

| 变量 | 作用 |
| --- | --- |
| `P2PCLI_SERVER` | 牵线服务器，`公钥@IP或域名:端口` 或 `wss://域名`，默认 `wss://p2pcli.aiqji.cn` |
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
# 局域网：列出附近设备，再按公钥连，不用牵线服务器
p2pcli-dev --server "" nearby
p2pcli-dev --server "" connect <公钥> --password abc234 --run 'echo hi'
```

`--relay-only` 强制走中继，`--bind-ip` 只绑某个网卡，`--no-lan` 关掉局域网发现，`--lan-hidden` 只看别人不广播本机。

## 已验证

- macOS ↔ macOS 局域网直连；macOS ↔ Windows 隔两层 NAT 的 IPv4 打洞直连；强制中继
- 桌面软件三个方向（macOS→macOS、macOS→Windows、Windows→macOS）真界面走通：输设备码、弹窗同意、标签页里跑命令、中文显示
- IPv6 直连流程：同进程测试和两台 Mac 跨机（用局域网 IPv6 代替公网 IPv6），第一个包就走 IPv6

还没验：真公网 IPv6 隔着两边路由器防火墙的直连、自建中继走 HTTPS 的正式部署。

## 许可证

MIT，见 [LICENSE](LICENSE)。


### 终端会话保持

桌面端终端由被连设备上的独立后台进程托管：macOS/Linux 使用 tmux，Windows 使用原生 ConPTY 的 [psmux](https://github.com/psmux/psmux)，不需要 WSL。

- Windows 与 macOS 发行版内置会话管理器，用户无需安装 tmux、psmux、Homebrew 或 WSL，首次启动也不联网下载组件。
- 按打包目标系统和 CPU 架构选择组件：Windows 只带对应架构的 psmux 3.4.0；Mac 只带对应架构的 tmux 3.7c。不会把另一系统的二进制放进安装包。Apple Silicon 和 Intel 分包；仅显式选择 universal 时使用已提交的双架构 Mac 文件。
- 桌面端通过应用目录中的绝对路径调用组件，不依赖用户 PATH。若安装内容损坏则提示重新安装，不会悄悄调用系统中其他版本。独立 core/CLI 仍可使用自行安装的工具。
- 连接设备后显示后台会话列表，可新建、恢复或明确结束会话；终端工具栏的会话按钮可再次打开列表。
- 断线、关闭标签和退出 P2PCLI 只断开附着客户端。再次连接同一设备，可恢复原进程、工作目录、变量和终端画面。
- 点击“结束会话”并确认，或者在 shell 内执行 `exit`，才会结束该会话。系统重启、注销或后台 tmux/psmux 服务被结束仍会使进程退出。
- 会话按双方设备公钥隔离；重连仍需要正常认证。不要删除设备密钥，否则新身份不能恢复旧身份的会话。
- 协议已升级到 v2，连接双方需要一起更新。开发用一次性 CLI 命令使用临时终端并保留真实退出码，不创建后台会话。

预览页 `preview.html?s=sessions` 提供后台会话模拟数据。真实会话恢复回归用例为 `persistent_session_survives_disconnect_and_host_restart`，默认忽略，开发者可用 `P2PCLI_TEST_SESSION_PROGRAM` 指定准备好的组件绝对路径后显式运行，无需全局安装；测试会起独立的测试命名空间。

### 会话异常日志

在**被控端**打开标题栏的「网络诊断 → 查看日志」，会在文件管理器中选中 `p2pcli.log`。默认级别即可记录，无需运行诊断命令或设置 `RUST_LOG`。

- 记录创建、恢复、主动结束、网络流断开，以及连接客户端的 PID、退出码和信号；客户端退出不等于后台会话退出。
- 应用运行期间每 10 秒查询后台会话，包括已脱离的会话。发现未通过应用主动结束的会话消失时，记录上次后台 PID、socket 和当时的组件进程快照，不自动重建或终止会话。shell 内执行 `exit` 也会表现为会话消失，不能仅凭这条记录判定崩溃。
- Mac 还会提取最近三天、最多五份 tmux 系统崩溃报告中的时间、PID、异常和终止原因，需与故障时间关联；没有系统报告时不能凭空还原后台服务的退出原因。不记录终端输入或输出内容。
- 日志重启后追加保留，每份约 5 MiB，最多保留当前文件和四份历史文件（`p2pcli.log.1` 至 `.4`）。应用启动会留下新进程标记，可用于核对开发模式重启；强制结束进程可能没有正常退出记录。


### 仓库内置的预编译会话组件

Windows 的 psmux 3.4.0 和 macOS 的 tmux 3.7c 已作为真实二进制文件纳入 Git，许可证一并提交。拉取完整仓库后即可使用，无需另装 tmux、psmux 或 Homebrew；开发启动、打包和应用首次启动都不会下载或编译这些组件。项目本身仍需正常安装 Node/Rust 开发依赖。

- Windows 提供官方 x64、ARM64、x86 便携包中的 `psmux.exe`。
- Mac 提供官方 Apple Silicon、Intel 预编译版，最低支持 **macOS 15**。只依赖系统动态库，不需要额外安装 libevent 等库。
- Mac 通用版也已提交；它是两份未修改的官方二进制切片组成的 FAT 文件，没有重新编译或修改最低系统版本。
- `pnpm run dev` 启动前、`pnpm run build` 的前置步骤仅检查本地文件的大小、SHA-256、CPU 架构、Mac 最低系统版本和动态库路径。缺失或损坏时中止并提示恢复仓库文件。
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
