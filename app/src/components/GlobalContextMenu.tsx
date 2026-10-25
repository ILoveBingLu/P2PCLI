import { useEffect, useRef } from "react";
import { ClipboardPaste, Copy, Scissors, TextSelect } from "lucide-react";
import { toast } from "@/components/ui/toast";
import { useContextMenu, type MenuAction } from "./ContextMenu";

/** 有选区的输入框类型；number、email 这些拿不到 selectionStart */
const TEXT_TYPES = new Set(["text", "search", "url", "tel", "password"]);

type Field = HTMLInputElement | HTMLTextAreaElement;

function editableField(target: EventTarget | null): Field | null {
  if (target instanceof HTMLTextAreaElement) return target;
  if (target instanceof HTMLInputElement && TEXT_TYPES.has(target.type)) return target;
  return null;
}

const fail = (what: string) => (error: unknown) => toast.error(`${what}失败：${String(error)}`);

/** 先发一个粘贴事件，让输入框自己的粘贴处理先接（比如设备码去掉短横线）；没人接再直接插入文字 */
function pasteInto(el: Field, text: string) {
  const data = new DataTransfer();
  data.setData("text/plain", text);
  const event = new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: data });
  // 有的 WebView 不认构造参数里的 clipboardData
  if (!event.clipboardData) Object.defineProperty(event, "clipboardData", { value: data });
  if (el.dispatchEvent(event)) document.execCommand("insertText", false, text);
}

/** 输入框里的剪切、复制、粘贴、全选。用 execCommand 改内容，受控输入框能收到改动，也能撤销 */
function fieldActions(el: Field): MenuAction[] {
  const start = el.selectionStart ?? 0;
  const end = el.selectionEnd ?? 0;
  const selected = el.value.slice(start, end);
  const secret = el instanceof HTMLInputElement && el.type === "password";
  const writable = !el.readOnly && !el.disabled;
  // 菜单会抢走焦点，动手前把焦点和选区还回去
  const restore = () => {
    el.focus();
    el.setSelectionRange(start, end);
  };
  return [
    {
      id: "cut", label: "剪切", icon: <Scissors className="size-4" />, disabled: !selected || secret || !writable,
      onAction: () => void navigator.clipboard.writeText(selected).then(() => {
        restore();
        document.execCommand("delete");
      }, fail("剪切")),
    },
    {
      id: "copy", label: "复制", icon: <Copy className="size-4" />, disabled: !selected || secret,
      onAction: () => void navigator.clipboard.writeText(selected).then(restore, fail("复制")),
    },
    {
      id: "paste", label: "粘贴", icon: <ClipboardPaste className="size-4" />, disabled: !writable,
      onAction: () => void navigator.clipboard.readText().then((text) => {
        restore();
        pasteInto(el, text);
      }, fail("粘贴")),
    },
    {
      id: "select-all", label: "全选", icon: <TextSelect className="size-4" />, separated: true, disabled: !el.value,
      onAction: () => {
        el.focus();
        el.select();
      },
    },
  ];
}

/**
 * 整个窗口都不弹 WebView 自带的右键菜单（重新加载、检查元素那些）。
 * 各处自己的右键菜单照旧；没有的地方：输入框给剪切、复制、粘贴、全选，选中了文字给复制，别的什么都不弹。
 * 开发版按住 Shift 右键还是原生菜单，留着检查元素用
 */
export function GlobalContextMenu() {
  const menu = useContextMenu();
  const open = useRef(menu.open);
  open.current = menu.open;

  useEffect(() => {
    // 挂在 document 上、冒泡阶段：React 的事件先处理，自己弹了菜单的会 preventDefault，这里就不管了
    const onContextMenu = (e: MouseEvent) => {
      if (import.meta.env.DEV && e.shiftKey) return;
      if (e.defaultPrevented) return;
      e.preventDefault();
      const field = editableField(e.target);
      if (field) return open.current(e, fieldActions(field));
      const text = window.getSelection()?.toString() ?? "";
      if (text.trim())
        open.current(e, [{
          id: "copy", label: "复制", icon: <Copy className="size-4" />,
          onAction: () => void navigator.clipboard.writeText(text).catch(fail("复制")),
        }]);
    };
    document.addEventListener("contextmenu", onContextMenu);
    return () => document.removeEventListener("contextmenu", onContextMenu);
  }, []);

  return menu.element;
}
