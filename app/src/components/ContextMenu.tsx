import { useCallback, useEffect, useRef, useState, type MouseEvent, type ReactNode } from "react";
import { Dropdown, Label, dropdownVariants } from "@heroui/react";
import { cn } from "@/lib/utils";

// 只用了 Dropdown 的弹层和菜单，没有外面那层 Root，样式要自己套上
const slots = dropdownVariants();

export interface MenuAction {
  id: string;
  label: string;
  icon?: ReactNode;
  /** 删除这类操作，显示成红色 */
  danger?: boolean;
  disabled?: boolean;
  /** 在这一项上面画一条分隔线 */
  separated?: boolean;
  onAction: () => void;
}

/**
 * 右键菜单：组件库没有现成的，用 Dropdown 的菜单做，弹在鼠标位置。
 * 用法：const menu = useContextMenu(); <div onContextMenu={(e) => menu.open(e, actions)} /> 再把 menu.element 放进页面。
 */
export function useContextMenu() {
  const [state, setState] = useState<{ x: number; y: number; actions: MenuAction[] } | null>(null);
  const anchor = useRef<HTMLDivElement>(null);
  const popover = useRef<HTMLDivElement>(null);

  const open = (e: MouseEvent | globalThis.MouseEvent, actions: MenuAction[]) => {
    e.preventDefault();
    e.stopPropagation();
    // 键盘上的菜单键触发时没有鼠标位置，就弹在元素左下角；挂在 document 上的监听拿不到元素，用事件落在的那个
    let { clientX: x, clientY: y } = e;
    if (x === 0 && y === 0) {
      const el = e.currentTarget instanceof Element ? e.currentTarget : e.target instanceof Element ? e.target : null;
      const r = el?.getBoundingClientRect();
      x = (r?.left ?? 0) + 12;
      y = r?.bottom ?? 0;
    }
    setState({ x, y, actions });
  };

  const close = useCallback(() => setState(null), []);
  const isOpen = state !== null;

  useEffect(() => {
    if (!isOpen) return;
    const onOutside = (e: Event) => {
      if (popover.current && !e.composedPath().includes(popover.current)) close();
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        close();
      }
    };
    // 非模态弹层不会自动响应外部点击；捕获阶段也能接住终端等区域拦截的事件。
    document.addEventListener("pointerdown", onOutside, true);
    document.addEventListener("contextmenu", onOutside, true);
    document.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("pointerdown", onOutside, true);
      document.removeEventListener("contextmenu", onOutside, true);
      document.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("blur", close);
    };
  }, [isOpen, close]);

  const element = (
    <>
      <div
        ref={anchor}
        aria-hidden="true"
        style={{ position: "fixed", left: state?.x ?? 0, top: state?.y ?? 0, width: 1, height: 1, pointerEvents: "none" }}
      />
      {state && (
        <Dropdown.Popover
          ref={popover}
          isOpen
          // 右键菜单不能遮住背后的操作区域，否则再次右键会落到透明遮罩上。
          isNonModal
          onContextMenu={(e) => { e.preventDefault(); e.stopPropagation(); }}
          onOpenChange={(open) => !open && close()}
          triggerRef={anchor}
          placement="bottom start"
          offset={2}
          className={cn(slots.popover(), "min-w-44")}
        >
          <Dropdown.Menu
            className={slots.menu()}
            aria-label="操作"
            autoFocus="first"
            disabledKeys={state.actions.filter((a) => a.disabled).map((a) => a.id)}
            onAction={(key) => {
              const action = state.actions.find((a) => a.id === key);
              close();
              action?.onAction();
            }}
          >
            {state.actions.map((a) => (
              <Dropdown.Item
                key={a.id}
                id={a.id}
                textValue={a.label}
                variant={a.danger ? "danger" : undefined}
                className={a.separated ? "context-menu-separated" : undefined}
              >
                {a.icon}
                <Label>{a.label}</Label>
              </Dropdown.Item>
            ))}
          </Dropdown.Menu>
        </Dropdown.Popover>
      )}
    </>
  );

  return { open, close, element };
}
