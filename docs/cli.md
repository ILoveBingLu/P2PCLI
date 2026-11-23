**简体中文** | [English](cli.en.md)

# 命令行工具 p2pcli

装好软件会带一个命令行工具 `p2pcli`（Mac 放在 `~/.local/bin`，Windows 自动加进 PATH；Linux 将副本保存在应用数据目录的 `bin` 中，避免 AppImage 临时挂载路径失效。设置 → 文件传输 里能看状态、一键加 PATH），P2PCLI 开着就能用：

```sh
p2pcli devices                       # 列出设备、在线和延迟
p2pcli send 设备名 报告.pdf 项目目录   # 发文件或文件夹；设备名可以只写一部分，也可以写设备码
p2pcli send 设备名 a.txt --to ~/work  # 放到对方电脑的这个目录，同名文件盖掉
p2pcli pull 设备名 ~/work/out.log     # 把对方电脑上的文件拉到当前目录，--to 换本机目录
p2pcli exec 设备名 -- ls -la ~/work   # 在对方电脑上跑一条命令，输出和退出码带回来
p2pcli cat 设备名 ~/work/main.rs -n    # 直接读对方电脑上的文件，不用拉过来
p2pcli edit 设备名 ~/work/main.rs --old '旧的' --new '新的'   # 直接改对方电脑上的文件：按原文替换
printf '内容' | p2pcli write 设备名 ~/work/notes.md         # 用标准输入整个写入（新建或整个重写）
p2pcli grep 设备名 'TODO' ~/work      # 搜内容；还有 ls、find、mkdir、mv、rm（移到废纸篓）
p2pcli inbox                         # 看最近收到的文件
p2pcli send 设备名 a.txt --json       # 输出 JSON 给程序和 AI 读；失败时退出码不是 0
```

## 认证

`send` 不带 `--to` 时只往对方的收件目录放，不盖已有的文件，对方认识这台设备就直接收。`send --to`、`pull`、`exec` 能碰对方电脑上任意位置，所以走和开终端一样的认证：配对密码对、对方点过同意（或勾了以后不用确认）才行，连上后复用同一条连接。两台设备之间如果谁都没连过谁，第一次要先在界面里连一次、输对方的配对密码（连的时候可以勾“互相信任”）；对方连过本机，或者对方勾了受信，就不用密码。对方没勾受信时每次新连接都要它点同意。被拉走、被写入文件时，对方的传输记录和提醒里看得到，跑命令时标题栏会算一个正在用的终端。两边都要是 0.1.5 以上，对方版本太旧会提示更新。

## 直接读写对方的文件

直接读写对方的文件（`cat`、`write`、`edit`、`ls`、`find`、`grep`、`mkdir`、`mv`、`rm`）就是给 AI 用的“跨设备编辑”：本机的 Claude Code、Codex 这类 AI 改另一台电脑上的代码时，不用先拉过来、改完再发回去。`edit` 的规则和 AI 自带的编辑工具一样：原文要一字不差、只出现一次（或加 `--all`），写回时核对文件在读出来以后没被别人改过（对方终端里的程序、编辑器里的保存），改过就报错不盖掉；CRLF 换行的文件也对得上。只支持 UTF-8 文本、不超过 5 MB，别的用 `pull`/`send`。`rm` 移到对方的废纸篓，能找回。需要对方也是带编辑器的版本（0.1.9 以上）。

## 让 AI 自己会用（多机互动）

软件启动时会自动把一份使用说明（Agent Skills 格式的 `SKILL.md`）放进本机已安装的 AI 的技能目录，AI 新开的会话就知道有 `p2pcli`、什么时候该用、出错怎么办，不用用户教：

- Claude Code：`~/.claude/skills/p2pcli/`（`CLAUDE_CONFIG_DIR` 换了位置就跟着换）
- Codex、Gemini CLI、Cursor、OpenCode、Copilot CLI、Kimi CLI：通用的 `~/.agents/skills/p2pcli/`
- Qwen Code：`~/.qwen/skills/p2pcli/`；CodeBuddy：`~/.codebuddy/skills/p2pcli/`

只往已经存在的 AI 配置目录里写，不替没装的 AI 建目录；只写一个叫 `p2pcli` 的文件夹，不改 `CLAUDE.md`、`AGENTS.md`。文件里带 `managed-by: p2pcli` 标记，软件升级时只覆盖带标记的，别人放的同名技能不动。设置 → 多机互动 里能看到检测到哪些 AI、各自装没装，可以单个或全部安装、更新、删除，也能关掉“启动时自动安装和更新”（关掉只是不再自动动，已装的不会被删；开发版也能手动装，只是不自动装）。已经开着的 AI 窗口要重开才会读到。说明的原文在 `app/src-tauri/src/p2pcli-skill.md`，加了命令或改了用法要同步改它。

## exec 细节

- 命令交给对方的 shell 解析（macOS、Linux 用登录 shell，Windows 用 PowerShell），引号自己写好，用法同 ssh
- 没有伪终端，命令的输入是空的，不能交互，程序也不会因为连着终端而换样子；标准输出和标准错误分开回来，字节原样不乱码
- 退出码就是命令的退出码；超时（`--timeout 秒`）是 124，被信号结束是 128 加信号编号，P2PCLI 自己出错（找不到设备、连不上）是 255
- 默认在对方的用户目录跑，`--cwd` 换目录；`--json` 把标准输出、标准错误、退出码打包成 JSON
- 命令行被 Ctrl+C 或管道关了，对方会把整组进程一起结束；命令自己放到后台的进程不管，要长期跑的用 `nohup` 并重定向输出
