import { useCallback, useRef, useState, type MouseEvent, type ReactNode } from "react";
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

  const open = (e: MouseEvent, actions: MenuAction[]) => {
    e.preventDefault();
    e.stopPropagation();
    // 键盘上的菜单键触发时没有鼠标位置，就弹在元素左下角
    let { clientX: x, clientY: y } = e;
    if (x === 0 && y === 0) {
      const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
      x = r.left + 12;
      y = r.bottom;
    }
    setState({ x, y, actions });
  };

  const close = useCallback(() => setState(null), []);

  const element = (
    <>
      <div
        ref={anchor}
        aria-hidden="true"
        style={{ position: "fixed", left: state?.x ?? 0, top: state?.y ?? 0, width: 1, height: 1, pointerEvents: "none" }}
      />
      {state && (
        <Dropdown.Popover
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
