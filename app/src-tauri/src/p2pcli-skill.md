---
name: p2pcli
description: 在另一台电脑上办事：跑命令、把文件发过去、把对方的文件拉回来。对方设备在 `p2pcli devices` 的列表里（用 P2PCLI 连过）时，用 p2pcli 命令操作它。用户说另一台电脑、远程设备、某台机器、设备码、去那台机器上看看或跑一下或拿个文件，或者提到 P2PCLI 时用。Run commands on, send files to and pull files from another computer paired in P2PCLI (listed by `p2pcli devices`). Use the `p2pcli` command when the user refers to a remote machine, another computer, a device name or a device code.
metadata:
  managed-by: p2pcli
---

<!-- 这个文件由 P2PCLI 自动安装和更新，手改会被覆盖。想关掉或删掉：P2PCLI 设置 → 多机互动。 -->

# 用 p2pcli 操作其他电脑

P2PCLI 是点对点远程终端软件。本机的 P2PCLI 开着时，终端里的 `p2pcli` 命令可以对连过的设备办事：跑命令、发文件、拉文件。命令在本机终端里敲，不用打开 P2PCLI 的窗口。

先看有哪些设备、在不在线：

```sh
p2pcli devices          # 设备名、系统、在线与否、延迟、设备码；加 --json 给程序读
```

设备名可以只写一部分，也可以写设备码或公钥开头几位。对上多台会报错，换成设备码。

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

## 注意

这些命令能在对方电脑上做任何事，也能改它的文件。只做用户要求的事，删除、覆盖、重启这类不好撤回的操作先问用户。
