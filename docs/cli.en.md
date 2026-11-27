**English** | [简体中文](cli.md)

# The p2pcli command-line tool

The installer includes a `p2pcli` command-line tool (on Mac it is placed in `~/.local/bin`; on Windows it is added to PATH; on Linux a copy is kept in the app data `bin` directory so an AppImage's temporary mount path does not break it). Settings → File transfer shows the status and can add it to PATH. It works while P2PCLI is open:

```sh
p2pcli devices                       # list devices, online status, and latency
p2pcli send <device> report.pdf proj # send files or folders; the name can be partial, or use a device code
p2pcli send <device> a.txt --to ~/work  # put it in that directory on the other computer, overwriting the same name
p2pcli pull <device> ~/work/out.log  # pull a file into the current directory; --to changes the local directory
p2pcli exec <device> -- ls -la ~/work # run one command on the other computer and return output and the exit code
p2pcli cat <device> ~/work/main.rs -n # read a file on the other computer directly, without pulling it
p2pcli edit <device> ~/work/main.rs --old 'old' --new 'new'   # edit a file on the other computer in place: exact-text replace
printf 'text' | p2pcli write <device> ~/work/notes.md       # write stdin to a file (create or rewrite it)
p2pcli grep <device> 'TODO' ~/work   # search contents; there are also ls, find, mkdir, mv, and rm (moves to Trash)
p2pcli inbox                         # show recently received files
p2pcli pipe device ssh               # connect to the device's SSH service (an OpenSSH ProxyCommand); a port number also works
p2pcli mcp                           # run as an MCP server for AIs like Cursor, Claude Desktop and Cline
p2pcli send <device> a.txt --json    # JSON for programs and AI; a failure exits non-zero
```

## Authentication

`send` without `--to` only writes into the other side's inbox and does not overwrite existing files. A device that already knows this one accepts it immediately. `send --to`, `pull`, and `exec` can touch any path on the other computer, so they use the same approval as opening a terminal: the pairing password must be right and the other side must have approved (or marked later connections as trusted). After that, the same connection is reused. If neither device has ever connected to the other, the first time has to be in the app, with the other side's pairing password (you can check Trust both ways while connecting). If the other side has connected to this computer, or has marked it trusted, no password is needed. If the other side has not marked this computer trusted, each new connection still needs their approval. Pulls and writes show up in the other side's transfer history and notifications. A running command counts as a terminal in use in the title bar. Both sides need 0.1.5 or newer. An older peer is asked to update.

## Reading and writing the other side's files

Reading and writing the other side's files directly (`cat`, `write`, `edit`, `ls`, `find`, `grep`, `mkdir`, `mv`, `rm`) is cross-device editing for AI: Claude Code, Codex, and similar AIs on this computer can change code on another computer without pulling it here and sending it back. `edit` follows the same rules as an AI's built-in edit tool: the old text must match exactly and appear once (or pass `--all`). When writing back, it checks that the file hasn't changed since it was read (by a program in the other side's terminal, or a save in the editor), and if it has, it reports an error instead of overwriting. Files with CRLF line endings match too. Only UTF-8 text up to 5 MB is supported; use `pull`/`send` for anything else. `rm` moves to the other side's Trash, where it can be restored. The other side needs a version with the editor (0.1.9 or newer).

## Teaching your AI to use it (multi-device AI)

at startup the app drops a usage note (`SKILL.md` in the Agent Skills format) into the skill directory of each AI installed on this computer. New sessions then know that `p2pcli` exists, when to use it, and what to do when it fails, without being taught:

- Claude Code: `~/.claude/skills/p2pcli/` (follows `CLAUDE_CONFIG_DIR` if that is set)
- Codex, Gemini CLI, Cursor, OpenCode, Copilot CLI, and Kimi CLI: the shared `~/.agents/skills/p2pcli/`
- Antigravity CLI (`agy`): `~/.gemini/antigravity-cli/skills/p2pcli/`
- Qwen Code: `~/.qwen/skills/p2pcli/`. CodeBuddy: `~/.codebuddy/skills/p2pcli/`

It only writes into AI config directories that already exist. It does not create a directory for an AI that is not installed. It only writes a folder named `p2pcli`, and it does not edit `CLAUDE.md` or `AGENTS.md`. The file is marked `managed-by: p2pcli`. Upgrades overwrite only files with that mark, and leave a same-named skill that someone else installed alone. Settings → Multi-device AI shows which AIs were detected and whether the skill is installed. You can install, update, or remove one or all of them, and turn off Install and update automatically at startup (turning it off only stops the automatic work; installed skills are not deleted; a development build can still install manually, it just does not install automatically). AI windows that are already open need to be reopened before they see it. The source of the note is `app/src-tauri/src/p2pcli-skill.md`. Update it when commands or usage change.

## For AIs that use MCP (p2pcli mcp)

AIs like Cursor, Claude Desktop and Cline use MCP for tools and don't read the instructions above. Add the config below to their MCP settings and they can work on your other devices directly (P2PCLI must be running). Settings → Multi-device AI has a copy button with the full path of the command-line tool filled in (graphical AIs don't get your terminal's PATH), and another one that copies the add command for Claude Code.

```json
{ "mcpServers": { "p2pcli": { "command": "full path to p2pcli", "args": ["mcp"] } } }
```

Tools: `p2p_devices` (list devices), `p2p_exec` (run a command), `p2p_read_file` (with line numbers and paging), `p2p_write_file`, `p2p_edit_file` (exact replacement, same rules as `edit`), `p2p_list_dir`, `p2p_find_files`, `p2p_grep`, `p2p_mkdir`, `p2p_move`, `p2p_trash` (move to trash), `p2p_send_files`, `p2p_pull_files`, `p2p_screenshot` (a screenshot of the other screen as PNG). Each one is passed to the running P2PCLI, with the same authentication as the command line.

`p2p_screenshot` lets the AI see error dialogs, web pages and app windows on the other screen. The other side must allow it like viewing the screen (trusted devices follow its settings) and gets a system notification afterwards; it needs to run macOS or Windows and the new version. Direct paths come first; if only a relay is available the screenshot still works but is scaled to 1024 wide, and the result says so.

## As an SSH proxy (p2pcli pipe)

`p2pcli pipe device port` connects standard input and output to that port on the other computer's localhost and passes the bytes through; use `ssh` instead of a port to reach its SSH service. Use it as an OpenSSH ProxyCommand:

```sh
ssh -o ProxyCommand="p2pcli pipe device ssh" user@device
```

When the other side is a Mac or Linux, its P2PCLI starts an sshd as the current user, listening only on loopback, and you log in with the key the desktop app manages for you: no Remote Login and no password (both sides need the new version). When the other side is Windows, its system OpenSSH service is used. You rarely need to type this: right-click a device and choose Open in local VS Code…, and P2PCLI writes an alias to `~/.ssh/p2pcli_hosts` (see the [user guide](usage.en.md#open-in-local-vs-code-ssh)), after which `ssh p2p-device-name` just works. Messages go to standard error only, never into the data; it waits for a direct path first and warns when using a relay. On Windows, the other side doesn't learn that standard input has ended (a named pipe can't close just one direction), so it has to close the connection itself; SSH is not affected.

## exec details

- The command is parsed by the other side's shell (a login shell on macOS and Linux, PowerShell on Windows). Quote it yourself, the same way you would for ssh.
- There is no pseudoterminal. The command's input is empty, it cannot be interactive, and programs do not change their behavior because a terminal is attached. Standard output and standard error come back separately, as raw bytes.
- The exit code is the command's exit code. A timeout (`--timeout` seconds) is 124. A signal is 128 plus the signal number. A P2PCLI error (device not found, cannot connect) is 255.
- The default working directory is the other side's home directory. `--cwd` changes it. `--json` packs stdout, stderr, and the exit code into JSON.
- If the command line is closed with Ctrl+C or a closed pipe, the other side stops the whole process group. Processes the command itself backgrounded are left alone. For a long-running process, use `nohup` and redirect its output.
