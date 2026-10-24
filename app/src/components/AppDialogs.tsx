import { useCallback, useEffect, useRef, useState } from "react";
import {
  Activity,
  Check,
  Globe2,
  Laptop,
  LoaderCircle,
  Radio,
  RefreshCw,
  Settings2,
  ShieldAlert,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";
import {
  api,
  platformLabel,
  type Diagnosis,
  type IncomingRequest,
  type Overview,
  type SettingsView,
} from "../api";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export function ApproveDialog({
  req,
  onRespond,
}: {
  req: IncomingRequest;
  onRespond: (
    r: IncomingRequest,
    allow: boolean,
    trust: boolean,
  ) => Promise<void>;
}) {
  const [trust, setTrust] = useState(false);
  const [left, setLeft] = useState(60);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const sending = useRef(false);
  const expired = useRef(false);
  const deadline = useRef(Date.now() + 60000);

  const respond = useCallback(
    async (allow: boolean, remember = false) => {
      if (sending.current) return;
      sending.current = true;
      setBusy(true);
      setError(null);
      try {
        await onRespond(req, allow, remember);
      } catch (e) {
        setError(String(e));
      } finally {
        sending.current = false;
        setBusy(false);
      }
    },
    [onRespond, req],
  );

  useEffect(() => {
    const timer = window.setInterval(
      () =>
        setLeft(Math.max(0, Math.ceil((deadline.current - Date.now()) / 1000))),
      1000,
    );
    return () => window.clearInterval(timer);
  }, []);
  useEffect(() => {
    if (left === 0 && !expired.current) {
      expired.current = true;
      void respond(false);
    }
  }, [left, respond]);

  return (
    <AlertDialog open>
      <AlertDialogContent onEscapeKeyDown={(e) => e.preventDefault()}>
        <AlertDialogHeader>
          <div className="dialog-symbol">
            <ShieldCheck />
          </div>
          <AlertDialogTitle>新的连接请求</AlertDialogTitle>
          <AlertDialogDescription>
            一台远程设备请求使用你的本机终端。
          </AlertDialogDescription>
        </AlertDialogHeader>
        <div className="request-device">
          <div className="device-symbol">
            <Laptop />
          </div>
          <div className="min-w-0">
            <strong className="block truncate">{req.name}</strong>
            <span>{platformLabel(req.platform)}</span>
          </div>
          <Badge variant="outline">待授权</Badge>
        </div>
        <div className="fingerprint">
          <span>设备指纹</span>
          <code title={req.remote}>{req.remote.slice(0, 16)}</code>
        </div>
        <Alert variant="warning">
          <ShieldAlert />
          <AlertTitle>请确认你认识这台设备</AlertTitle>
          <AlertDescription>
            允许后，对方可以在你的电脑上执行命令，拥有与你操作终端相同的权限。
          </AlertDescription>
        </Alert>
        <FieldGroup>
          <Field orientation="horizontal">
            <Checkbox
              id="trust-device"
              checked={trust}
              onCheckedChange={(v) => setTrust(v === true)}
              disabled={busy || left === 0}
            />
            <div className="flex flex-col gap-1">
              <FieldLabel htmlFor="trust-device">信任这台设备</FieldLabel>
              <FieldDescription>
                以后连接免密码、免确认。仅建议用于自己的设备。
              </FieldDescription>
            </div>
          </Field>
        </FieldGroup>
        {error && (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}
        <AlertDialogFooter>
          <Button
            variant="outline"
            disabled={busy}
            autoFocus
            onClick={() => respond(false)}
          >
            拒绝{left > 0 ? `（${left}s）` : ""}
          </Button>
          <Button
            disabled={busy || left === 0}
            onClick={() => respond(true, trust)}
          >
            {busy ? (
              <LoaderCircle className="animate-spin" data-icon="inline-start" />
            ) : (
              <Check data-icon="inline-start" />
            )}
            允许连接
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

export function DiagDialog({ onClose }: { onClose: () => void }) {
  const [diagnosis, setDiagnosis] = useState<Diagnosis | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setDiagnosis(await api.diagnose());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);
  useEffect(() => {
    void run();
  }, [run]);
  const d = diagnosis;
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="dialog-wide">
        <DialogHeader>
          <div className="dialog-symbol">
            <Activity />
          </div>
          <DialogTitle>网络诊断</DialogTitle>
          <DialogDescription>
            查看直连条件、网络地址与中继状态。
          </DialogDescription>
        </DialogHeader>
        <div className="dialog-scroll">
          {busy && !d && (
            <div className="loading-state" role="status">
              <LoaderCircle className="animate-spin" />
              <span>正在检测网络，请稍候…</span>
            </div>
          )}
          {error && (
            <Alert variant="destructive">
              <AlertTitle>检测失败</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          {d && (
            <div className="flex flex-col gap-5">
              <Alert
                variant={
                  d.rating === "好"
                    ? "success"
                    : d.rating === "未知"
                      ? "default"
                      : "warning"
                }
              >
                <Radio />
                <AlertTitle>直连条件：{d.rating}</AlertTitle>
                <AlertDescription>{d.summary}</AlertDescription>
              </Alert>
              <dl className="network-details">
                <div>
                  <dt>公网 IPv4</dt>
                  <dd>{d.ipv4_public ?? "未检测到"}</dd>
                </div>
                <div>
                  <dt>公网 IPv6</dt>
                  <dd>{d.ipv6_addrs.join("、") || "未检测到"}</dd>
                </div>
                <div>
                  <dt>NAT 类型</dt>
                  <dd>
                    {
                      {
                        none: "无 NAT",
                        cone: "锥形 NAT",
                        symmetric: "对称 NAT",
                        unknown: "未知",
                      }[d.nat]
                    }
                  </dd>
                </div>
                <div>
                  <dt>中继连接</dt>
                  <dd>
                    {d.relay_connected
                      ? `已连接${d.relay_latency_ms != null ? ` · ${d.relay_latency_ms} ms` : ""}`
                      : "未连接"}
                  </dd>
                </div>
              </dl>
              <Separator />
              <ul className="diagnosis-notes">
                {d.notes.map((note, i) => (
                  <li key={i}>
                    <span />
                    {note}
                  </li>
                ))}
              </ul>
              <p className="detail-note">
                通过 STUN 检测 NAT；检测失败时使用中继探测结果。
              </p>
            </div>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={run} disabled={busy}>
            <RefreshCw
              className={busy ? "animate-spin" : undefined}
              data-icon="inline-start"
            />
            {busy ? "检测中…" : "重新检测"}
          </Button>
          <Button onClick={onClose}>完成</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function SettingsDialog({
  ov,
  onClose,
  onChanged,
}: {
  ov: Overview;
  onClose: () => void;
  onChanged: () => void;
}) {
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [server, setServer] = useState("");
  const [relay, setRelay] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [removing, setRemoving] = useState<string | null>(null);
  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const value = await api.getSettings();
      setSettings(value);
      setServer(value.server ?? "");
      setRelay(value.relay ?? "");
      setName(value.device_name ?? "");
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  const save = async () => {
    if (!settings || saving) return;
    setSaving(true);
    setError(null);
    try {
      await api.saveSettings(server, name, relay);
      await api.restart();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };
  const removeTrusted = async (id: string) => {
    setRemoving(id);
    try {
      await api.removeTrusted(id);
      onChanged();
      toast.success("已移除受信设备");
    } catch (e) {
      toast.error(String(e));
    } finally {
      setRemoving(null);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && !saving && onClose()}>
      <DialogContent className="dialog-wide">
        <DialogHeader>
          <div className="dialog-symbol">
            <Settings2 />
          </div>
          <DialogTitle>设置</DialogTitle>
          <DialogDescription>
            管理本机信息、连接服务器和受信设备。
          </DialogDescription>
        </DialogHeader>
        <Tabs defaultValue="general">
          <TabsList className="w-full">
            <TabsTrigger value="general">
              <Globe2 />
              连接设置
            </TabsTrigger>
            <TabsTrigger value="trusted">
              <ShieldCheck />
              受信设备（{ov.trusted.length}）
            </TabsTrigger>
          </TabsList>
          <div className="dialog-scroll">
            <TabsContent value="general" className="pt-5">
              {loading ? (
                <div className="loading-state" role="status">
                  <LoaderCircle className="animate-spin" />
                  正在读取设置…
                </div>
              ) : !settings ? (
                <Button variant="outline" onClick={load}>
                  重新加载设置
                </Button>
              ) : (
                <FieldGroup className="gap-5">
                  <Field data-disabled={saving}>
                    <FieldLabel htmlFor="device-name">设备名称</FieldLabel>
                    <Input
                      id="device-name"
                      value={name}
                      placeholder={ov.name}
                      onChange={(e) => setName(e.target.value)}
                      disabled={saving}
                    />
                    <FieldDescription>
                      对方看到的名称，留空使用电脑名称。
                    </FieldDescription>
                  </Field>
                  <Field data-disabled={saving}>
                    <FieldLabel htmlFor="server-address">牵线服务器</FieldLabel>
                    <Input
                      id="server-address"
                      value={server}
                      placeholder={
                        settings.effective_server ?? "公钥@IP:端口 或 wss://域名"
                      }
                      onChange={(e) => setServer(e.target.value)}
                      spellCheck={false}
                      disabled={saving}
                    />
                    <FieldDescription>
                      公钥@IP:端口 走 UDP，wss://域名 走 WebSocket。留空使用默认配置。
                    </FieldDescription>
                  </Field>
                  <Field data-disabled={saving}>
                    <FieldLabel htmlFor="relay-address">中继服务器</FieldLabel>
                    <Input
                      id="relay-address"
                      value={relay}
                      placeholder={
                        settings.effective_relay ?? "https://relay.example.com"
                      }
                      onChange={(e) => setRelay(e.target.value)}
                      spellCheck={false}
                      disabled={saving}
                    />
                    <FieldDescription>
                      直连失败时转发加密数据，多个地址用逗号分隔。
                    </FieldDescription>
                  </Field>
                  <Separator />
                  <div className="settings-details">
                    <span>数据目录</span>
                    <code>{settings.data_dir}</code>
                    <span>本机公钥</span>
                    <code>{ov.endpoint_id}</code>
                  </div>
                </FieldGroup>
              )}
            </TabsContent>
            <TabsContent value="trusted" className="pt-5">
              <p className="detail-note mb-4">
                受信设备连接本机时无需密码和确认。移除后，再次连接需要重新授权。
              </p>
              {ov.trusted.length === 0 ? (
                <Empty>
                  <EmptyHeader>
                    <EmptyMedia variant="icon">
                      <ShieldCheck />
                    </EmptyMedia>
                    <EmptyTitle>暂无受信设备</EmptyTitle>
                    <EmptyDescription>
                      在收到连接请求时，可以选择信任自己的设备。
                    </EmptyDescription>
                  </EmptyHeader>
                </Empty>
              ) : (
                <div className="trusted-list">
                  {ov.trusted.map((device) => (
                    <div className="trusted-device" key={device.endpoint_id}>
                      <div className="device-symbol">
                        <Laptop />
                      </div>
                      <div className="min-w-0 flex-1">
                        <strong className="block truncate">
                          {device.name}
                        </strong>
                        <span>{platformLabel(device.platform)}</span>
                      </div>
                      <Button
                        variant="destructive"
                        size="sm"
                        disabled={removing !== null}
                        onClick={() => removeTrusted(device.endpoint_id)}
                      >
                        {removing === device.endpoint_id ? (
                          <LoaderCircle
                            data-icon="inline-start"
                            className="animate-spin"
                          />
                        ) : (
                          <Trash2 data-icon="inline-start" />
                        )}
                        移除
                      </Button>
                    </div>
                  ))}
                </div>
              )}
            </TabsContent>
          </div>
        </Tabs>
        {error && (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={saving}>
            取消
          </Button>
          <Button onClick={save} disabled={loading || !settings || saving}>
            {saving && (
              <LoaderCircle data-icon="inline-start" className="animate-spin" />
            )}
            保存并重启
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
