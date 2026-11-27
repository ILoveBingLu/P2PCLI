// 远程画面的单独窗口：每台设备一个，终端右侧悬浮条里的显示器按钮打开。
// 画面在对方电脑上用硬件编码（H.264），这里用 WebCodecs 交给显卡解码，解出来的帧直接画到 canvas 上。
// 键盘按物理位置（KeyboardEvent.code）发过去，对方按它自己的键盘布局出字；鼠标坐标按画面的 0~1 发。
// 画面只走直连：连接落到中继上时对方暂停发画面，终端照常能用，直连恢复后自动接着放。
import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ChevronDown, ClipboardCopy, Crosshair, Expand, Keyboard, LoaderCircle, Monitor, MousePointer2, MousePointerClick, RefreshCw, Send, Shield, ShieldCheck, Shrink, Type, Upload, Volume2, VolumeX, X } from "lucide-react";
import { PcmPlayer } from "./audioPlayer";
import {
  api,
  type ScreenDisplay,
  type ScreenInput,
  type PathInfo,
  type RemoteInfo,
  type ScreenCodec,
  type ScreenCursorShape,
  type ScreenEncoderInfo,
  type ScreenMouseButton,
  type ScreenQuality,
  type ScreenStarted,
  type ScreenStreamEvent,
  type ScreenVideoConfig,
  type ScreenViewerMsg,
} from "../api";
import { Button } from "@/components/ui/button";
import { Toaster, toast } from "@/components/ui/toast";
import { ConnectionCard, Row, Section } from "./ConnectionCard";
import { t } from "@/i18n";

/** 这台电脑的屏幕是广色域（Display P3） */
const WIDE_GAMUT = typeof matchMedia === "function" && matchMedia("(color-gamut: p3)").matches;

/** 隔这么久问一次这台设备现在用哪条连接（断开、重连都能跟上） */
const CONNECTION_POLL_MS = 2000;
/** 鼠标移动和滚轮最多这么久合并发一次（约一帧） */
const INPUT_FLUSH_MS = 16;

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

/** 本机显卡能硬件解码的格式。H.265 只在显卡能解时才要（软件解 H.265 太费 CPU），H.264 总是能用 */
let codecsProbe: Promise<ScreenCodec[]> | null = null;
function supportedCodecs(): Promise<ScreenCodec[]> {
  codecsProbe ??= (async () => {
    if (typeof VideoDecoder === "undefined") return ["h264"];
    try {
      const s = await VideoDecoder.isConfigSupported({ codec: "hvc1.1.6.L123.B0", codedWidth: 1920, codedHeight: 1080, hardwareAcceleration: "prefer-hardware" });
      return s.supported ? ["h265", "h264"] : ["h264"];
    } catch {
      return ["h264"];
    }
  })();
  return codecsProbe;
}

/**
 * 显卡能不能解真正的 4:4:4（H.265 Range Extensions 档次，每个像素都有自己的颜色）。能才请对方用 4:4:4；
 * 不能就老老实实 4:2:0，不拿别的办法凑。解码器嘴上说能、实际解不了时记下来（见 hevc444Broken），这次运行里不再要
 */
let hevc444Probe: Promise<boolean> | null = null;
let hevc444Broken = false;
function supportsHevc444(): Promise<boolean> {
  hevc444Probe ??= (async () => {
    if (typeof VideoDecoder === "undefined") return false;
    if (!(await supportedCodecs()).includes("h265")) return false;
    // 档次 4（Range Extensions），几种常见的等级写法都试一下
    // 第一个是 NVIDIA 编码器实际出的 4:4:4 码流的写法（8 位、4:4:4 的限制标志），后面几个是别的常见写法
    for (const codec of ["hvc1.4.10.L123.9E.8", "hvc1.4.10.L153.9E.8", "hvc1.4.10.L123.90", "hev1.4.10.L123.90"]) {
      try {
        const s = await VideoDecoder.isConfigSupported({ codec, codedWidth: 1920, codedHeight: 1080, hardwareAcceleration: "prefer-hardware" });
        if (s.supported) return true;
      } catch { /* 下一个 */ }
    }
    return false;
  })();
  return hevc444Probe.then((ok) => ok && !hevc444Broken);
}

/** 解码出来的画面像素格式是不是 4:2:0 的（解码器把 4:4:4 降了）；不知道（显存里的画面）时是 null */
function is420Format(format: VideoPixelFormat | null): boolean | null {
  if (!format) return null;
  return format.startsWith("I420") || format.startsWith("NV12");
}

/** 解码配置里的 codec 字符串是哪种格式 */
function codecName(codec: string) {
  return codec.startsWith("hvc1") || codec.startsWith("hev1") ? "H.265" : "H.264";
}

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

/** 传输速率：小于 1 Mbps 时用 kbps，不然一直显示 0.0 */
function formatMbps(mbps: number) {
  return mbps < 1 ? `${Math.round(mbps * 1000)} kbps` : `${mbps.toFixed(1)} Mbps`;
}

/** 后端的错误文字带着“relay:”“denied:”这样的前缀，给人看时去掉 */
function readable(message: string) {
  return message.replace(/^(relay|denied|permission):\s*/, "");
}

/** 远程控制时和对方互通剪贴板，默认开；关掉的人记在本机 */
const CLIPBOARD_KEY = "p2pcli.screen.clipboard";
function loadShareClipboard() {
  try { return localStorage.getItem(CLIPBOARD_KEY) !== "off"; } catch { return true; }
}
const AUDIO_KEY = "p2pcli.screen.audio";
function loadAudioOn() {
  try { return localStorage.getItem(AUDIO_KEY) !== "off"; } catch { return true; }
}

export default function ScreenWindow() {
  const [device, setDevice] = useState<{ endpointId: string; name: string; platform: string } | null>(null);
  const [connId, setConnId] = useState<number | null>(null);
  const [phase, setPhase] = useState<Phase>({ kind: "waiting" });
  const [started, setStarted] = useState<ScreenStarted | null>(null);
  const [display, setDisplay] = useState<string | null>(null);
  const [paused, setPaused] = useState<"relay" | "connecting" | null>(null);
  /** 隐私屏：对方说现在盖没盖着；想不想盖（重连以后照着它再盖上）；等对方回话 */
  const [privacy, setPrivacy] = useState(false);
  const privacyWant = useRef(false);
  const [privacyBusy, setPrivacyBusy] = useState(false);
  const [stats, setStats] = useState<{ fps: number; bitrate: number; target?: number; encoder?: ScreenEncoderInfo; encodeMs?: number } | null>(null);
  const [quality, setQuality] = useState<ScreenQuality>("balanced");
  const [control, setControl] = useState(true);
  const [swapMeta, setSwapMeta] = useState(true);
  const [shareClipboard, setShareClipboard] = useState(loadShareClipboard);
  /** 放对方的声音；关掉时对方也不录了 */
  const [audioOn, setAudioOn] = useState(loadAudioOn);
  const audioOnRef = useRef(audioOn);
  audioOnRef.current = audioOn;
  /** 浏览器不让自动出声（这个窗口还没被点过）：提示点一下 */
  const [audioBlocked, setAudioBlocked] = useState(false);
  const player = useRef<PcmPlayer | null>(null);
  const [fullscreen, setFullscreen] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [hwNote, setHwNote] = useState<string | null>(null);
  // 连接详情：对方信息、线路，解码器在用的格式
  const [info, setInfo] = useState<RemoteInfo | null>(null);
  const [path, setPath] = useState<PathInfo | null>(null);
  /** 解码器在解什么：chroma444 是对方说码流是 4:4:4；format 是解出来第一帧的像素格式（看解码器有没有偷偷降成 4:2:0） */
  const [decode, setDecode] = useState<{ codec: string; width: number; height: number; hardware: boolean; chroma444: boolean; format?: VideoPixelFormat | null } | null>(null);
  const formatSeen = useRef(false);
  const chroma444Ref = useRef(false);
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
  /** 锁定鼠标时攒着的相对移动和滚轮，和普通的移动一样按帧合并 */
  const pendingRel = useRef<{ dx: number; dy: number } | null>(null);
  const pendingWheelHere = useRef<{ dx: number; dy: number } | null>(null);
  /** 鼠标锁定在画面里（玩游戏用）：发移动量，不发位置 */
  const [locked, setLocked] = useState(false);
  const lockedRef = useRef(false);
  lockedRef.current = locked;
  /** 送文字的小框 */
  const [textOpen, setTextOpen] = useState(false);
  const [textValue, setTextValue] = useState("");
  /** 拖着文件进窗口时的提示 */
  const [dropHint, setDropHint] = useState(false);
  // 延迟和首帧：收到每帧的时间（按序号），本机从收到到画出来的平均耗时，对方同意以后多久出第一帧
  const recvAt = useRef(new Map<number, number>());
  const decodeMs = useRef<number | null>(null);
  const [decodeShown, setDecodeShown] = useState<number | null>(null);
  const liveAt = useRef<number | null>(null);
  const firstFrameDone = useRef(false);
  const [firstFrameMs, setFirstFrameMs] = useState<number | null>(null);
  /** 对方同意了，过了一会儿还没收到第一帧：多半是对方显示器关了或在睡眠 */
  const [noFrame, setNoFrame] = useState(false);
  const flushScheduled = useRef(false);
  const lastFlush = useRef(0);
  const controlRef = useRef(control);
  controlRef.current = control && !!started?.control;
  const swapRef = useRef(swapMeta);
  swapRef.current = swapMeta;
  /** 对方替我们转换快捷键：键原样发，开关告诉对方；老版本的对方由本机对调 ⌘ 和 Ctrl */
  const translateRef = useRef(false);
  translateRef.current = !!started?.translate_keys;
  /** 现在互通剪贴板：自己开着、在操作、对方也同意 */
  const clipboardRef = useRef(false);
  clipboardRef.current = shareClipboard && controlRef.current && !!started?.clipboard;
  /** 下一次发消息前先把本机剪贴板发过去 */
  const syncClipboard = useRef(false);

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
    lastFlush.current = performance.now();
    const id = screenId.current;
    const msgs = queue.current;
    if (pendingMove.current) { msgs.push({ t: "input", ev: pendingMove.current }); pendingMove.current = null; }
    if (pendingWheel.current) { msgs.push({ t: "input", ev: { k: "wheel", ...pendingWheel.current } }); pendingWheel.current = null; }
    if (pendingRel.current) { msgs.push({ t: "input", ev: { k: "relative_move", ...pendingRel.current } }); pendingRel.current = null; }
    if (pendingWheelHere.current) { msgs.push({ t: "input", ev: { k: "wheel_here", ...pendingWheelHere.current } }); pendingWheelHere.current = null; }
    queue.current = [];
    const clipboard = syncClipboard.current;
    syncClipboard.current = false;
    if (id == null || (msgs.length === 0 && !clipboard)) return;
    void api.screenSend(id, msgs, clipboard).catch(() => {});
  }, []);

  const send = useCallback((msg: ScreenViewerMsg, coalesce = false) => {
    if (!coalesce) {
      // 先把攒着的移动发掉，保证顺序：先移到那儿再按下
      if (pendingMove.current) { queue.current.push({ t: "input", ev: pendingMove.current }); pendingMove.current = null; }
      queue.current.push(msg);
      flush();
      return;
    }
    // 按 16 毫秒合并一次。不用 requestAnimationFrame：窗口被系统当成不可见、不出新帧时它根本不触发，
    // 移动和滚轮就一直攒着发不出去（画面照样在动，因为画面是解码回调画的），对面只收到点击
    if (!flushScheduled.current) {
      flushScheduled.current = true;
      window.setTimeout(flush, Math.max(0, INPUT_FLUSH_MS - (performance.now() - lastFlush.current)));
    }
  }, [flush]);

  // ---------------------------------------------------------------- 解码

  const resetDecoder = useCallback(() => {
    try { decoder.current?.close(); } catch { /* 已经关了 */ }
    decoder.current = null;
    needKey.current = true;
  }, []);

  // 本机实际收到的：每秒多少帧、多少字节。对方报的码率是编码器的目标（上限），画面不动时实际传得少得多
  const recvFrames = useRef(0);
  const recvBytes = useRef(0);
  const [recv, setRecv] = useState<{ fps: number; mbps: number } | null>(null);
  useEffect(() => {
    let last = performance.now();
    const timer = window.setInterval(() => {
      const now = performance.now();
      const secs = (now - last) / 1000;
      if (screenId.current != null) {
        setRecv({ fps: Math.round(recvFrames.current / secs), mbps: recvBytes.current * 8 / secs / 1e6 });
        setDecodeShown(decodeMs.current);
        // 对方同意 5 秒了还一帧都没有：提示可能是显示器关着
        if (!firstFrameDone.current && liveAt.current != null && now - liveAt.current > 5000) setNoFrame(true);
      }
      recvFrames.current = 0;
      recvBytes.current = 0;
      last = now;
    }, 1000);
    return () => window.clearInterval(timer);
  }, []);

  // 诊断：解出了多少帧、出错几次，每 5 秒报给后端记日志
  const decodedCount = useRef(0);
  const errorCount = useRef(0);
  const hardwareRef = useRef(false);
  useEffect(() => {
    let last = performance.now();
    const timer = window.setInterval(() => {
      const id = screenId.current;
      const now = performance.now();
      if (id != null) {
        void api.screenReport(id, decodedCount.current, errorCount.current, decoder.current?.decodeQueueSize ?? 0, hardwareRef.current, (now - last) / 1000).catch(() => {});
      }
      decodedCount.current = 0;
      errorCount.current = 0;
      last = now;
    }, 5000);
    return () => window.clearInterval(timer);
  }, []);

  const draw = useCallback((frame: VideoFrame) => {
    decodedCount.current += 1;
    // 从收到这帧到画出来用了多久（平滑一下）；时间戳就是序号 × 1000
    const now = performance.now();
    const seq = Math.round(frame.timestamp / 1000);
    const at = recvAt.current.get(seq);
    if (at != null) {
      recvAt.current.delete(seq);
      decodeMs.current = decodeMs.current == null ? now - at : decodeMs.current * 0.9 + (now - at) * 0.1;
    }
    if (!firstFrameDone.current && liveAt.current != null) {
      firstFrameDone.current = true;
      setFirstFrameMs(Math.round(now - liveAt.current));
      setNoFrame(false);
    }
    // 记下解出来的像素格式：诊断面板按实际解出来的说 4:4:4 还是 4:2:0
    if (!formatSeen.current) {
      formatSeen.current = true;
      const format = frame.format;
      setDecode((d) => (d ? { ...d, format } : d));
    }
    const canvas = canvasRef.current;
    if (canvas) {
      if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
        canvas.width = frame.displayWidth;
        canvas.height = frame.displayHeight;
      }
      // 画布按屏幕的色域开：广色域屏幕上 P3 的画面不会被压回 sRGB；sRGB 的画面浏览器自己换算
      canvas.getContext("2d", { colorSpace: WIDE_GAMUT ? "display-p3" : "srgb" })?.drawImage(frame, 0, 0);
    }
    frame.close();
  }, []);

  const configure = useCallback(async (config: ScreenVideoConfig) => {
    resetDecoder();
    if (typeof VideoDecoder === "undefined") {
      setPhase({ kind: "error", message: t("这台电脑的系统版本太旧，不支持硬件解码画面，请更新系统") });
      return;
    }
    const chroma444 = config.chroma444 === true;
    chroma444Ref.current = chroma444;
    formatSeen.current = false;
    const base: VideoDecoderConfig = {
      codec: config.codec,
      codedWidth: config.width,
      codedHeight: config.height,
      description: b64(config.description),
      optimizeForLatency: true,
      // 电脑屏幕的颜色：sRGB 曲线、BT.709 矩阵；范围和色域按对方解码配置里标的（老版本不标，是视频范围、sRGB）。
      // 明确告诉解码器，不让它按电视的 BT.709 曲线去猜，颜色和对方屏幕上一致
      colorSpace: {
        primaries: (config.primaries === "p3" ? "smpte432" : "bt709") as VideoColorPrimaries,
        transfer: "iec61966-2-1",
        matrix: "bt709",
        fullRange: config.full_range === true,
      },
    };
    // 先要显卡解码，不行再让系统自己挑
    let chosen: VideoDecoderConfig = { ...base, hardwareAcceleration: "prefer-hardware" };
    try {
      const support = await VideoDecoder.isConfigSupported(chosen);
      if (!support.supported && chroma444) {
        // 探测时说能解 4:4:4，真到这个码流又不行：记下来，重新打开画面，这次只要 4:2:0
        hevc444Broken = true;
        setAttempt((n) => n + 1);
        return;
      }
      if (!support.supported) {
        chosen = base;
        setHwNote(t("显卡不支持这个画面，改用系统解码"));
      } else {
        setHwNote(null);
      }
    } catch {
      chosen = base;
    }
    setDecode({ codec: config.codec, width: config.width, height: config.height, hardware: chosen.hardwareAcceleration === "prefer-hardware", chroma444 });
    hardwareRef.current = chosen.hardwareAcceleration === "prefer-hardware";
    const d = new VideoDecoder({
      output: draw,
      error: () => {
        // 4:4:4 一帧都没解出来就出错：这块显卡其实解不了，重新打开画面，这次只要 4:2:0
        if (chroma444Ref.current && !formatSeen.current) {
          hevc444Broken = true;
          resetDecoder();
          setAttempt((n) => n + 1);
          return;
        }
        // 解码出错：重建解码器，要一个关键帧
        errorCount.current += 1;
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
    recvAt.current.set(ev.seq, performance.now());
    // 解码出错丢掉的帧不会画出来，别一直攒着
    if (recvAt.current.size > 240) recvAt.current.delete(recvAt.current.keys().next().value!);
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
      case "audio": {
        if (!player.current) {
          player.current = new PcmPlayer();
          player.current.onBlocked = setAudioBlocked;
        }
        player.current.setMuted(!audioOnRef.current);
        player.current.push(ev.pcm);
        return;
      }
      case "frame":
        recvFrames.current += 1;
        recvBytes.current += ev.data.byteLength;
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
        else if (ev.msg.t === "stats") setStats({ fps: ev.msg.fps, bitrate: ev.msg.bitrate, target: ev.msg.target_fps, encoder: ev.msg.encoder, encodeMs: ev.msg.encode_us ? ev.msg.encode_us / 1000 : undefined });
        else if (ev.msg.t === "cursor") setRemoteCursor({ x: ev.msg.x, y: ev.msg.y, visible: ev.msg.visible });
        else if (ev.msg.t === "cursor_shape") setLook(cursorLook(ev.msg.shape));
        else if (ev.msg.t === "privacy") {
          setPrivacy(ev.msg.on);
          setPrivacyBusy(false);
          if (ev.msg.note) toast.notice(ev.msg.note);
        }
        return;
      case "notice":
        // 剪贴板内容走中继传：中继只是兜底，会比较慢
        if (ev.kind === "clip_relay") toast.notice(t("剪贴板里的内容正在经中继传，会比较慢；直连接通后会自动切过去"));
        return;
      case "closed":
        screenId.current = null;
        resetDecoder();
        setStats(null);
        setRecv(null);
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
        const [codecs, chroma444] = await Promise.all([supportedCodecs(), supportsHevc444()]);
        if (cancelled) return;
        const opened = await api.screenOpen(connId, quality, true, display, codecs, chroma444, isMac ? "mac" : "pc", swapRef.current, audioOnRef.current, onEvent);
        if (cancelled) { void api.screenClose(opened.screen_id); return; }
        screenId.current = opened.screen_id;
        setStarted(opened.started);
        setDisplay(opened.started.display);
        // 重连以后对方那边的隐私屏已经撤了：之前开着就再盖上
        setPrivacy(false);
        setPrivacyBusy(false);
        if (privacyWant.current && opened.started.privacy && opened.started.control) {
          setPrivacyBusy(true);
          void api.screenSend(opened.screen_id, [{ t: "privacy", on: true }]).catch(() => setPrivacyBusy(false));
        }
        if (opened.started.clipboard && !shareClipboard) void api.screenSetClipboard(opened.screen_id, false).catch(() => {});
        setPaused(null);
        liveAt.current = performance.now();
        firstFrameDone.current = false;
        setFirstFrameMs(null);
        setNoFrame(false);
        decodeMs.current = null;
        recvAt.current.clear();
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
  // 换了画布（切到 4:4:4 时整块换掉）也要重新设
  useEffect(applyCursor, [applyCursor, look, remoteCursor?.visible, control, started]);

  /** 锁定鼠标时一个 CSS 像素算对方屏幕上几个像素：按画面显示的大小和对方屏幕的实际像素换算 */
  const remotePerCss = () => {
    const canvas = canvasRef.current;
    const d = started?.displays.find((x) => x.id === display);
    if (!canvas || !canvas.width) return 1;
    const r = canvas.getBoundingClientRect();
    const scale = Math.min(r.width / canvas.width, r.height / canvas.height);
    const shownWidth = canvas.width * scale;
    return shownWidth > 0 ? (d?.width ?? canvas.width) / shownWidth : 1;
  };

  const onPointerMove = (e: React.PointerEvent) => {
    if (lockedRef.current) {
      if (!controlRef.current) return;
      const k = remotePerCss();
      const r = pendingRel.current ?? { dx: 0, dy: 0 };
      r.dx += e.movementX * k; r.dy += e.movementY * k;
      pendingRel.current = r;
      send({ t: "input", ev: { k: "relative_move", ...r } }, true);
      return;
    }
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
    if (lockedRef.current) {
      const button = BUTTONS[e.button];
      if (!button) return;
      e.preventDefault();
      send({ t: "input", ev: { k: "button_here", button, down } });
      return;
    }
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
      const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? canvas.clientHeight : 1;
      if (lockedRef.current) {
        const w = pendingWheelHere.current ?? { dx: 0, dy: 0 };
        w.dx += e.deltaX * unit; w.dy += e.deltaY * unit;
        pendingWheelHere.current = w;
        send({ t: "input", ev: { k: "wheel_here", ...w } }, true);
        return;
      }
      const p = pos(e);
      if (!p) return;
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

  const toggleRef = useRef<() => Promise<void>>(async () => {});
  // 键盘：画面获得焦点时所有键都发过去（包括 Tab、退格），本窗口不处理
  useEffect(() => {
    const remoteMac = device?.platform === "macos";
    const mapCode = (code: string) => {
      // 两边系统不同时，⌘ 和 Ctrl 对调：在 Mac 上按 ⌘C 到 Windows 上就是 Ctrl+C，反过来也一样
      if (translateRef.current || !swapRef.current || isMac === remoteMac) return code;
      const swap: Record<string, string> = { MetaLeft: "ControlLeft", MetaRight: "ControlRight", ControlLeft: "MetaLeft", ControlRight: "MetaRight" };
      return swap[code] ?? code;
    };
    const onKey = (e: KeyboardEvent, down: boolean) => {
      if (!e.code || e.code === "Unidentified") return;
      // 进出全屏的快捷键本机自己处理，不发给对方：macOS ⌃⌘F，别的系统 F11
      if (isMac ? e.code === "KeyF" && e.metaKey && e.ctrlKey : e.code === "F11") {
        e.preventDefault();
        e.stopPropagation();
        if (down && !e.repeat) void toggleRef.current();
        return;
      }
      const code = mapCode(e.code);
      if (down) {
        if (!controlRef.current || document.activeElement !== canvasRef.current) return;
        pressed.current.add(code);
        // 粘贴键（Ctrl/⌘+V、Shift+Insert）：马上看一眼本机剪贴板，刚复制的先过去，对方读到的就是它
        if (clipboardRef.current && (e.code === "KeyV" || e.code === "Insert")) syncClipboard.current = true;
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

  // 浏览器要用户点过、按过键才让出声：窗口里随便点一下就开
  useEffect(() => {
    const wake = () => player.current?.resume();
    window.addEventListener("pointerdown", wake, true);
    window.addEventListener("keydown", wake, true);
    return () => {
      window.removeEventListener("pointerdown", wake, true);
      window.removeEventListener("keydown", wake, true);
      player.current?.close();
      player.current = null;
    };
  }, []);
  const toggleAudio = () => {
    const on = !audioOn;
    setAudioOn(on);
    try { localStorage.setItem(AUDIO_KEY, on ? "on" : "off"); } catch { /* 存不了就只管这次 */ }
    player.current?.setMuted(!on);
    if (on) player.current?.resume();
    if (screenId.current != null) void api.screenSetAudio(screenId.current, on).catch(() => {});
  };
  const hasAudio = !!started?.audio_token;

  const toggleShareClipboard = () => {
    const on = !shareClipboard;
    setShareClipboard(on);
    try { localStorage.setItem(CLIPBOARD_KEY, on ? "on" : "off"); } catch { /* 存不了就只管这次 */ }
    if (screenId.current != null) void api.screenSetClipboard(screenId.current, on).catch(() => {});
  };

  // 切到“只看”：按着的键和鼠标全松开
  useEffect(() => {
    if (!control) releaseAll();
  }, [control, releaseAll]);

  // ---------------------------------------------------------------- 锁定鼠标、送文字、拖文件

  // 锁定鼠标：系统按 Esc 解锁，这里跟着系统的状态走
  useEffect(() => {
    const onChange = () => setLocked(!!canvasRef.current && document.pointerLockElement === canvasRef.current);
    const onError = () => toast.error(t("这个窗口锁定不了鼠标"));
    document.addEventListener("pointerlockchange", onChange);
    document.addEventListener("pointerlockerror", onError);
    return () => {
      document.removeEventListener("pointerlockchange", onChange);
      document.removeEventListener("pointerlockerror", onError);
    };
  }, []);
  const canExtraInput = phase.kind === "live" && control && !!started?.control && !!started?.input_v2;
  // 不能操作了（切到只看、对方断了）就解锁
  useEffect(() => {
    if (!canExtraInput && document.pointerLockElement) document.exitPointerLock();
  }, [canExtraInput]);
  const toggleLock = () => {
    if (locked) { document.exitPointerLock(); return; }
    const canvas = canvasRef.current;
    if (!canvas) return;
    canvas.focus();
    try {
      const r = canvas.requestPointerLock() as unknown;
      if (r instanceof Promise) r.then(() => toast.notice(t("鼠标锁定在画面里了，按 Esc 退出"))).catch(() => toast.error(t("这个窗口锁定不了鼠标")));
      else toast.notice(t("鼠标锁定在画面里了，按 Esc 退出"));
    } catch {
      toast.error(t("这个窗口锁定不了鼠标"));
    }
  };

  const sendText = () => {
    const text = textValue;
    if (!text || !canExtraInput) return;
    send({ t: "input", ev: { k: "text", text } });
    setTextValue("");
    setTextOpen(false);
    requestAnimationFrame(() => canvasRef.current?.focus());
  };

  // 拖文件进画面窗口：发给这台设备，发完告诉对方电脑上的路径
  const deviceRef = useRef(device);
  deviceRef.current = device;
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    const timers: number[] = [];
    void getCurrentWebview().onDragDropEvent(async (event) => {
      const e = event.payload;
      const target = deviceRef.current;
      if (e.type === "leave") return setDropHint(false);
      if (e.type === "enter" || e.type === "over") return setDropHint(!!target);
      setDropHint(false);
      if (!target || !e.paths.length) return;
      try {
        const id = await api.sendFiles(target.endpointId, e.paths);
        toast.notice(t("正在发给 {{name}}…", { name: target.name }));
        // 发完了报一声：对方电脑上存在哪
        const check = window.setInterval(() => {
          void api.transfers().then((list) => {
            const x = list.find((tr) => tr.id === id);
            if (!x || x.finished_at == null) return;
            window.clearInterval(check);
            if (x.state === "done") toast.success(t("已发到 {{name}}：{{path}}", { name: target.name, path: x.paths.length > 1 ? (x.dir ?? x.paths[0]) : (x.paths[0] ?? x.dir ?? "") }));
            else toast.error(t("没发成：{{error}}", { error: x.error ?? x.state }));
          }).catch(() => {});
        }, 1000);
        timers.push(check);
      } catch (error) {
        toast.error(t("发不了：{{error}}", { error: String(error) }));
      }
    }).then((u) => { if (disposed) u(); else unlisten = u; }).catch(() => {});
    return () => { disposed = true; unlisten?.(); timers.forEach((x) => window.clearInterval(x)); };
  }, []);

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
  // macOS 的简单全屏系统不认作全屏（isFullscreen 是假），自己记着
  const fullscreenRef = useRef(false);
  fullscreenRef.current = fullscreen;
  const ownFullscreen = useRef(false);
  const toggleFullscreen = useCallback(async () => {
    const on = await api.screenSetFullscreen(!fullscreenRef.current).catch(() => fullscreenRef.current);
    ownFullscreen.current = on;
    setFullscreen(on);
  }, []);
  // 按 macOS 的绿灯进出系统全屏也要跟上
  useEffect(() => {
    const w = getCurrentWindow();
    let alive = true;
    const unlisten = w.onResized(() => {
      void w.isFullscreen().then((v) => { if (alive) setFullscreen(ownFullscreen.current || v); }).catch(() => {});
    });
    return () => { alive = false; void unlisten.then((f) => f()); };
  }, []);

  // 全屏时工具栏收起来，画面铺满；鼠标碰到屏幕顶端、或者碰到顶部正中的小拉手才滑下来，移开一会儿再收回去。
  // 只靠顶端不可靠（Windows 上有人推到顶也出不来），拉手一直在，像 Windows 远程桌面的连接栏。
  // 连接详情弹着的时候不收（弹窗不在工具栏里，鼠标移过去会算“离开”）
  const toolbarRef = useRef<HTMLDivElement>(null);
  const [toolbarShown, setToolbarShown] = useState(false);
  const hideTimer = useRef<number | null>(null);
  const cancelHide = useCallback(() => {
    if (hideTimer.current != null) window.clearTimeout(hideTimer.current);
    hideTimer.current = null;
  }, []);
  const scheduleHide = useCallback((delay: number) => {
    cancelHide();
    hideTimer.current = window.setTimeout(function check() {
      const bar = toolbarRef.current;
      if (bar && (bar.matches(":hover") || bar.querySelector('[aria-expanded="true"]'))) {
        hideTimer.current = window.setTimeout(check, 500);
        return;
      }
      hideTimer.current = null;
      setToolbarShown(false);
    }, delay);
  }, [cancelHide]);
  useEffect(() => {
    if (!fullscreen) {
      cancelHide();
      setToolbarShown(false);
      return;
    }
    // 刚进全屏先露一下，让人知道工具栏在上面
    setToolbarShown(true);
    scheduleHide(1500);
    const onMove = (e: PointerEvent) => {
      if (e.clientY > 4) return;
      cancelHide();
      setToolbarShown(true);
    };
    window.addEventListener("pointermove", onMove);
    return () => {
      window.removeEventListener("pointermove", onMove);
      cancelHide();
    };
  }, [fullscreen, cancelHide, scheduleHide]);
  const revealToolbar = useCallback(() => {
    cancelHide();
    setToolbarShown(true);
  }, [cancelHide]);
  toggleRef.current = toggleFullscreen;
  const reconnect = () => setAttempt((n) => n + 1);

  const platformsDiffer = !!device?.platform && (device.platform === "macos") !== isMac;
  const live = phase.kind === "live";
  const canControl = !!started?.control;
  const latency = stats?.encodeMs != null && path?.rtt_ms != null && decodeShown != null ? stats.encodeMs + path.rtt_ms / 2 + decodeShown : null;

  return (
    <div className="screen-window" data-fullscreen={fullscreen ? "" : undefined} data-toolbar-shown={toolbarShown ? "" : undefined}>
      <div ref={toolbarRef} className={`screen-toolbar${nativeMac ? " titlebar-macos" : ""}${nativeWindows ? " titlebar-windows" : ""}`}
        data-tauri-drag-region={fullscreen ? undefined : true}
        onPointerEnter={() => { if (fullscreen) cancelHide(); }}
        onPointerLeave={() => { if (fullscreen) scheduleHide(600); }}>
        <div className="screen-toolbar-title" data-tauri-drag-region={fullscreen ? undefined : true}>
          <Monitor />
          <span className="truncate">{device?.name ?? ""}</span>
        </div>
        {live && recv && !paused && (
          <span className="screen-stats" title={t("每秒收到的帧数 · 画面实际传输速率")}>
            {t("{{fps}} 帧/秒", { fps: recv.fps })} · {formatMbps(recv.mbps)}
          </span>
        )}
        {connId != null && info && (
          <ConnectionCard connId={connId} info={info} path={path ?? undefined} placement="bottom end" className="screen-path-button"
            extra={live ? (
              <Section>
                <Row label={t("画面")} value={paused === "relay" ? t("已暂停（连接走中继）") : paused ? t("正在找直连线路") : decode ? `${decode.width} × ${decode.height}` : t("统计中…")} />
                <Row label={t("帧率")} value={recv && !paused ? t("{{fps}} 帧/秒", { fps: recv.fps }) : "—"} hint={t("本机每秒收到的画面帧数。对方画面不动时会发得少")} />
                {stats?.target ? <Row label={t("帧率上限")} value={t("{{fps}} 帧/秒", { fps: stats.target })} hint={t("按两边屏幕的刷新率定，取小的那个；流畅档固定 30")} /> : null}
                <Row label={t("传输速率")} value={recv && !paused ? formatMbps(recv.mbps) : "—"} hint={t("画面每秒实际传过来的数据量")} />
                <Row label={t("码率上限")} value={stats && !paused ? `${(stats.bitrate / 1e6).toFixed(1)} Mbps` : "—"} hint={t("对方编码器最多用到的码率，网络差时会自动往下降；画面变化少时实际用不到这么多")} />
                <Row label={t("编码")} value={decode ? `${codecName(decode.codec)} · ${decode.codec}` : "—"} mono />
                <Row label={t("色彩")} value={!decode ? "—" : !decode.chroma444 ? "4:2:0" : is420Format(decode.format ?? null) ? t("4:2:0（本机解码器把 4:4:4 降成了 4:2:0）") : "4:4:4"}
                  hint={t("4:4:4 是每个像素都有自己的颜色，彩色小字、细线清楚；4:2:0 是四个像素共用一份颜色。两边显卡都支持 H.265 的 4:4:4 时才有，不支持就是 4:2:0")} />
                <Row label={t("解码")} value={decode ? (decode.hardware && !hwNote ? t("显卡硬件解码") : t("系统解码")) : "—"} />
                <Row label={t("对方编码")} value={stats?.encoder ? `${stats.encoder.name} · ${stats.encoder.hardware ? t("硬件") : t("软件")}` : "—"}
                  hint={t("对方用哪个编码器压画面。软件编码会卡、很费对方的 CPU，一般是对方显卡驱动有问题")} />
                <Row label={t("延迟（估计）")} value={latency != null ? t("约 {{ms}} 毫秒", { ms: Math.round(latency) }) : "—"}
                  hint={latency != null ? t("对方编码 {{e}} 毫秒 + 网络单程约 {{n}} 毫秒 + 本机解码显示 {{d}} 毫秒。不算对方屏幕刷新、抓屏和本机屏幕刷新的时间", { e: stats!.encodeMs!.toFixed(1), n: Math.round(path!.rtt_ms! / 2), d: decodeShown!.toFixed(1) }) : t("对方是老版本，或者还在统计")} />
                <Row label={t("声音")} value={!hasAudio ? t("对方不能传声音") : audioOn ? t("Opus 48 kHz 立体声") : t("已静音")}
                  hint={t("对方电脑正在放的声音。静音时对方不录，不占带宽；对方是老版本或者系统不支持时没有声音")} />
                <Row label={t("首帧")} value={firstFrameMs != null ? t("{{ms}} 毫秒", { ms: firstFrameMs }) : "—"} hint={t("对方同意以后，过了多久画面出来")} />
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
            title={!started?.translate_keys
              ? (swapMeta ? t("⌘ 和 Ctrl 已对调（按本机习惯用快捷键），点一下改回原样") : t("⌘ 和 Ctrl 按原样发送，点一下对调"))
              : isMac
                ? (swapMeta ? t("快捷键按 Mac 的习惯：⌘C 复制、⌘← 到行首、⌥← 按词跳、⌘Q 退出，对方换成 Windows 的按法；单按 ⌘ 是 Win 键，⌃ 点击是右键。点一下改成原样发送") : t("按键原样发送（⌘ 就是对方的 Win 键），点一下改成按 Mac 的习惯"))
                : (swapMeta ? t("快捷键按 Windows 的习惯：Ctrl+C 复制、Home 到行首、Ctrl+← 按词跳，对方换成 Mac 的按法；对方前台是终端时 Ctrl 还是 Ctrl（Ctrl+C 中断），Ctrl+Shift+C/V 复制粘贴。点一下改成原样发送") : t("按键原样发送（Ctrl 就是 Mac 的 ⌃），点一下改成按 Windows 的习惯"))}
            aria-label={started?.translate_keys ? t("按本机习惯用快捷键") : t("对调 ⌘ 和 Ctrl")}
            onPress={() => {
              const on = !swapMeta;
              setSwapMeta(on);
              if (started?.translate_keys) send({ t: "translate_keys", on });
            }}>
            <Keyboard />
          </Button>
        )}
        {live && canControl && started?.clipboard && (
          <Button variant="ghost" size="icon-sm" className="screen-tool" data-active={shareClipboard ? "" : undefined}
            title={shareClipboard ? t("剪贴板互通：文字、图片、文件都能在两台电脑之间复制粘贴，粘贴时才传。点一下关掉") : t("剪贴板不互通，点一下打开")}
            aria-label={t("剪贴板互通")}
            onPress={toggleShareClipboard}>
            <ClipboardCopy />
          </Button>
        )}
        {live && canControl && started?.privacy && (
          <Button variant="ghost" size="icon-sm" className="screen-tool" data-active={privacy ? "" : undefined}
            disabled={privacyBusy}
            title={privacy
              ? t("隐私屏开着：对方电脑的屏幕被盖住，那边的键盘鼠标也被拦住，只有你能操作。断开时自动撤掉。点一下撤掉")
              : t("隐私屏：盖住对方电脑的屏幕、拦住那边的键盘鼠标，旁边的人看不到你在做什么，你这边照常看和操作")}
            aria-label={t("隐私屏")}
            onPress={() => {
              const on = !privacy;
              privacyWant.current = on;
              setPrivacyBusy(true);
              send({ t: "privacy", on });
            }}>
            {privacy ? <ShieldCheck /> : <Shield />}
          </Button>
        )}
        {live && hasAudio && (
          <Button variant="ghost" size="icon-sm" className="screen-tool"
            title={audioOn ? t("正在放对方电脑的声音，点一下静音") : t("对方电脑的声音已静音，点一下打开")}
            aria-label={audioOn ? t("静音") : t("打开声音")}
            onPress={toggleAudio}>
            {audioOn ? <Volume2 /> : <VolumeX />}
          </Button>
        )}
        {canExtraInput && (
          <Button variant="ghost" size="icon-sm" className="screen-tool" data-active={textOpen ? "" : undefined}
            title={t("送一段文字过去：用本机的输入法打，对方没装中文输入法也能输入")}
            aria-label={t("送文字")}
            onPress={() => setTextOpen((v) => !v)}>
            <Type />
          </Button>
        )}
        {canExtraInput && (
          <Button variant="ghost" size="icon-sm" className="screen-tool" data-active={locked ? "" : undefined}
            title={locked ? t("鼠标锁定在画面里（玩游戏用），按 Esc 解锁") : t("把鼠标锁定在画面里：发移动量而不是位置，第一人称游戏转视角要用这个")}
            aria-label={t("锁定鼠标")}
            onPress={toggleLock}>
            <Crosshair />
          </Button>
        )}
        <Button variant="ghost" size="icon-sm" className="screen-tool" title={`${fullscreen ? t("退出全屏") : t("全屏")}（${isMac ? "⌃⌘F" : "F11"}）`} aria-label={t("全屏")} onPress={() => void toggleFullscreen()}>
          {fullscreen ? <Shrink /> : <Expand />}
        </Button>
      </div>
      {fullscreen && !toolbarShown && (
        <button type="button" className="screen-reveal-tab" aria-label={t("显示工具栏")} title={t("显示工具栏")}
          onPointerEnter={revealToolbar} onClick={revealToolbar}>
          <ChevronDown />
        </button>
      )}
      <div className="screen-stage">
        <canvas ref={canvasRef} className="screen-canvas" tabIndex={0}
          data-control={live && control && canControl ? "" : undefined}
          onPointerMove={onPointerMove}
          onPointerDown={(e) => onPointerButton(e, true)}
          onPointerUp={(e) => onPointerButton(e, false)}
          onPointerLeave={() => { insideRef.current = false; applyCursor(); }}
          onContextMenu={(e) => e.preventDefault()} />
        {live && (!(control && canControl) || locked) && look && remoteCursor?.visible && (() => {
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
        {textOpen && canExtraInput && (
          <div className="screen-text-panel" role="dialog" aria-label={t("送文字")}>
            <textarea autoFocus value={textValue} maxLength={10000}
              placeholder={t("在这里打字（可以用本机的输入法），回车发送，Shift + 回车换行")}
              onChange={(e) => setTextValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") { e.preventDefault(); setTextOpen(false); canvasRef.current?.focus(); }
                else if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); sendText(); }
              }} />
            <div className="screen-text-actions">
              <Button variant="ghost" size="icon-sm" aria-label={t("关闭")} title={t("关闭")} onPress={() => setTextOpen(false)}><X /></Button>
              <Button size="sm" isDisabled={!textValue} onPress={sendText}><Send data-icon="inline-start" />{t("发送")}</Button>
            </div>
          </div>
        )}
        {live && hasAudio && audioOn && audioBlocked && (
          <button type="button" className="screen-audio-hint" onClick={() => player.current?.resume()}>
            <Volume2 />{t("点一下画面就能听到对方的声音")}
          </button>
        )}
        {dropHint && device && (
          <div className="screen-overlay screen-overlay-soft screen-drop-hint">
            <Upload />
            <p>{t("松开就发给 {{name}}", { name: device.name })}</p>
          </div>
        )}
        {live && noFrame && !paused && (
          <div className="screen-overlay screen-overlay-soft">
            <Monitor />
            <p>{t("还没收到对方的画面")}</p>
            <p className="screen-overlay-note">{t("对方的显示器可能关着或在睡眠。在画面上动一下鼠标、按个键，通常就能把它叫醒")}</p>
          </div>
        )}
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
      <Toaster />
    </div>
  );
}
