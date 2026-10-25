import { useLayoutEffect, useState, type KeyboardEvent, type ReactNode, type RefObject } from "react";
import { ArrowUp, ChevronDown } from "lucide-react";
import type { AgentInfo } from "../api";
import { Button } from "@/components/ui/button";
import { AgentIcon } from "./Agent";

const HISTORY_KEY = "p2pcli.composer.history";
const HISTORY_MAX = 50;
/** 输入框跟着内容长高，最多这么高，再多就在里面滚 */
const MAX_HEIGHT = 180;

export const isMac = navigator.platform.toLowerCase().includes("mac");
/** 在终端和输入框之间切换焦点的快捷键；Windows 上 Ctrl+L 是清屏，所以加 Shift */
export const COMPOSER_SHORTCUT = isMac ? "⌘L" : "Ctrl+Shift+L";

function loadHistory(): string[] {
  try {
    const list = JSON.parse(localStorage.getItem(HISTORY_KEY) ?? "[]");
    return Array.isArray(list) ? list.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function saveHistory(list: string[]) {
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(list));
  } catch {
    // 存不了历史不影响发送
  }
}

/** 认出命令行 AI 时终端下面的输入框：在本机打完一整句再发过去，中文输入法顺、远程不卡、长段落好改。
 *  回车发送，Shift+回车换行；输入框是空的时候上下箭头翻历史；Esc 回到终端。
 *  AI 在等你确认时不让发：这时候发“1”加回车，就等于替你点了“是” */
export function AgentComposer({ agent, draft, onDraft, onSend, onTerminal, onHide, inputRef, tools }: {
  agent: AgentInfo;
  /** 放在输入框右边的东西，比如切模型的按钮 */
  tools?: ReactNode;
  draft: string;
  onDraft: (text: string) => void;
  /** 整段发给终端里的 AI，再补一个回车 */
  onSend: (text: string) => void;
  onTerminal: () => void;
  onHide: () => void;
  inputRef: RefObject<HTMLTextAreaElement | null>;
}) {
  const [history, setHistory] = useState(loadHistory);
  /** 正在翻第几条历史；没在翻是 null */
  const [browsing, setBrowsing] = useState<number | null>(null);
  const blocked = agent.state === "waiting" ? `${agent.name} 在等你确认，先在终端里选` : null;

  useLayoutEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    // scrollHeight 不含边框，框是 border-box，要把上下边框补上，不然差两像素就冒滚动条
    const full = el.scrollHeight + el.offsetHeight - el.clientHeight;
    el.style.height = `${Math.min(full, MAX_HEIGHT)}px`;
    el.style.overflowY = full > MAX_HEIGHT ? "auto" : "hidden";
  }, [draft, inputRef]);

  const submit = () => {
    const text = draft.trim();
    if (!text || blocked) return;
    onSend(text);
    const next = [text, ...history.filter((h) => h !== text)].slice(0, HISTORY_MAX);
    setHistory(next);
    saveHistory(next);
    setBrowsing(null);
    onDraft("");
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    // 输入法选字时按的回车是确认候选词，不是发送
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === "Enter" && !e.shiftKey && !e.altKey && !e.metaKey && !e.ctrlKey) {
      e.preventDefault();
      submit();
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      onTerminal();
      return;
    }
    const el = e.currentTarget;
    // 多行内容里上下箭头先在行间移动，到了第一行 / 最后一行再翻历史
    const onFirstLine = !el.value.slice(0, el.selectionStart).includes("\n");
    const onLastLine = !el.value.slice(el.selectionEnd).includes("\n");
    // 只在输入框是空的、或者已经在翻历史时翻，免得把正在写的内容冲掉
    if (e.key === "ArrowUp" && onFirstLine && (draft === "" || browsing != null) && history.length) {
      e.preventDefault();
      const i = browsing == null ? 0 : Math.min(browsing + 1, history.length - 1);
      setBrowsing(i);
      onDraft(history[i]);
    } else if (e.key === "ArrowDown" && browsing != null && onLastLine) {
      e.preventDefault();
      const i = browsing - 1;
      setBrowsing(i < 0 ? null : i);
      onDraft(i < 0 ? "" : history[i]);
    }
  };

  return (
    <div className="agent-composer" data-blocked={blocked ? "" : undefined}>
      <AgentIcon kind={agent.kind} className="agent-composer-icon size-4 shrink-0" />
      <textarea
        ref={inputRef}
        rows={1}
        value={draft}
        placeholder={blocked ?? `给 ${agent.name} 发消息，回车发送，Shift+回车换行，${COMPOSER_SHORTCUT} 切回终端`}
        aria-label={`给 ${agent.name} 发消息`}
        spellCheck={false}
        onChange={(e) => {
          onDraft(e.target.value);
          setBrowsing(null);
        }}
        onKeyDown={onKeyDown}
      />
      {/* 已经写了字就看不到占位符，单独说一声为什么发不出去；写好的字留着，确认完再发 */}
      {blocked && draft.trim() && <span className="agent-composer-hint">先在终端里确认</span>}
      {tools}
      <Button variant="ghost" size="icon-sm" aria-label="收起输入框" title={`收起输入框（${COMPOSER_SHORTCUT} 再打开）`} onPress={onHide}>
        <ChevronDown />
      </Button>
      <Button size="icon-sm" aria-label="发送" title="发送（回车）" isDisabled={!draft.trim() || !!blocked} onPress={submit}>
        <ArrowUp />
      </Button>
    </div>
  );
}
