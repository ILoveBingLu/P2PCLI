import { useEffect, useState } from "react";
import { Chip, Description, Label } from "@heroui/react";
import { api } from "../api";
import { collaborationAgo, type CollaborationDelivery } from "../lib/collaboration";
import { t } from "@/i18n";

function deliveryLabel(state: string) {
  return { pending: t("等待同步"), delivered: t("已送达"), rejected: t("被拒绝"), needs_retry: t("需要你核对") }[state] ?? state;
}
function deliveryColor(state: string) {
  return state === "delivered" ? "success" : state === "pending" ? "warning" : "danger";
}

/** 协调设备连不上时，本机先记着要发的请求，连上再发。这里列出来；没有就不显示 */
export function CollaborationDeliveries() {
  const [items, setItems] = useState<CollaborationDelivery[]>([]);
  const [error, setError] = useState("");
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
  }, []);
  if (!items.length && !error) return null;
  return <div className="collab-deliveries">
    <Label>{t("本机待同步的请求")}</Label>
    <Description>{t("协调设备连不上时，请求先记在本机，连上后自动发过去。")}</Description>
    {error && <p className="collab-error-text" role="alert">{t("读不到：")}{error}</p>}
    {items.map((item) => <div className="collab-delivery" key={item.id}>
      <Chip size="sm" variant="soft" color={deliveryColor(item.state)}>{deliveryLabel(item.state)}</Chip>
      <div>
        <span>{item.operation === "task_create" ? t("新建任务") : item.operation === "member_set" ? t("改设备权限") : t("改任务")}</span>
        <Description>{collaborationAgo(item.updated_at)}{item.last_error ? ` · ${item.last_error}` : ""}</Description>
      </div>
    </div>)}
  </div>;
}
