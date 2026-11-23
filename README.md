**English** | [简体中文](README.zh-CN.md)

# P2PCLI

A peer-to-peer remote terminal. Enter another computer's device code and pairing password, and after the other side approves, you use its terminal inside the app window. Command-line AIs such as Claude and Codex run with the accounts and environment on that computer.

- **Direct connection first**: IPv6 direct, then IPv4 hole punching, then a relay as the fallback. End-to-end encrypted; a relay cannot read the traffic.
- **Pairs like remote-desktop software**: a 9-digit device code plus a 6-digit pairing password, and the other side still clicks Allow. No code is needed on the same local network.
- **Terminal tabs**: sessions keep running in the background after a disconnect or after the app quits, and you pick them up when you reconnect.
- **Knows the AI in a session**: which AI it is, whether it is working or waiting for you, and a reminder when it needs you. An input box under the terminal sends a whole sentence at once.
- **Code editor**: edit files on the other computer in place, with completion, go to definition, errors, and a Git panel.
- **Files, port forwarding, remote control**: drop files into a terminal to send them, forward the other side's dev server to this computer in one click, and view or control its screen.
- **The `p2pcli` command-line tool**: send files, run commands, and read or edit files on other devices from a terminal. Your local AI can use it too.

The interface follows the system language: Chinese locales use Simplified Chinese, every other locale uses English. Change it under Settings → Connection → Language.

## Download and install

Download a build from [Releases](../../releases):

| System | Installer |
| --- | --- |
| macOS 15 or later | Apple silicon `P2PCLI_x.y.z_aarch64.dmg`, Intel `P2PCLI_x.y.z_x64.dmg` |
| Windows 10 or later | Most PCs `P2PCLI_x.y.z_x64-setup.exe`, Snapdragon and other ARM chips `P2PCLI_x.y.z_arm64-setup.exe`, 32-bit `P2PCLI_x.y.z_x86-setup.exe` |
| Linux (x64 / ARM64) | `.AppImage`, `.deb`, `.rpm`. See [Linux desktop support](docs/linux-desktop.md) |

Not sure which one: on a Mac, Apple menu → About This Mac, then look at Chip. On Windows, Settings → System → About, then look at System type.

After installation the app checks for updates on its own. When one is available, click Update and restart on the main screen.

The installers are not paid-signed, so allow the app once the first time you open it:

- **macOS**: double-click it. The dialog only has Done and Move to Trash, so click Done. Then open System Settings → Privacy & Security, scroll down to Security, click Open Anyway, and enter your login password. A dialog appears again; click Open Anyway. If Open Anyway is missing, double-click P2PCLI again first. The button only shows for about an hour after macOS blocks the app.
- **Windows**: if Windows protected your PC, click More info → Run anyway.

## How to use it

1. Install and open P2PCLI on both computers.
2. On the computer being connected to, open This computer from the right side of the title bar and send the device code and pairing password to the other side. (The lock next to the password pins it.)
3. On the connecting side, click the plus button in the title bar, enter the code and password, and click Connect. The other side clicks Allow.
4. You now have a terminal on the other computer. Click + for another tab.

Devices you have connected to stay on the left, and one click connects again. See the [user guide](docs/usage.en.md) for everything else.

## Documentation

- [User guide](docs/usage.en.md): local terminals, the local network, mutual trust, the code editor, port forwarding, remote control, file transfer, command-line AIs
- [The p2pcli command-line tool](docs/cli.en.md): send files, run commands on the other computer, read and edit its files in place
- [Self-hosting the rendezvous server and relay](docs/self-hosting.en.md): the default server works out of the box; read this to run your own
- [Development](docs/development.en.md): how it works, building, releasing, environment variables

## License

MIT. See [LICENSE](LICENSE).
