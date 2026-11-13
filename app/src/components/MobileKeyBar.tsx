import type { PointerEvent } from "react";
import { t } from "@/i18n";

/** 手机键盘上没有、终端里又常用的键。data 是发给终端的原始字节 */
const KEYS: { label: string; data: string; title: string }[] = [
  { label: "Esc", data: "\x1b", title: "Esc" },
  { label: "Tab", data: "\t", title: "Tab" },
  { label: "⇧Tab", data: "\x1b[Z", title: "Shift+Tab" },
  { label: "^C", data: "\x03", title: "Ctrl+C" },
  { label: "↑", data: "\x1b[A", title: "↑" },
  { label: "↓", data: "\x1b[B", title: "↓" },
  { label: "←", data: "\x1b[D", title: "←" },
  { label: "→", data: "\x1b[C", title: "→" },
  { label: "^D", data: "\x04", title: "Ctrl+D" },
  { label: "^Z", data: "\x1a", title: "Ctrl+Z" },
  { label: "^L", data: "\x0c", title: "Ctrl+L" },
  { label: "^R", data: "\x12", title: "Ctrl+R" },
  { label: "|", data: "|", title: "|" },
  { label: "~", data: "~", title: "~" },
  { label: "/", data: "/", title: "/" },
  { label: "-", data: "-", title: "-" },
];

/**
 * 手机上终端下面的一排按键。按下时不抢终端的焦点，软键盘不会收起；
 * 横着能滑，放得下全部。
 */
export function MobileKeyBar({ onKeys }: { onKeys: (data: string) => void }) {
  // 在按下那一刻就发并拦住默认行为：点按钮会让终端失焦，软键盘跟着收起
  const press = (data: string) => (e: PointerEvent) => {
    e.preventDefault();
    onKeys(data);
  };
  return (
    <div className="mobile-keybar" role="toolbar" aria-label={t("终端按键")}>
      {KEYS.map((key) => (
        <button
          key={key.title}
          type="button"
          className="mobile-key"
          aria-label={key.title}
          onPointerDown={press(key.data)}
        >
          {key.label}
        </button>
      ))}
    </div>
  );
}
