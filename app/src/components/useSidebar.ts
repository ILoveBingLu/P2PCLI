import { useCallback, useEffect, useState } from "react";

const STORAGE_KEY = "p2pcli.sidebar";
const isMac = navigator.platform.toLowerCase().includes("mac");
// Ctrl+B 是 tmux 前缀和 readline 的光标左移，Windows 上改用 Ctrl+Shift+B，不抢终端的键
export const sidebarShortcut = isMac ? "⌘B" : "Ctrl+Shift+B";

function isShortcut(e: KeyboardEvent) {
  if (e.code !== "KeyB" || e.altKey) return false;
  return isMac
    ? e.metaKey && !e.ctrlKey && !e.shiftKey
    : e.ctrlKey && e.shiftKey && !e.metaKey;
}

/** 侧边栏收起状态，记在本机，快捷键在终端获得焦点时也生效 */
export function useSidebar() {
  const [open, setOpen] = useState(() => {
    try {
      return localStorage.getItem(STORAGE_KEY) !== "collapsed";
    } catch {
      return true;
    }
  });
  const toggle = useCallback(() => setOpen((v) => !v), []);

  useEffect(() => {
    try {
      localStorage.setItem(STORAGE_KEY, open ? "open" : "collapsed");
    } catch {
      // 存不了就只在本次生效
    }
  }, [open]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!isShortcut(e)) return;
      // 捕获阶段拦下，不让 xterm 把它当输入发给对面
      e.preventDefault();
      e.stopPropagation();
      if (!e.repeat) toggle();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [toggle]);

  return { open, toggle };
}
