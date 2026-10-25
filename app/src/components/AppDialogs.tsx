import { useCallback, useEffect, useRef, useState } from "react";
import {
  Activity,
  Check,
  Laptop,
  LoaderCircle,
  Radio,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
} from "lucide-react";
import { Checkbox, Description } from "@heroui/react";
import {
  api,
  platformLabel,
  type Diagnosis,
  type IncomingRequest,
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
import { Tooltip, TooltipTrigger } from "@/components/ui/tooltip";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  FieldGroup,
} from "@/components/ui/field";
import { Separator } from "@/components/ui/separator";

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
      <AlertDialogContent>
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
          <Tooltip content={req.remote}>
            <TooltipTrigger role="group" aria-label="完整设备指纹" className="ml-auto">
              <code>{req.remote.slice(0, 16)}</code>
            </TooltipTrigger>
          </Tooltip>
        </div>
        <Alert variant="warning">
          <ShieldAlert />
          <AlertTitle>请确认你认识这台设备</AlertTitle>
          <AlertDescription>
            允许后，对方可以在你的电脑上执行命令，拥有与你操作终端相同的权限。
          </AlertDescription>
        </Alert>
        <FieldGroup>
          <div>
            <Checkbox
              isSelected={trust}
              onChange={setTrust}
              isDisabled={busy || left === 0}
            >
              <Checkbox.Content className="gap-3 text-sm">
                <Checkbox.Control>
                  <Checkbox.Indicator>
                    <Check className="size-3" />
                  </Checkbox.Indicator>
                </Checkbox.Control>
                信任这台设备
              </Checkbox.Content>
              <Description className="pl-7 text-xs text-muted-foreground">
                以后连接免密码、免确认。仅建议用于自己的设备。
              </Description>
            </Checkbox>
          </div>
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

export function DiagDialog({ onClose, publicKey }: { onClose: () => void; publicKey: string | null }) {
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
          <section className="mt-5" aria-labelledby="diagnosis-details-title">
            <h3 id="diagnosis-details-title" className="mb-3 text-sm font-medium">详细信息</h3>
            <dl className="network-details">
              <div>
                <dt>本机公钥</dt>
                <dd><code className="select-text break-all">{publicKey ?? "未获取到"}</code></dd>
              </div>
            </dl>
          </section>
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
