import { useCallback, useEffect, useRef, useState, type PointerEvent, type ReactNode } from "react";

/** 滑块离上下边缘留的空 */
const EDGE = 4;
/** 内容很长时滑块也不小于这么高 */
const MIN_THUMB = 28;
/** 停止滚动后过这么久淡出 */
const HIDE_AFTER_MS = 900;

/** 悬浮滚动条：系统滚动条藏起来，右边叠一根细滑块，不占内容宽度。
 *  滚动时、鼠标移进来时出现，停下一会儿淡出；滑块可以拖。Windows 的 WebView2 没有这种原生滚动条，所以自己画 */
export function OverlayScroll({ className, children }: { className?: string; children: ReactNode }) {
  const viewport = useRef<HTMLDivElement>(null);
  const hideTimer = useRef(0);
  const drag = useRef<{ y: number; top: number } | null>(null);
  const [thumb, setThumb] = useState({ top: 0, height: 0 });
  const [scrolling, setScrolling] = useState(false);
  const [dragging, setDragging] = useState(false);

  const measure = useCallback(() => {
    const el = viewport.current;
    if (!el) return;
    const { scrollTop, scrollHeight, clientHeight } = el;
    if (scrollHeight <= clientHeight + 1) {
      setThumb({ top: 0, height: 0 });
      return;
    }
    const track = clientHeight - EDGE * 2;
    const height = Math.max(MIN_THUMB, (track * clientHeight) / scrollHeight);
    const top = EDGE + ((track - height) * scrollTop) / (scrollHeight - clientHeight);
    setThumb({ top, height });
  }, []);

  // 窗口大小、内容高度变了都要重算滑块
  useEffect(() => {
    const el = viewport.current;
    if (!el) return;
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    for (const child of Array.from(el.children)) observer.observe(child);
    const mutations = new MutationObserver(() => {
      for (const child of Array.from(el.children)) observer.observe(child);
      measure();
    });
    mutations.observe(el, { childList: true });
    return () => {
      observer.disconnect();
      mutations.disconnect();
      window.clearTimeout(hideTimer.current);
    };
  }, [measure]);

  const onScroll = () => {
    measure();
    setScrolling(true);
    window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => setScrolling(false), HIDE_AFTER_MS);
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    const el = viewport.current;
    if (!el || e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { y: e.clientY, top: el.scrollTop };
    setDragging(true);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const el = viewport.current;
    if (!el || !drag.current) return;
    // 滑块走一像素，内容按比例走多少
    const track = el.clientHeight - EDGE * 2 - thumb.height;
    if (track <= 0) return;
    el.scrollTop = drag.current.top + ((e.clientY - drag.current.y) * (el.scrollHeight - el.clientHeight)) / track;
  };
  const endDrag = () => {
    drag.current = null;
    setDragging(false);
  };

  return (
    <div className={`overlay-scroll${className ? ` ${className}` : ""}`} data-scrolling={scrolling || dragging || undefined}>
      <div ref={viewport} className="overlay-scroll-viewport" onScroll={onScroll}>
        {children}
      </div>
      {thumb.height > 0 && (
        <div
          className="overlay-scroll-thumb"
          data-dragging={dragging || undefined}
          style={{ height: thumb.height, transform: `translateY(${thumb.top}px)` }}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          aria-hidden="true"
        />
      )}
    </div>
  );
}
