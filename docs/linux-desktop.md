# Linux 桌面支持

状态：Linux 代码分支、内置组件、打包配置和发布工作流已接入。本轮未构建应用、未运行 Linux Rust 测试、未完成桌面或跨设备实机验收；不能将静态检查通过视作安装包已经发布。

## 目标与安装包

| 架构 | Rust 目标 | GitHub runner | 发布文件 |
| --- | --- | --- | --- |
| Intel / AMD 64 位 | `x86_64-unknown-linux-gnu` | `ubuntu-22.04` | `P2PCLI_<版本>_x64.AppImage` / `.deb` / `.rpm` |
| ARM64 | `aarch64-unknown-linux-gnu` | `ubuntu-22.04-arm` | `P2PCLI_<版本>_arm64.AppImage` / `.deb` / `.rpm` |

使用 Ubuntu 22.04 作为构建基线，以避免较新的 glibc 提高最低运行要求。需要有图形桌面和 WebKitGTK 4.1 支持；不承诺兼容任意旧发行版。Linux 32 位、musl/Alpine 桌面和 Flatpak 不在本次支持范围。

- Debian / Ubuntu：通过 `sudo apt install ./P2PCLI_<版本>_x64.deb` 安装，ARM64 替换文件名。
- Fedora 等 RPM 发行版：通过 `sudo dnf install ./P2PCLI_<版本>_x64.rpm` 安装，依赖由包管理器处理。
- AppImage：先 `chmod +x P2PCLI_<版本>_x64.AppImage`，再直接运行。需要 FUSE 2 兼容库；Ubuntu 22.04 包名为 `libfuse2`，24.04 为 `libfuse2t64`。无法使用 FUSE 时优先使用 DEB/RPM；解包运行不作为在线更新验收方式。
- 不用 root 启动桌面应用；DEB/RPM 在线更新由 updater 请求系统管理员授权。AppImage 文件应放在当前用户可写的位置。
- 更新清单为 Linux 分别提供 `linux-<架构>-appimage`、`linux-<架构>-deb`、`linux-<架构>-rpm`，不会把 AppImage 当作 DEB/RPM 更新。此行为依赖仓库当前 Tauri CLI 2.12.4 / updater 2.14.0 的按包类型选择与签名支持。

## 本机集成

- `tauri.linux.conf.json` 使用默认原生窗口装饰，保留桌面环境的关闭、最小化、最大化和窗口管理行为。
- 每个安装包内置该架构的官方静态 tmux 3.7c 和 `p2pcli`。tmux 安装名为 `p2pcli-tmux`，不会与系统 tmux 冲突；绝对路径定位，不从 PATH 猜版本。
- 后台会话使用现有 Unix 实现。组件源码、许可证及文件/归档哈希记录在 `scripts/session-tools.json` 和 `resources/session-tools/linux/`。
- CLI 副本保存在 `$XDG_DATA_HOME/io.github.ilovebinglu.p2pcli/bin/p2pcli`，未设置 XDG 时使用 `~/.local/share`。副本不依赖 AppImage 的临时挂载目录；更新时以原子替换刷新，不中断已运行的 CLI。
- 设置里的“加 PATH”按 shell 写入启动文件：Linux Bash 使用 `.bashrc`，Zsh 使用 `.zshrc`，Fish 使用 `.config/fish/config.fish`，其他 shell 使用 `.profile`。不会在后台擅自修改 Unix shell 配置。
- 文件管理器和外部链接复用 `xdg-open`。通知依赖桌面的通知服务；托盘依赖 AppIndicator 支持，部分 GNOME 桌面需启用对应扩展。托盘创建失败时关闭窗口允许正常退出；托盘存在但桌面未显示时可再次启动应用唤回主窗口。
- mDNS、设备身份、配对、QUIC、文件传输和协作使用现有跨平台实现，不增加 Linux 专用配对协议。局域网发现失败时检查本机防火墙对 mDNS/组播的限制，不自动修改防火墙。

## 开发依赖

Ubuntu 22.04 / Debian 12 安装系统依赖后，按 README 安装 Rust、Node 24 和 pnpm：

```sh
sudo apt-get update
sudo apt-get install -y build-essential curl wget file \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev \
  libfuse2 patchelf rpm xdg-utils
pnpm install --frozen-lockfile
```

不构建应用的检查：

```sh
pnpm run check
node scripts/check-session-tools.mjs --all
node --test scripts/check-session-tools.test.mjs scripts/check-app-release.test.mjs
node scripts/check-agent-input.mjs
```

最后一条只启动独立测试命名空间中的内置 tmux/psmux，不构建或启动桌面应用，不接触用户会话。Linux CI 在原生 x64 / ARM64 runner 上执行它。

## 发布与验收

1. Actions 手动运行“安装包发布”，或推 `ci/` 分支，只构建并上传 Artifacts，不创建 GitHub Release。普通 `main` 推送不打包。
2. Linux 两个 runner 分别检查组件、验证输入定向与中文、构建三个安装格式，并运行 CLI 安装单元测试。AppImage 不走跨架构打包。
3. 开发者验证 Linux 安装包：X11 / Wayland 下启动、窗口控制、托盘唤回、通知、文件选择和打开目录。
4. 验证本机终端、中文输入与粘贴、关闭标签后恢复会话，以及退出/重启 AppImage 后 CLI 仍存在且能连接本机控制口。
5. 与 Windows / Mac 各跑一次局域网发现、配对、终端、双向文件传输、协作派发/领取/结果返回和方向光点。
6. 用两个递增测试版本分别验证 AppImage / DEB / RPM 更新，确认包类型与 CPU 架构匹配、签名错误被拒绝、管理员授权取消不损坏安装。
7. 验收后更新版本号并推同版本 `vX.Y.Z` 标签。全部七个目标成功后才汇总发布；任一安装包或签名缺失会中止。发布包含 `latest.json`、独立 `.sig`、`SHA256SUMS`。

工作流使用现有 `TAURI_SIGNING_PRIVATE_KEY` secret，不新增密钥、不改变 updater 公钥、不自动创建版本标签。
