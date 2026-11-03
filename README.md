**English** | [简体中文](README.zh-CN.md)

# P2PCLI

A peer-to-peer remote terminal. On one computer, enter another computer's device code and pairing password. After the other side approves, you use their terminal inside the app window. Command-line AIs such as Claude and Codex run with the accounts and environment on that computer.

- Direct connection first: IPv6 direct, then IPv4 hole punching, then a relay as the fallback. Traffic is end-to-end encrypted. A relay only forwards bytes and cannot read them.
- Pairing works like remote-desktop software: a 9-digit device code plus a 6-digit pairing password (it can be pinned so it survives restarts). A correct password still needs the receiving side to click Allow.
- The terminal lives in the app: several tabs, and the terminal fills the main workspace.
- Devices send files directly: drop them into a terminal, use the right-click menu, or run `p2pcli send` and `p2pcli pull` in a terminal. `p2pcli exec` runs a command on the other computer, and a command-line AI can use it too.
- It recognizes the command-line AI running in a session: which AI it is, whether it is working or waiting for you, and a reminder when it needs you or finishes. An input box under the terminal sends a whole sentence at once.
- Tauri 2 + React + xterm.js. Each installer carries session components only for its own platform and architecture.

The interface follows the system language. Chinese locales (`zh`, including `zh-CN`, `zh-TW`, and `zh-HK`) use Simplified Chinese. Every other locale uses English. Switch anytime under Settings → Connection → Language. The choice is saved on this computer.

## Download and install

Download a build from [Releases](../../releases):

- macOS 15 or later. Apple silicon: `P2PCLI_x.y.z_aarch64.dmg`. Intel: `P2PCLI_x.y.z_x64.dmg`. (Apple menu → About This Mac, then look at Chip.)
- Windows 10 or later. Most PCs: `P2PCLI_x.y.z_x64-setup.exe`. Snapdragon and other ARM chips: `P2PCLI_x.y.z_arm64-setup.exe`. 32-bit: `P2PCLI_x.y.z_x86-setup.exe`. (Settings → System → About, then look at System type.)
- Linux: x64 and ARM64, each as `.AppImage`, `.deb`, and `.rpm`. Linux packaging is wired into the next tag release. The first CI build and on-device checks are still pending. Install requirements and the verification list are in [Linux desktop support](docs/linux-desktop.md).

After installation the app checks for updates on its own (once after launch, then every 6 hours, and manually under Settings → About). When a version is available, click Update and restart on the main screen. Update packages are signed with the publisher's private key, and the app installs only packages whose signature matches. If an HTTP proxy is set, update checks and downloads use it too.

The installers are not paid-signed, so the operating system asks the first time you open the app:

- macOS: if Apple cannot verify P2PCLI, click Done, then System Settings → Privacy & Security, and click Open Anyway.
- Windows: if Windows protected your PC, click More info → Run anyway.

The app uses `wss://p2pcli.aiqji.cn` as its rendezvous server by default, so it works as soon as it is installed. To use your own rendezvous server (self-hosted or Cloudflare, below), change it in the title-bar settings.

## How to use it

1. Install and open P2PCLI on both computers.
2. The computer being connected to opens This computer from the right side of the title bar, then copies the device code and pairing password. The pairing password changes every launch by default. The lock next to it pins the password so it survives restarts and the other side can keep using it.
3. The connecting side clicks the plus button in the title bar, enters the code and password, and clicks Connect. It can also pick a device from connection history on the left. The receiving side clicks Allow. After one successful connection the app remembers that device's pairing password, and later clicks from history or nearby devices fill it in. If the other side changes the password, you are asked to enter it again. Removing the device from history also forgets the password.
4. Once connected, it is a terminal on the other computer. Click + for another tab. Devices and sessions on the left have a right-click menu: new session, refresh sessions, end session, copy the device code, and remove from history.

This computer at the top of the left side is the local machine. Expand it to open a local terminal or reattach a background session. Tabs, background sessions, and command-line AI status work the same way as a remote device, without the network, pairing, or approval. A local terminal tab says This computer at the bottom, and closing the tab does not stop it. Without tmux (psmux on Windows), a new local terminal is an ordinary terminal: it works, and it disappears when the app quits.

After launch, devices in connection history show online status and latency on their own (no dialog, and it does not count as a connection; latency refreshes every 2 seconds, and a remote that loses the network or shuts down shows offline within a few seconds). A device that has approved you once also lists the sessions you opened on it (id and status only). Click a session to connect and open it. While it is offline, the last seen sessions stay visible. The receiving side manages these paired devices under Settings → Trusted devices. Latency and the session list need a current P2PCLI on the other side. Older versions show Update available.

Computers on the same local network do not need a device code. After both apps are open they appear under Nearby on the left. One click connects (the pairing password and approval are still required, unless the device is trusted). A history entry that is online on the LAN is marked LAN, and connecting from there does not go through the rendezvous server, so it still works if the internet is down. Guest Wi-Fi and networks with client isolation cannot see each other; use a device code there. The first time Windows opens the app, allow it in the firewall prompt. A network set to Public may also hide devices. To hide this computer from the same network, turn off Show this computer on the local network in settings.

Mutual trust: check Trust both ways while connecting. The other device is saved as trusted on this computer, and later it can control this computer with no password and no approval (so a command-line AI there can operate this computer directly). Remove it anytime under Settings → Trusted devices. If you do not check it, a device this computer has successfully connected to can connect back without the pairing password, but this computer still asks in a dialog. If the window is in the background, a system notification is sent too. If you click Deny, or nobody clicks, the same device is rejected for 30 seconds instead of asking again. Devices that have never connected still need the pairing password.

While someone is using this computer, the right side of the title bar shows who. Open it to see who, how many terminals, and how long, and disconnect at any time. A device you mark as not needing confirmation next time connects later with no password and no dialog. Only do that for your own computers. Closing the window leaves the app in the tray, and other devices can still connect.

The left side of the terminal status bar shows the session's current directory. Open it to list the directories on that computer: click a folder to enter it, use the buttons at the top to go up a level or back to the home directory, type to filter, or use the arrow keys and Enter. When you have the one you want, click Switch here (or ⌘/Ctrl+Enter) and the terminal runs cd into it. You can only switch while the terminal is idle at a prompt. While a program or command-line AI is running you can browse but not switch, so the cd is never typed into that program. Both sides need a version that supports this. An older version on the other side shows Update available.

Code editor: the code icon on the right of the terminal tab bar (or Open in editor in the directory list) opens an editor beside the terminal, so you can change files on the other computer directly instead of sending them back and forth. On the left is the file tree from the other computer. On the right is Monaco, the editor inside VS Code, with syntax highlighting, find and replace, and multiple cursors. Editing happens in memory on this computer, and the network is used only once to open and once to save, so typing stays smooth even over a slow cross-border link. Press ⌘/Ctrl+S to save. When saving, the app checks whether the file was changed on the other computer in the meantime (for example by an AI in the terminal). If it was, it asks whether to use their version or overwrite it with yours, and never overwrites silently. A file with no local changes is refreshed from the other computer when you switch back to it. A UTF-8 BOM and the original line endings are kept, and the file is written in place, so permissions and symlinks stay the same. Binary files, files not in UTF-8 (such as GBK), and files over 5 MB are not opened in the editor. If the connection drops, open files and unsaved changes are kept, and you can save after reconnecting. Each device has its own editor, so switching to another device's tab shows that device's editor. Drag the divider to resize it. The editor loads the first time it is opened, so startup is not slower. It adds about 3 MB to the installer. Both sides need a version that supports it. It uses the same permissions as opening a terminal and grants nothing extra.

The editor can also: find a file by name with ⌘/Ctrl+P (the file list is read once when it opens, then filtered on this computer, so typing doesn't wait for the network); search file contents across the directory on the other computer with ⌘/Ctrl+Shift+F, with match case, whole word, and regular expressions, where clicking a result jumps to that line, and files in .gitignore, the .git directory, binary files, and files over 2 MB are skipped; and, from the right-click menu in the file tree, create a file or folder, rename, move to Trash (deleted items go to the Trash or Recycle Bin on the other computer and can be restored; renaming never overwrites an existing file with the same name), copy the path, or open a folder as the root. While the editor is open it checks the other computer every 3 seconds. A file with no local changes is replaced with the latest content. A file with unsaved changes shows a warning right away, and you can compare before deciding. A deleted file is marked, and saving can recreate it. The file tree refreshes as well. In a Git repository, the file tree and tabs color files that are modified (M), added or untracked (U), or in conflict (!). The status bar shows the current branch, and a modified file can be compared side by side with the version in the latest Git commit, while you keep editing. Reading Git status doesn't take the repository's index lock, so git commands in the terminal aren't affected. If git isn't installed on the other computer, these markers don't appear.

Completion, navigation, and errors: when you open a Rust, Go, Python, TypeScript/JavaScript, C/C++, Swift, or similar file, P2PCLI starts that language's server on the other computer (rust-analyzer, gopls, basedpyright/pyright, TypeScript 7's built-in server or typescript-language-server, clangd, sourcekit-lsp, and others) and brings completion, hover docs, parameter hints, go to definition (F12, including other files), find references, symbol highlighting, formatting, and error squiggles into the editor over the P2P connection. It sees the whole project's types and dependencies, not just the current file. The server is found through the PATH of the other computer's terminal, plus Homebrew, the Xcode Command Line Tools, and the rust-analyzer bundled with VS Code or Cursor, so no setup is needed. Files in the same project (such as a Cargo workspace) share one server. The status bar shows its state (starting, indexing progress, errors). If none is installed, it shows "No completion" with install instructions in the tooltip; click it to try again after installing. When the editor closes or the connection drops, the server and any checks it started end with it.

Also: the Problems tab on the left lists errors and warnings the language server reports for the whole project, including files that aren't open (such as what rust-analyzer finds with cargo check). Click one to jump to it, open the tab with ⌘/Ctrl+Shift+M, and see the counts in the status bar. Rename a symbol with F2 and apply quick fixes (auto-import, suggested fixes, and so on) with ⌘/Ctrl+. The affected files open in the editor and are marked unsaved; review them and click Save all (⌘/Ctrl+Alt+S). Saving still checks whether the files changed on the other computer. ⌘/Ctrl+Shift+O jumps to a function in the current file, and ⌘/Ctrl+T finds functions and types by name across the project. File paths in terminal output, such as compiler errors and AI output (src/main.rs:42:5, main.ts(12,3), files in a Python traceback), open in the editor at that line when you ⌘/Ctrl+click them. Relative paths are resolved from the terminal's current folder.

The right side of the terminal status bar shows the path and latency for this connection. Open it for details: network latency, key echo (the time from a keypress until the character appears; it is not measured while the remote program is scrolling), upload and download bandwidth, and packet loss; whether the path is IPv6 direct, IPv4 direct, or relay, and both addresses; the other side's app version, operating system, and session manager (tmux or psmux). It refreshes once a second while open and does not collect stats while closed. Version and operating system are visible only when both sides are 0.1.5 or newer.

## File transfer

- Right-click a device on the left and choose Send files or Send a folder, or drop files into that device's terminal. After the transfer, the path on the other computer is filled into the terminal, so Claude or Codex running there can read it.
- Received files go to `Downloads/P2PCLI/<sender name>/`. Name conflicts get a number instead of being overwritten. Files from a paired device, or from a device you have connected to, are accepted automatically. Other devices need approval.
- Parallel chunks: a file is split into 8 MiB pieces, with up to 8 streams at once. Each piece has a BLAKE3 checksum and counts only after the checksum matches. A retry skips pieces that already arrived. Older peers automatically fall back to one stream. On high-latency, lossy links such as cross-border paths this is about twice to several times faster. The transfer panel shows live speed and how many streams are in use.
- After each file, BLAKE3 is checked again. If a transfer stops halfway, sending the same batch again resumes from the break.
- Both sides need 0.1.3 or newer.

The installer includes a `p2pcli` command-line tool (on Mac it is placed in `~/.local/bin`; on Windows it is added to PATH; on Linux a copy is kept in the app data `bin` directory so an AppImage's temporary mount path does not break it). Settings → File transfer shows the status and can add it to PATH. It works while P2PCLI is open:

```sh
p2pcli devices                       # list devices, online status, and latency
p2pcli send <device> report.pdf proj # send files or folders; the name can be partial, or use a device code
p2pcli send <device> a.txt --to ~/work  # put it in that directory on the other computer, overwriting the same name
p2pcli pull <device> ~/work/out.log  # pull a file into the current directory; --to changes the local directory
p2pcli exec <device> -- ls -la ~/work # run one command on the other computer and return output and the exit code
p2pcli inbox                         # show recently received files
p2pcli send <device> a.txt --json    # JSON for programs and AI; a failure exits non-zero
```

`send` without `--to` only writes into the other side's inbox and does not overwrite existing files. A device that already knows this one accepts it immediately. `send --to`, `pull`, and `exec` can touch any path on the other computer, so they use the same approval as opening a terminal: the pairing password must be right and the other side must have approved (or marked later connections as trusted). After that, the same connection is reused. If neither device has ever connected to the other, the first time has to be in the app, with the other side's pairing password (you can check Trust both ways while connecting). If the other side has connected to this computer, or has marked it trusted, no password is needed. If the other side has not marked this computer trusted, each new connection still needs their approval. Pulls and writes show up in the other side's transfer history and notifications. A running command counts as a terminal in use in the title bar. Both sides need 0.1.5 or newer. An older peer is asked to update.

Multi-device AI: at startup the app drops a usage note (`SKILL.md` in the Agent Skills format) into the skill directory of each AI installed on this computer. New sessions then know that `p2pcli` exists, when to use it, and what to do when it fails, without being taught:

- Claude Code: `~/.claude/skills/p2pcli/` (follows `CLAUDE_CONFIG_DIR` if that is set)
- Codex, Gemini CLI, Cursor, OpenCode, Copilot CLI, and Kimi CLI: the shared `~/.agents/skills/p2pcli/`
- Qwen Code: `~/.qwen/skills/p2pcli/`. CodeBuddy: `~/.codebuddy/skills/p2pcli/`

It only writes into AI config directories that already exist. It does not create a directory for an AI that is not installed. It only writes a folder named `p2pcli`, and it does not edit `CLAUDE.md` or `AGENTS.md`. The file is marked `managed-by: p2pcli`. Upgrades overwrite only files with that mark, and leave a same-named skill that someone else installed alone. Settings → Multi-device AI shows which AIs were detected and whether the skill is installed. You can install, update, or remove one or all of them, and turn off Install and update automatically at startup (turning it off only stops the automatic work; installed skills are not deleted; a development build can still install manually, it just does not install automatically). AI windows that are already open need to be reopened before they see it. The source of the note is `app/src-tauri/src/p2pcli-skill.md`. Update it when commands or usage change.

`exec` details:

- The command is parsed by the other side's shell (a login shell on macOS and Linux, PowerShell on Windows). Quote it yourself, the same way you would for ssh.
- There is no pseudoterminal. The command's input is empty, it cannot be interactive, and programs do not change their behavior because a terminal is attached. Standard output and standard error come back separately, as raw bytes.
- The exit code is the command's exit code. A timeout (`--timeout` seconds) is 124. A signal is 128 plus the signal number. A P2PCLI error (device not found, cannot connect) is 255.
- The default working directory is the other side's home directory. `--cwd` changes it. `--json` packs stdout, stderr, and the exit code into JSON.
- If the command line is closed with Ctrl+C or a closed pipe, the other side stops the whole process group. Processes the command itself backgrounded are left alone. For a long-running process, use `nohup` and redirect its output.

## Recognizing the AI in a session

When an ordinary terminal sits at an empty prompt, launch capsules for the AIs installed on the controlled computer float at the bottom. Click one to start it in the current directory. The capsules hide while you type a command or enter another program, and come back at an empty prompt. Detection results are cached. The refresh button next to them detects again. Detection and launch both happen on the controlled side, so both sides need a version that includes the launcher. An unknown custom prompt hides the capsules rather than guessing.

Launch uses the AI's own permission settings by default. Right-click the Claude Code capsule to choose Launch, then allow switching to bypass all prompts (this appends `--allow-dangerously-skip-permissions`). That only adds the mode as an option. It does not enter bypass mode by itself.

Settings → AI launch sets a default launch command for each AI detected on this computer, including a full path, environment variables, and arguments. Leave it empty to restore the detected command. The command stays on the controlled computer. The connecting side cannot upload a command. After saving, later launches use it. If Claude Code should skip tool confirmation after launch, set its command to `claude --dangerously-skip-permissions`. A normal click on the capsule then bypasses confirmation too. Use that only in trusted projects. It does not change operating-system permissions, and it is not `sudo`. Custom commands run in the local shell. Do not put tokens or passwords in the command.

While a remote session is running a command-line AI, the sidebar, the terminal tab, and the status bar show its icon and state: Working, Waiting for you, or Idle. If a busy screen does not change for 10 minutes it is marked May be stuck, and it is not mistaken for idle. A long tool call can also be quiet for a while, so check the terminal. When it stops to ask you to approve an action, or when it finishes, P2PCLI tells you: a system notification if the window is in the background, or a toast if the window is in front but you are not looking at that session. Click View to switch to it. If you are already looking at that session, it stays quiet.

- Recognized: Claude Code, Codex, Gemini CLI, OpenCode, Cursor Agent, Copilot CLI, Qwen Code, Kimi CLI, CodeBuddy, Qoder CLI, iFlow CLI, Kiro CLI, Trae CLI, Amp, Aider, Crush, Droid, Goose, Auggie, and ZCode (Z.ai's zcode-app-cli). The controlled computer walks the session's process tree and looks at the program name and the full command line (Node and Python AIs are named node or python, so the script path matters). Add a new one as a row in the table in `crates/core/src/agents.rs`.
- State is read from the session title and the last few dozen lines of the screen. Numbered options and "Enter to confirm" at the bottom mean it is waiting for you. "✻ Cogitating…" or "esc to interrupt" means it is working. The full screen is parsed only on the controlled computer. The other side receives the AI type, the state, and a confirmation menu and request summary when those can be identified.
- If a product changes its on-screen text, the state can be wrong. The rules are in `agents.rs`, and the tests include real captured screens.
- Quota: the terminal status bar shows the AI account quota on that computer, for example "5 hours 35% · 7 days 75%". It reminds you once at 90% and once when the quota is used up. Claude Code reads `~/.claude/.statusline-ratelimits.json` written by a status-line script on the controlled computer (Claude Code only gives quota data to a status-line script, so without one there is nothing to read). Codex reads the last quota note in the newest session log under `~/.codex/sessions`. This only reads local files. It does not use the network or touch login credentials.
- Input box: while you are looking at a session that is running an AI, an input box appears under the terminal. You compose a whole sentence on this computer, then send it. IME composition does not stall, and long paragraphs are easy to edit. Enter sends, Shift+Enter inserts a line break, and the up and down arrows walk previous messages when the box is empty. Dropping a file on the box fills in the path on the other computer after the transfer. ⌘L (Ctrl+Shift+L on Windows) moves between the terminal and the input box. Esc returns to the terminal. After the box is collapsed, the same shortcut or the toggle at the top right of the box opens it again. A send is pasted as one block, so the AI does not treat a newline in the middle as Enter. Sending is blocked while the AI is waiting for confirmation, so a sentence is not taken as a menu choice.
- Confirmation list: common numbered single-choice prompts from Claude Code, Codex, and Gemini CLI, and OpenCode's standard three-item permission menu, float above the input box with the request summary and the original options. Before a click, the controlled side checks the screen, the AI process, and the option number again, then sends arrow keys and Enter. The same confirmation can be submitted only once. Multi-select, free text, a truncated menu, or an uncertain highlight stays in the terminal. Both sides need to be updated. Only the recognized menu and the nearby request summary are sent, not the whole screen.
- Model, effort, and permission mode: a Claude Code session has three buttons to the right of the input box. Model and effort switch the same way as typing `/model` and `/effort` in the terminal. Claude Code also stores that as the default for new sessions on that computer (its own rule; other sessions that are already running are not affected). Permission mode can only be cycled with Shift+Tab. Each press is checked, and it stops when the target is reached. That changes only this session. Bypass all prompts is in the cycle only if the process was started with `--dangerously-skip-permissions`. Nothing can be switched while it is waiting for you, and model and effort wait until the current turn finishes. Current values: model and effort are read from this session's transcript on the controlled computer (`~/.claude/sessions/<pid>.json` points at the record under `~/.claude/projects`, or under `CLAUDE_CONFIG_DIR` if that is set). Before a transcript exists, the opening banner is used. Permission mode is read from the bottom line of the screen. Codex's model list depends on the account, so two buttons open its own `/model` and `/permissions` menus. OpenCode has model, effort, and permissions: model opens `/models`, effort opens `/variants` (the levels come from the model and provider config), and permissions open the command palette with the default Ctrl+P and search `permissions`. The user confirms automatic approval in the terminal. It is not bypassed directly. Checked against OpenCode v1.19.4. Older builds without variants or the auto-approve entry need an OpenCode update. If Ctrl+P was customized, the command palette can still be opened in the terminal. The three entries are disabled while OpenCode is working or waiting for confirmation. Current options are shown in OpenCode's own menus, and Claude's settings are not treated as OpenCode's current values. ZCode has the same three buttons. All three are read from its bottom status bar (`◈ model ─ ◉ mode ─ ⚡ effort`). The models and the effort levels for each model are read from `~/.zcode/cli/config.json` on the controlled computer. Only model names and levels are read, not keys. Switching sends `/model provider/model` and `/effort <level>`. Permission mode cycles with Shift+Tab (build → edit → yolo → plan). ZCode's own rules: the model changes only this session, effort becomes the default for new ZCode sessions on that computer, and permission mode becomes the default for that directory.
- Both sides need 0.1.4 or newer. Connected devices refresh state every 4 seconds.
- AI brand icons are static SVGs from [Lobe Icons](https://github.com/lobehub/lobe-icons) (MIT), stored in `app/src/assets/agents` and shown as images. See the LICENSE there. `@lobehub/icons` is not imported: it pulls in the whole Lobe UI, and new releases often fail pnpm's default publish-age check (packages published within 24 hours are not installed). For a new CLI, add an SVG named after the kind in `agents.rs`. Anything not included uses a generic robot icon.

## How it works

```
crates/core     core library: identity, connections, pairing checks, remote terminal, rendezvous client, path selection
crates/server   rendezvous server: device codes, online status, and addresses (terminal data does not pass through it); UDP and WebSocket
cloudflare/     Cloudflare Workers build of the rendezvous server (WebSocket)
crates/dev      command line for development tests
app/            desktop app (src-tauri is the Rust side, src is the UI)
```

The connection layer is [iroh](https://github.com/n0-computer/iroh) (QUIC). Each device has a long-lived private key. The public key is the device identity. Both ends of a connection authenticate each other and encrypt the traffic.

1. On startup each device connects to the rendezvous server (UDP or WebSocket), registers, receives a stable device code, and then reports its current addresses.
2. The connecting side looks up the other side's public key and addresses by device code, and connects.
3. The connecting side sends an opening message with the pairing password. The receiving side checks it (5 wrong tries from the same device lock for 10 minutes, and too many failures across all devices also pause), then asks the owner in a dialog.
4. After the owner approves, each new tab opens a QUIC stream. The receiving side starts a real terminal on it (the user's login shell on macOS, PowerShell on Windows).

IPv6 direct (`crates/core/src/v6.rs`): a public IPv6 address has no NAT. The interface address is the external address. There is nothing to punch through, only stateful firewalls on both routers. When both sides have public IPv6, the connecting side asks the rendezvous server to tell the receiving side to send packets to the connecting side's IPv6 address at the same time, which opens the receiving side's firewall. The connecting side connects directly to the other IPv6 address. The first packet is already direct and does not go through a relay. If it is not up within 1.5 seconds, the normal path runs in parallel (connect through the relay first, then hole-punch). Whichever path works first is used.

LAN discovery (`crates/core/src/lan.rs`): the official [iroh-mdns-address-lookup](https://docs.rs/iroh-mdns-address-lookup) broadcasts and discovers on the local network. The service name is `p2pcli`, and only P2PCLI devices are accepted. The device name and operating system are in the broadcast's extra data, and that extra data is sent only on the LAN. iroh's built-in extra-data interface would also publish it to public DNS, so a wrapper adds it only to the copy handed to mDNS. A LAN connection is direct by public key and does not use the rendezvous server. Broadcast contents cannot be verified, so they do not create new history entries. They only update records that already exist.

Multi-interface discovery uses a patch in `vendor/iroh-mdns-address-lookup` to name the IPv4 multicast interfaces explicitly. It covers every enabled, multicast-capable interface that is not loopback and not a TUN device, and it does not depend on a default multicast interface that might point at VMware. The resident discovery and a refresh scan use the same interface selection. Each scan updates the resident instance's interface list. The desktop app scans every 30 seconds, and it can be refreshed by hand. IPv6 keeps the original behavior. The source and scope of the patch are in that directory's `PATCHES.md`. System routes do not need to change, and virtual adapters do not need to be disabled.

Congestion control (`crates/core/src/quic.rs`): iroh's default Cubic treats loss as congestion. On a cross-border link with a round trip of more than 100 ms and about 1% loss, file transfer drops to about 1 Mbps. The app picks an algorithm from the link delay: BBR3 when the minimum round trip is 20 ms or more, and Cubic on a LAN or a short link (BBR3 is slower there). Measured on a simulated link (`cargo test -p p2pcli-core --test lossy_link -- --ignored --nocapture --test-threads=1`): at 106 ms round trip and 1.3% loss, throughput went from 0.9 Mbps to about 40 Mbps, and LAN speed stayed the same. The sending side chooses its own algorithm, so both directions speed up only after both sides are 0.1.5 or newer. Do not enlarge a single stream's window: the receive buffer holds at most 1024 out-of-order chunks, and a larger window with interleaved loss gets the connection closed by the peer. Speed comes from parallel chunks (`crates/core/src/files/lanes.rs`), and each stream has its own copy of that limit. The real transfer code measured on a simulated link (`cargo test -p p2pcli-core --test lossy_link -- --ignored --nocapture --test-threads=1 file_transfer_over`): at 106 ms and 1.3% loss, one stream is 38 Mbps and parallel streams are about 73 Mbps; at 106 ms, 300 Mbps, and 0.1% loss, 58 versus 208 Mbps; LAN and short links stay even.

Path selection (`crates/core/src/path.rs`): IPv6 is used unless it is more than 50 ms slower than IPv4, then IPv4, and the relay only when neither works. iroh's built-in rule gives IPv6 only 3 ms of slack and also requires a path to be 5 ms faster before switching, which cannot prefer IPv6, so the app uses iroh's path-selection interface (that interface is marked unstable, and the iroh version is pinned).

## Rendezvous server

The rendezvous server only issues device codes, exchanges addresses, and forwards the IPv6 packet-send notice. Terminal data does not pass through it. There are two ways to connect, and the client picks from the address format:

| Form | Channel | Use |
| --- | --- | --- |
| `pubkey@ip:port` or `pubkey@host:port` | UDP (iroh, encrypted, and the server identity is authenticated) | You have a public server |
| `wss://host` | WebSocket (TLS) | A platform that only speaks HTTP, such as Cloudflare Workers, with no server to maintain |

If the two forms point at different servers, the devices cannot look each other up. If one self-hosted server opens both listeners, devices on UDP and devices on WebSocket can look each other up.

### Self-hosted server

One command on a Linux server (with systemd, x86_64 or aarch64):

```sh
curl -fsSL https://raw.githubusercontent.com/ILoveBingLu/P2PCLI/main/crates/server/install.sh | sudo bash
```

The script downloads the published binary, checks the checksum, installs it as a service that starts on boot, and prints the address to enter in the app (`pubkey@ip:port`). After that, allow UDP 7788 in the cloud security group. Running it again upgrades (it replaces the program and leaves the existing service config alone). Arguments go after `bash -s --`, for example `| sudo bash -s -- --port 9000`. `--ws-port` opens a WebSocket listener, `--mirror` adds a proxy prefix when a server in China downloads slowly from GitHub, `--uninstall` removes it, and `--help` lists the rest.

You can also run it directly:

```sh
p2pcli-server --data-dir /var/lib/p2pcli --port 7788 --ws-port 7789 --public-ip <public-ip>
```

At startup it prints the UDP address to enter (`pubkey@ip:port`). The IP can be a hostname that resolves to the server. It is resolved on each connection, so changing DNS is enough when the server's IP changes. `--ws-port` is plain WebSocket. For `wss://`, put a reverse proxy in front (for example Caddy: `rdv.example.com { reverse_proxy 127.0.0.1:7789 }`), or put the hostname behind a CDN such as Cloudflare or Alibaba Cloud ESA. A CDN only forwards HTTP, so a hostname on a CDN can only use the `wss://` form. The benefit is that the server IP stays private and the CDN absorbs attacks.

### Cloudflare Workers

`cloudflare/` is the same protocol as a Workers app, in its own folder, deployed with one command:

```sh
cd cloudflare
./deploy.sh your-domain        # Windows: .\deploy.ps1 your-domain
```

The domain must be hosted on Cloudflare. After deploy, clients use `wss://your-domain`. Details, a local trial, and cost are in [cloudflare/README.md](cloudflare/README.md).

## Relay

The relay that forwards terminal data when a direct path cannot be opened is separate. The official public iroh relays are very slow from China, or unreachable. The relay is [iroh-relay](https://github.com/n0-computer/iroh), and it needs a server with a public IP:

```sh
iroh-relay --config-path relay.toml   # automatic Let's Encrypt certificate
```

Allow TCP 80/443 (the relay) and UDP 7842 (the relay's address probe). Enter the relay address in settings, or set `P2PCLI_RELAY`.

## Development

Cross-device, cross-session collaboration is wired into core storage, the network protocol, the `collab` CLI, and the desktop entry points. It is experimental. The manual two-device create, claim, report, and accept flow is in [the collaboration manual](docs/collaboration-manual.md). Design boundaries and acceptance criteria are in [the collaboration design](docs/cross-device-collaboration.md). The managed worker currently has only mock subprocess and JSON tests. A real long-running AI task, and an end-to-end check on two physical devices, are not done yet.

Rust 1.88 or newer, Node 24, and pnpm are required.

Run these from the repository root. You do not need to enter `app`:

```sh
cargo test --workspace              # unit tests, two devices in one process, and the IPv6 direct path
pnpm install                        # install app dependencies
pnpm run dev                        # start the desktop app in development
pnpm run build --bundles app,dmg    # macOS release package
pnpm run build --bundles nsis       # Windows release package
pnpm run build --bundles appimage,deb,rpm  # Linux release packages for the host architecture
```

To release the app, set the version in `app/src-tauri/tauri.conf.json` and push a `vX.Y.Z` tag of the same version (the tag's notes become the update notes). GitHub Actions builds seven targets at once: Mac (Apple silicon and Intel), Windows (x64, ARM64, and 32-bit), and Linux (x64 and ARM64). Each Linux target produces an AppImage, a DEB, and an RPM. Installers, update packages, signatures, `latest.json`, and `SHA256SUMS` are attached to the release (`.github/workflows/app-release.yml`). Linux updates match architecture and package format. DEB and RPM updates need administrator approval. After a packaging change, push a `ci/` branch, or run the release workflow by hand in Actions, to build without publishing. The artifacts stay in Artifacts. Only a version tag publishes. An ordinary push to `main` does not package.

Linux development dependencies, native runners, and the acceptance steps are in [Linux desktop support](docs/linux-desktop.md). Artwork for the dmg window background and the Windows installer is in `app/src-tauri/installer`. The Mac menu-bar icon is `app/src-tauri/icons/tray.svg`. After editing an SVG, regenerate the images on a Mac with `CHROME=<Chrome executable> node scripts/render-art.mjs`. Update packages must be signed: put `TAURI_SIGNING_PRIVATE_KEY` (the private key file contents, with no password) in the repository Secrets, and the public key in `plugins.updater.pubkey` in `tauri.conf.json`. If the private key is lost, installed copies can no longer receive updates. A local release build does not need the private key. Update artifacts are produced, and a signature is required, only with `--config '{"bundle":{"createUpdaterArtifacts":true}}'`.

To release the rendezvous server, set the version in the root `Cargo.toml` and push a `server-vX.Y.Z` tag of the same version. GitHub Actions builds Linux x86_64 and aarch64, with checksums, and attaches them to the release (`.github/workflows/server-release.yml`). The one-line installer downloads from there.

UI preview: from the root, run `pnpm run dev:web`, then open `http://localhost:1420/preview.html?s=empty` (`s` can also be `request`, `busy`, or `relay`). The page renders the UI with fake data. `pnpm run build:web` builds only the frontend.

UI strings are in `app/src/i18n`. `pnpm run check:i18n` checks that every string has English and Simplified Chinese.

The UI uses [HeroUI v3](https://heroui.com/), Tailwind CSS v4, and [Lucide](https://lucide.dev/) icons, in a dark desktop workspace layout. Buttons, inputs, cards, dialogs, tabs, checkboxes, toasts, and tooltips are HeroUI. App sizing and semantic wrappers live in `app/src/components/ui`. Theme and layout styles live in `app/src/styles.css`. Focus does not draw an extra ring. Keyboard interaction and form errors stay.

Base controls use HeroUI's default dark theme, radius, size, and interaction styles. Wrappers adapt product props and layout. They do not redraw the controls. Inputs on the settings page use the library's `secondary` variant so they stand off the dialog background.

Windows and macOS both use a single custom title bar. Network diagnosis and settings stay in the title bar. Windows uses [tauri-plugin-window-controls](https://github.com/insd47/tauri-plugin-window-controls) 0.2.1, patched in `vendor/tauri-plugin-window-controls` to fix icon gray levels and remove the unfocused fade, keeping the original MIT license. The controls are a Win32 child window, drawn with the system icon font, and they send system window commands. Windows 11 snap layouts work. They are not the default DWM title-bar buttons reused directly. macOS uses an overlay title bar and the system red, yellow, and green buttons. `trafficLightPosition` is `{ x: 14, y: 24 }`, which corrects buttons that sat too high on a 44-logical-pixel title bar. Tao/Wry's `y` adjusts the native title-bar container height. It is not the button center, so it cannot be half the web title-bar height. Final alignment on different macOS versions needs a real machine. Closing the window still hides to the tray.

The title-bar gear opens settings in the main area. Existing terminal sessions keep running when you return to the workspace. The Proxy section can enable an HTTP proxy, for example `http://127.0.0.1:7890` (the proxy app's HTTP or mixed port). The proxy is used for WebSocket rendezvous and HTTP(S) relays. UDP rendezvous and direct peer connections do not go through it. SOCKS5, HTTPS proxies, and username/password authentication are not supported, and the system proxy is not read automatically. Saving applies immediately. The app does not need to restart. Changing the device name or rendezvous address keeps the current terminal connection. Changing the proxy or relay rebuilds the network endpoint, so remote connections need to be opened again, but background terminal sessions keep running. A server or relay set by environment variable still wins over the settings page.

Platform window config is in `app/src-tauri/tauri.windows.conf.json` and `app/src-tauri/tauri.macos.conf.json`. The window array replaces the main config as a whole, so a size change has to be copied to both. The Windows main window is created from Rust setup through the plugin builder. The title bar is 44 logical pixels tall, and the frontend reserves 140 pixels on the right for the three buttons and DPI rounding. `windows-app-manifest.xml` declares the system compatibility a layered child window needs.

`pnpm run check` is a TypeScript check only. It does not emit a build. The preview page also accepts `s=unconfigured` (no server), `s=offline`, and `s=error` (connection and diagnosis failed) for the error states.

Add or remove app dependencies from the root as well:

```sh
pnpm add <package>       # runtime dependency
pnpm add -D <package>    # dev dependency
pnpm remove <package>    # remove a dependency
```

The root `package.json` holds the app scripts and dependencies. `pnpm-lock.yaml` locks versions. The UI and Tauri sources stay in `app`.

The desktop app uses a custom title bar that combines the brand, the current device, the terminal count, and the window controls. Drag empty space or text on the title bar to move the window. Double-click to maximize or restore. The close button hides to the tray. Quit is still the tray menu. Restart the dev process after changing window config or permissions. In a browser-only preview the window buttons are disabled.

Icons:

- macOS: the source is `app/src-tauri/icons/AppIcon.icon` (Icon Composer's layered liquid-glass icon, which Icon Composer can open and edit). After editing, run `./build-macos-icons.sh` in `app/src-tauri/icons`. It writes `Assets.car` (used from macOS 26, and it follows light, dark, tinted, and clear appearances) and `icon.icns` (the Dock icon for older systems and development). Commit both. Packaging does not need Xcode. The script needs Xcode 26 or newer.
- Windows and the PNGs in the UI: the source is `app/icon-source.svg`. Generate with `pnpm exec tauri icon app/icon-source.svg -o app/src-tauri/icons`, then put back the `icon.icns` produced by the script above.

Environment variables:

| Variable | Purpose |
| --- | --- |
| `P2PCLI_SERVER` | Rendezvous server, `pubkey@host:port` or `wss://host`. Default `wss://p2pcli.aiqji.cn`. |
| `P2PCLI_RELAY` | Self-hosted relay addresses, comma-separated |
| `P2PCLI_DATA_DIR` | Data directory, for running two copies on one machine |
| `RUST_LOG` | Log level. The log is `p2pcli.log` in the data directory. |
| `P2PCLI_SELFTEST` | Debug builds only: the self-test script, see `app/src/selftest.ts` |
| `P2PCLI_IPV6_ULA` | Tests: treat LAN IPv6 addresses that start with fd as public |

Set `P2PCLI_DEFAULT_SERVER` and `P2PCLI_DEFAULT_RELAY` at package time to compile a different default server into the installer.

Cross-machine test commands:

```sh
# the computer being connected to
p2pcli-dev --server <pubkey@ip:port> host --password abc234 --approve yes
# the connecting side: run one command and print the output and the path
p2pcli-dev --server <pubkey@ip:port> connect <device-code> --password abc234 --run 'echo hi' --watch 5
# LAN: list nearby devices, then connect by public key, with no rendezvous server
p2pcli-dev --server "" nearby
p2pcli-dev --server "" connect <public-key> --password abc234 --run 'echo hi'
```

`--relay-only` forces the relay. `--bind-ip` binds one interface. `--no-lan` turns off LAN discovery. `--lan-hidden` watches others without broadcasting this computer.

## What has been verified

These notes cover the existing connection and terminal behavior. They do not mean the new collaboration protocol, the managed worker, or a two-device task loop has been checked on real machines.

- macOS ↔ macOS direct on a LAN; macOS ↔ Windows IPv4 hole punching through two layers of NAT; forced relay
- The desktop app in all three directions (macOS→macOS, macOS→Windows, Windows→macOS) through the real UI: enter a device code, approve the dialog, run a command in a tab, and display Chinese
- The IPv6 direct path: an in-process test, and two Macs (LAN IPv6 standing in for public IPv6), with the first packet already on IPv6

Not verified yet: a real public IPv6 direct connection through both routers' firewalls, and a production self-hosted relay over HTTPS.

## License

MIT. See [LICENSE](LICENSE).

### Terminal sessions stay running

Desktop terminals are hosted by a separate background process on the controlled device: tmux on macOS and Linux, and [psmux](https://github.com/psmux/psmux) (native ConPTY) on Windows. WSL is not required.

- Windows, macOS, and Linux release configs include the session manager. Users do not install tmux, psmux, Homebrew, or WSL, and the first launch does not download components.
- The component matches the packaged operating system and CPU architecture: Windows carries only the matching psmux 3.4.0; Mac and Linux carry only the matching tmux 3.7c. The Linux file is named `p2pcli-tmux` and does not take `/usr/bin/tmux`. Another system's binary is not put in the installer. Apple silicon and Intel are separate packages. The committed dual-architecture Mac file is used only when universal is selected explicitly.
- The desktop app calls the component by an absolute path inside the app directory. It does not depend on the user's PATH. If the installed file is damaged, the app asks for a reinstall instead of quietly calling some other version on the system. A standalone core or CLI build can still use a tool the user installed.
- After connecting, the background session list is shown. Sessions can be created, resumed, or explicitly ended. The session button on the terminal toolbar opens the list again.
- Disconnecting, closing a tab, and quitting P2PCLI only detach the client. Connecting to the same device again restores the original process, working directory, variables, and terminal screen.
- A session ends when End session is confirmed, or when `exit` is run in the shell. A system restart, a logout, or stopping the background tmux or psmux service still exits the processes.
- Sessions are isolated by both devices' public keys. Reconnecting still needs the normal authentication. Do not delete the device key, or the new identity cannot restore the old identity's sessions.
- The protocol is v2, so both sides of a connection need to update together. One-shot CLI commands used in development use a temporary terminal, keep the real exit code, and do not create a background session.

`preview.html?s=sessions` provides fake background sessions. The real resume regression is `persistent_session_survives_disconnect_and_host_restart`. It is ignored by default. Set `P2PCLI_TEST_SESSION_PROGRAM` to the absolute path of a prepared component and run it explicitly. A global install is not required. The test starts its own test namespace.

### Session failure logs

On the **controlled** computer, open Network diagnosis → View logs in the title bar. The file manager selects `p2pcli.log`. The default level is enough. A diagnosis command and `RUST_LOG` are not required.

- It records create, resume, an explicit end, a network stream disconnect, and the connecting client's PID, exit code, and signal. A client exiting is not the background session exiting.
- While the app is running it queries background sessions every 10 seconds, including detached ones. If a session disappears without the app ending it, the log records the last background PID, the socket, and a snapshot of the component processes at that time. It does not recreate or kill the session. `exit` inside the shell also looks like a disappearing session, so that record alone is not a crash.
- On Mac it also extracts the time, PID, exception, and termination reason from up to five tmux system crash reports from the last three days. Those have to be matched to the failure time. Without a system report, the background service's exit reason cannot be reconstructed. Terminal input and output are not logged.
- Logs are appended across restarts. Each file is about 5 MiB, and the current file plus four history files are kept (`p2pcli.log.1` through `.4`). Startup leaves a new-process mark, which can be matched to a development restart. Killing the process may leave no clean exit record.

### Session components committed in the repository

Windows psmux 3.4.0, and macOS and Linux tmux 3.7c, are committed as real binaries, with their licenses. A full clone is enough. tmux, psmux, and Homebrew do not need to be installed separately. Dev startup, packaging, and the app's first launch do not download or compile these components. The project itself still needs the normal Node and Rust development dependencies.

- Windows includes `psmux.exe` from the official x64, ARM64, and x86 portable packages.
- Mac includes the official Apple silicon and Intel prebuilt binaries. The minimum is **macOS 15**. They depend only on system dynamic libraries. libevent and similar libraries do not need to be installed.
- A Mac universal build is committed too. It is a FAT file of two unmodified official binary slices. It was not recompiled, and the minimum system version was not changed.
- Linux includes the official x86_64 and ARM64 static musl ELF binaries. The component itself does not depend on the distro's tmux, libevent, or ncurses libraries. The desktop app still depends on GTK and WebKitGTK.
- Before `pnpm run dev`, and as a step before `pnpm run build`, the component files are checked for size, SHA-256, CPU architecture, the Mac minimum system version, dynamic-library paths, and static linking of the Linux ELF. A missing or damaged file stops the command and asks you to restore the repository file.
- `app/src-tauri/binaries` holds the executables. `app/src-tauri/resources/session-tools` holds the license and source notes for each platform. `scripts/session-tools.json` pins the official source, the archive, and each file's SHA-256.
- Tauri's platform config references only that platform's `externalBin` and license directory, then picks one binary for the target architecture. The repository contains every version. An installer does not carry another platform's component or an unrelated architecture. The Mac universal package is used only when `universal-apple-darwin` is requested explicitly.
- In development Tauri copies the component next to the executable. In a Mac release the component is ad-hoc signed together with the app (`signingIdentity` is `-` in `tauri.macos.conf.json`). It is not notarized. Git keeps the executable bit on the Mac files.

Check or print the plan without building the app:

```sh
pnpm run check:session-tools
pnpm run check:session-tools --all
pnpm run check:session-tools --target aarch64-pc-windows-msvc
pnpm run check:session-tools --target aarch64-apple-darwin --print-plan
node --test scripts/check-session-tools.test.mjs
```

The default target is Tauri's `TAURI_ENV_TARGET_TRIPLE`, so `pnpm run build --target ...` adapts by itself. A manual check can pass `--target`. Otherwise the Rust host target is used. The old command `pnpm run prepare:session-tools` remains as a local check entry point. Download and source-build steps were removed. The historical `.cache/session-tools` cache is unused and is not packaged.

When maintainers upgrade a component, they should take the release files from the official project listed in the manifest, check the release archive hash, update the binary, the license, and the manifest together, and run the check for every platform. Do not add download or compile logic to the development startup script.
