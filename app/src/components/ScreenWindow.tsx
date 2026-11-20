// 远程画面的单独窗口：每台设备一个，终端右侧悬浮条里的显示器按钮打开。
// 画面在对方电脑上用硬件编码（H.264），这里用 WebCodecs 交给显卡解码，解出来的帧直接画到 canvas 上。
// 键盘按物理位置（KeyboardEvent.code）发过去，对方按它自己的键盘布局出字；鼠标坐标按画面的 0~1 发。
// 画面只走直连：连接落到中继上时对方暂停发画面，终端照常能用，直连恢复后自动接着放。
import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Expand, Keyboard, LoaderCircle, Monitor, MousePointer2, MousePointerClick, RefreshCw, Shrink } from "lucide-react";
import {
  api,
  type ScreenDisplay,
  type ScreenInput,
  type PathInfo,
  type RemoteInfo,
  type ScreenCursorShape,
  type ScreenMouseButton,
  type ScreenQuality,
  type ScreenStarted,
  type ScreenStreamEvent,
  type ScreenVideoConfig,
  type ScreenViewerMsg,
} from "../api";
import { Button } from "@/components/ui/button";
import { ConnectionCard, Row, Section } from "./ConnectionCard";
import { t } from "@/i18n";

/** 隔这么久问一次这台设备现在用哪条连接（断开、重连都能跟上） */
const CONNECTION_POLL_MS = 2000;

type Phase =
  | { kind: "waiting" } // 还没连上这台设备
  | { kind: "asking" } // 等对方同意
  | { kind: "live" }
  | { kind: "error"; message: string }
  | { kind: "ended"; message: string | null };

const isMac = navigator.userAgent.includes("Mac");
// 和主窗口一样：标题栏自己画，给系统原生的窗口按钮留位置
const nativeMac = isTauri() && /Macintosh/.test(navigator.userAgent);
const nativeWindows = isTauri() && /Windows/.test(navigator.userAgent);

function b64(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

const BUTTONS: ScreenMouseButton[] = ["left", "middle", "right", "back", "forward"];

/** 对方指针的样子做成本机能用的指针：按屏幕点的大小（Retina 的 2 倍图缩回 1 倍），热点也换算过去 */
interface CursorLook { url: string; width: number; height: number; hotX: number; hotY: number; css: string }

function cursorLook(s: ScreenCursorShape): CursorLook | null {
  try {
    const full = document.createElement("canvas");
    full.width = s.width;
    full.height = s.height;
    full.getContext("2d")!.putImageData(new ImageData(new Uint8ClampedArray(b64(s.rgba)), s.width, s.height), 0, 0);
    const scale = s.scale > 0 ? s.scale : 1;
    const width = Math.max(1, Math.round(s.width / scale));
    const height = Math.max(1, Math.round(s.height / scale));
    const small = document.createElement("canvas");
    small.width = width;
    small.height = height;
    const ctx = small.getContext("2d")!;
    ctx.imageSmoothingQuality = "high";
    ctx.drawImage(full, 0, 0, width, height);
    const url = small.toDataURL("image/png");
    const hotX = Math.min(width - 1, Math.max(0, Math.round(s.hot_x / scale)));
    const hotY = Math.min(height - 1, Math.max(0, Math.round(s.hot_y / scale)));
    return { url, width, height, hotX, hotY, css: `url(${url}) ${hotX} ${hotY}, default` };
  } catch {
    return null;
  }
}

/** 后端的错误文字带着“relay:”“denied:”这样的前缀，给人看时去掉 */
function readable(message: string) {
  return message.replace(/^(relay|denied|permission):\s*/, "");
}

export default function ScreenWindow() {
  const [device, setDevice] = useState<{ endpointId: string; name: string; platform: string } | null>(null);
  const [connId, setConnId] = useState<number | null>(null);
  const [phase, setPhase] = useState<Phase>({ kind: "waiting" });
  const [started, setStarted] = useState<ScreenStarted | null>(null);
  const [display, setDisplay] = useState<string | null>(null);
  const [paused, setPaused] = useState<"relay" | "connecting" | null>(null);
  const [stats, setStats] = useState<{ fps: number; bitrate: number } | null>(null);
  const [quality, setQuality] = useState<ScreenQuality>("balanced");
  const [control, setControl] = useState(true);
  const [swapMeta, setSwapMeta] = useState(true);
  const [fullscreen, setFullscreen] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [hwNote, setHwNote] = useState<string | null>(null);
  // 连接详情：对方信息、线路，解码器在用的格式
  const [info, setInfo] = useState<RemoteInfo | null>(null);
  const [path, setPath] = useState<PathInfo | null>(null);
  const [decode, setDecode] = useState<{ codec: string; width: number; height: number; hardware: boolean } | null>(null);
  // 对方单独报来的指针：样子和位置。对方是老版本时收不到，那时指针画在画面里
  const [look, setLook] = useState<CursorLook | null>(null);
  const [remoteCursor, setRemoteCursor] = useState<{ x: number; y: number; visible: boolean } | null>(null);
  const lookRef = useRef<CursorLook | null>(null);
  lookRef.current = look;
  const remoteCursorRef = useRef(remoteCursor);
  remoteCursorRef.current = remoteCursor;
  const insideRef = useRef(false);
  // 窗口大小变了，只看时画的对方指针要跟着挪
  const [, setLayoutTick] = useState(0);
  useEffect(() => {
    const onResize = () => setLayoutTick((n) => n + 1);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const canvasRef = useRef<HTMLCanvasElement>(null);
  const screenId = useRef<number | null>(null);
  const decoder = useRef<VideoDecoder | null>(null);
  const needKey = useRef(true);
  const queue = useRef<ScreenViewerMsg[]>([]);
  const pendingMove = useRef<ScreenInput | null>(null);
  const pendingWheel = useRef<{ dx: number; dy: number; x: number; y: number } | null>(null);
  const flushScheduled = useRef(false);
  const controlRef = useRef(control);
  controlRef.current = control && !!started?.control;
  const swapRef = useRef(swapMeta);
  swapRef.current = swapMeta;

  // 自己是哪台设备的
  useEffect(() => {
    void api.screenWindowInfo().then((info) => setDevice({ endpointId: info.endpoint_id, name: info.device_name, platform: info.platform })).catch(() => {});
  }, []);

  // 这台设备现在用哪条连接
  useEffect(() => {
    if (!device) return;
    let stop = false;
    let current: number | null = null;
    const check = () => api.screenConnection(device.endpointId, current).then((id) => {
      if (stop) return;
      current = id ?? null;
      setConnId(current);
    }).catch(() => {});
    void check();
    const timer = window.setInterval(check, CONNECTION_POLL_MS);
    const unlisten = listen("remote-closed", () => void check());
    return () => { stop = true; window.clearInterval(timer); void unlisten.then((u) => u()); };
  }, [device]);

  // 标题栏连接按钮上的线路和延时：每 2 秒取一次；对方信息换连接时取一次
  useEffect(() => {
    if (connId == null) { setPath(null); return; }
    let stop = false;
    void api.remoteInfo(connId).then((i) => { if (!stop) setInfo(i); }).catch(() => {});
    const tick = () => api.pathInfo(connId).then((p) => { if (!stop) setPath(p); }).catch(() => {});
    void tick();
    const timer = window.setInterval(tick, 2000);
    return () => { stop = true; window.clearInterval(timer); };
  }, [connId]);

  useEffect(() => {
    if (!device) return;
    void getCurrentWindow().setTitle(t("{{device}} — 远程画面", { device: device.name })).catch(() => {});
  }, [device]);

  // ---------------------------------------------------------------- 发消息：鼠标移动和滚轮按帧合并，其他的按顺序马上发

  const flush = useCallback(() => {
    flushScheduled.current = false;
    const id = screenId.current;
    const msgs = queue.current;
    if (pendingMove.current) { msgs.push({ t: "input", ev: pendingMove.current }); pendingMove.current = null; }
    if (pendingWheel.current) { msgs.push({ t: "input", ev: { k: "wheel", ...pendingWheel.current } }); pendingWheel.current = null; }
    queue.current = [];
    if (id == null || msgs.length === 0) return;
    void api.screenSend(id, msgs).catch(() => {});
  }, []);

  const send = useCallback((msg: ScreenViewerMsg, coalesce = false) => {
    if (!coalesce) {
      // 先把攒着的移动发掉，保证顺序：先移到那儿再按下
      if (pendingMove.current) { queue.current.push({ t: "input", ev: pendingMove.current }); pendingMove.current = null; }
      queue.current.push(msg);
      flush();
      return;
    }
    if (!flushScheduled.current) {
      flushScheduled.current = true;
      requestAnimationFrame(flush);
    }
  }, [flush]);

  // ---------------------------------------------------------------- 解码

  const resetDecoder = useCallback(() => {
    try { decoder.current?.close(); } catch { /* 已经关了 */ }
    decoder.current = null;
    needKey.current = true;
  }, []);

  const draw = useCallback((frame: VideoFrame) => {
    const canvas = canvasRef.current;
    if (canvas) {
      if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
        canvas.width = frame.displayWidth;
        canvas.height = frame.displayHeight;
      }
      canvas.getContext("2d")?.drawImage(frame, 0, 0);
    }
    frame.close();
  }, []);

  const configure = useCallback(async (config: ScreenVideoConfig) => {
    resetDecoder();
    if (typeof VideoDecoder === "undefined") {
      setPhase({ kind: "error", message: t("这台电脑的系统版本太旧，不支持硬件解码画面，请更新系统") });
      return;
    }
    const base: VideoDecoderConfig = {
      codec: config.codec,
      codedWidth: config.width,
      codedHeight: config.height,
      description: b64(config.description),
      optimizeForLatency: true,
      // 两边编码器出的都是电脑屏幕的颜色：sRGB 色域和曲线、BT.709 矩阵、视频范围。
      // 明确告诉解码器，不让它按电视的 BT.709 曲线去猜，颜色和对方屏幕上一致
      colorSpace: { primaries: "bt709", transfer: "iec61966-2-1", matrix: "bt709", fullRange: false },
    };
    // 先要显卡解码，不行再让系统自己挑
    let chosen: VideoDecoderConfig = { ...base, hardwareAcceleration: "prefer-hardware" };
    try {
      const support = await VideoDecoder.isConfigSupported(chosen);
      if (!support.supported) {
        chosen = base;
        setHwNote(t("显卡不支持这个画面，改用系统解码"));
      } else {
        setHwNote(null);
      }
    } catch {
      chosen = base;
    }
    setDecode({ codec: config.codec, width: config.width, height: config.height, hardware: chosen.hardwareAcceleration === "prefer-hardware" });
    const d = new VideoDecoder({
      output: draw,
      error: () => {
        // 解码出错：重建解码器，要一个关键帧
        resetDecoder();
        send({ t: "key_frame" });
      },
    });
    try {
      d.configure(chosen);
    } catch (e) {
      setPhase({ kind: "error", message: t("解码器配置失败：{{message}}", { message: String(e) }) });
      return;
    }
    decoder.current = d;
    needKey.current = true;
  }, [draw, resetDecoder, send]);

  // 配置解码器要先异步问显卡支不支持；这期间紧跟着到的关键帧先存着，配好再按顺序喂进去，不然会一直等不到关键帧
  const configuring = useRef<Promise<void>>(Promise.resolve());
  const configPending = useRef(0);
  const held = useRef<Extract<ScreenStreamEvent, { t: "frame" }>[]>([]);
  const lastKeyAsk = useRef(0);

  const decodeFrame = useCallback((ev: Extract<ScreenStreamEvent, { t: "frame" }>) => {
    const d = decoder.current;
    if (!d || d.state !== "configured" || (needKey.current && !ev.key)) {
      // 解不了这帧：跟对方要一个关键帧，一秒最多要一次
      if (Date.now() - lastKeyAsk.current > 1000) {
        lastKeyAsk.current = Date.now();
        send({ t: "key_frame" });
      }
      return;
    }
    needKey.current = false;
    try {
      d.decode(new EncodedVideoChunk({ type: ev.key ? "key" : "delta", timestamp: ev.seq * 1000, data: ev.data }));
    } catch {
      resetDecoder();
      send({ t: "key_frame" });
    }
  }, [resetDecoder, send]);

  const onEvent = useCallback((ev: ScreenStreamEvent) => {
    switch (ev.t) {
      case "config":
        configPending.current += 1;
        configuring.current = configuring.current.then(async () => {
          await configure(ev.config);
          configPending.current -= 1;
          if (configPending.current === 0) {
            const frames = held.current;
            held.current = [];
            frames.forEach(decodeFrame);
          }
        });
        return;
      case "frame":
        if (configPending.current > 0) {
          held.current.push(ev);
          return;
        }
        decodeFrame(ev);
        return;
      case "host":
        if (ev.msg.t === "paused") setPaused(ev.msg.reason);
        else if (ev.msg.t === "resumed") setPaused(null);
        else if (ev.msg.t === "switched") setDisplay(ev.msg.display);
        else if (ev.msg.t === "stats") setStats({ fps: ev.msg.fps, bitrate: ev.msg.bitrate });
        else if (ev.msg.t === "cursor") setRemoteCursor({ x: ev.msg.x, y: ev.msg.y, visible: ev.msg.visible });
        else if (ev.msg.t === "cursor_shape") setLook(cursorLook(ev.msg.shape));
        return;
      case "closed":
        screenId.current = null;
        resetDecoder();
        setStats(null);
        setPhase({ kind: "ended", message: ev.reason ? readable(ev.reason) : null });
        return;
    }
  }, [configure, decodeFrame, resetDecoder]);

  // ---------------------------------------------------------------- 开始看：连上设备就开，断了等重连再开

  useEffect(() => {
    if (connId == null) {
      if (screenId.current != null) setPhase({ kind: "ended", message: t("和这台设备的连接断开了，重新连上后会自动接着看") });
      else setPhase({ kind: "waiting" });
      return;
    }
    let cancelled = false;
    void (async () => {
      const ok = await api.remoteSupportsScreen(connId).catch(() => false);
      if (cancelled) return;
      if (!ok) {
        setPhase({ kind: "error", message: t("对方的 P2PCLI 版本较旧或系统不支持远程画面，请更新对方的 P2PCLI") });
        return;
      }
      setPhase({ kind: "asking" });
      try {
        const opened = await api.screenOpen(connId, quality, true, display, onEvent);
        if (cancelled) { void api.screenClose(opened.screen_id); return; }
        screenId.current = opened.screen_id;
        setStarted(opened.started);
        setDisplay(opened.started.display);
        setPaused(null);
        setPhase({ kind: "live" });
        // 画面拿到焦点，直接就能打字
        requestAnimationFrame(() => canvasRef.current?.focus());
      } catch (e) {
        if (!cancelled) setPhase({ kind: "error", message: readable(String(e)) });
      }
    })();
    return () => {
      cancelled = true;
      if (screenId.current != null) void api.screenClose(screenId.current);
      screenId.current = null;
      resetDecoder();
    };
    // 画质、屏幕在看的过程中换，不重新开
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connId, attempt]);

  // 关窗口时停掉
  useEffect(() => {
    const stop = () => { if (screenId.current != null) void api.screenClose(screenId.current); };
    window.addEventListener("beforeunload", stop);
    return () => window.removeEventListener("beforeunload", stop);
  }, []);

  // ---------------------------------------------------------------- 键鼠

  /** 鼠标在画面上的位置（0~1）；画面按比例缩放居中，四周黑边外的不算 */
  const pos = useCallback((e: { clientX: number; clientY: number }) => {
    const canvas = canvasRef.current;
    if (!canvas || !canvas.width) return null;
    const r = canvas.getBoundingClientRect();
    const scale = Math.min(r.width / canvas.width, r.height / canvas.height);
    const w = canvas.width * scale, h = canvas.height * scale;
    const left = r.left + (r.width - w) / 2, top = r.top + (r.height - h) / 2;
    const x = (e.clientX - left) / w, y = (e.clientY - top) / h;
    return { x: Math.min(1, Math.max(0, x)), y: Math.min(1, Math.max(0, y)), inside: x >= 0 && x <= 1 && y >= 0 && y <= 1 };
  }, []);

  /** 画面上的本机指针：操作时换成对方指针的样子（跟手，零延迟）；对方藏起指针时也藏；
   * 对方是老版本（指针画在画面里）就藏起本机的，免得两个指针；在四周黑边上、只看时用普通箭头 */
  const applyCursor = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    let css = "default";
    if (controlRef.current && insideRef.current) {
      const remote = remoteCursorRef.current;
      if (!remote) css = "none";
      else if (!remote.visible) css = "none";
      else css = lookRef.current?.css ?? "default";
    }
    if (canvas.style.cursor !== css) canvas.style.cursor = css;
  }, []);
  useEffect(applyCursor, [applyCursor, look, remoteCursor?.visible, control, started]);

  const onPointerMove = (e: React.PointerEvent) => {
    const p = pos(e);
    if (p && insideRef.current !== p.inside) {
      insideRef.current = p.inside;
      applyCursor();
    }
    if (!controlRef.current || !p) return;
    pendingMove.current = { k: "move", x: p.x, y: p.y };
    send({ t: "input", ev: pendingMove.current }, true);
  };
  const onPointerButton = (e: React.PointerEvent, down: boolean) => {
    if (!controlRef.current) return;
    const p = pos(e);
    if (!p || (down && !p.inside)) return;
    const button = BUTTONS[e.button];
    if (!button) return;
    e.preventDefault();
    if (down) { (e.target as HTMLElement).setPointerCapture?.(e.pointerId); canvasRef.current?.focus(); }
    send({ t: "input", ev: { k: "button", button, down, x: p.x, y: p.y } });
  };

  // 滚轮要用非被动监听才能拦住本窗口自己滚动
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onWheel = (e: WheelEvent) => {
      if (!controlRef.current) return;
      e.preventDefault();
      const p = pos(e);
      if (!p) return;
      const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? canvas.clientHeight : 1;
      const w = pendingWheel.current ?? { dx: 0, dy: 0, x: p.x, y: p.y };
      w.dx += e.deltaX * unit; w.dy += e.deltaY * unit; w.x = p.x; w.y = p.y;
      pendingWheel.current = w;
      send({ t: "input", ev: { k: "wheel", ...w } }, true);
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", onWheel);
  }, [pos, send, phase.kind]);

  /** 发出去按下、还没发抬起的键 */
  const pressed = useRef(new Set<string>());
  /** 窗口或画面失焦、切到只看时：对面按着的键和鼠标全松开 */
  const releaseAll = useCallback(() => {
    pressed.current.clear();
    if (screenId.current != null) send({ t: "input", ev: { k: "release_all" } });
  }, [send]);

  // 键盘：画面获得焦点时所有键都发过去（包括 Tab、退格），本窗口不处理
  useEffect(() => {
    const remoteMac = device?.platform === "macos";
    const mapCode = (code: string) => {
      // 两边系统不同时，⌘ 和 Ctrl 对调：在 Mac 上按 ⌘C 到 Windows 上就是 Ctrl+C，反过来也一样
      if (!swapRef.current || isMac === remoteMac) return code;
      const swap: Record<string, string> = { MetaLeft: "ControlLeft", MetaRight: "ControlRight", ControlLeft: "MetaLeft", ControlRight: "MetaRight" };
      return swap[code] ?? code;
    };
    const onKey = (e: KeyboardEvent, down: boolean) => {
      if (!e.code || e.code === "Unidentified") return;
      const code = mapCode(e.code);
      if (down) {
        if (!controlRef.current || document.activeElement !== canvasRef.current) return;
        pressed.current.add(code);
      } else {
        // 抬起只要按下时发过就一定发，焦点挪走了也发，免得对面一直按着
        if (!pressed.current.delete(code)) return;
        // macOS 上按着 ⌘ 时其它键抬起收不到：松开 ⌘ 时把一起按着的键都松开
        if (e.code === "MetaLeft" || e.code === "MetaRight") {
          for (const other of [...pressed.current]) {
            if (!/^(Shift|Control|Alt|Meta)/.test(other)) {
              pressed.current.delete(other);
              send({ t: "input", ev: { k: "key", code: other, down: false } });
            }
          }
        }
      }
      e.preventDefault();
      e.stopPropagation();
      send({ t: "input", ev: { k: "key", code, down } });
    };
    const kd = (e: KeyboardEvent) => onKey(e, true);
    const ku = (e: KeyboardEvent) => onKey(e, false);
    window.addEventListener("keydown", kd, true);
    window.addEventListener("keyup", ku, true);
    window.addEventListener("blur", releaseAll);
    const canvas = canvasRef.current;
    canvas?.addEventListener("blur", releaseAll);
    return () => {
      window.removeEventListener("keydown", kd, true);
      window.removeEventListener("keyup", ku, true);
      window.removeEventListener("blur", releaseAll);
      canvas?.removeEventListener("blur", releaseAll);
    };
  }, [device, send, releaseAll]);

  // 切到“只看”：按着的键和鼠标全松开
  useEffect(() => {
    if (!control) releaseAll();
  }, [control, releaseAll]);

  // ---------------------------------------------------------------- 工具栏

  const changeQuality = (q: ScreenQuality) => {
    setQuality(q);
    if (screenId.current != null) send({ t: "set_quality", quality: q });
  };
  const changeDisplay = (d: ScreenDisplay) => {
    if (d.id === display) return;
    setDisplay(d.id);
    if (screenId.current != null) send({ t: "switch", display: d.id });
  };
  const toggleFullscreen = async () => {
    const w = getCurrentWindow();
    const next = !(await w.isFullscreen());
    await w.setFullscreen(next);
    setFullscreen(next);
  };
  const reconnect = () => setAttempt((n) => n + 1);

  const platformsDiffer = !!device?.platform && (device.platform === "macos") !== isMac;
  const live = phase.kind === "live";
  const canControl = !!started?.control;

  return (
    <div className="screen-window" data-fullscreen={fullscreen ? "" : undefined}>
      <div className={`screen-toolbar${nativeMac ? " titlebar-macos" : ""}${nativeWindows ? " titlebar-windows" : ""}`} data-tauri-drag-region>
        <div className="screen-toolbar-title" data-tauri-drag-region>
          <Monitor />
          <span className="truncate">{device?.name ?? ""}</span>
        </div>
        {connId != null && info && (
          <ConnectionCard connId={connId} info={info} path={path ?? undefined} placement="bottom end" className="screen-path-button"
            extra={live ? (
              <Section>
                <Row label={t("画面")} value={paused === "relay" ? t("已暂停（连接走中继）") : paused ? t("正在找直连线路") : decode ? `${decode.width} × ${decode.height}` : t("统计中…")} />
                <Row label={t("帧率")} value={stats && !paused ? t("{{fps}} 帧/秒", { fps: stats.fps }) : "—"} />
                <Row label={t("画面码率")} value={stats && !paused ? `${(stats.bitrate / 1e6).toFixed(1)} Mbps` : "—"} hint={t("对方编码器现在用的码率，网络差时会自动往下降")} />
                <Row label={t("编码")} value={decode ? `H.264 · ${decode.codec}` : "—"} mono />
                <Row label={t("解码")} value={decode ? (decode.hardware && !hwNote ? t("显卡硬件解码") : t("系统解码")) : "—"} />
              </Section>
            ) : undefined} />
        )}
        {live && started && started.displays.length > 1 && (
          <div className="screen-segment" role="group" aria-label={t("屏幕")}>
            {started.displays.map((d) => (
              <button key={d.id} type="button" data-active={d.id === display ? "" : undefined} onClick={() => changeDisplay(d)}
                title={`${d.name} · ${d.width}×${d.height}`}>{d.name}</button>
            ))}
          </div>
        )}
        {live && (
          <div className="screen-segment" role="group" aria-label={t("画质")}>
            {([["sharp", t("清晰")], ["balanced", t("均衡")], ["smooth", t("流畅")]] as const).map(([q, label]) => (
              <button key={q} type="button" data-active={quality === q ? "" : undefined} onClick={() => changeQuality(q)}>{label}</button>
            ))}
          </div>
        )}
        {live && (
          <Button variant="ghost" size="icon-sm" className="screen-tool"
            disabled={!canControl}
            title={!canControl ? (started?.control_note ?? t("对方只允许查看")) : control ? t("正在操作对方电脑，点一下改成只看") : t("只看不操作，点一下开始操作")}
            aria-label={control ? t("操作") : t("只看")}
            onPress={() => setControl((c) => !c)}>
            {control && canControl ? <MousePointerClick /> : <MousePointer2 />}
          </Button>
        )}
        {live && canControl && platformsDiffer && (
          <Button variant="ghost" size="icon-sm" className="screen-tool" data-active={swapMeta ? "" : undefined}
            title={swapMeta ? t("⌘ 和 Ctrl 已对调（按本机习惯用快捷键），点一下改回原样") : t("⌘ 和 Ctrl 按原样发送，点一下对调")}
            aria-label={t("对调 ⌘ 和 Ctrl")}
            onPress={() => setSwapMeta((v) => !v)}>
            <Keyboard />
          </Button>
        )}
        <Button variant="ghost" size="icon-sm" className="screen-tool" title={fullscreen ? t("退出全屏") : t("全屏")} aria-label={t("全屏")} onPress={() => void toggleFullscreen()}>
          {fullscreen ? <Shrink /> : <Expand />}
        </Button>
      </div>
      <div className="screen-stage">
        <canvas ref={canvasRef} className="screen-canvas" tabIndex={0}
          data-control={live && control && canControl ? "" : undefined}
          onPointerMove={onPointerMove}
          onPointerDown={(e) => onPointerButton(e, true)}
          onPointerUp={(e) => onPointerButton(e, false)}
          onPointerLeave={() => { insideRef.current = false; applyCursor(); }}
          onContextMenu={(e) => e.preventDefault()} />
        {live && !(control && canControl) && look && remoteCursor?.visible && (() => {
          // 只看时按对方报来的位置画它的指针（画面按比例缩放、四周有黑边，要换算）
          const canvas = canvasRef.current;
          if (!canvas || !canvas.width) return null;
          const r = canvas.getBoundingClientRect();
          const stage = canvas.parentElement!.getBoundingClientRect();
          const scale = Math.min(r.width / canvas.width, r.height / canvas.height);
          const w = canvas.width * scale, h = canvas.height * scale;
          const left = r.left - stage.left + (r.width - w) / 2 + remoteCursor.x * w - look.hotX;
          const top = r.top - stage.top + (r.height - h) / 2 + remoteCursor.y * h - look.hotY;
          return <img className="screen-remote-cursor" src={look.url} alt="" width={look.width} height={look.height} style={{ left, top }} />;
        })()}
        {phase.kind !== "live" && (
          <div className="screen-overlay">
            {phase.kind === "waiting" && <><LoaderCircle className="animate-spin" /><p>{t("正在等这台设备连上…")}</p><p className="screen-overlay-note">{t("在主窗口里连上这台设备后，画面会自动打开")}</p></>}
            {phase.kind === "asking" && <><LoaderCircle className="animate-spin" /><p>{t("正在请对方允许查看画面…")}</p><p className="screen-overlay-note">{t("对方的受信设备直接打开；别的设备要对方在弹窗里点允许")}</p></>}
            {(phase.kind === "error" || phase.kind === "ended") && (
              <>
                <Monitor />
                <p>{phase.kind === "ended" && !phase.message ? t("画面已结束") : phase.message}</p>
                {connId != null && <Button variant="outline" onPress={reconnect}><RefreshCw data-icon="inline-start" />{t("重新打开画面")}</Button>}
              </>
            )}
          </div>
        )}
        {live && paused && (
          <div className="screen-overlay screen-overlay-soft">
            <LoaderCircle className="animate-spin" />
            <p>{paused === "relay" ? t("连接现在走的是中继，画面暂停了") : t("正在找直连线路…")}</p>
            <p className="screen-overlay-note">{t("远程画面只走直连，中继只给终端用。终端照常能用，直连恢复后画面自动接着放")}</p>
          </div>
        )}
      </div>
    </div>
  );
}
