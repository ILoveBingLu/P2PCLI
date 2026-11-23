import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";

interface TrailPoint {
  x: number;
  y: number;
  t: number;
}

interface Spark {
  x: number;
  y: number;
  vx: number;
  vy: number;
  drag: number;
  life: number;
  maxLife: number;
  size: number;
  len: number;
  color: [number, number, number];
}

interface Dust {
  x: number;
  y: number;
  vx: number;
  vy: number;
  life: number;
  maxLife: number;
  size: number;
  twinkle: number;
  color: [number, number, number];
}

function makeHDRSprite(stops: Array<[number, string]>): HTMLCanvasElement {
  const size = 256;
  const c = document.createElement("canvas");
  c.width = size;
  c.height = size;
  const gctx = c.getContext("2d");
  if (gctx) {
    const grad = gctx.createRadialGradient(size / 2, size / 2, 0, size / 2, size / 2, size / 2);
    for (const [offset, color] of stops) grad.addColorStop(offset, color);
    gctx.fillStyle = grad;
    gctx.fillRect(0, 0, size, size);
  }
  return c;
}

function stepSpring(val: number, vel: number, target: number, stiffness: number, damping: number, dt: number): [number, number] {
  const force = -stiffness * (val - target) - damping * vel;
  const nv = vel + force * dt;
  const np = val + nv * dt;
  return [np, nv];
}

const STREAM_START = 0.04;
const IMPACT_T = 0.62;
const FADE_START_MS = 3050;
const FADE_DURATION_MS = 420;

export function SplashScreen({ onFadeStart, onDone }: { onFadeStart?: () => void; onDone: () => void }) {
  const stageRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const wordmarkRef = useRef<HTMLHeadingElement>(null);

  const elChevronRef = useRef<SVGGElement>(null);
  const elCursorRef = useRef<SVGGElement>(null);
  const elPointerRef = useRef<SVGGElement>(null);
  const elRaysRef = useRef<SVGGElement>(null);
  const maskCutoutRef = useRef<SVGUseElement>(null);

  const [exiting, setExiting] = useState(false);

  const onFadeStartRef = useRef(onFadeStart);
  const onDoneRef = useRef(onDone);
  onFadeStartRef.current = onFadeStart;
  onDoneRef.current = onDone;

  useEffect(() => {
    const stage = stageRef.current;
    const canvas = canvasRef.current;
    const wordmark = wordmarkRef.current;
    if (!stage || !canvas || !wordmark) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      stage.classList.add("is-burst", "is-aura", "is-settled");
      const t1 = window.setTimeout(() => {
        setExiting(true);
        onFadeStartRef.current?.();
      }, 600);
      const t2 = window.setTimeout(() => {
        onDoneRef.current();
      }, 600 + FADE_DURATION_MS);
      return () => {
        window.clearTimeout(t1);
        window.clearTimeout(t2);
      };
    }

    let W = 0;
    let H = 0;
    let dpr = Math.min(window.devicePixelRatio || 1, 2);
    let blastX = 0;
    let blastY = 0;

    const glowEmerald = makeHDRSprite([
      [0.00, "rgba(255, 255, 255, 1.0)"],
      [0.08, "rgba(167, 243, 208, 0.85)"],
      [0.24, "rgba(52, 211, 153, 0.36)"],
      [0.58, "rgba(52, 211, 153, 0.06)"],
      [1.00, "rgba(0, 0, 0, 0.0)"],
    ]);

    const glowBlue = makeHDRSprite([
      [0.00, "rgba(255, 255, 255, 1.0)"],
      [0.08, "rgba(191, 219, 254, 0.88)"],
      [0.24, "rgba(96, 165, 250, 0.38)"],
      [0.58, "rgba(96, 165, 250, 0.07)"],
      [1.00, "rgba(0, 0, 0, 0.0)"],
    ]);

    const glowWhite = makeHDRSprite([
      [0.00, "rgba(255, 255, 255, 1.0)"],
      [0.10, "rgba(224, 242, 254, 0.85)"],
      [0.32, "rgba(125, 211, 252, 0.25)"],
      [0.70, "rgba(56, 189, 248, 0.04)"],
      [1.00, "rgba(0, 0, 0, 0.0)"],
    ]);

    const updateMetrics = () => {
      const rect = stage.getBoundingClientRect();
      dpr = Math.min(window.devicePixelRatio || 1, 2);
      W = Math.max(1, rect.width);
      H = Math.max(1, rect.height);
      canvas.width = Math.round(W * dpr);
      canvas.height = Math.round(H * dpr);

      const wr = wordmark.getBoundingClientRect();
      blastX = wr.left - rect.left + wr.width * 0.5;
      blastY = wr.top - rect.top + wr.height * 0.5;
    };

    const ro = new ResizeObserver(updateMetrics);
    ro.observe(stage);
    updateMetrics();

    const springs = {
      chevron: { startAt: 1.38, x: -1350, vx: 0, y: 0, vy: 0, s: 0.78, vs: 0, r: -16, vr: 0, op: 0 },
      cursor: { startAt: 1.60, x: 0, vx: 0, y: 1200, vy: 0, s: 0.78, vs: 0, r: 0, vr: 0, op: 0 },
      pointer: { startAt: 1.82, x: 1250, vx: 0, y: 1250, vy: 0, s: 0.78, vs: 0, r: 16, vr: 0, op: 0 },
      rays: { startAt: 2.16, s: 0.04, vs: 0, r: -16, vr: 0, op: 0 },
    };

    const applyLogoTransforms = () => {
      const c = springs.chevron;
      if (elChevronRef.current) {
        elChevronRef.current.style.opacity = c.op.toFixed(3);
        elChevronRef.current.style.transform = `translate(${c.x.toFixed(1)}px, ${c.y.toFixed(1)}px) rotate(${c.r.toFixed(2)}deg) scale(${c.s.toFixed(3)})`;
        elChevronRef.current.style.transformOrigin = "276px 565px";
      }

      const u = springs.cursor;
      if (elCursorRef.current) {
        elCursorRef.current.style.opacity = u.op.toFixed(3);
        elCursorRef.current.style.transform = `translate(${u.x.toFixed(1)}px, ${u.y.toFixed(1)}px) scale(${u.s.toFixed(3)})`;
        elCursorRef.current.style.transformOrigin = "582px 710px";
      }

      const p = springs.pointer;
      const pTf = `translate(${p.x.toFixed(1)}px, ${p.y.toFixed(1)}px) rotate(${p.r.toFixed(2)}deg) scale(${p.s.toFixed(3)})`;
      if (elPointerRef.current) {
        elPointerRef.current.style.opacity = p.op.toFixed(3);
        elPointerRef.current.style.transform = pTf;
        elPointerRef.current.style.transformOrigin = "690px 535px";
      }
      if (maskCutoutRef.current) {
        maskCutoutRef.current.style.opacity = p.op.toFixed(3);
        maskCutoutRef.current.style.transform = pTf;
        maskCutoutRef.current.style.transformOrigin = "690px 535px";
      }

      const y = springs.rays;
      if (elRaysRef.current) {
        elRaysRef.current.style.opacity = y.op.toFixed(3);
        elRaysRef.current.style.transform = `rotate(${y.r.toFixed(2)}deg) scale(${y.s.toFixed(3)})`;
        elRaysRef.current.style.transformOrigin = "586px 416px";
      }
    };

    applyLogoTransforms();

    const trailA: TrailPoint[] = [];
    const trailB: TrailPoint[] = [];
    const sparks: Spark[] = [];
    const ambientDust: Dust[] = [];

    let burstDone = false;
    let settled = false;
    const startTime = performance.now();
    let lastTime = startTime;
    let rafId = 0;
    const timers: number[] = [];

    const getStreamPos = (u: number, side: "A" | "B") => {
      const r = Math.max(W, H) * 0.56 * Math.pow(1 - u, 1.38);
      const baseAngle = side === "A" ? Math.PI * 0.92 : -Math.PI * 0.08;
      const angle = baseAngle + u * Math.PI * 0.72;
      return {
        x: blastX + Math.cos(angle) * r,
        y: blastY + Math.sin(angle) * r * 0.46,
      };
    };

    const drawStreamTrail = (pts: TrailPoint[], t: number, tailLife: number, rgb: [number, number, number], sprite: HTMLCanvasElement) => {
      if (pts.length < 2) return;
      const [r, g, b] = rgb;
      const fadeOut = t > IMPACT_T ? Math.max(0, 1 - (t - IMPACT_T) / 0.22) : 1;
      if (fadeOut <= 0.01) return;

      ctx.lineCap = "round";
      ctx.lineJoin = "round";

      const layers = [
        { w: 18, a: 0.10, white: false },
        { w: 6.5, a: 0.34, white: false },
        { w: 1.8, a: 0.96, white: true },
      ];

      for (const layer of layers) {
        for (let i = 1; i < pts.length; i++) {
          const p0 = pts[i - 1];
          const p1 = pts[i];
          const age = t - p1.t;
          const ratio = Math.max(0, 1 - age / tailLife);
          const ease = ratio * ratio;
          const alpha = ease * layer.a * fadeOut;
          if (alpha <= 0.005) continue;

          ctx.strokeStyle = layer.white
            ? `rgba(255, 255, 255, ${alpha})`
            : `rgba(${r}, ${g}, ${b}, ${alpha})`;
          ctx.lineWidth = layer.w * (0.2 + 0.8 * ease);
          ctx.beginPath();
          ctx.moveTo(p0.x, p0.y);
          ctx.lineTo(p1.x, p1.y);
          ctx.stroke();
        }
      }

      if (t <= IMPACT_T) {
        const head = pts[pts.length - 1];
        const u = Math.min(1, (t - STREAM_START) / (IMPACT_T - STREAM_START));
        const hs = 64 + 34 * u;
        ctx.globalAlpha = (0.65 + 0.35 * u) * fadeOut;
        ctx.drawImage(sprite, head.x - hs * 0.5, head.y - hs * 0.5, hs, hs);
        ctx.globalAlpha = 1;
      }
    };

    const triggerBurst = () => {
      if (burstDone) return;
      burstDone = true;
      updateMetrics();
      stage.classList.add("is-burst");

      for (let i = 0; i < 95; i++) {
        const angle = Math.random() * Math.PI * 2;
        const spd = 180 + Math.pow(Math.random(), 0.65) * Math.min(W, 1200) * 1.15;
        const vx = Math.cos(angle) * spd * (1 + 0.35 * Math.abs(Math.cos(angle)));
        const vy = Math.sin(angle) * spd * 0.72;
        const isLeft = Math.cos(angle) < 0;
        const color: [number, number, number] = Math.random() < 0.36
          ? [255, 255, 255]
          : (isLeft ? [52, 211, 153] : [96, 165, 250]);

        sparks.push({
          x: blastX + (Math.random() - 0.5) * 10,
          y: blastY + (Math.random() - 0.5) * 8,
          vx,
          vy,
          drag: 3.4 + Math.random() * 2.6,
          life: 0,
          maxLife: 0.65 + Math.random() * 1.1,
          size: 0.8 + Math.random() * 1.35,
          len: 0.032 + Math.random() * 0.02,
          color,
        });
      }

      for (let i = 0; i < 36; i++) {
        const a = Math.random() * Math.PI * 2;
        const d = 24 + Math.random() * Math.min(W, H) * 0.24;
        const drift = 8 + Math.random() * 24;
        ambientDust.push({
          x: blastX + Math.cos(a) * d,
          y: blastY + Math.sin(a) * d * 0.65,
          vx: Math.cos(a) * drift,
          vy: Math.sin(a) * drift - 3,
          life: 0,
          maxLife: 2.6 + Math.random() * 2.2,
          size: 0.7 + Math.random() * 1.2,
          twinkle: Math.random() * Math.PI * 2,
          color: Math.cos(a) < 0 ? [52, 211, 153] : [96, 165, 250],
        });
      }

      timers.push(window.setTimeout(() => {
        stage.classList.add("is-sheen");
      }, 320));
    };

    const render = (now: number) => {
      const dt = Math.min((now - lastTime) / 1000, 1 / 30);
      lastTime = now;
      const t = (now - startTime) / 1000;

      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.globalCompositeOperation = "source-over";
      ctx.fillStyle = "#000000";
      ctx.fillRect(0, 0, W, H);

      ctx.globalCompositeOperation = "lighter";

      const tailLife = 0.24;
      if (t >= STREAM_START && t <= IMPACT_T + 0.28) {
        if (t <= IMPACT_T) {
          const subSteps = 4;
          const t0 = Math.max(STREAM_START, t - dt);
          for (let s = 1; s <= subSteps; s++) {
            const st = t0 + ((t - t0) * s) / subSteps;
            if (st > IMPACT_T) break;
            const rawU = (st - STREAM_START) / (IMPACT_T - STREAM_START);
            const u = Math.pow(rawU, 1.35);
            const pA = getStreamPos(u, "A");
            const pB = getStreamPos(u, "B");
            trailA.push({ x: pA.x, y: pA.y, t: st });
            trailB.push({ x: pB.x, y: pB.y, t: st });
          }
        }

        while (trailA.length && t - trailA[0].t > tailLife) trailA.shift();
        while (trailB.length && t - trailB[0].t > tailLife) trailB.shift();

        drawStreamTrail(trailA, t, tailLife, [52, 211, 153], glowEmerald);
        drawStreamTrail(trailB, t, tailLife, [96, 165, 250], glowBlue);
      }

      if (t >= IMPACT_T && !burstDone) {
        triggerBurst();
      }

      if (burstDone) {
        const et = t - IMPACT_T;
        if (et >= 0 && et < 0.85) {
          const k = et / 0.85;
          const alpha = Math.pow(1 - k, 2.2);

          const coreSize = 260 * (0.45 + 0.95 * (1 - Math.pow(1 - Math.min(1, et / 0.25), 3)));
          ctx.globalAlpha = alpha * 0.95;
          ctx.drawImage(glowWhite, blastX - coreSize * 0.5, blastY - coreSize * 0.5, coreSize, coreSize);
          ctx.globalAlpha = 1;

          const halfW = Math.min(W * 0.48, 480) * (0.35 + 0.65 * (1 - Math.pow(1 - Math.min(1, et / 0.22), 3)));
          const fg = ctx.createLinearGradient(blastX - halfW, blastY, blastX + halfW, blastY);
          fg.addColorStop(0.0, "rgba(52, 211, 153, 0)");
          fg.addColorStop(0.28, `rgba(52, 211, 153, ${0.28 * alpha})`);
          fg.addColorStop(0.46, `rgba(224, 242, 254, ${0.82 * alpha})`);
          fg.addColorStop(0.50, `rgba(255, 255, 255, ${1.0 * alpha})`);
          fg.addColorStop(0.54, `rgba(191, 219, 254, ${0.82 * alpha})`);
          fg.addColorStop(0.72, `rgba(96, 165, 250, ${0.28 * alpha})`);
          fg.addColorStop(1.0, "rgba(96, 165, 250, 0)");
          ctx.fillStyle = fg;
          ctx.fillRect(blastX - halfW, blastY - 1.1, halfW * 2, 2.2);

          const r = 16 + Math.min(W, H) * 0.34 * (1 - Math.pow(1 - k, 3.2));
          ctx.strokeStyle = `rgba(224, 242, 254, ${alpha * 0.35})`;
          ctx.lineWidth = 1.4 * (1 - k * 0.6);
          ctx.beginPath();
          ctx.arc(blastX, blastY, r, 0, Math.PI * 2);
          ctx.stroke();
        }
      }

      for (let i = sparks.length - 1; i >= 0; i--) {
        const s = sparks[i];
        s.life += dt;
        if (s.life >= s.maxLife) {
          sparks.splice(i, 1);
          continue;
        }
        const damp = Math.exp(-s.drag * dt);
        s.vx *= damp;
        s.vy *= damp;
        s.x += s.vx * dt;
        s.y += s.vy * dt;

        const p = s.life / s.maxLife;
        const a = Math.pow(1 - p, 1.8);
        const [r, g, b] = s.color;
        const tx = s.x - s.vx * s.len;
        const ty = s.y - s.vy * s.len;

        const grad = ctx.createLinearGradient(tx, ty, s.x, s.y);
        grad.addColorStop(0, `rgba(${r}, ${g}, ${b}, 0)`);
        grad.addColorStop(0.7, `rgba(${r}, ${g}, ${b}, ${a * 0.8})`);
        grad.addColorStop(1, `rgba(255, 255, 255, ${a})`);

        ctx.strokeStyle = grad;
        ctx.lineWidth = s.size * (1 - p * 0.45);
        ctx.lineCap = "round";
        ctx.beginPath();
        ctx.moveTo(tx, ty);
        ctx.lineTo(s.x, s.y);
        ctx.stroke();
      }

      for (let i = ambientDust.length - 1; i >= 0; i--) {
        const d = ambientDust[i];
        d.life += dt;
        if (d.life >= d.maxLife) {
          ambientDust.splice(i, 1);
          continue;
        }
        d.x += d.vx * dt;
        d.y += d.vy * dt;
        d.vx *= Math.exp(-0.55 * dt);
        d.vy *= Math.exp(-0.55 * dt);

        const p = d.life / d.maxLife;
        const a = Math.min(1, p / 0.14) * Math.pow(1 - p, 1.5) * (0.55 + 0.45 * Math.sin(d.twinkle + d.life * 7)) * 0.55;
        const [r, g, b] = d.color;
        ctx.fillStyle = `rgba(${r}, ${g}, ${b}, ${a})`;
        ctx.beginPath();
        ctx.arc(d.x, d.y, d.size, 0, Math.PI * 2);
        ctx.fill();
      }

      const c = springs.chevron;
      if (t >= c.startAt) {
        c.op = Math.min(1, c.op + dt * 5.5);
        [c.x, c.vx] = stepSpring(c.x, c.vx, 0, 210, 14.5, dt);
        [c.s, c.vs] = stepSpring(c.s, c.vs, 1, 260, 12.5, dt);
        [c.r, c.vr] = stepSpring(c.r, c.vr, 0, 210, 13.5, dt);
      }

      const u = springs.cursor;
      if (t >= u.startAt) {
        u.op = Math.min(1, u.op + dt * 5.5);
        [u.y, u.vy] = stepSpring(u.y, u.vy, 0, 210, 14.5, dt);
        [u.s, u.vs] = stepSpring(u.s, u.vs, 1, 260, 12.5, dt);
      }

      const p = springs.pointer;
      if (t >= p.startAt) {
        p.op = Math.min(1, p.op + dt * 5.5);
        [p.x, p.vx] = stepSpring(p.x, p.vx, 0, 210, 14.5, dt);
        [p.y, p.vy] = stepSpring(p.y, p.vy, 0, 210, 14.5, dt);
        [p.s, p.vs] = stepSpring(p.s, p.vs, 1, 260, 12.5, dt);
        [p.r, p.vr] = stepSpring(p.r, p.vr, 0, 210, 13.5, dt);
      }

      const y = springs.rays;
      if (t >= y.startAt) {
        y.op = Math.min(1, y.op + dt * 8);
        [y.s, y.vs] = stepSpring(y.s, y.vs, 1, 320, 12.0, dt);
        [y.r, y.vr] = stepSpring(y.r, y.vr, 0, 280, 13.0, dt);
      }

      applyLogoTransforms();

      if (!settled && t >= 2.45) {
        settled = true;
        stage.classList.add("is-aura", "is-settled");
      }

      rafId = window.requestAnimationFrame(render);
    };

    rafId = window.requestAnimationFrame(render);

    timers.push(window.setTimeout(() => {
      setExiting(true);
      onFadeStartRef.current?.();
    }, FADE_START_MS));

    timers.push(window.setTimeout(() => {
      onDoneRef.current();
    }, FADE_START_MS + FADE_DURATION_MS));

    return () => {
      window.cancelAnimationFrame(rafId);
      ro.disconnect();
      for (const id of timers) window.clearTimeout(id);
    };
  }, []);

  return (
    <div
      ref={stageRef}
      className={cn("splash-screen", exiting && "is-exiting")}
      data-tauri-drag-region
      aria-label="P2PCLI 启动屏"
    >
      <canvas ref={canvasRef} className="splash-canvas" aria-hidden="true" />

      <div className="splash-center">
        <div className="splash-icon-wrap">
          <div className="splash-icon-aura" />
          <svg className="splash-logo-svg" viewBox="141 103 760 760" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
            <defs>
              <linearGradient id="splash-g-chevron" x1="0" y1="0" x2="1" y2="1">
                <stop offset="0%" stopColor="#6ee7b7" />
                <stop offset="100%" stopColor="#34d399" />
              </linearGradient>
              <linearGradient id="splash-g-cursor" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor="#ffffff" />
                <stop offset="100%" stopColor="#cbd5e1" />
              </linearGradient>
              <linearGradient id="splash-g-pointer" x1="0" y1="0" x2="1" y2="1">
                <stop offset="0%" stopColor="#93c5fd" />
                <stop offset="100%" stopColor="#3b82f6" />
              </linearGradient>
              <path
                id="splash-p2p-pointer"
                d="M 578.89 424.56 A 11.95 11.95 0 0 1 594.56 408.89 L 858.56 516.89 A 12.00 12.00 0 0 1 856.78 539.66 L 752.41 564.65 A 24.00 24.00 0 0 0 734.65 582.38 L 709.69 686.78 A 12.00 12.00 0 0 1 686.89 688.56 Z"
              />
              <mask id="splash-p2p-gap" maskUnits="userSpaceOnUse" x="0" y="0" width="1024" height="1024">
                <rect width="1024" height="1024" fill="#fff" />
                <use ref={maskCutoutRef} href="#splash-p2p-pointer" fill="#000" stroke="#000" strokeWidth={132} strokeLinejoin="round" />
              </mask>
            </defs>

            <g ref={elChevronRef} stroke="url(#splash-g-chevron)" strokeWidth={84} strokeLinecap="round" strokeLinejoin="round" fill="none">
              <path d="M 196 420 L 356 565 L 196 710" />
            </g>

            <g ref={elCursorRef}>
              <rect x={424} y={668} width={316} height={84} rx={42} fill="url(#splash-g-cursor)" mask="url(#splash-p2p-gap)" />
            </g>

            <g ref={elPointerRef}>
              <use href="#splash-p2p-pointer" fill="url(#splash-g-pointer)" stroke="url(#splash-g-pointer)" strokeWidth={60} strokeLinejoin="round" />
            </g>

            <g ref={elRaysRef} fill="none" stroke="url(#splash-g-pointer)" strokeWidth={60} strokeLinecap="round">
              <path d="M 650.00 336.00 L 698.00 290.40" />
              <path d="M 484.40 384.00 L 414.80 364.80" />
              <path d="M 506.00 480.00 L 460.40 528.00" />
              <path d="M 554.00 314.40 L 534.80 244.80" />
            </g>
          </svg>
        </div>

        <div className="splash-wordmark-wrap">
          <div className="splash-wordmark-clip">
            <h1 ref={wordmarkRef} className="splash-wordmark" aria-label="P2PCLI">
              <span className="ch" style={{ ["--tx" as string]: "72px", ["--tdelay" as string]: "40ms" }}>P</span>
              <span className="ch" style={{ ["--tx" as string]: "40px", ["--tdelay" as string]: "18ms" }}>2</span>
              <span className="ch" style={{ ["--tx" as string]: "14px", ["--tdelay" as string]: "0ms" }}>P</span>
              <span className="ch" style={{ ["--tx" as string]: "-14px", ["--tdelay" as string]: "0ms" }}>C</span>
              <span className="ch" style={{ ["--tx" as string]: "-40px", ["--tdelay" as string]: "18ms" }}>L</span>
              <span className="ch" style={{ ["--tx" as string]: "-72px", ["--tdelay" as string]: "40ms" }}>I</span>
            </h1>
          </div>
          <div className="splash-tagline">P2P REMOTE CONTROL &amp; TERMINAL</div>
        </div>
      </div>
    </div>
  );
}
