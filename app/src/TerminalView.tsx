// 一个远程终端标签页：xterm.js 画面，字节经 Rust 端走 iroh 连接
import { useEffect, useRef, type MouseEvent } from "react";
import { ClipboardPaste, Copy, TextSelect } from "lucide-react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { Terminal } from "@xterm/xterm";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";
import type { TermStatus, OpenedTerminal } from "./api";
import { useContextMenu } from "./components/ContextMenu";
import { toast } from "./components/ui/toast";
import { terminalResizeFrame } from "./lib/terminalResizeFrame";
import { emptyShellPrompt, promptGuard } from "./lib/launcherPrompt";

const isMac = navigator.platform.toLowerCase().includes("mac");

/**
 * 按容器实际宽高算行列，铺满不留边。xterm 6 的滚动条是浮在内容上的，
 * 官方 FitAddon 还按老办法给它扣掉 14px，右边会空出一条，所以自己算。
 * 读字符尺寸用的是 xterm 内部接口，FitAddon 也是这么读的；xterm 版本锁死，升级时要重看。
 */
function fitTerminal(term: Terminal, el: HTMLElement, beforeResize?: () => void) {
  const core = (term as unknown as {
    _core: { _renderService: { dimensions: { css: { cell: { width: number; height: number } } } } };
  })._core;
  const cell = core._renderService.dimensions.css.cell;
  if (!cell.width || !cell.height || el.clientWidth === 0 || el.clientHeight === 0) return;
  const cols = Math.max(2, Math.floor(el.clientWidth / cell.width));
  const rows = Math.max(1, Math.floor(el.clientHeight / cell.height));
  if (cols === term.cols && rows === term.rows) return;
  beforeResize?.();
  // resize 自己会使渲染缓存失效，不要提前清空仍在显示的画布。
  term.resize(cols, rows);
}

const theme = {
  background: "#111114",
  foreground: "#e6e8ee",
  // 悬浮滚动条：半透明，不滚的时候自己淡出
  scrollbarSliderBackground: "#ffffff1f",
  scrollbarSliderHoverBackground: "#ffffff38",
  scrollbarSliderActiveBackground: "#ffffff52",
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

/** 终端对外的操作：底部输入框靠它把整段文字发给终端里的 AI */
export interface TerminalHandle {
  /** 当成粘贴发过去：对面程序开了整段粘贴就整段发，多行不会被当成几次回车 */
  paste: (text: string) => void;
  /** 按一下回车 */
  enter: () => void;
  /** 原样发几个按键，比如 Shift+Tab 是 \x1b[Z */
  keys: (data: string) => void;
  focus: () => void;
  canLaunch: () => boolean;
}

interface Props {
  connId: number;
  sessionId?: string;
  active: boolean;
  onStatus: (s: TermStatus) => void;
  onReady: (termId: number, sessionId: string) => void;
  onHandle?: (handle: TerminalHandle | null) => void;
  onPromptReady?: (ready: boolean) => void;
}

export function TerminalView({ connId, sessionId, active, onStatus, onReady, onHandle, onPromptReady }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const resizeFrameRef = useRef<ReturnType<typeof terminalResizeFrame> | null>(null);
  const inputReady = useRef(false);
  const menu = useContextMenu();
  const cb = useRef({ onStatus, onReady, onHandle, onPromptReady });
  cb.current = { onStatus, onReady, onHandle, onPromptReady };

  const openContextMenu = (e: MouseEvent<HTMLDivElement>) => {
    // 在捕获阶段拦截，避免 xterm 的隐藏输入框弹出 WebView 原生菜单。
    e.preventDefault();
    e.stopPropagation();
    const term = termRef.current;
    if (!term) return;
    // 菜单获取焦点前保存选区，复制不受菜单打开后的焦点变化影响。
    const selected = term.getSelection();
    const focus = () => { if (termRef.current === term) term.focus(); };
    menu.open(e, [
      {
        id: "copy", label: "复制", icon: <Copy className="size-4" />, disabled: !selected,
        onAction: () => {
          void navigator.clipboard.writeText(selected)
            .catch((error) => toast.error(`复制失败：${String(error)}`))
            .finally(focus);
        },
      },
      {
        id: "paste", label: "粘贴", icon: <ClipboardPaste className="size-4" />, disabled: !inputReady.current,
        onAction: () => {
          void navigator.clipboard.readText().then((text) => {
            if (termRef.current === term && inputReady.current) term.paste(text);
          }).catch((error) => toast.error(`粘贴失败：${String(error)}`)).finally(focus);
        },
      },
      {
        id: "select-all", label: "全选", icon: <TextSelect className="size-4" />, separated: true,
        onAction: () => { term.selectAll(); focus(); },
      },
    ]);
  };

  useEffect(() => {
    if (!active) menu.close();
  }, [active, menu.close]);

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
    term.loadAddon(new Unicode11Addon());
    term.unicode.activeVersion = "11";
    term.open(el);
    try {
      // 保留上一帧，只有改尺寸时才复制，用于遮住远端重绘的中间画面。
      const gl = new WebglAddon(true);
      gl.onContextLoss(() => gl.dispose());
      term.loadAddon(gl);
    } catch {
      // 没有 WebGL 就用默认渲染
    }
    termRef.current = term;
    window.__ptTest?.terms.push(term);
    fitTerminal(term, el);
    const resizeFrame = terminalResizeFrame(term, el);
    resizeFrameRef.current = resizeFrame;

    let termId: number | null = null;
    let disposed = false;
    let resizeTimer: number | undefined;
    const guard = promptGuard();
    const prompt = () => {
      const buffer = term.buffer.active;
      const row = buffer.baseY + buffer.cursorY;
      const line = buffer.getLine(row);
      const before = line?.translateToString(false, 0, buffer.cursorX) ?? "";
      const after = line?.translateToString(true, buffer.cursorX) ?? "";
      return {
        signature: `${buffer.type}:${row}:${buffer.cursorX}:${line?.translateToString(true) ?? ""}`,
        empty: inputReady.current && buffer.type === "normal" && !line?.isWrapped && !after.trim() && emptyShellPrompt(before),
      };
    };
    const canLaunch = () => { const p = prompt(); return guard.ready(p.signature, p.empty); };
    const reportPrompt = () => cb.current.onPromptReady?.(canLaunch());
    const send = (data: string, binary = false) => {
      guard.input(prompt().signature);
      cb.current.onPromptReady?.(false);
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

    cb.current.onHandle?.({
      paste: (text) => {
        // xterm 按对面程序有没有开整段粘贴（bracketed paste）来包；没开的话换行用 ESC+回车，Claude Code 和 Codex 都当换行
        if (term.modes.bracketedPasteMode) term.paste(text);
        else send(text.replace(/\r?\n/g, "\x1b\r"));
      },
      enter: () => send("\r"),
      keys: (data) => send(data),
      focus: () => term.focus(),
      canLaunch,
    });

    const output = new Channel<ArrayBuffer>();
    output.onmessage = (buf) => { if (!disposed) resizeFrame.write(new Uint8Array(buf)); };
    const status = new Channel<TermStatus>();
    status.onmessage = (s) => {
      if (!disposed) { inputReady.current = false; cb.current.onPromptReady?.(false); cb.current.onStatus(s); }
    };

    // Let StrictMode's setup/cleanup rehearsal finish before creating a persistent session.
    const openTimer = window.setTimeout(() => {
      if (disposed) return;
      void invoke<OpenedTerminal>("open_terminal", { connId, sessionId: sessionId ?? null, cols: term.cols, rows: term.rows, output, status })
      .then(({ term_id: id, session_id }) => {
        if (disposed) {
          invoke("term_close", { termId: id });
          return;
        }
        termId = id;
        inputReady.current = true;
        reportPrompt();
        cb.current.onReady(id, session_id);
        // 打开期间窗口可能变过大小
        invoke("term_resize", { termId: id, cols: term.cols, rows: term.rows });
      })
      .catch((err) => {
        if (disposed) return;
        term.write(`\r\n\x1b[31m打开终端失败：${err}\x1b[0m\r\n`);
        cb.current.onStatus({ type: "closed", reason: String(err) });
      });
    }, 0);

    const subs = [
      term.onWriteParsed(reportPrompt),
      term.onData((d) => send(d)),
      term.onBinary((d) => send(d, true)),
      term.onResize(({ cols, rows }) => {
        // 本地已等布局稳定才调整行列，立即同步远端，避免延迟后再重画一轮。
        if (termId != null) invoke("term_resize", { termId, cols, rows });
      }),
    ];
    const ro = new ResizeObserver(() => {
      // 输入框动画会逐帧改变容器高度。合并本地 fit，而不只是合并远端通知，
      // 否则 xterm 会在动画期间多次清画布、重排行，然后再收到远端重绘。
      window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => {
        if (!disposed) fitTerminal(term, el, resizeFrame.begin);
      }, 120);
    });
    ro.observe(el);

    return () => {
      disposed = true;
      cb.current.onHandle?.(null);
      cb.current.onPromptReady?.(false);
      inputReady.current = false;
      if (termRef.current === term) termRef.current = null;
      window.clearTimeout(openTimer);
      window.clearTimeout(resizeTimer);
      ro.disconnect();
      resizeFrame.dispose();
      if (resizeFrameRef.current === resizeFrame) resizeFrameRef.current = null;
      subs.forEach((s) => s.dispose());
      if (termId != null) invoke("term_close", { termId });
      term.dispose();
    };
  // sessionId identifies this attachment at mount; onReady publishes the assigned ID.
  }, [connId]);

  useEffect(() => {
    if (!active) return;
    const id = requestAnimationFrame(() => {
      if (box.current && termRef.current) fitTerminal(termRef.current, box.current, resizeFrameRef.current?.begin);
      termRef.current?.focus();
    });
    return () => cancelAnimationFrame(id);
  }, [active]);

  return <>
    <div className="term" ref={box} style={{ display: active ? "block" : "none" }}
      onContextMenuCapture={openContextMenu} />
    {active && menu.element}
  </>;
}
