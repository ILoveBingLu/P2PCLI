import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { CircleAlert, Download, LoaderCircle, X } from "lucide-react";
import { formatSize } from "../api";
import type { useUpdater } from "./useUpdater";

/** 鼠标移开后等这么久再收起，免得在边上来回晃时一闪一闪 */
const COLLAPSE_DELAY_MS = 160;
/** 算下载速度用最近这么久的进度 */
const SPEED_WINDOW_MS = 2000;

type Phase = "available" | "downloading" | "installing" | "failed";

/** 更新提示：标题栏正中一个胶囊，像灵动岛，只占一小条不挡操作。
 *  有新版本时显示版本号、更新和稍后；点了更新变成进度环，鼠标移上去展开看进度条、速度和剩余时间。
 *  只有下载中能展开：另外几种状态胶囊上有按钮，一悬停就展开的话按钮会从鼠标底下跑掉 */
export function UpdateIsland({ updater }: { updater: ReturnType<typeof useUpdater> }) {
  const [hover, setHover] = useState(false);
  const collapseTimer = useRef(0);
  const compact = useRef<HTMLDivElement>(null);
  const expanded = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<{ width: number; height: number } | null>(null);
  const speed = useDownloadSpeed(updater.installing ? updater.progress : null);

  const update = updater.notify ? updater.update : null;
  const phase: Phase = updater.installing
    ? updater.progress?.event === "installing" ? "installing" : "downloading"
    : updater.installError ? "failed" : "available";
  const open = hover && phase === "downloading";

  // 胶囊的宽高跟着当前显示的那层内容走，变化时用过渡动画撑开或收起
  useLayoutEffect(() => {
    const el = open ? expanded.current : compact.current;
    if (!el) return;
    const measure = () => setSize({ width: el.offsetWidth, height: el.offsetHeight });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [open, phase, !!update]);

  useEffect(() => () => window.clearTimeout(collapseTimer.current), []);

  if (!update) return null;

  const enter = () => {
    window.clearTimeout(collapseTimer.current);
    setHover(true);
  };
  const leave = () => {
    window.clearTimeout(collapseTimer.current);
    collapseTimer.current = window.setTimeout(() => setHover(false), COLLAPSE_DELAY_MS);
  };

  const p = updater.progress?.event === "downloading" ? updater.progress : null;
  const percent = p?.total ? Math.min(100, Math.floor((p.downloaded / p.total) * 100)) : null;
  const left = p?.total && speed ? (p.total - p.downloaded) / speed : null;

  return (
    <div
      className="update-island"
      data-open={open || undefined}
      data-phase={phase}
      style={size ? { width: size.width, height: size.height } : undefined}
      onMouseEnter={enter}
      onMouseLeave={leave}
    >
      {/* 读屏只播报阶段变化，不念一直在变的百分比 */}
      <span className="sr-only" role="status">
        {{ available: `发现新版本 ${update.version}`, downloading: "正在下载更新", installing: "正在安装更新，马上重启", failed: "更新失败" }[phase]}
      </span>
      {/* 收起时的一小条 */}
      <div ref={compact} className="island-layer island-compact" aria-hidden={open}>
        {phase === "available" && (
          <>
            <Download className="island-icon text-accent" />
            <span title={update.notes ? `更新内容：\n${update.notes}` : undefined}>新版本 v{update.version}</span>
            <button type="button" className="island-pill-button" title="下载并安装，完成后自动重启" onClick={updater.install}>更新</button>
            <button type="button" className="island-close" aria-label="稍后再说" title="稍后再说" onClick={updater.dismiss}><X /></button>
          </>
        )}
        {phase === "downloading" && (
          <>
            {percent == null ? <LoaderCircle className="island-icon animate-spin" /> : <ProgressRing value={percent} />}
            <span className="island-percent">{percent == null ? "下载中" : `${percent}%`}</span>
          </>
        )}
        {phase === "installing" && (
          <>
            <LoaderCircle className="island-icon animate-spin" />
            <span>正在安装，马上重启</span>
          </>
        )}
        {phase === "failed" && (
          <>
            <CircleAlert className="island-icon text-danger" />
            <span title={updater.installError ?? undefined}>更新失败</span>
            <button type="button" className="island-pill-button" onClick={updater.install}>重试</button>
            <button type="button" className="island-close" aria-label="稍后再说" title="稍后再说" onClick={updater.dismiss}><X /></button>
          </>
        )}
      </div>

      {/* 下载中鼠标移上来展开的卡片 */}
      <div ref={expanded} className="island-layer island-expanded" aria-hidden={!open}>
        {phase === "downloading" && (
          <>
            <div className="island-title">
              <span>正在下载 v{update.version}</span>
              <span className="ml-auto tabular-nums">{percent == null ? "" : `${percent}%`}</span>
            </div>
            <div className="island-bar" data-indeterminate={percent == null || undefined}>
              <span style={percent == null ? undefined : { width: `${percent}%` }} />
            </div>
            <div className="island-muted flex tabular-nums">
              <span>
                {p ? (p.total ? `${formatSize(p.downloaded)} / ${formatSize(p.total)}` : formatSize(p.downloaded)) : "正在连接…"}
                {speed ? ` · ${formatSize(speed)}/s` : ""}
              </span>
              {left != null && <span className="ml-auto">还剩 {formatLeft(left)}</span>}
            </div>
            <p className="island-muted">下载完会自动安装并重启，正在进行的远程连接需要重新连。</p>
          </>
        )}
      </div>
    </div>
  );
}

/** 小进度环 */
function ProgressRing({ value }: { value: number }) {
  const r = 6;
  const c = 2 * Math.PI * r;
  return (
    <svg className="island-icon island-ring" viewBox="0 0 16 16" aria-hidden="true">
      <circle cx="8" cy="8" r={r} />
      <circle cx="8" cy="8" r={r} strokeDasharray={c} strokeDashoffset={c * (1 - value / 100)} />
    </svg>
  );
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
  if (seconds < 60) return `${Math.max(1, Math.round(seconds))} 秒`;
  return `${Math.round(seconds / 60)} 分钟`;
}
