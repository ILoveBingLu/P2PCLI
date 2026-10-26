import { useEffect, useId, useRef, type CSSProperties, type PointerEvent, type ReactNode } from "react";
import { cn } from "@/lib/utils";

/** 跟随鼠标的弹簧：临界阻尼不回弹，0.35 秒左右跟上 */
const RESPONSE = 0.35;
const OMEGA = (2 * Math.PI) / RESPONSE;

type Spring = { p: number; v: number; to: number };

/** 一层图形：颜色上浅下深，depth 是倾斜到底时错开多少（占图标边长的百分比） */
type Layer = { name: string; top: string; base: string; depth: number; shape: ReactNode };

// 图层、坐标、颜色都照 src-tauri/icons/AppIcon.icon，从下往上：尖括号、下划线、光芒、指针。
// 不要图标的深色底，画布和 assets/logo.svg 一样裁成图形居中的正方形
const LAYERS: Layer[] = [
  {
    name: "chevron",
    top: "#86efc4",
    base: "#34d399",
    depth: 2.5,
    shape: (
      <g fill="none" strokeWidth={84} strokeLinecap="round">
        <line x1={196} y1={420} x2={356} y2={565} />
        <line x1={356} y1={565} x2={196} y2={710} />
      </g>
    ),
  },
  {
    name: "cursor",
    top: "#ffffff",
    base: "#c9ced6",
    depth: 4,
    shape: <rect x={424} y={668} width={316} height={84} rx={42} stroke="none" />,
  },
  {
    name: "rays",
    top: "#a9cdfd",
    base: "#60a5fa",
    depth: 6,
    shape: (
      <g fill="none" strokeWidth={60} strokeLinecap="round">
        <path d="M 698.00 290.40 L 650.00 336.00" />
        <path d="M 484.40 384.00 L 414.80 364.80" />
        <path d="M 506.00 480.00 L 460.40 528.00" />
        <path d="M 534.80 244.80 L 554.00 314.40" />
      </g>
    ),
  },
  {
    name: "pointer",
    top: "#a9cdfd",
    base: "#60a5fa",
    depth: 7.5,
    shape: (
      <path
        strokeWidth={60}
        strokeLinejoin="round"
        d="M 578.89 424.56 A 11.95 11.95 0 0 1 594.56 408.89 L 858.56 516.89 A 12.00 12.00 0 0 1 856.78 539.66 L 752.41 564.65 A 24.00 24.00 0 0 0 734.65 582.38 L 709.69 686.78 A 12.00 12.00 0 0 1 686.89 688.56 Z"
      />
    ),
  },
];

/** 玻璃质感的一层：底下一道投影，本体上浅下深，左上边缘一圈高光，右下边缘一圈弱反光 */
function GlassLayer({ id, layer }: { id: string; layer: Layer }) {
  const shape = `${id}-${layer.name}`;
  const box = { maskUnits: "userSpaceOnUse", x: 0, y: 0, width: 1024, height: 1024 } as const;
  return (
    <svg viewBox="141 103 760 760" className="glass-logo-layer" style={{ "--d": layer.depth } as CSSProperties}>
      <defs>
        <g id={shape}>{layer.shape}</g>
        <linearGradient id={`${shape}-fill`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor={layer.top} />
          <stop offset="1" stopColor={layer.base} />
        </linearGradient>
        {/* 滤镜区域按整张画布：按图形边界算会漏掉描边宽度，WebKit 会把模糊裁成方块 */}
        <filter id={`${shape}-blur`} filterUnits="userSpaceOnUse" x={0} y={0} width={1024} height={1024}>
          <feGaussianBlur stdDeviation="16" />
        </filter>
        <mask id={`${shape}-hi`} {...box}>
          <use href={`#${shape}`} fill="#fff" stroke="#fff" />
          <use href={`#${shape}`} fill="#000" stroke="#000" transform="translate(10 13)" />
        </mask>
        <mask id={`${shape}-lo`} {...box}>
          <use href={`#${shape}`} fill="#fff" stroke="#fff" />
          <use href={`#${shape}`} fill="#000" stroke="#000" transform="translate(-7 -9)" />
        </mask>
      </defs>
      <g className="glass-logo-shadow">
        <use href={`#${shape}`} fill="#000" stroke="#000" filter={`url(#${shape}-blur)`} />
      </g>
      <use href={`#${shape}`} fill={`url(#${shape}-fill)`} stroke={`url(#${shape}-fill)`} className="glass-logo-body" />
      <use href={`#${shape}`} fill="#fff" stroke="#fff" mask={`url(#${shape}-hi)`} className="glass-logo-hi" />
      <use href={`#${shape}`} fill="#fff" stroke="#fff" mask={`url(#${shape}-lo)`} className="glass-logo-lo" />
    </svg>
  );
}

/**
 * 关于页的 app 图标，液态玻璃质感，没有底板。
 * 鼠标移上来时整个图标朝鼠标那边往里倾，各层朝鼠标方向错开，越上面的层错开越多，投影留在原地，看着像浮起来。
 * 系统设置了减少动态效果时只显示静止的图标。
 */
export function GlassLogo({ className }: { className?: string }) {
  const id = "glass-" + useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const ref = useRef<HTMLDivElement>(null);
  const springs = useRef({
    x: { p: 0, v: 0, to: 0 } as Spring,
    y: { p: 0, v: 0, to: 0 } as Spring,
    lift: { p: 0, v: 0, to: 0 } as Spring,
  });
  const frame = useRef(0);
  const last = useRef(0);

  useEffect(() => () => cancelAnimationFrame(frame.current), []);

  const tick = (now: number) => {
    const dt = Math.min((now - last.current) / 1000, 1 / 30);
    last.current = now;
    const all = springs.current;
    let moving = false;
    for (const s of [all.x, all.y, all.lift]) {
      s.v += (OMEGA * OMEGA * (s.to - s.p) - 2 * OMEGA * s.v) * dt;
      s.p += s.v * dt;
      if (Math.abs(s.to - s.p) < 1e-3 && Math.abs(s.v) < 1e-3) {
        s.p = s.to;
        s.v = 0;
      } else {
        moving = true;
      }
    }
    const style = ref.current?.style;
    style?.setProperty("--gx", all.x.p.toFixed(4));
    style?.setProperty("--gy", all.y.p.toFixed(4));
    style?.setProperty("--gl", all.lift.p.toFixed(4));
    frame.current = moving ? requestAnimationFrame(tick) : 0;
  };

  const aim = (x: number, y: number, lift: number) => {
    const all = springs.current;
    all.x.to = x;
    all.y.to = y;
    all.lift.to = lift;
    if (!frame.current) {
      last.current = performance.now();
      frame.current = requestAnimationFrame(tick);
    }
  };

  const onMove = (e: PointerEvent<HTMLDivElement>) => {
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const r = e.currentTarget.getBoundingClientRect();
    const clamp = (v: number) => Math.max(-1, Math.min(1, v));
    aim(clamp((e.clientX - r.left) / r.width * 2 - 1), clamp((e.clientY - r.top) / r.height * 2 - 1), 1);
  };

  return (
    <div
      ref={ref}
      aria-hidden="true"
      className={cn("glass-logo", className)}
      onPointerMove={onMove}
      onPointerLeave={() => aim(0, 0, 0)}
    >
      <div className="glass-logo-stack">
        {LAYERS.map((layer) => <GlassLayer key={layer.name} id={id} layer={layer} />)}
      </div>
    </div>
  );
}
