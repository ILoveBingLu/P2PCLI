import { useEffect, type ReactNode } from "react";
import { createPortal } from "react-dom";

/** 从屏幕底部弹出的面板：点遮罩或按返回关掉 */
export function Sheet({ open, title, onClose, children }: {
  open: boolean;
  title?: string;
  onClose: () => void;
  children: ReactNode;
}) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);
  if (!open) return null;
  return createPortal(
    <div className="m-sheet-root">
      <div className="m-sheet-backdrop" aria-hidden="true" onClick={onClose} />
      <div className="m-sheet" role="dialog" aria-modal="true" aria-label={title}>
        <div className="m-sheet-grip" aria-hidden="true" />
        {title && <h2 className="m-sheet-title">{title}</h2>}
        {children}
      </div>
    </div>,
    document.body,
  );
}

/** 面板里一行可以点的操作 */
export function SheetAction({ icon, label, description, danger, disabled, onPress }: {
  icon: ReactNode;
  label: string;
  description?: string;
  danger?: boolean;
  disabled?: boolean;
  onPress: () => void;
}) {
  return (
    <button type="button" className="m-sheet-action" data-danger={danger ? "" : undefined} disabled={disabled} onClick={onPress}>
      <span className="m-sheet-action-icon" aria-hidden="true">{icon}</span>
      <span className="m-sheet-action-text">
        <span>{label}</span>
        {description && <small>{description}</small>}
      </span>
    </button>
  );
}
