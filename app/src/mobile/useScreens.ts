import { useCallback, useEffect, useRef, useState } from "react";

export type Screen =
  | { name: "home" }
  | { name: "device"; endpointId: string }
  | { name: "terminal"; key: number }
  | { name: "settings" };

/**
 * 手机上的页面栈。进一页就压一条浏览历史，系统的返回键、返回手势就是退一页。
 * 底部弹出的面板也压一条：返回时先关面板，不退页。
 */
export function useScreens() {
  const [stack, setStack] = useState<Screen[]>([{ name: "home" }]);
  /** 上一次是往里进还是往回退，页面按它从右边滑进来还是从左边 */
  const [dir, setDir] = useState<"forward" | "back" | "none">("none");
  /** 当前历史记录的深度：首页是 1，开着的面板也算一层 */
  const depth = useRef(1);
  /** 开着的面板怎么关；返回键先交给它 */
  const overlay = useRef<(() => void) | null>(null);
  /** 面板从界面上关掉后，退掉它那条历史的定时器。history.back() 是异步的：
   *  关面板紧接着进新页时，先退再进会把新页也退掉，所以晚一拍再退，期间要进新页就直接复用那条历史 */
  const pendingBack = useRef<number | null>(null);
  /** 面板那条历史还在（开着，或者刚关、还没退掉） */
  const takeOverlayEntry = () => {
    const had = overlay.current !== null || pendingBack.current !== null;
    overlay.current = null;
    if (pendingBack.current !== null) window.clearTimeout(pendingBack.current);
    pendingBack.current = null;
    return had;
  };

  useEffect(() => {
    history.replaceState({ depth: 1 }, "");
    const onPop = (e: PopStateEvent) => {
      const d = typeof e.state?.depth === "number" ? e.state.depth : 1;
      depth.current = d;
      const close = overlay.current;
      if (close) {
        overlay.current = null;
        close();
        return;
      }
      setDir("back");
      setStack((s) => (s.length > d ? s.slice(0, Math.max(1, d)) : s));
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  const push = useCallback((screen: Screen) => {
    if (takeOverlayEntry()) {
      // 从面板里进新的一页：面板那条历史直接换成新页，不用先退再进
      history.replaceState({ depth: depth.current }, "");
    } else {
      depth.current += 1;
      history.pushState({ depth: depth.current }, "");
    }
    setDir("forward");
    setStack((s) => [...s, screen]);
  }, []);
  /** 换掉最上面这一页，比如从一个终端切到另一个 */
  const replace = useCallback((screen: Screen) => {
    setDir("none");
    setStack((s) => [...s.slice(0, -1), screen]);
  }, []);
  const back = useCallback(() => {
    // 面板那条历史还在就连它一起退
    const steps = takeOverlayEntry() ? 2 : 1;
    if (depth.current > steps) history.go(-steps);
  }, []);
  /** 一直退回首页 */
  const home = useCallback(() => {
    takeOverlayEntry();
    if (depth.current > 1) history.go(1 - depth.current);
  }, []);
  /** 开面板时调用：返回键会调 close 关掉它 */
  const openOverlay = useCallback((close: () => void) => {
    // 刚关一个面板就开下一个：接着用那条历史
    if (takeOverlayEntry()) {
      overlay.current = close;
      return;
    }
    overlay.current = close;
    depth.current += 1;
    history.pushState({ depth: depth.current, overlay: true }, "");
  }, []);
  /** 面板从界面上关掉时调用，把它那条历史退掉 */
  const closeOverlay = useCallback(() => {
    if (!overlay.current) return;
    overlay.current = null;
    pendingBack.current = window.setTimeout(() => {
      pendingBack.current = null;
      history.back();
    }, 0);
  }, []);

  return { stack, dir, top: stack[stack.length - 1], push, replace, back, home, openOverlay, closeOverlay };
}
