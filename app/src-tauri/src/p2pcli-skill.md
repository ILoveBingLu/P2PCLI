---
name: p2pcli
description: 在另一台电脑上办事：直接读、改、搜对方电脑上的文件和代码（不用来回传文件）、跑命令、把文件发过去、把对方的文件拉回来。对方设备在 `p2pcli devices` 的列表里（用 P2PCLI 连过）时，用 p2pcli 命令操作它。用户说另一台电脑、远程设备、某台机器、设备码、去那台机器上看看或改一下或跑一下或拿个文件，或者提到 P2PCLI 时用。Read, edit and search files on another computer paired in P2PCLI in place (no copying back and forth), run commands there, and send or pull files. Use the `p2pcli` command when the user refers to a remote machine, another computer, a device name or a device code.
metadata:
  managed-by: p2pcli
---

<!-- 这个文件由 P2PCLI 自动安装和更新，手改会被覆盖。想关掉或删掉：P2PCLI 设置 → 多机互动。 -->

# 用 p2pcli 操作其他电脑

P2PCLI 是点对点远程终端软件。本机的 P2PCLI 开着时，终端里的 `p2pcli` 命令可以对连过的设备办事：直接读写对方的文件、跑命令、发文件、拉文件。命令在本机终端里敲，不用打开 P2PCLI 的窗口。

先看有哪些设备、在不在线：

```sh
p2pcli devices          # 设备名、系统、在线与否、延迟、设备码；加 --json 给程序读
```

设备名可以只写一部分，也可以写设备码或公钥开头几位。对上多台会报错，换成设备码。

## 直接读、改、搜对方电脑上的文件

要看或改对方电脑上的代码、配置，用下面这些命令直接在原地改，**不要**先 `pull` 到本机、改完再 `send` 回去。它们和你自带的读、写、改、搜工具一一对应：

```sh
p2pcli cat 设备 ~/proj/src/main.rs                 # 读文件（UTF-8 文本，不超过 5 MB）
p2pcli cat 设备 ~/proj/src/main.rs -n --offset 120 --limit 60   # 带行号，只看第 120 行起的 60 行
p2pcli edit 设备 ~/proj/src/main.rs --old '旧的一段' --new '新的一段'   # 按原文替换
p2pcli write 设备 ~/proj/notes.md < 本机文件        # 用标准输入的内容整个写入（新建或整个重写）
p2pcli ls 设备 ~/proj                               # 列目录，目录后面带 /
p2pcli find 设备 ~/proj '*.rs'                      # 按名字找文件，认 .gitignore；可用 * ? **
p2pcli grep 设备 'fn main' ~/proj                   # 搜内容（正则；-F 按普通文字，-i 不分大小写，-w 全字）
p2pcli mkdir 设备 ~/proj/src/new -p                 # 建目录
p2pcli mv 设备 ~/proj/a.rs ~/proj/b.rs              # 改名或移动，不会盖掉已有的文件
p2pcli rm 设备 ~/proj/old.rs                        # 移到对方的废纸篓，能找回
```

- `edit` 和你自带的编辑工具规则一样：`--old` 要和文件里一字不差（包括缩进、空格、换行），而且只能出现一次；出现多次会报出现了几次，多带几行上下文让它唯一，或者确实要全换时加 `--all`。先 `cat` 看清楚再改。
- 多行的原文和新文用 heredoc 传，避免引号出错：

  ```sh
  p2pcli edit 设备 ~/proj/src/main.rs --old "$(cat <<'EOF'
      let x = 1;
      let y = 2;
  EOF
  )" --new "$(cat <<'EOF'
      let x = 10;
  EOF
  )"
  ```

- 改的时候会核对文件在读出来之后有没有被别人改过（比如对方终端里的程序、用户在 P2PCLI 编辑器里保存）；改过就报错不盖掉，重新 `cat` 一遍再改。文件用 CRLF 换行时，命令行里写的 LF 原文也能对上，改完还是 CRLF。
- `write` 是整个替换，只用来新建文件或整个重写；改一部分用 `edit`。
- 路径是对方电脑上的路径：绝对路径、`~` 开头，或者相对路径（从对方的用户目录算起）。对方是 Windows 时写 `C:\Users\...` 或 `~\...`。
- `grep` 没找到时退出码是 1；所有命令加 `--json` 给程序读（`cat --json` 带文件哈希和总行数）。
- 二进制文件、非 UTF-8 文件、超过 5 MB 的文件不能 `cat`/`edit`，这类用 `pull`/`send`。
- 跑构建、测试、git 还是用下面的 `exec`。

## 在对方电脑上跑命令

```sh
p2pcli exec 设备 -- 命令...                       # 默认在对方的用户目录跑
p2pcli exec 设备 --cwd ~/work -- 'git status'      # 换目录
p2pcli exec 设备 --timeout 60 -- 'npm test'        # 超过 60 秒结束它
p2pcli --json exec 设备 -- 'ls'                    # 输出打包成 JSON：stdout、stderr、code
```

- 命令交给对方的 shell 去解析，引号自己写好。有管道、`&&`、重定向的整条命令用单引号括起来。
- 对方是 macOS 或 Linux 时用它的登录 shell，是 Windows 时用 PowerShell，写命令前先看 `p2pcli devices` 里的系统。
- 标准输出和标准错误分开回来，原样不乱码。退出码就是命令的退出码；超时是 124，被信号结束是 128 加信号编号，p2pcli 自己出错（找不到设备、连不上）是 255。
- 命令的输入是空的，不能交互。会问确认的命令加 `-y` 之类的参数，需要输入密码的命令不要跑。
- 命令放到后台的进程不管。要长期跑的东西用 `nohup 命令 > 日志 2>&1 &`，再另外读日志。
- 本机终端里按 Ctrl+C 或管道关了，对方会把这条命令整组结束。

## 传文件

```sh
p2pcli pull 设备 对方的路径...             # 把对方的文件或文件夹拉到当前目录
p2pcli pull 设备 ~/out.log --to ~/Downloads  # 拉到指定目录，不存在会建
p2pcli send 设备 本机文件...                # 发到对方的收件目录（下载/P2PCLI/本机名/），重名自动加编号
p2pcli send 设备 本机文件... --to ~/work    # 放到对方电脑的这个目录
p2pcli inbox                               # 本机最近收到的文件
```

- 对方的路径是对方电脑上的路径；`~` 开头或相对路径从对方的用户目录算起。
- `pull` 和 `send --to` 会盖掉同名文件，覆盖前先确认；`send` 不带 `--to` 不会盖已有的。
- 文件夹整个传，传完会核对，断了再来一次会接着传。命令结束时会打印文件在目标电脑上的路径。

## 出错时怎么办

- 提示 P2PCLI 没在运行：让用户先打开 P2PCLI。
- 提示需要某台设备的配对密码：说明那台设备和本机之前从没互相连过。让用户先在 P2PCLI 里连一次它并输入它的配对密码，连的时候勾选互相信任，之后你就能直接操作，对方也能直接操作本机。不要自己去猜或要密码。
- 提示等某台设备同意：对方电脑上弹了窗、没在看软件时还发了系统通知，请用户让对方点允许，然后等待，不要反复重试。对方点了拒绝，短时间内再试也会直接被拒，别硬试。
- 对方以前连过本机时，本机去操作它不需要密码，只需要它点允许；对方勾了互相信任或受信，连点允许都不用。
- 设备离线、或版本太旧需要更新：照实告诉用户。
- 找不到设备：用 `p2pcli devices` 看真实的名字，别凭印象猜。

## 跨设备、跨会话协作

协作项目把任务、消息和结果绑定到协调设备上的一个项目。协作会话必须先登记，凭据文件只保存本机的 `coordinator`、`project_id` 和 `auth.session_id`/`auth.token`，文件由 P2PCLI 创建并限制为本人可读。不要打印 token，也不要把一个项目的凭据用于另一个项目。

先在协调设备创建项目，再给参与设备授权。`--coordinator` 不写表示本机，跨设备时使用设备公钥或 `p2pcli devices` 得到的设备标识；所有需要会话身份的命令都使用同一项目的 `--session-file`。

```sh
p2pcli collab project create --name "Windows 验证"
p2pcli collab project list
p2pcli collab member set <设备公钥> --role worker --project <项目 ID>
p2pcli collab session register --project <项目 ID> --name "Windows worker" --mode managed-adapter --session-file ~/.config/p2pcli/windows-session.json
p2pcli collab session list --project <项目 ID> --session-file ~/.config/p2pcli/windows-session.json
```

`managed-adapter` 只登记接入模式，不代表任意 AI 已经自动接手。适配器或会话必须明确领取任务并提交结果。会话登记成功时，命令只输出会话信息和凭据路径，token 留在受限凭据文件中。

创建任务可以使用完整 `TaskSpec` JSON，也可以使用参数。任务版本省略时，CLI 先读取当前任务，再用读取到的版本发送条件更新；领取、进度、结果、停止、取消、验收和重新排队的请求发生传输不确定时，不由后台自动重发。使用同一个 `--request-id` 或本地 request 文件重试，避免重复执行。

```sh
p2pcli collab task create --project <项目 ID> --file task-spec.json --request-id create-task-1
p2pcli collab task list --project <项目 ID> --session-file ~/.config/p2pcli/session.json
p2pcli collab task claim <任务 ID> --project <项目 ID> --session-file ~/.config/p2pcli/session.json
p2pcli collab task progress <任务 ID> --attempt <attempt ID> --file progress.txt --project <项目 ID> --session-file ~/.config/p2pcli/session.json
p2pcli collab task result <任务 ID> --attempt <attempt ID> --file result.json --project <项目 ID> --session-file ~/.config/p2pcli/session.json
p2pcli collab task accept <任务 ID> --project <项目 ID> --session-file ~/.config/p2pcli/session.json
```

`task wait --timeout 60` 只结束等待，不取消任务。`--file` 可用于进度和结果文本，结果 JSON 应包含 `summary`、`completed`、`remaining`、`verification`、`artifact_ids` 和 `failed`。

消息和收件箱也需要明确的会话凭据：

```sh
p2pcli collab inbox --project <项目 ID> --session-file ~/.config/p2pcli/session.json
p2pcli collab message send --device <设备公钥> --session <会话 ID> --file message.txt --project <项目 ID> --session-file ~/.config/p2pcli/session.json
p2pcli collab message consume <消息 ID> --project <项目 ID> --session-file ~/.config/p2pcli/session.json
```

读取 `inbox` 后，P2PCLI 会先把消息快照持久化到本机，再回送 `MessageDelivered`。这只表示消息已写入本机收件箱，不表示 AI 会话已消费；只有显式 `message consume` 才会标记为 `consumed`。后台同步会重新拉取已订阅收件箱并补发交付确认，不会自动消费消息。

写请求断线时可能输出 `delivery: queued_local`。它只表示请求已安全写入本机发件队列，远端业务操作尚未得到确认；不要把它当作任务已创建、消息已送达或任务已开始。可用下面的命令检查本机投递记录；该命令不需要项目或会话凭据，输出不包含请求体、auth 或 token：

```sh
p2pcli --json collab deliveries
```

`transport_error` 表示没有收到可判断的远端业务回复；协作回复里的 `error.code` 表示远端已经处理并拒绝。先查看投递记录或使用同一个 request ID 重试，不能为领取、进度、结果等执行操作生成新的 request ID。

## 注意

这些命令能在对方电脑上做任何事，也能改它的文件。只做用户要求的事，删除、覆盖、重启这类不好撤回的操作先问用户。
