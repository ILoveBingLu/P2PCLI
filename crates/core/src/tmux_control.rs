//! 用 tmux 的控制模式（`tmux -C`）接后台会话，学 iTerm2 的 tmux 集成。
//!
//! 普通接法（`tmux attach`）里，屏幕是 tmux 画的：往上翻历史要进 tmux 的复制模式，滚轮每一格都要发过来、
//! 等 tmux 重画整屏再传回去，隔着网络一格一顿。控制模式下 tmux 不画屏幕，只把会话里程序的原始输出
//! （`%output`）交出来，原样转给连接方，历史攒在连接方自己的终端里，滚动、选字都不走网络。
//!
//! - 刚接上时用 `capture-pane` 把已有的历史和当前画面拉一次，再按 `display` 查到的光标、键盘鼠标模式补齐状态
//! - 键盘输入换成 `send-keys -H`（按字节的十六进制），改尺寸换成 `refresh-client -C`
//! - 连接方收得慢、输出积压超过 [`PAUSE_AFTER_SECS`] 秒时 tmux 会暂停这个窗格（`%pause`）：这时不补发积压的，
//!   直接恢复并重画一张最新画面，像 mosh 一样跳过来不及看的中间画面

use std::collections::VecDeque;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

use crate::proto::{Frame, Msg, read_frame, write_data, write_msg};
use crate::sessions::ControlTarget;

/// 告诉连接方这条终端走控制模式：历史在它自己那边，滚轮别再转给 tmux。
/// 用一个没人用的 OSC，旧版本的终端认不出会直接忽略
pub const CONTROL_MARK: &[u8] = b"\x1b]7737;p2pcli-control\x07";
/// 刚接上时最多补多少行历史
const HISTORY_LINES: u32 = 5000;
/// 输出积压超过这么多秒，tmux 暂停这个窗格，改成重画最新画面
const PAUSE_AFTER_SECS: u32 = 2;
/// 一条 send-keys 最多带多少字节，免得一行命令太长
const INPUT_CHUNK: usize = 256;
/// 连接方断开后等 tmux 自己退出的时间，过了就结束它
const DETACH_GRACE: Duration = Duration::from_secs(3);
/// 窗格状态：编号、是否在备用屏幕、光标、滚动区、键盘和鼠标模式
const STATE_FORMAT: &str = "#{pane_id}\t#{alternate_on}\t#{cursor_x}\t#{cursor_y}\t#{alternate_saved_x}\t#{alternate_saved_y}\t\
#{scroll_region_upper}\t#{scroll_region_lower}\t#{cursor_flag}\t#{keypad_cursor_flag}\t#{keypad_flag}\t#{bracket_paste_flag}\t\
#{mouse_standard_flag}\t#{mouse_button_flag}\t#{mouse_all_flag}\t#{mouse_sgr_flag}\t#{mouse_utf8_flag}\t#{wrap_flag}\t\
#{insert_flag}\t#{origin_flag}\t#{pane_height}";

/// 发出去的每条命令回来一段 %begin…%end，按顺序对上
#[derive(Debug, Clone, Copy, PartialEq)]
enum Reply {
    Ignore,
    State,
    /// 历史加当前画面（备用屏幕时是备用屏幕的画面）
    Lines,
    /// 被全屏程序盖住的普通屏幕（capture-pane -a）
    Saved,
}

#[derive(Debug, Default, Clone, PartialEq)]
struct PaneState {
    pane: String,
    alternate: bool,
    cursor: (u32, u32),
    saved_cursor: (u32, u32),
    region: (u32, u32),
    cursor_visible: bool,
    app_cursor: bool,
    app_keypad: bool,
    bracket_paste: bool,
    mouse_standard: bool,
    mouse_button: bool,
    /// 1003：鼠标一动就报（tmux 的 mouse_all_flag；mouse_any_flag 是“开了任意一种”，不是这个）
    mouse_all: bool,
    mouse_sgr: bool,
    mouse_utf8: bool,
    wrap: bool,
    insert: bool,
    origin: bool,
    height: usize,
}

impl PaneState {
    fn parse(line: &[u8]) -> Option<Self> {
        let text = String::from_utf8_lossy(line);
        let f: Vec<&str> = text.trim_end().split('\t').collect();
        if f.len() < 21 || !f[0].starts_with('%') {
            return None;
        }
        let n = |i: usize| f[i].parse::<u32>().unwrap_or(0);
        let on = |i: usize| f[i] == "1";
        Some(PaneState {
            pane: f[0].to_string(),
            alternate: on(1),
            cursor: (n(2), n(3)),
            saved_cursor: (n(4), n(5)),
            region: (n(6), n(7)),
            cursor_visible: on(8),
            app_cursor: on(9),
            app_keypad: on(10),
            bracket_paste: on(11),
            mouse_standard: on(12),
            mouse_button: on(13),
            mouse_all: on(14),
            mouse_sgr: on(15),
            mouse_utf8: on(16),
            wrap: on(17),
            insert: on(18),
            origin: on(19),
            height: n(20) as usize,
        })
    }
}

/// 一次补画要的东西：状态和两段画面，三样到齐了才画
#[derive(Default)]
struct Snapshot {
    initial: bool,
    state: Option<PaneState>,
    lines: Vec<Vec<u8>>,
    saved: Vec<Vec<u8>>,
}

/// 把 capture-pane 拉到的画面变成一段终端输出，在连接方的终端里原样还原。
/// initial 时先整个清掉（包括回滚区）再把历史补进去；中途重画只清屏幕，回滚区留着
fn render(snap: &Snapshot) -> Option<Vec<u8>> {
    let state = snap.state.as_ref()?;
    let height = state.height.max(1);
    let split = snap.lines.len().saturating_sub(height);
    let (history, screen) = snap.lines.split_at(split);
    let mut out = Vec::new();
    if snap.initial {
        out.extend_from_slice(CONTROL_MARK);
        out.extend_from_slice(b"\x1bc");
    } else {
        out.extend_from_slice(b"\x1b[?1049l\x1b[r\x1b[0m\x1b[H\x1b[2J");
    }
    if snap.initial {
        for line in history {
            out.extend_from_slice(line);
            out.extend_from_slice(b"\x1b[0m\r\n");
        }
    }
    let put = |out: &mut Vec<u8>, lines: &[Vec<u8>]| {
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                out.extend_from_slice(b"\r\n");
            }
            out.extend_from_slice(line);
            out.extend_from_slice(b"\x1b[0m");
        }
    };
    if state.alternate {
        put(&mut out, &snap.saved);
        let (x, y) = state.saved_cursor;
        out.extend_from_slice(format!("\x1b[{};{}H\x1b[?1049h\x1b[H", y + 1, x + 1).as_bytes());
    }
    put(&mut out, screen);
    let mode = |out: &mut Vec<u8>, code: &str, on: bool| out.extend_from_slice(format!("\x1b[{code}{}", if on { 'h' } else { 'l' }).as_bytes());
    let (upper, lower) = state.region;
    if lower > upper && (upper != 0 || lower as usize + 1 != height) {
        out.extend_from_slice(format!("\x1b[{};{}r", upper + 1, lower + 1).as_bytes());
    }
    mode(&mut out, "?6", state.origin);
    let (x, y) = state.cursor;
    out.extend_from_slice(format!("\x1b[{};{}H", y + 1, x + 1).as_bytes());
    mode(&mut out, "?25", state.cursor_visible);
    mode(&mut out, "?1", state.app_cursor);
    out.extend_from_slice(if state.app_keypad { b"\x1b=" } else { b"\x1b>" });
    mode(&mut out, "?2004", state.bracket_paste);
    // 鼠标模式先关后开：终端收到任何一种鼠标模式的“关”都会把鼠标上报整个关掉，开着的要放在后面
    let mouse = [("?1000", state.mouse_standard), ("?1002", state.mouse_button), ("?1003", state.mouse_all), ("?1006", state.mouse_sgr), ("?1005", state.mouse_utf8)];
    for on in [false, true] {
        for (code, _) in mouse.iter().filter(|(_, m)| *m == on) {
            mode(&mut out, code, on);
        }
    }
    mode(&mut out, "?7", state.wrap);
    mode(&mut out, "4", state.insert);
    Some(out)
}

/// %output 里小于空格的字节和反斜杠写成 \ooo（三位八进制），还原成原样
fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\\' && i + 3 < data.len() && data[i + 1..i + 4].iter().all(|b| (b'0'..=b'7').contains(b)) {
            out.push((data[i + 1] - b'0') * 64 + (data[i + 2] - b'0') * 8 + (data[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

/// `%output %3 数据` 或 `%extended-output %3 积压毫秒 … : 数据`：返回窗格编号和还原后的数据
fn parse_output(line: &[u8]) -> Option<(&[u8], Vec<u8>)> {
    if let Some(rest) = line.strip_prefix(b"%output ") {
        let sp = rest.iter().position(|&b| b == b' ')?;
        return Some((&rest[..sp], unescape(&rest[sp + 1..])));
    }
    let rest = line.strip_prefix(b"%extended-output ")?;
    let sp = rest.iter().position(|&b| b == b' ')?;
    let sep = rest.windows(3).position(|w| w == b" : ")?;
    Some((&rest[..sp], unescape(&rest[sep + 3..])))
}

/// 键盘输入换成 send-keys：按字节写十六进制，中文、控制键、粘贴都原样进去
fn input_commands(name: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in data.chunks(INPUT_CHUNK) {
        out.extend_from_slice(format!("send-keys -t {name} -H").as_bytes());
        for b in chunk {
            out.extend_from_slice(format!(" {b:02x}").as_bytes());
        }
        out.push(b'\n');
    }
    out
}

/// 刚接上：退出遗留的复制模式（旧版本连接方用滚轮翻过历史），查状态，拉历史和画面
fn restore_command(name: &str) -> String {
    format!(
        "copy-mode -q -t {name} ; display -p -t {name} '{STATE_FORMAT}' ; capture-pane -p -e -S -{HISTORY_LINES} -E - -t {name} ; capture-pane -p -e -a -t {name}\n"
    )
}

/// 暂停以后：恢复输出，重画最新画面（不补历史）
fn resync_command(name: &str, pane: &str) -> String {
    format!(
        "refresh-client -A '{pane}:continue' ; display -p -t {name} '{STATE_FORMAT}' ; capture-pane -p -e -t {name} ; capture-pane -p -e -a -t {name}\n"
    )
}

const RESTORE_REPLIES: [Reply; 4] = [Reply::Ignore, Reply::State, Reply::Lines, Reply::Saved];

enum Input {
    Data(Vec<u8>),
    Resize(u16, u16),
}

/// 用控制模式接上 target 这个会话，在 recv/send 上跑到结束。调用前对面已经发过 Open
pub async fn serve_control<R, W>(mut recv: R, mut send: W, cols: u16, rows: u16, target: ControlTarget, session_id: String) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send,
{
    let name = target.name.clone();
    let mut cmd = Command::new(&target.program);
    cmd.args(["-L", &target.namespace, "-C", "attach-session", "-t", &name])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd.env_remove("TMUX").env_remove("PSMUX").env_remove("PSMUX_SESSION_NAME");
    cmd.env("TERM", "xterm-256color").env("COLORTERM", "truecolor").env("TERM_PROGRAM", "P2PCLI");
    if std::env::var_os("LANG").is_none() {
        cmd.env("LANG", "en_US.UTF-8");
    }
    let mut child = cmd.spawn().context("启动会话管理器的控制模式失败")?;
    let pid = child.id();
    tracing::info!(?pid, terminal = %session_id, "终端连接客户端已启动（控制模式）");
    let mut stdin: Option<ChildStdin> = child.stdin.take();
    let stdout = child.stdout.take().context("拿不到控制模式的输出")?;
    if let Some(mut stderr) = child.stderr.take() {
        let session = session_id.clone();
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf).await;
            if !buf.is_empty() {
                tracing::warn!(terminal = %session, stderr = %String::from_utf8_lossy(&buf).trim(), "控制模式输出了错误");
            }
        });
    }

    write_msg(&mut send, &Msg::Opened { shell: "tmux".into(), session_id: session_id.clone() }).await?;

    // tmux 的输出一行一条；单独一个任务读，主循环里收发互不打断。通道小一点，连接方收得慢时 tmux 自己会暂停
    let (line_tx, mut lines) = mpsc::channel::<Vec<u8>>(16);
    tokio::spawn(async move {
        let mut reader = BufReader::with_capacity(64 * 1024, stdout);
        loop {
            let mut line = Vec::new();
            match reader.read_until(b'\n', &mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if line.last() == Some(&b'\n') {
                        line.pop();
                    }
                    if line_tx.send(line).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    // 连接方发来的键盘输入和改尺寸
    let (input_tx, mut inputs) = mpsc::channel::<Input>(64);
    let input_session = session_id.clone();
    let reader = tokio::spawn(async move {
        loop {
            match read_frame(&mut recv).await {
                Ok(Some(Frame::Data(b))) => {
                    if input_tx.send(Input::Data(b.to_vec())).await.is_err() {
                        break;
                    }
                }
                Ok(Some(Frame::Msg(Msg::Resize { cols, rows }))) => {
                    if input_tx.send(Input::Resize(cols, rows)).await.is_err() {
                        break;
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => {
                    tracing::info!(terminal = %input_session, "终端输入流关闭：远端脱离或断线");
                    break;
                }
                Err(error) => {
                    tracing::warn!(terminal = %input_session, %error, "终端输入流异常断开");
                    break;
                }
            }
        }
    });
    let _reader_guard = AbortOnDrop(reader.abort_handle());

    let mut pending: VecDeque<Reply> = VecDeque::new();
    let setup = format!(
        "refresh-client -C {}x{} ; refresh-client -f pause-after={PAUSE_AFTER_SECS} ; {}",
        cols.max(2),
        rows.max(2),
        restore_command(&name)
    );
    write_stdin(&mut stdin, setup.as_bytes()).await;
    pending.extend([Reply::Ignore, Reply::Ignore]);
    pending.extend(RESTORE_REPLIES);
    let mut snapshot = Some(Snapshot { initial: true, ..Default::default() });
    let mut pane = String::new();
    // 收着的一段 %begin…%end：开头那行的编号，和收到的行
    let mut block: Option<(Vec<u8>, Vec<Vec<u8>>)> = None;
    let mut input_done = false;
    let mut deadline: Option<Instant> = None;

    loop {
        tokio::select! {
            line = lines.recv() => {
                let Some(line) = line else { break };
                if let Some((id, collected)) = block.as_mut() {
                    let end = line.strip_prefix(b"%end ").or_else(|| line.strip_prefix(b"%error "));
                    if end.is_some_and(|rest| rest.starts_with(id)) {
                        let ok = line.starts_with(b"%end ");
                        let collected = std::mem::take(collected);
                        let ours = id.ends_with(b" 1");
                        block = None;
                        if !ours {
                            continue;
                        }
                        let Some(reply) = pending.pop_front() else { continue };
                        let Some(snap) = snapshot.as_mut() else { continue };
                        match reply {
                            Reply::Ignore => {}
                            Reply::State => snap.state = if ok { collected.first().and_then(|l| PaneState::parse(l)) } else { None },
                            Reply::Lines => snap.lines = if ok { collected } else { Vec::new() },
                            Reply::Saved => {
                                snap.saved = if ok { collected } else { Vec::new() };
                                let snap = snapshot.take().unwrap();
                                match render(&snap) {
                                    Some(out) => {
                                        pane = snap.state.map(|s| s.pane).unwrap_or_default();
                                        for chunk in out.chunks(32 * 1024) {
                                            write_data(&mut send, chunk).await?;
                                        }
                                    }
                                    None => tracing::warn!(terminal = %session_id, "控制模式读不到窗格状态，画面可能不全"),
                                }
                            }
                        }
                    } else {
                        collected.push(line);
                    }
                    continue;
                }
                if let Some(rest) = line.strip_prefix(b"%begin ") {
                    // 编号是“时间 序号”，结尾的标志位 1 表示是本客户端发的命令
                    block = Some((rest.to_vec(), Vec::new()));
                    continue;
                }
                if let Some((id, data)) = parse_output(&line) {
                    // 补画还没到齐时的输出已经算在画面里了，丢掉；别的窗格的不管
                    if snapshot.is_none() && id == pane.as_bytes() && !data.is_empty() {
                        write_data(&mut send, &data).await?;
                    }
                    continue;
                }
                if let Some(id) = line.strip_prefix(b"%pause ") {
                    if id == pane.as_bytes() && snapshot.is_none() {
                        tracing::info!(terminal = %session_id, "连接方收得慢，跳过积压的输出，重画最新画面");
                        write_stdin(&mut stdin, resync_command(&name, &pane).as_bytes()).await;
                        pending.extend(RESTORE_REPLIES);
                        snapshot = Some(Snapshot::default());
                    }
                    continue;
                }
                if line.starts_with(b"%exit") {
                    tracing::info!(terminal = %session_id, reason = %String::from_utf8_lossy(&line), "控制模式结束");
                }
            }
            input = inputs.recv(), if !input_done => match input {
                Some(Input::Data(bytes)) => write_stdin(&mut stdin, &input_commands(&name, &bytes)).await,
                Some(Input::Resize(c, r)) => {
                    pending.push_back(Reply::Ignore);
                    write_stdin(&mut stdin, format!("refresh-client -C {}x{}\n", c.max(2), r.max(2)).as_bytes()).await;
                }
                None => {
                    // 连接方走了：关掉输入，tmux 的控制客户端自己退出，后台会话留着
                    input_done = true;
                    stdin.take();
                    deadline = Some(Instant::now() + DETACH_GRACE);
                    tracing::info!(terminal = %session_id, "关闭终端连接客户端（不结束后台会话）");
                }
            },
            _ = async { sleep_until(deadline.unwrap()).await }, if deadline.is_some() => {
                let _ = child.start_kill();
                break;
            }
        }
    }
    let code = match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) => status.code().map(|c| c as u32),
        _ => {
            let _ = child.start_kill();
            None
        }
    };
    tracing::info!(terminal = %session_id, ?pid, ?code, "终端连接客户端已退出（控制模式）");
    let _ = write_msg(&mut send, &Msg::Exit { code }).await;
    let _ = send.shutdown().await;
    Ok(())
}

async fn write_stdin(stdin: &mut Option<ChildStdin>, data: &[u8]) {
    if let Some(w) = stdin.as_mut()
        && (w.write_all(data).await.is_err() || w.flush().await.is_err())
    {
        stdin.take();
    }
}

struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_octal() {
        assert_eq!(unescape(b"a\\033[1mb\\134c\\015\\012"), b"a\x1b[1mb\\c\r\n");
        assert_eq!(unescape("中\\011".as_bytes()), "中\t".as_bytes());
        // 结尾不完整的不动
        assert_eq!(unescape(b"x\\01"), b"x\\01");
    }

    #[test]
    fn parses_both_output_forms() {
        let (pane, data) = parse_output(b"%output %3 hi\\015\\012").unwrap();
        assert_eq!((pane, data.as_slice()), (&b"%3"[..], &b"hi\r\n"[..]));
        let (pane, data) = parse_output(b"%extended-output %0 12 : a : b").unwrap();
        assert_eq!((pane, data.as_slice()), (&b"%0"[..], &b"a : b"[..]));
        assert!(parse_output(b"%layout-change @0 x").is_none());
    }

    #[test]
    fn input_is_hex_in_chunks() {
        let cmds = input_commands("s", "é\r".as_bytes());
        assert_eq!(cmds, b"send-keys -t s -H c3 a9 0d\n");
        let long = vec![b'a'; INPUT_CHUNK + 1];
        assert_eq!(input_commands("s", &long).iter().filter(|&&b| b == b'\n').count(), 2);
    }

    #[test]
    fn renders_history_screen_and_modes() {
        let state = PaneState::parse(b"%0\t0\t2\t1\t0\t0\t0\t1\t1\t1\t0\t1\t0\t0\t0\t1\t0\t1\t0\t0\t2").unwrap();
        let snap = Snapshot { initial: true, state: Some(state), lines: vec![b"old".to_vec(), b"$ ls".to_vec(), b"$ ".to_vec()], saved: vec![] };
        let out = String::from_utf8(render(&snap).unwrap()).unwrap();
        assert!(out.starts_with("\x1b]7737;p2pcli-control\x07\x1bc"));
        assert!(out.contains("old\x1b[0m\r\n$ ls\x1b[0m\r\n$ \x1b[0m"));
        assert!(out.contains("\x1b[2;3H"));
        assert!(out.contains("\x1b[?1h") && out.contains("\x1b[?2004h") && out.contains("\x1b[?1006h"));
        assert!(out.find("\x1b[?1006h").unwrap() > out.find("\x1b[?1003l").unwrap(), "开着的鼠标模式要在关的后面");
        assert!(!out.contains("\x1b[?1049h"));
    }

    #[test]
    fn renders_alternate_screen_over_saved_one() {
        let state = PaneState::parse(b"%0\t1\t0\t0\t3\t4\t0\t1\t1\t0\t0\t0\t0\t0\t0\t0\t0\t1\t0\t0\t2").unwrap();
        let snap = Snapshot { initial: false, state: Some(state), lines: vec![b"vim".to_vec(), b"~".to_vec()], saved: vec![b"$ vim".to_vec()] };
        let out = String::from_utf8(render(&snap).unwrap()).unwrap();
        let saved = out.find("$ vim").unwrap();
        let switch = out.find("\x1b[5;4H\x1b[?1049h").unwrap();
        assert!(saved < switch && switch < out.find("vim\x1b[0m\r\n~").unwrap());
        assert!(!out.contains(std::str::from_utf8(CONTROL_MARK).unwrap()));
    }
}
