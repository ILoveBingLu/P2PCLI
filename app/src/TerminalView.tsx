// 一个远程终端标签页：xterm.js 画面，字节经 Rust 端走 iroh 连接
import { useEffect, useRef } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";
import type { TermStatus, OpenedTerminal } from "./api";

const isMac = navigator.platform.toLowerCase().includes("mac");

const theme = {
  background: "#111114",
  foreground: "#e6e8ee",
  cursor: "#e6e8ee",
  selectionBackground: "#8b7cf84d",
  black: "#1b1f2a",
  red: "#f87171",
  green: "#34d399",
  yellow: "#fbbf24",
  blue: "#60a5fa",
  magenta: "#c084fc",
  cyan: "#22d3ee",
  white: "#d1d5db",
  brightBlack: "#4b5563",
  brightRed: "#fca5a5",
  brightGreen: "#6ee7b7",
  brightYellow: "#fde68a",
  brightBlue: "#93c5fd",
  brightMagenta: "#d8b4fe",
  brightCyan: "#67e8f9",
  brightWhite: "#f9fafb",
};

interface Props {
  connId: number;
  sessionId?: string;
  active: boolean;
  onStatus: (s: TermStatus) => void;
  onReady: (termId: number, sessionId: string) => void;
}

export function TerminalView({ connId, sessionId, active, onStatus, onReady }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const cb = useRef({ onStatus, onReady });
  cb.current = { onStatus, onReady };

  useEffect(() => {
    const el = box.current!;
    const term = new Terminal({
      fontFamily: '"SF Mono", Menlo, Monaco, "Cascadia Mono", Consolas, "PingFang SC", "Microsoft YaHei UI", monospace',
      fontSize: 13,
      lineHeight: 1.15,
      cursorBlink: true,
      scrollback: 10000,
      allowProposedApi: true,
      macOptionIsMeta: true,
      theme,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new Unicode11Addon());
    term.unicode.activeVersion = "11";
    term.open(el);
    try {
      const gl = new WebglAddon();
      gl.onContextLoss(() => gl.dispose());
      term.loadAddon(gl);
    } catch {
      // 没有 WebGL 就用默认渲染
    }
    termRef.current = term;
    fitRef.current = fit;
    window.__ptTest?.terms.push(term);
    if (el.offsetWidth > 0) fit.fit();

    let termId: number | null = null;
    let disposed = false;
    const send = (data: string, binary = false) => {
      if (termId != null) invoke("term_input", { termId, data, binary });
    };

    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown") return true;
      // Shift+回车换行，Claude Code 和 Codex 都认 ESC+回车
      if (e.key === "Enter" && e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey) {
        send("\x1b\r");
        return false;
      }
      if (!isMac && e.ctrlKey && !e.altKey) {
        const k = e.key.toLowerCase();
        // Windows 习惯：有选中时 Ctrl+C 复制，Ctrl+V 粘贴
        if (k === "c" && (e.shiftKey || term.hasSelection())) {
          navigator.clipboard.writeText(term.getSelection());
          term.clearSelection();
          return false;
        }
        if (k === "v") return false;
      }
      return true;
    });

    const output = new Channel<ArrayBuffer>();
    output.onmessage = (buf) => term.write(new Uint8Array(buf));
    const status = new Channel<TermStatus>();
    status.onmessage = (s) => cb.current.onStatus(s);

    invoke<OpenedTerminal>("open_terminal", { connId, sessionId: sessionId ?? null, cols: term.cols, rows: term.rows, output, status })
      .then(({ term_id: id, session_id }) => {
        if (disposed) {
          invoke("term_close", { termId: id });
          return;
        }
        termId = id;
        cb.current.onReady(id, session_id);
        // 打开期间窗口可能变过大小
        invoke("term_resize", { termId: id, cols: term.cols, rows: term.rows });
      })
      .catch((err) => {
        term.write(`\r\n\x1b[31m打开终端失败：${err}\x1b[0m\r\n`);
        cb.current.onStatus({ type: "closed", reason: String(err) });
      });

    const subs = [
      term.onData((d) => send(d)),
      term.onBinary((d) => send(d, true)),
      term.onResize(({ cols, rows }) => {
        if (termId != null) invoke("term_resize", { termId, cols, rows });
      }),
    ];
    const ro = new ResizeObserver(() => {
      if (el.offsetWidth > 0 && el.offsetHeight > 0) fit.fit();
    });
    ro.observe(el);

    return () => {
      disposed = true;
      ro.disconnect();
      subs.forEach((s) => s.dispose());
      if (termId != null) invoke("term_close", { termId });
      term.dispose();
    };
  // sessionId identifies this attachment at mount; onReady publishes the assigned ID.
  }, [connId]);

  useEffect(() => {
    if (!active) return;
    const id = requestAnimationFrame(() => {
      if (box.current && box.current.offsetWidth > 0) fitRef.current?.fit();
      termRef.current?.focus();
    });
    return () => cancelAnimationFrame(id);
  }, [active]);

  return <div className="term" ref={box} style={{ display: active ? "block" : "none" }} />;
}
