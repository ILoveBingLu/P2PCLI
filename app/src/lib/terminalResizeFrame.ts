import type { Terminal } from "@xterm/xterm";

/** 保留调整尺寸前的画面，遮住画布重建及远端分批清屏的中间帧。 */
export function terminalResizeFrame(term: Terminal, host: HTMLElement) {
  let frame: HTMLElement | null = null;
  let quietTimer: ReturnType<typeof setTimeout> | undefined;
  let deadline: ReturnType<typeof setTimeout> | undefined;
  let renderFallback: ReturnType<typeof setTimeout> | undefined;
  let rendered: { dispose(): void } | undefined;
  let pendingWrites = 0;
  let disposed = false;
  let generation = 0;

  const cancelRelease = () => {
    clearTimeout(quietTimer);
    clearTimeout(renderFallback);
    rendered?.dispose();
    rendered = undefined;
  };
  const remove = () => {
    cancelRelease();
    clearTimeout(deadline);
    frame?.remove();
    frame = null;
  };
  const release = () => {
    if (!frame || disposed) return;
    cancelRelease();
    // write 的回调只表示解析完成，等真正绘制完才移走旧画面。
    rendered = term.onRender(remove);
    term.refresh(0, term.rows - 1);
    // 隐藏标签页不一定触发 onRender，不能因此永久保留遮罩。
    renderFallback = setTimeout(remove, 150);
  };

  return {
    begin() {
      if (disposed) return;
      generation++;
      cancelRelease();
      if (!frame) {
        const source = host.querySelector<HTMLElement>(".xterm");
        if (!source) return;
        const copy = source.cloneNode(true) as HTMLElement;
        copy.style.width = `${source.clientWidth}px`;
        copy.style.height = `${source.clientHeight}px`;
        copy.removeAttribute("id");
        copy.querySelectorAll("[id]").forEach((node) => node.removeAttribute("id"));
        const originals = source.querySelectorAll("canvas");
        const copies = copy.querySelectorAll("canvas");
        try {
          originals.forEach((canvas, index) => {
            const ctx = copies[index].getContext("2d");
            if (!ctx) throw new Error("canvas unavailable");
            ctx.drawImage(canvas, 0, 0);
          });
        } catch {
          // 截帧失败时正常重绘，不能盖上一张空白图。
          return;
        }
        frame = host.ownerDocument.createElement("div");
        frame.className = "terminal-resize-frame";
        frame.setAttribute("aria-hidden", "true");
        frame.inert = true;
        frame.append(copy);
        host.append(frame);
        // 远端没输出也必须恢复显示；连续 resize 不延长这个上限。
        deadline = setTimeout(remove, 1000);
      }
    },
    write(data: Uint8Array) {
      if (disposed) return;
      const version = generation;
      pendingWrites++;
      if (frame) cancelRelease();
      term.write(data, () => {
        pendingWrites--;
        if (!disposed && frame && version === generation && pendingWrites === 0) {
          // 合并同一次远端重绘的多个数据块，不把清屏后的半帧显示出来。
          quietTimer = setTimeout(release, 80);
        }
      });
    },
    dispose() {
      disposed = true;
      remove();
    },
  };
}
