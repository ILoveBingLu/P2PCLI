import { useEffect, useId, useRef, useState, type PointerEvent, type ReactNode } from "react";
import { Circle, Network, X } from "lucide-react";
import { createPortal } from "react-dom";
import { canLink, endpointKey, endpointLabel, type CollaborationEndpoint, type CollaborationLink } from "../lib/collaborationLinks";
import { relationAvailable, relationKey, taskWire, type CollaborationRelation, type WireActivity } from "../lib/collaborationWires";
import { CollaborationWireDetails } from "./CollaborationWireDetails";
import { taskStateLabels } from "../lib/collaboration";
import { useCollaboration } from "./CollaborationContext";
import { Button } from "./ui/button";
import { Tooltip, TooltipTrigger } from "./ui/tooltip";
import "../collaboration-links.css";
import { t } from "@/i18n";

interface Point { x: number; y: number }
interface Drag { source: CollaborationEndpoint; start: Point; end: Point; pointer: number | null }
interface PortContext {
  drag: Drag | null;
  begin: (endpoint: CollaborationEndpoint, event: PointerEvent<HTMLButtonElement>) => void;
  activate: (endpoint: CollaborationEndpoint, element: HTMLButtonElement) => void;
  focus: (endpoint: CollaborationEndpoint | null) => void;
}
interface WireShape {
  id: string; start: Point; end: Point; path: string; length: number; links: CollaborationLink[];
  records: CollaborationRelation[]; activities: WireActivity[]; available: boolean; availableCount: number; selected: boolean; collapsed: boolean;
}
import { createContext, useContext } from "react";
const Ports = createContext<PortContext | null>(null);

function endpointAt(x: number, y: number): CollaborationEndpoint | null {
  const row = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-collab-endpoint]");
  return row?.dataset.collabEndpoint && !row.dataset.collabDisabled ? JSON.parse(row.dataset.collabEndpoint) : null;
}
function pointFor(endpoint: CollaborationEndpoint): Point | null {
  const rows = Array.from(document.querySelectorAll<HTMLElement>("[data-collab-endpoint]"));
  const key = endpointKey(endpoint);
  const row = rows.find((row) => {
    if (row.dataset.collabKey !== key) return false;
    const box = row.getBoundingClientRect();
    const sidebar = row.closest(".connection-sidebar-inner")?.getBoundingClientRect();
    return box.height > 0 && box.width > 0 && !!sidebar && sidebar.width > 0 &&
      box.right > 0 && box.left < window.innerWidth && box.top >= sidebar.top && box.bottom <= sidebar.bottom;
  });
  const button = row?.querySelector<HTMLElement>(".collab-port");
  if (button) {
    const box = button.getBoundingClientRect();
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  }
  return endpoint.terminalId ? pointFor({ deviceId: endpoint.deviceId, deviceName: endpoint.deviceName }) : null;
}
function linePath(start: Point, end: Point) {
  const boundary = document.querySelector(".connection-sidebar")?.getBoundingClientRect().right ?? Infinity;
  const gutter = Math.min(Math.max(start.x, end.x) + 23, boundary - 3);
  if (start.x === end.x && start.y === end.y) {
    return `M ${start.x} ${start.y} C ${gutter} ${start.y - 16}, ${gutter} ${end.y + 16}, ${end.x} ${end.y}`;
  }
  const bend = Math.sign(end.y - start.y) * Math.min(12, Math.abs(end.y - start.y) / 3);
  return `M ${start.x} ${start.y} C ${gutter} ${start.y}, ${gutter} ${start.y}, ${gutter} ${start.y + bend} ` +
    `L ${gutter} ${end.y - bend} C ${gutter} ${end.y}, ${gutter} ${end.y}, ${end.x} ${end.y}`;
}

export function CollaborationPorts({ children }: { children: ReactNode }) {
  const collab = useCollaboration();
  const [drag, setDrag] = useState<Drag | null>(null);
  const dragRef = useRef<Drag | null>(null);
  const suppressClick = useRef(false);
  const [lines, setLines] = useState<WireShape[]>([]);
  const [focused, setFocused] = useState<CollaborationEndpoint | null>(null);
  const [hoveredLine, setHoveredLine] = useState<string | null>(null);
  const [details, setDetails] = useState<CollaborationLink[] | null>(null);
  const markerId = useId().replaceAll(":", "");
  const [reducedMotion, setReducedMotion] = useState(() => window.matchMedia("(prefers-reduced-motion: reduce)").matches);
  const [hover, setHover] = useState<CollaborationEndpoint | null>(null);
  const task = collab.tasks.find((task) => task.id === collab.selected);
  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setReducedMotion(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
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
    const update = () => {
      const groups = new Map<string, WireShape>();
      const selected = task ? taskWire(task, collab.sessions, collab.overview?.endpoint_id ?? "") : null;
      const add = (link: CollaborationLink, record?: CollaborationRelation, activity?: WireActivity) => {
        let start = pointFor(link.source), end = pointFor(link.destination);
        if (!start || !end || !canLink(link.source, link.destination)) return;
        if (start.y > end.y || start.y === end.y && start.x > end.x) [start, end] = [end, start];
        const id = JSON.stringify([start.x, start.y, end.x, end.y]);
        let group = groups.get(id);
        if (!group) {
          group = { id, start, end, path: "", length: 0, links: [], records: [], activities: [], available: true, availableCount: 0, selected: false,
            collapsed: start.x === end.x && start.y === end.y };
          groups.set(id, group);
        }
        if (!group.links.some((item) => relationKey(item) === relationKey(link))) group.links.push(link);
        if (record) {
          group.records.push(record);
          const available = !collab.loading && !collab.error &&
            relationAvailable(link, collab.overview, collab.sessions, collab.members, Date.now());
          group.available &&= available;
          group.availableCount += Number(available);
        }
        if (activity) group.activities.push(activity);
        group.selected ||= !!selected && relationKey(link) === relationKey(selected);
      };
      for (const record of collab.relations) add(record, record);
      for (const activity of collab.activities) add(activity, undefined, activity);
      if (selected) add(selected);
      const next = [...groups.values()];
      const measure = document.createElementNS("http://www.w3.org/2000/svg", "path");
      for (let index = 0; index < next.length; index++) {
        next[index].path = linePath(next[index].start, next[index].end);
        measure.setAttribute("d", next[index].path);
        next[index].length = measure.getTotalLength();
      }
      setLines((current) => JSON.stringify(current) === JSON.stringify(next) ? current : next);
    };
    update();
    const timer = window.setInterval(update, 250);
    return () => window.clearInterval(timer);
  }, [task, collab.sessions, collab.members, collab.relations, collab.activities, collab.error, collab.loading, collab.overview]);

  const activeLine = drag ? { start: pointFor(drag.source) ?? drag.start, end: hover ? pointFor(hover) ?? drag.end : drag.end } : null;
  const touches = (link: CollaborationLink) => !!focused && [link.source, link.destination].some((endpoint) =>
    endpoint.deviceId === focused.deviceId && (!focused.terminalId || endpoint.terminalId === focused.terminalId));
  return <Ports value={{
    drag, focus: setFocused,
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
    {(activeLine || lines.length > 0) && createPortal(<svg className="collab-wire" role="group" aria-label={t("协作关系与传输")}
      data-dragging={!!drag || undefined}>
      <defs>
        <marker id={markerId} viewBox="0 0 8 8" refX="5" refY="4" markerWidth="5" markerHeight="5" orient="auto-start-reverse">
          <path d="M 1 1 L 5 4 L 1 7" fill="none" stroke="context-stroke" strokeWidth="1.5" />
        </marker>
      </defs>
      {lines.map((wire) => {
        const highlighted = hoveredLine === wire.id || wire.links.some(touches) || wire.selected;
        const dimmed = !!(focused || hoveredLine) && !highlighted;
        const names = `${collab.deviceName(wire.links[0].source.deviceId)} ↔ ${collab.deviceName(wire.links[0].destination.deviceId)}`;
        const availability = wire.available ? t("当前可用") : wire.availableCount ? t("{{availableCount}} 条可用", { availableCount: wire.availableCount }) : t("离线或会话不可用");
        const status = wire.records.length ? t("{{length}} 条协作关系 · {{availability}}", { length: wire.records.length, availability }) : wire.selected ? t("任务关系") : t("活动连接");
        const pending = collab.tasks.filter((item) => item.state === "queued" &&
          wire.links.some((link) => relationKey(link) === relationKey(taskWire(item, collab.sessions, collab.overview?.endpoint_id ?? ""))));
        const label = [names, status, ...pending.slice(0, 2).map((item) => t("待领取：{{title}}", { title: item.spec.title })), ...wire.activities.slice(0, 4).map((item) => item.label),
          ...(wire.activities.length > 4 ? [t("另有 {{value}} 项活动", { value: wire.activities.length - 4 })] : [])].join("\n");
        const pulses = new Map<string, { activity: WireActivity; reverse: boolean }>();
        for (const activity of wire.activities) {
          const start = pointFor(activity.source);
          const reverse = !!start && (start.x !== wire.start.x || start.y !== wire.start.y);
          pulses.set(`${activity.kind}:${reverse}`, { activity, reverse });
        }
        return <g key={wire.id} className="collab-relation" data-online={wire.availableCount > 0}
          data-active={wire.activities.length > 0 || undefined}
          data-highlighted={highlighted || undefined} data-dimmed={dimmed || undefined}
          data-failed={wire.activities.some((activity) => activity.failed) || undefined}>
          <path className="collab-relation-path" d={wire.path} markerStart={`url(#${markerId})`} markerEnd={`url(#${markerId})`} />
          {!wire.available && [wire.start, wire.end].map((point, index) =>
            <circle key={index} className="collab-relation-end" cx={point.x} cy={point.y} r="2.5" />)}
          <path className="collab-wire-hit" d={wire.path} role="button" tabIndex={drag ? -1 : 0} aria-label={label}
            strokeDasharray={`${Math.max(0, wire.length - 32)} ${wire.length + 32}`} strokeDashoffset="-16"
            onMouseEnter={() => setHoveredLine(wire.id)} onMouseLeave={() => setHoveredLine(null)}
            onFocus={() => setHoveredLine(wire.id)} onBlur={() => setHoveredLine(null)}
            onClick={() => { if (!drag) setDetails(wire.links); }}
            onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); setDetails(wire.links); } }}>
            <title>{label}</title>
          </path>
          {[...pulses].slice(0, 6).map(([key, { activity, reverse }]) => {
            const path = reverse ? linePath(wire.end, wire.start) : wire.path;
            const end = reverse ? wire.start : wire.end;
            return reducedMotion || activity.failed || wire.collapsed ?
              <circle key={key} className="collab-activity-static" data-kind={activity.kind} data-failed={activity.failed || undefined}
                cx={end.x} cy={end.y} r="3.5"><title>{activity.label}</title></circle> :
              <circle key={`${key}:${activity.id}`} className="collab-pulse" data-kind={activity.kind} data-direction={reverse ? "reverse" : "forward"} r="3">
                <title>{activity.label}</title>
                <animateMotion dur={activity.kind === "file" ? "1.5s" : "1.2s"} repeatCount="indefinite" path={path} />
              </circle>;
          })}
        </g>;
      })}
      {activeLine && <>
        <path className="collab-wire-path" data-state="linking"
          data-valid={drag && hover ? canLink(drag.source, hover) : undefined} d={linePath(activeLine.start, activeLine.end)} />
        <circle className="collab-drag-end" cx={activeLine.end.x} cy={activeLine.end.y} r="3" />
      </>}
    </svg>, document.body)}
    {details && <CollaborationWireDetails links={details} onClose={() => setDetails(null)} />}
  </Ports>;
}

export function CollaborationAnchor({ endpoint, children, disabled = false }: {
  endpoint: CollaborationEndpoint; children: ReactNode; disabled?: boolean;
}) {
  const ports = useContext(Ports);
  const collab = useCollaboration();
  const source = ports?.drag?.source;
  const valid = source ? canLink(source, endpoint) : false;
  const label = `${source ? t("连接到") : t("协作连线")} ${endpointLabel(endpoint)}`;
  return <div className="collab-anchor" data-collab-endpoint={JSON.stringify(endpoint)} data-collab-disabled={disabled || undefined}
    onMouseEnter={() => ports?.focus(endpoint)} onMouseLeave={() => ports?.focus(null)}
    data-collab-key={endpointKey(endpoint)} data-link-source={source && endpointKey(source) === endpointKey(endpoint) || undefined}
    data-link-target={source && valid && !disabled || undefined}>
    <div className="collab-anchor-content">{children}</div>
    <Tooltip content={disabled ? t("设备离线或会话状态未确认") : label}>
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
      <Network /><span>{t("协作")}</span><small>{collab.error ? t("未同步") : collab.loading ? t("同步中") : tasks.length ? t("{{length}} 项", { length: tasks.length }) : t("暂无任务")}</small>
    </Button>
    {collab.linking && <div className="collab-link-status" role="status"><span>{t("选择协作目标")}</span>
      <Button variant="ghost" size="icon-sm" aria-label={t("取消连线")} title={t("取消连线")} onPress={() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))}><X /></Button>
    </div>}
    {selected && <Button variant="ghost" className="collab-sidebar-task" aria-pressed
      onPress={() => collab.setOpen(true)}>
      <span>{selected.spec.title}</span><small>{taskStateLabels()[selected.state]}</small>
    </Button>}
  </div>;
}
