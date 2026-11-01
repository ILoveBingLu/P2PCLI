import { useEffect, useState } from "react";
import { Accordion } from "@heroui/react";
import { Clock3, RefreshCw } from "lucide-react";
import { api } from "../api";
import { collaborationTime, shortId, type CollaborationDelivery } from "../lib/collaboration";
import { Button } from "./ui/button";

const labels: Record<string, string> = { pending: "待同步", delivered: "已送达", rejected: "被拒绝", needs_retry: "需人工核对" };
export function CollaborationDeliveries() {
  const [items, setItems] = useState<CollaborationDelivery[]>([]);
  const [error, setError] = useState("");
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let active = true, reading = false;
    const read = async () => {
      if (reading) return;
      reading = true;
      try { const next = await api.collabDeliveries(); if (active) { setItems(next); setError(""); } }
      catch (e) { if (active) setError(String(e)); }
      finally { reading = false; }
    };
    void read();
    const timer = window.setInterval(() => void read(), 3000);
    return () => { active = false; window.clearInterval(timer); };
  }, [revision]);
  const pending = items.filter((item) => item.state !== "delivered").length;
  if (!items.length && !error) return null;
  return <Accordion className="collab-deliveries" hideSeparator>
    <Accordion.Item id="delivery-log">
      <Accordion.Heading level={2}>
        <Accordion.Trigger className="collab-delivery-trigger">
          <Clock3 />
          <span>本机投递记录</span>
          <span className="collab-delivery-count">{pending > 0 ? `${pending} 项待处理` : `${items.length} 项已送达`}</span>
          <Accordion.Indicator />
        </Accordion.Trigger>
      </Accordion.Heading>
      <Accordion.Panel>
        <div className="collab-delivery-content">
          <div className="collab-delivery-heading"><span className="collab-note">投递确认不代表任务执行完成</span><Button size="icon-sm" variant="ghost" title="刷新投递记录" aria-label="刷新投递记录" onPress={() => setRevision((n) => n + 1)}><RefreshCw /></Button></div>
          {error && <p role="alert" className="collab-error-text">投递记录暂不可用：{error}</p>}
          <div className="collab-delivery-list">{items.map((item) => <div className="collab-delivery" key={item.id}>
            <span className="collab-state" data-state={item.state === "pending" ? "blocked" : item.state === "delivered" ? "completed" : "failed"}>{labels[item.state] ?? item.state}</span>
            <div><strong>{item.operation === "task_create" ? "创建任务" : item.operation}</strong><p className="collab-note">{item.target ? shortId(item.target) : "本机"} · {shortId(item.request_id)} · {collaborationTime(item.updated_at)}</p>{item.last_error && <p className="collab-error-text">{item.last_error}</p>}</div>
          </div>)}</div>
        </div>
      </Accordion.Panel>
    </Accordion.Item>
  </Accordion>;
}
