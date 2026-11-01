import { useEffect, useRef, useState, type PointerEvent, type ReactNode } from "react";
import { Circle, Network, X } from "lucide-react";
import { createPortal } from "react-dom";
import { canLink, endpointKey, endpointLabel, taskDestination, taskSource, type CollaborationEndpoint } from "../lib/collaborationLinks";
import { taskStateLabels } from "../lib/collaboration";
import { useCollaboration } from "./CollaborationContext";
import { Button } from "./ui/button";
import { Tooltip, TooltipTrigger } from "./ui/tooltip";
import "../collaboration-links.css";

interface Point { x: number; y: number }
interface Drag { source: CollaborationEndpoint; start: Point; end: Point; pointer: number | null }
interface PortContext {
  drag: Drag | null;
  begin: (endpoint: CollaborationEndpoint, event: PointerEvent<HTMLButtonElement>) => void;
  activate: (endpoint: CollaborationEndpoint, element: HTMLButtonElement) => void;
}
import { createContext, useContext } from "react";
const Ports = createContext<PortContext | null>(null);

function endpointAt(x: number, y: number): CollaborationEndpoint | null {
  const row = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-collab-endpoint]");
  return row?.dataset.collabEndpoint ? JSON.parse(row.dataset.collabEndpoint) : null;
}
function pointFor(endpoint: CollaborationEndpoint): Point | null {
  const rows = Array.from(document.querySelectorAll<HTMLElement>("[data-collab-endpoint]"));
  const key = endpointKey(endpoint);
  const row = rows.find((row) => {
    if (row.dataset.collabKey !== key) return false;
    const box = row.getBoundingClientRect();
    const sidebar = row.closest("aside")?.getBoundingClientRect();
    return box.height > 0 && box.width > 0 && !!sidebar && sidebar.width > 0 &&
      box.top >= sidebar.top && box.bottom <= sidebar.bottom;
  });
  const button = row?.querySelector<HTMLElement>(".collab-port");
  if (button) {
    const box = button.getBoundingClientRect();
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  }
  return endpoint.terminalId ? pointFor({ deviceId: endpoint.deviceId, deviceName: endpoint.deviceName }) : null;
}
function linePath(start: Point, end: Point) {
  const gutter = Math.max(start.x, end.x) + 23;
  return `M ${start.x} ${start.y} C ${gutter} ${start.y}, ${gutter} ${end.y}, ${end.x} ${end.y}`;
}

export function CollaborationPorts({ children }: { children: ReactNode }) {
  const collab = useCollaboration();
  const [drag, setDrag] = useState<Drag | null>(null);
  const dragRef = useRef<Drag | null>(null);
  const suppressClick = useRef(false);
  const [line, setLine] = useState<{ start: Point; end: Point } | null>(null);
  const [hover, setHover] = useState<CollaborationEndpoint | null>(null);
  const task = collab.tasks.find((task) => task.id === collab.selected);
  const setCurrent = (value: Drag | null) => {
    dragRef.current = value; setDrag(value); collab.setLinking(!!value);
  };
  const finish = (destination: CollaborationEndpoint | null) => {
    const current = dragRef.current;
    if (current && destination && canLink(current.source, destination)) {
      collab.setLink({ source: current.source, destination });
    }
    setCurrent(null); setHover(null);
  };

  useEffect(() => {
    if (!drag) return;
    const move = (event: globalThis.PointerEvent) => {
      const current = dragRef.current;
      if (!current || (current.pointer !== null && current.pointer !== event.pointerId)) return;
      if (current.pointer !== null) event.preventDefault();
      const end = { x: event.clientX, y: event.clientY };
      const next = { ...current, end };
      dragRef.current = next; setDrag(next); setHover(endpointAt(end.x, end.y));
      const scroll = document.querySelector<HTMLElement>(".connection-sidebar-inner");
      if (scroll) {
        const box = scroll.getBoundingClientRect();
        if (end.x >= box.left && end.x <= box.right) {
          if (end.y < box.top + 28) scroll.scrollTop -= 14;
          if (end.y > box.bottom - 28) scroll.scrollTop += 14;
        }
      }
    };
    const up = (event: globalThis.PointerEvent) => {
      const current = dragRef.current;
      if (!current || current.pointer !== event.pointerId) return;
      const distance = Math.hypot(event.clientX - current.start.x, event.clientY - current.start.y);
      suppressClick.current = true;
      window.setTimeout(() => { suppressClick.current = false; }, 0);
      if (distance < 5) setCurrent({ ...current, pointer: null });
      else finish(endpointAt(event.clientX, event.clientY));
    };
    const cancel = () => setCurrent(null);
    const key = (event: KeyboardEvent) => { if (event.key === "Escape") { event.preventDefault(); cancel(); } };
    window.addEventListener("pointermove", move, { passive: false });
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("blur", cancel);
    window.addEventListener("keydown", key, true);
    return () => {
      window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel); window.removeEventListener("blur", cancel);
      window.removeEventListener("keydown", key, true);
    };
  // Keep listeners stable while the pointer moves; callbacks use the current drag ref.
  }, [!!drag]);

  useEffect(() => {
    if (!task || drag) { setLine(null); return; }
    const update = () => {
      const session = collab.sessions.find((session) => session.id === task.spec.assignee.session_id && session.device_id === task.spec.assignee.device_id);
      const start = pointFor(taskSource(task));
      const destination = taskDestination(task);
      const end = pointFor({ ...destination, terminalId: destination.deviceId === task.spec.assignee.device_id ? session?.terminal?.terminal_id : undefined });
      setLine((current) => {
        if (!start || !end) return null;
        return current?.start.x === start.x && current.start.y === start.y && current.end.x === end.x && current.end.y === end.y
          ? current : { start, end };
      });
    };
    update();
    const timer = window.setInterval(update, 250);
    return () => window.clearInterval(timer);
  }, [task, collab.sessions, !!drag]);

  const activeLine = drag ? { start: pointFor(drag.source) ?? drag.start, end: hover ? pointFor(hover) ?? drag.end : drag.end } : line;
  return <Ports value={{
    drag,
    begin: (endpoint, event) => {
      if (event.button !== 0 || collab.busy || collab.link) return;
      if (dragRef.current?.pointer === null) return;
      event.preventDefault(); event.stopPropagation();
      suppressClick.current = false;
      event.currentTarget.focus();
      setCurrent({ source: endpoint, start: { x: event.clientX, y: event.clientY }, end: { x: event.clientX, y: event.clientY }, pointer: event.pointerId });
    },
    activate: (endpoint, element) => {
      if (suppressClick.current) { suppressClick.current = false; return; }
      if (dragRef.current) { finish(endpoint); return; }
      const box = element.getBoundingClientRect();
      const point = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
      setCurrent({ source: endpoint, start: point, end: point, pointer: null });
    },
  }}>
    {children}
    {activeLine && createPortal(<svg className="collab-wire" aria-hidden="true">
      <path className="collab-wire-path" data-state={drag ? "linking" : task?.state}
        data-valid={drag && hover ? canLink(drag.source, hover) : undefined} d={linePath(activeLine.start, activeLine.end)} />
      {!drag && task && <path className="collab-wire-hit" d={linePath(activeLine.start, activeLine.end)}
        onClick={() => collab.setOpen(true)} />}
      <circle cx={activeLine.end.x} cy={activeLine.end.y} r="3" />
    </svg>, document.body)}
  </Ports>;
}

export function CollaborationAnchor({ endpoint, children, disabled = false }: {
  endpoint: CollaborationEndpoint; children: ReactNode; disabled?: boolean;
}) {
  const ports = useContext(Ports);
  const collab = useCollaboration();
  const source = ports?.drag?.source;
  const valid = source ? canLink(source, endpoint) : false;
  const label = `${source ? "连接到" : "协作连线"} ${endpointLabel(endpoint)}`;
  return <div className="collab-anchor" data-collab-endpoint={disabled ? undefined : JSON.stringify(endpoint)}
    data-collab-key={endpointKey(endpoint)} data-link-source={source && endpointKey(source) === endpointKey(endpoint) || undefined}
    data-link-target={source && valid && !disabled || undefined}>
    <div className="collab-anchor-content">{children}</div>
    <Tooltip content={disabled ? "设备离线或会话状态未确认" : label}>
      <TooltipTrigger<"button"> render={(props) => <button {...props} type="button" className="collab-port"
        aria-label={label} aria-pressed={source ? endpointKey(source) === endpointKey(endpoint) : false}
        disabled={disabled || collab.busy || !!collab.link}
        onPointerDown={(event) => ports?.begin(endpoint, event)}
        onClick={(event) => { event.stopPropagation(); ports?.activate(endpoint, event.currentTarget); }}><Circle /></button>} />
    </Tooltip>
  </div>;
}

export function CollaborationSidebarStatus() {
  const collab = useCollaboration();
  const tasks = collab.tasks.filter((task) => !["completed", "cancelled"].includes(task.state));
  const selected = collab.tasks.find((task) => task.id === collab.selected);
  return <div className="collab-sidebar-status" data-linking={collab.linking || undefined}>
    <Button variant={collab.open ? "secondary" : "ghost"} className="collab-sidebar-toggle"
      onPress={() => collab.setOpen(!collab.open)}>
      <Network /><span>协作</span><small>{collab.error ? "未同步" : collab.loading ? "同步中" : tasks.length ? `${tasks.length} 项` : "暂无任务"}</small>
    </Button>
    {collab.linking && <div className="collab-link-status" role="status"><span>选择协作目标</span>
      <Button variant="ghost" size="icon-sm" aria-label="取消连线" title="取消连线" onPress={() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))}><X /></Button>
    </div>}
    {selected && <Button variant="ghost" className="collab-sidebar-task" aria-pressed
      onPress={() => collab.setOpen(true)}>
      <span>{selected.spec.title}</span><small>{taskStateLabels[selected.state]}</small>
    </Button>}
  </div>;
}
