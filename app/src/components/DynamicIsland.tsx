import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ArrowDown, ArrowUp, ArrowUpDown, CircleAlert, Download, LoaderCircle, X } from "lucide-react";
import { formatSize, type Transfer } from "../api";
import type { useUpdater } from "./useUpdater";
import { TransferRow } from "./TransferPanel";
import { cn } from "@/lib/utils";
import { t } from "@/i18n";

/** 鼠标移开后等这么久再收起，免得在边上来回晃时一闪一闪 */
const COLLAPSE_DELAY_MS = 160;
/** 算下载速度用最近这么久的进度 */
const SPEED_WINDOW_MS = 2000;
/** 传完的显示这么久对勾再消失；鼠标停在岛上时不走 */
const DONE_KEEP_MS = 6000;
/** 失败的等人关，太久以前的就不再显示 */
const FAILED_KEEP_MS = 10 * 60 * 1000;
/** 展开的卡片里最多列这么多个传输 */
const MAX_ROWS = 5;
/** 消失动画的时长，和样式里的 island-leave 一致 */
const LEAVE_MS = 320;

/** 岛上显示的一件事：传文件或者更新 */
interface Activity {
  id: "transfer" | "update";
  /** 收起时的一小条 */
  compact: ReactNode;
  /** 鼠标移上去展开的卡片；收起时那条上有按钮的不展开（一悬停就展开，按钮会从鼠标底下跑掉） */
  expanded?: ReactNode;
  /** 分出去当小圆泡时显示的东西（一个图标或进度环） */
  bubble: ReactNode;
  bubbleLabel: string;
  /** 读屏播报：只在阶段变化时变，不念一直在跳的百分比 */
  announce: string;
  /** 变了就让岛弹一下：开始、传完、失败这些时刻 */
  beat: string;
  tone?: "success" | "error";
}

/**
 * 标题栏正中的灵动岛：传文件、下载更新时出现，没事时不占地方。
 * 从标题栏里冒出来，宽高按内容用弹簧曲线伸缩，内容换的时候从模糊里浮现；状态变了弹一下，失败抖一下。
 * 同时有两件事（比如传文件时在下载更新），另一件分出去成右边的小圆泡，点一下互换。
 */
export function DynamicIsland({ updater, transfers }: { updater: ReturnType<typeof useUpdater>; transfers: Transfer[] }) {
  const [hover, setHover] = useState(false);
  const transfer = useTransferActivity(transfers, hover);
  const update = useUpdateActivity(updater);
  const [swapped, setSwapped] = useState(false);

  // 默认传文件优先（正在发生、和手上的事相关）；正在安装更新时更新优先（马上要重启）
  const list = [transfer, update].filter((a): a is Activity => !!a);
  if (update?.beat === "installing") list.sort((a, b) => (a.id === "update" ? -1 : b.id === "update" ? 1 : 0));
  if (swapped && list.length === 2) list.reverse();
  useEffect(() => { if (list.length < 2) setSwapped(false); }, [list.length]);
  const [primary, secondary] = list;

  // 都没事了：放完消失动画再拿掉
  const last = useRef<Activity | null>(null);
  const [leaving, setLeaving] = useState(false);
  useEffect(() => {
    if (primary) { last.current = primary; setLeaving(false); return; }
    if (!last.current) return;
    setLeaving(true);
    const timer = window.setTimeout(() => { last.current = null; setLeaving(false); }, LEAVE_MS);
    return () => window.clearTimeout(timer);
  }, [primary]);
  const shown = primary ?? (leaving ? last.current : null);
  if (!shown) return null;

  return (
    <div className="island-stack" data-leaving={!primary || undefined}>
      <IslandShell activity={shown} onHover={setHover} />
      {secondary && primary && (
        <button type="button" className="island-bubble" data-tone={secondary.tone} key={secondary.id}
          aria-label={secondary.bubbleLabel} title={secondary.bubbleLabel} onClick={() => setSwapped((s) => !s)}>
          {secondary.bubble}
        </button>
      )}
    </div>
  );
}

/** 岛本身：量当前那层内容的宽高，变化时用弹簧过渡；内容换了从模糊里浮现；beat 变了弹一下 */
function IslandShell({ activity, onHover }: { activity: Activity; onHover: (hover: boolean) => void }) {
  const [hover, setHover] = useState(false);
  const collapseTimer = useRef(0);
  const compact = useRef<HTMLDivElement>(null);
  const expanded = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<{ width: number; height: number } | null>(null);
  const open = hover && !!activity.expanded;

  useLayoutEffect(() => {
    const el = open ? expanded.current : compact.current;
    if (!el) return;
    const measure = () => setSize({ width: el.offsetWidth, height: el.offsetHeight });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [open, activity.id]);

  // 状态变化时弹一下：换个 key 让动画重新播
  const [beat, setBeat] = useState(0);
  const lastBeat = useRef(activity.beat);
  useEffect(() => {
    if (lastBeat.current === activity.beat) return;
    lastBeat.current = activity.beat;
    setBeat((n) => n + 1);
  }, [activity.beat]);

  useEffect(() => () => window.clearTimeout(collapseTimer.current), []);
  useEffect(() => { onHover(hover); }, [hover, onHover]);

  const enter = () => { window.clearTimeout(collapseTimer.current); setHover(true); };
  const leave = () => {
    window.clearTimeout(collapseTimer.current);
    collapseTimer.current = window.setTimeout(() => setHover(false), COLLAPSE_DELAY_MS);
  };

  return (
    <div className="dynamic-island" data-open={open || undefined} data-tone={activity.tone} data-beat={beat % 2 ? "a" : beat ? "b" : undefined}
      style={size ? { width: size.width, height: size.height } : undefined} onMouseEnter={enter} onMouseLeave={leave}>
      <span className="sr-only" role="status">{activity.announce}</span>
      <div ref={compact} className="island-layer island-compact" aria-hidden={open}>
        {/* 换了内容（另一件事、另一个阶段）就重新挂上，播浮现动画 */}
        <div key={activity.id + ":" + activity.beat} className="island-morph">{activity.compact}</div>
      </div>
      <div ref={expanded} className="island-layer island-expanded" aria-hidden={!open}>
        {activity.expanded}
      </div>
    </div>
  );
}

/** 小进度环；done 时画一个对勾 */
function ProgressRing({ value, done }: { value: number; done?: boolean }) {
  const r = 6;
  const c = 2 * Math.PI * r;
  if (done) {
    return (
      <svg className="island-icon island-check" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r={r} />
        <path d="M5.2 8.3 7.2 10.2 10.9 6.1" />
      </svg>
    );
  }
  return (
    <svg className="island-icon island-ring" viewBox="0 0 16 16" aria-hidden="true">
      <circle cx="8" cy="8" r={r} />
      <circle cx="8" cy="8" r={r} strokeDasharray={c} strokeDashoffset={c * (1 - value / 100)} />
    </svg>
  );
}

// ---------------- 传文件 ----------------

function useTransferActivity(transfers: Transfer[], hover: boolean): Activity | null {
  const [dismissed, setDismissed] = useState<Set<string>>(new Set());
  const [now, setNow] = useState(Date.now());
  // 鼠标停在岛上时时间不往前走：传完的不会在你要点“在文件夹里显示”时消失
  const hoverRef = useRef(hover);
  hoverRef.current = hover;
  useEffect(() => {
    const timer = window.setInterval(() => { if (!hoverRef.current) setNow(Date.now()); }, 1000);
    return () => window.clearInterval(timer);
  }, []);

  const shown = useMemo(() => transfers.filter((x) => {
    if (dismissed.has(x.id)) return false;
    if (x.finished_at == null) return true;
    const age = now - x.finished_at * 1000;
    return x.state === "failed" ? age < FAILED_KEEP_MS : age < DONE_KEEP_MS;
  }), [transfers, dismissed, now]);
  if (!shown.length) return null;

  const active = shown.filter((x) => x.finished_at == null);
  const failed = shown.filter((x) => x.state === "failed");
  const done = shown.filter((x) => x.state === "done");
  const dismiss = (id: string) => setDismissed((ids) => new Set(ids).add(id));

  const expanded = (
    <>
      <div className="island-title">
        <span>{active.length ? t("正在传 {{count}} 个", { count: active.length }) : t("文件传输")}</span>
        {!active.length && <button type="button" className="island-close ml-auto" aria-label={t("全部关闭")} title={t("全部关闭")}
          onClick={() => setDismissed((ids) => new Set([...ids, ...shown.map((x) => x.id)]))}><X /></button>}
      </div>
      <div className="island-rows">
        {shown.slice(0, MAX_ROWS).map((x) => <TransferRow key={x.id} transfer={x} onDismiss={() => dismiss(x.id)} />)}
        {shown.length > MAX_ROWS && <p className="island-muted">{t("还有 {{count}} 个", { count: shown.length - MAX_ROWS })}</p>}
      </div>
    </>
  );

  // 收起时：有正在传的就显示进度；都传完了显示对勾；有失败的显示失败
  if (active.length) {
    const total = active.reduce((n, x) => n + x.total, 0);
    const sent = active.reduce((n, x) => n + x.done, 0);
    const percent = total ? Math.min(100, Math.floor((sent * 100) / total)) : 0;
    const one = active.length === 1 ? active[0] : null;
    const sending = active.every((x) => x.direction === "send");
    const receiving = active.every((x) => x.direction === "receive");
    const waiting = one && (one.state === "waiting" || one.state === "connecting");
    const Arrow = sending ? ArrowUp : receiving ? ArrowDown : ArrowUpDown;
    return {
      id: "transfer",
      compact: (
        <>
          <Arrow className={cn("island-icon island-arrow", sending ? "is-up" : receiving ? "is-down" : "")} />
          <span className="island-label">
            {one ? (waiting ? (one.direction === "send" ? t("等 {{name}} 同意…", { name: one.peer_name }) : t("准备接收…")) : one.label) : t("{{count}} 个传输", { count: active.length })}
          </span>
          {waiting ? <LoaderCircle className="island-icon animate-spin" /> : <><ProgressRing value={percent} /><span className="island-percent">{percent}%</span></>}
        </>
      ),
      expanded,
      bubble: <ProgressRing value={percent} />,
      bubbleLabel: t("文件传输 {{percent}}%", { percent }),
      announce: one ? (one.direction === "send" ? t("正在发送 {{label}}", { label: one.label }) : t("正在接收 {{label}}", { label: one.label })) : t("正在传 {{count}} 个", { count: active.length }),
      beat: `run:${active.map((x) => x.id).join(",")}:${waiting ? "wait" : "go"}`,
    };
  }
  if (failed.length) {
    const first = failed[0];
    return {
      id: "transfer",
      tone: "error",
      compact: (
        <>
          <CircleAlert className="island-icon text-danger" />
          <span className="island-label">{failed.length > 1 ? t("{{count}} 个传输失败", { count: failed.length }) : first.direction === "send" ? t("没发出去：{{label}}", { label: first.label }) : t("没收到：{{label}}", { label: first.label })}</span>
        </>
      ),
      expanded,
      bubble: <CircleAlert className="island-icon text-danger" />,
      bubbleLabel: t("文件传输失败"),
      announce: t("文件传输失败"),
      beat: `failed:${failed.map((x) => x.id).join(",")}`,
    };
  }
  const first = done[0];
  return {
    id: "transfer",
    tone: "success",
    compact: (
      <>
        <ProgressRing value={100} done />
        <span className="island-label">
          {done.length > 1 ? t("{{count}} 个都传完了", { count: done.length })
            : first.direction === "send" ? t("已发到 {{name}}", { name: first.peer_name }) : t("已收到 {{label}}", { label: first.label })}
        </span>
        {done.length === 1 && <span className="island-muted-inline">{formatSize(first.total)}</span>}
      </>
    ),
    expanded,
    bubble: <ProgressRing value={100} done />,
    bubbleLabel: t("文件传完了"),
    announce: t("文件传完了"),
    beat: `done:${done.map((x) => x.id).join(",")}`,
  };
}

// ---------------- 更新 ----------------

function useUpdateActivity(updater: ReturnType<typeof useUpdater>): Activity | null {
  const speed = useDownloadSpeed(updater.installing ? updater.progress : null);
  const update = updater.notify ? updater.update : null;
  if (!update) return null;
  const phase = updater.installing
    ? updater.progress?.event === "installing" ? "installing" : "downloading"
    : updater.installError ? "failed" : "available";
  const p = updater.progress?.event === "downloading" ? updater.progress : null;
  const percent = p?.total ? Math.min(100, Math.floor((p.downloaded / p.total) * 100)) : null;
  const left = p?.total && speed ? (p.total - p.downloaded) / speed : null;
  const announce = { available: t("发现新版本 {{version}}", { version: update.version }), downloading: t("正在下载更新"), installing: t("正在安装更新，马上重启"), failed: t("更新失败") }[phase];

  if (phase === "available") {
    return {
      id: "update", announce, beat: "available",
      compact: (
        <>
          <Download className="island-icon text-accent" />
          <span title={update.notes ? t("更新内容：\n{{notes}}", { notes: update.notes }) : undefined}>{t("新版本 v")}{update.version}</span>
          <button type="button" className="island-pill-button" title={t("下载并安装，完成后自动重启")} onClick={updater.install}>{t("更新")}</button>
          <button type="button" className="island-close" aria-label={t("稍后再说")} title={t("稍后再说")} onClick={updater.dismiss}><X /></button>
        </>
      ),
      bubble: <Download className="island-icon text-accent" />,
      bubbleLabel: t("发现新版本 {{version}}", { version: update.version }),
    };
  }
  if (phase === "failed") {
    return {
      id: "update", announce, beat: "failed", tone: "error",
      compact: (
        <>
          <CircleAlert className="island-icon text-danger" />
          <span title={updater.installError ?? undefined}>{t("更新失败")}</span>
          <button type="button" className="island-pill-button" onClick={updater.install}>{t("重试")}</button>
          <button type="button" className="island-close" aria-label={t("稍后再说")} title={t("稍后再说")} onClick={updater.dismiss}><X /></button>
        </>
      ),
      bubble: <CircleAlert className="island-icon text-danger" />,
      bubbleLabel: t("更新失败"),
    };
  }
  if (phase === "installing") {
    return {
      id: "update", announce, beat: "installing",
      compact: <><LoaderCircle className="island-icon animate-spin" /><span>{t("正在安装，马上重启")}</span></>,
      bubble: <LoaderCircle className="island-icon animate-spin" />,
      bubbleLabel: t("正在安装更新，马上重启"),
    };
  }
  return {
    id: "update", announce, beat: "downloading",
    compact: (
      <>
        {percent == null ? <LoaderCircle className="island-icon animate-spin" /> : <ProgressRing value={percent} />}
        <span className="island-percent">{percent == null ? t("下载中") : `${percent}%`}</span>
      </>
    ),
    expanded: (
      <>
        <div className="island-title">
          <span>{t("正在下载 v")}{update.version}</span>
          <span className="ml-auto tabular-nums">{percent == null ? "" : `${percent}%`}</span>
        </div>
        <div className="island-bar" data-indeterminate={percent == null || undefined}>
          <span style={percent == null ? undefined : { width: `${percent}%` }} />
        </div>
        <div className="island-muted flex tabular-nums">
          <span>
            {p ? (p.total ? `${formatSize(p.downloaded)} / ${formatSize(p.total)}` : formatSize(p.downloaded)) : t("正在连接…")}
            {speed ? ` · ${formatSize(speed)}/s` : ""}
          </span>
          {left != null && <span className="ml-auto">{t("还剩")} {formatLeft(left)}</span>}
        </div>
        <p className="island-muted">{t("下载完会自动安装并重启，正在进行的远程连接需要重新连。")}</p>
      </>
    ),
    bubble: percent == null ? <LoaderCircle className="island-icon animate-spin" /> : <ProgressRing value={percent} />,
    bubbleLabel: t("正在下载更新"),
  };
}

/** 按最近两秒的进度算下载速度（字节每秒），算不出来是 null */
function useDownloadSpeed(progress: ReturnType<typeof useUpdater>["progress"]) {
  const samples = useRef<{ at: number; bytes: number }[]>([]);
  const [speed, setSpeed] = useState<number | null>(null);
  useEffect(() => {
    if (progress?.event !== "downloading") {
      samples.current = [];
      setSpeed(null);
      return;
    }
    const now = performance.now();
    const list = [...samples.current, { at: now, bytes: progress.downloaded }].filter((s) => now - s.at <= SPEED_WINDOW_MS);
    samples.current = list;
    const first = list[0];
    const ms = now - first.at;
    if (ms >= 300) setSpeed(((progress.downloaded - first.bytes) * 1000) / ms);
  }, [progress]);
  return speed;
}

function formatLeft(seconds: number) {
  if (seconds < 60) return t("{{value}} 秒", { value: Math.max(1, Math.round(seconds)) });
  return t("{{value}} 分钟", { value: Math.round(seconds / 60) });
}
