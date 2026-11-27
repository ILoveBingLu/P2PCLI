**English** | [简体中文](development.md)

# Development

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

Congestion control (`crates/core/src/quic.rs`): Cubic is the default. The app used to switch to BBR3 when the round trip was 20 ms or more. On a simulated link with uniform random loss BBR3 is much faster (about 40 Mbps versus about 1 Mbps for Cubic at 106 ms and 1.3% loss), but on a real cross-border link (about 80 ms round trip) noq's BBR3 kept lowering its bandwidth estimate after a few scattered losses, down to about 0.5 Mbps, and never recovered, while Cubic held 30 to 80 Mbps on the same link. So it is back to Cubic. `P2PCLI_CC=bbr3` (always BBR3) or `auto` (choose by delay) switches back for comparison. Do not enlarge a single stream's window: the receive buffer holds at most 1024 out-of-order chunks, and a larger window with interleaved loss gets the connection closed by the peer. Speed comes from parallel chunks (`crates/core/src/files/lanes.rs`), and each stream has its own copy of that limit. The real transfer code measured on a simulated link (`cargo test -p p2pcli-core --test lossy_link -- --ignored --nocapture --test-threads=1 file_transfer_over`): at 106 ms and 1.3% loss, one stream is 38 Mbps and parallel streams are about 73 Mbps; at 106 ms, 300 Mbps, and 0.1% loss, 58 versus 208 Mbps; LAN and short links stay even.

Path selection (`crates/core/src/path.rs`): IPv6 is used unless it is more than 50 ms slower than IPv4, then IPv4, and the relay only when neither works. iroh's built-in rule gives IPv6 only 3 ms of slack and also requires a path to be 5 ms faster before switching, which cannot prefer IPv6, so the app uses iroh's path-selection interface (that interface is marked unstable, and the iroh version is pinned).

Direct first for heavy traffic (`path::wait_direct`): right after connecting, a connection is often still on the relay and switches to a direct path once hole punching succeeds a few seconds later. File transfers, large clipboard content, port forwarding, reverse forwarding, proxy sharing, `p2pcli pipe` and screenshots wait up to 3 seconds (`path::DIRECT_WAIT`) for a direct path before starting; if none appears they use the relay anyway and the app or command line says it will be slower. The remote screen only uses direct paths (it pauses on a relay).

Each of these newer features is a capability that is used only when both sides report it during the handshake, so older versions never receive the new messages: `reverse_forward_v1` (reverse forwarding and proxy sharing, `crates/core/src/reverse.rs`), `ssh_v1` (the other side provides an SSH service, a user-mode sshd on macOS and Linux, `crates/core/src/ssh_host.rs`) and `screenshot_v1` (single screenshots). The remote-control shared clipboard is negotiated in the `screen_v1` open request (`crates/core/src/screen_clip.rs`). The side being connected to never opens streams toward the connecting side; when it needs data from the connecting side (clipboard, reverse forwarding) it says so on an existing stream and the connecting side opens a new one.

## Setup

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

## Releases

To release the app, set the version in `app/src-tauri/tauri.conf.json` and push a `vX.Y.Z` tag of the same version (the tag's notes become the update notes). GitHub Actions builds seven targets at once: Mac (Apple silicon and Intel), Windows (x64, ARM64, and 32-bit), and Linux (x64 and ARM64). Each Linux target produces an AppImage, a DEB, and an RPM. Installers, update packages, signatures, `latest.json`, and `SHA256SUMS` are attached to the release (`.github/workflows/app-release.yml`). Linux updates match architecture and package format. DEB and RPM updates need administrator approval. After a packaging change, push a `ci/` branch, or run the release workflow by hand in Actions, to build without publishing. The artifacts stay in Artifacts. Only a version tag publishes. An ordinary push to `main` does not package.

Linux development dependencies, native runners, and the acceptance steps are in [Linux desktop support](linux-desktop.md). Artwork for the dmg window background and the Windows installer is in `app/src-tauri/installer`. The Mac menu-bar icon is `app/src-tauri/icons/tray.svg`. After editing an SVG, regenerate the images on a Mac with `CHROME=<Chrome executable> node scripts/render-art.mjs`. Update packages must be signed: put `TAURI_SIGNING_PRIVATE_KEY` (the private key file contents, with no password) in the repository Secrets, and the public key in `plugins.updater.pubkey` in `tauri.conf.json`. If the private key is lost, installed copies can no longer receive updates. A local release build does not need the private key. Update artifacts are produced, and a signature is required, only with `--config '{"bundle":{"createUpdaterArtifacts":true}}'`.

The Mac app is signed with a self-signed code signing certificate (`P2PCLI Self-Signed Code Signing`). Put `MACOS_CERTIFICATE` (the p12 file as base64) and `MACOS_CERTIFICATE_PASSWORD` in the repository Secrets. The release workflow imports it into a temporary keychain and uses `APPLE_SIGNING_IDENTITY` to override the `-` in the config. An ad-hoc signature's designated requirement is the hash of the whole program, which changes every version, so macOS treats each update as a different app and users have to grant Screen Recording and Accessibility again. Signed with the same certificate, the requirement becomes "signed by this certificate", and permissions survive updates. Apple did not issue the certificate, so the first launch still has to be allowed under Privacy & Security. Every version must use the same certificate; if it is lost or replaced, every user has to grant permissions once more. A tag release stops if these Secrets are missing. Local builds still use ad-hoc signing.

To release the rendezvous server, set the version in the root `Cargo.toml` and push a `server-vX.Y.Z` tag of the same version. GitHub Actions builds Linux x86_64 and aarch64, with checksums, and attaches them to the release (`.github/workflows/server-release.yml`). The one-line installer downloads from there.

## UI

UI preview: from the root, run `pnpm run dev:web`, then open `http://localhost:1420/preview.html?s=empty` (`s` can also be `request`, `busy`, or `relay`). The page renders the UI with fake data. `pnpm run build:web` builds only the frontend.

UI strings are in `app/src/i18n`. `pnpm run check:i18n` checks that every string has English and Simplified Chinese.

The UI uses [HeroUI v3](https://heroui.com/), Tailwind CSS v4, and [Lucide](https://lucide.dev/) icons, in a dark desktop workspace layout. Buttons, inputs, cards, dialogs, tabs, checkboxes, toasts, and tooltips are HeroUI. App sizing and semantic wrappers live in `app/src/components/ui`. Theme and layout styles live in `app/src/styles.css`. Focus does not draw an extra ring. Keyboard interaction and form errors stay.

Base controls use HeroUI's default dark theme, radius, size, and interaction styles. Wrappers adapt product props and layout. They do not redraw the controls. Inputs on the settings page use the library's `secondary` variant so they stand off the dialog background.

Windows and macOS both use a single custom title bar. Network diagnosis and settings stay in the title bar. Windows uses [tauri-plugin-window-controls](https://github.com/insd47/tauri-plugin-window-controls) 0.2.1, patched in `vendor/tauri-plugin-window-controls` to fix icon gray levels and remove the unfocused fade, keeping the original MIT license. The controls are a Win32 child window, drawn with the system icon font, and they send system window commands. Windows 11 snap layouts work. They are not the default DWM title-bar buttons reused directly. macOS uses an overlay title bar and the system red, yellow, and green buttons. `trafficLightPosition` is `{ x: 14, y: 24 }`, which corrects buttons that sat too high on a 44-logical-pixel title bar. Tao/Wry's `y` adjusts the native title-bar container height. It is not the button center, so it cannot be half the web title-bar height. Final alignment on different macOS versions needs a real machine. Closing the window still hides to the tray.

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

## Icons

- macOS: the source is `app/src-tauri/icons/AppIcon.icon` (Icon Composer's layered liquid-glass icon, which Icon Composer can open and edit). After editing, run `./build-macos-icons.sh` in `app/src-tauri/icons`. It writes `Assets.car` (used from macOS 26, and it follows light, dark, tinted, and clear appearances) and `icon.icns` (the Dock icon for older systems and development). Commit both. Packaging does not need Xcode. The script needs Xcode 26 or newer.
- Windows and the PNGs in the UI: the source is `app/icon-source.svg`. Generate with `pnpm exec tauri icon app/icon-source.svg -o app/src-tauri/icons`, then put back the `icon.icns` produced by the script above.

## Environment variables

| Variable | Purpose |
| --- | --- |
| `P2PCLI_SERVER` | Rendezvous server, `pubkey@host:port` or `wss://host`. Default `wss://p2pcli.aiqji.cn`. |
| `P2PCLI_RELAY` | Self-hosted relay addresses, comma-separated |
| `P2PCLI_DATA_DIR` | Data directory, for running two copies on one machine |
| `RUST_LOG` | Log level. The log is `p2pcli.log` in the data directory. |
| `P2PCLI_SELFTEST` | Debug builds only: the self-test script, see `app/src/selftest.ts` |
| `P2PCLI_IPV6_ULA` | Tests: treat LAN IPv6 addresses that start with fd as public |

Set `P2PCLI_DEFAULT_SERVER` and `P2PCLI_DEFAULT_RELAY` at package time to compile a different default server into the installer.

## Cross-machine tests

```sh
# the computer being connected to
p2pcli-dev --server <pubkey@ip:port> host --password abc234 --approve yes
# the connecting side: run one command and print the output and the path
p2pcli-dev --server <pubkey@ip:port> connect <device-code> --password abc234 --run 'echo hi' --watch 5
# LAN: list nearby devices, then connect by public key, with no rendezvous server
p2pcli-dev --server "" nearby
p2pcli-dev --server "" connect <public-key> --password abc234 --run 'echo hi'
```

Adding `--borrow-proxy 127.0.0.1:7890` on the connecting side shares that proxy with the other side before running the command (check `HTTPS_PROXY` and whether `curl` reaches the internet there). `--relay-only` forces the relay. `--bind-ip` binds one interface. `--no-lan` turns off LAN discovery. `--lan-hidden` watches others without broadcasting this computer.

## What has been verified

These notes cover the existing connection and terminal behavior. They do not mean the new collaboration protocol, the managed worker, or a two-device task loop has been checked on real machines.

- macOS ↔ macOS direct on a LAN; macOS ↔ Windows IPv4 hole punching through two layers of NAT; forced relay
- The desktop app in all three directions (macOS→macOS, macOS→Windows, Windows→macOS) through the real UI: enter a device code, approve the dialog, run a command in a tab, and display Chinese
- The IPv6 direct path: an in-process test, and two Macs (LAN IPv6 standing in for public IPv6), with the first packet already on IPv6

Not verified yet: a real public IPv6 direct connection through both routers' firewalls, and a production self-hosted relay over HTTPS.

## Collaboration (experimental)

Cross-device, cross-session collaboration is wired into core storage, the network protocol, the `collab` CLI, and the desktop entry points. It is experimental. The manual two-device create, claim, report, and accept flow is in [the collaboration manual](collaboration-manual.md). Design boundaries and acceptance criteria are in [the collaboration design](cross-device-collaboration.md). The managed worker currently has only mock subprocess and JSON tests. A real long-running AI task, and an end-to-end check on two physical devices, are not done yet.

## Terminal sessions stay running

Desktop terminals are hosted by a separate background process on the controlled device: tmux on macOS and Linux, and [psmux](https://github.com/psmux/psmux) (native ConPTY) on Windows. WSL is not required.

- Windows, macOS, and Linux release configs include the session manager. Users do not install tmux, psmux, Homebrew, or WSL, and the first launch does not download components.
- The component matches the packaged operating system and CPU architecture: Windows carries only the matching psmux 3.4.0; Mac and Linux carry only the matching tmux 3.7c. The Linux file is named `p2pcli-tmux` and does not take `/usr/bin/tmux`. Another system's binary is not put in the installer. Apple silicon and Intel are separate packages. The committed dual-architecture Mac file is used only when universal is selected explicitly.
- The desktop app calls the component by an absolute path inside the app directory. It does not depend on the user's PATH. If the installed file is damaged, the app asks for a reinstall instead of quietly calling some other version on the system. A standalone core or CLI build can still use a tool the user installed.
- After connecting, the background session list is shown. Sessions can be created, resumed, or explicitly ended. The session button on the terminal toolbar opens the list again.
- Disconnecting, closing a tab, and quitting P2PCLI only detach the client. Connecting to the same device again restores the original process, working directory, variables, and terminal screen.
- On macOS and Linux, sessions are attached through tmux control mode (`tmux -C`), the way iTerm2's tmux integration works (`crates/core/src/tmux_control.rs`). tmux does not draw the screen; it hands over the program's raw output, which is forwarded as is, so history builds up in the viewer's own terminal and scrolling and selection stay local, with no network round trip. On attach, `capture-pane` fills in the last 5000 lines of history and the current screen, and the cursor and keyboard and mouse modes from `display` are restored. If the viewer falls more than 2 seconds behind, tmux pauses the pane, and the host skips the backlog and redraws the latest screen. With an older viewer, or a Windows host (psmux control mode is unverified), the ordinary attach is used and the wheel goes to tmux copy mode. The real-tmux regressions are `control_mode_keeps_history_on_viewer` and `control_mode_skips_backlog_for_slow_viewer`, run the same way as below.
- A session ends when End session is confirmed, or when `exit` is run in the shell. A system restart, a logout, or stopping the background tmux or psmux service still exits the processes.
- Sessions are isolated by both devices' public keys. Reconnecting still needs the normal authentication. Do not delete the device key, or the new identity cannot restore the old identity's sessions.
- The protocol is v2, so both sides of a connection need to update together. One-shot CLI commands used in development use a temporary terminal, keep the real exit code, and do not create a background session.

`preview.html?s=sessions` provides fake background sessions. The real resume regression is `persistent_session_survives_disconnect_and_host_restart`. It is ignored by default. Set `P2PCLI_TEST_SESSION_PROGRAM` to the absolute path of a prepared component and run it explicitly. A global install is not required. The test starts its own test namespace.

## Session failure logs

On the **controlled** computer, open Network diagnosis → View logs in the title bar. The file manager selects `p2pcli.log`. The default level is enough. A diagnosis command and `RUST_LOG` are not required.

- It records create, resume, an explicit end, a network stream disconnect, and the connecting client's PID, exit code, and signal. A client exiting is not the background session exiting.
- While the app is running it queries background sessions every 10 seconds, including detached ones. If a session disappears without the app ending it, the log records the last background PID, the socket, and a snapshot of the component processes at that time. It does not recreate or kill the session. `exit` inside the shell also looks like a disappearing session, so that record alone is not a crash.
- On Mac it also extracts the time, PID, exception, and termination reason from up to five tmux system crash reports from the last three days. Those have to be matched to the failure time. Without a system report, the background service's exit reason cannot be reconstructed. Terminal input and output are not logged.
- Logs are appended across restarts. Each file is about 5 MiB, and the current file plus four history files are kept (`p2pcli.log.1` through `.4`). Startup leaves a new-process mark, which can be matched to a development restart. Killing the process may leave no clean exit record.

## Session components committed in the repository

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
