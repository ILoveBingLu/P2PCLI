import { useRef, useState } from "react";
import {
  ArrowUpRight,
  Check,
  Copy,
  Eye,
  EyeOff,
  History,
  Laptop,
  LoaderCircle,
  LockKeyhole,
  Monitor,
  RefreshCw,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";
import {
  api,
  formatCode,
  platformLabel,
  type ConnectFail,
  type Overview,
  type PathInfo,
  type RemoteInfo,
} from "../api";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";

export function ServerStatus({ ov }: { ov: Overview | null }) {
  const state = ov?.server.state;
  const label = {
    none: "未配置服务器",
    connecting: "连接中",
    online: "在线",
    offline: "离线",
  };
  return (
    <span
      className="server-status"
      data-state={state ?? "loading"}
      title={ov?.server.state === "offline" ? ov.server.error : undefined}
    >
      <span className="status-dot" />
      {state ? label[state] : "加载中"}
    </span>
  );
}

interface Props {
  ov: Overview | null;
  onConnected: (
    connId: number,
    info: RemoteInfo,
    code: string | null,
    path: PathInfo,
  ) => void;
  onRefresh: () => void;
}

export function ConnectionSidebar({
  ov,
  onConnected,
  onRefresh,
}: Props) {
  const [target, setTarget] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [showPassword, setShowPassword] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const pwRef = useRef<HTMLInputElement>(null);
  const connecting = useRef(false);

  const copy = async (text: string, what: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(what);
      toast.success(`${what}已复制`);
      window.setTimeout(
        () => setCopied((value) => (value === what ? null : value)),
        1800,
      );
    } catch {
      toast.error("复制失败，请手动选择并复制");
    }
  };

  const doConnect = async (t: string, pw: string) => {
    if (connecting.current || !t.trim()) return;
    connecting.current = true;
    setBusy(true);
    setError(null);
    try {
      const c = await api.connect(t.trim(), pw || null);
      onConnected(c.conn_id, c.info, c.code, c.path);
      setPassword("");
    } catch (e) {
      const f = e as ConnectFail;
      setError(f.message ?? String(e));
      if (f.reject === "need_password" || f.reject === "bad_password") {
        setTarget(formatCode(t.replace(/\D/g, "")) || t);
        window.setTimeout(() => pwRef.current?.focus(), 0);
      }
    } finally {
      connecting.current = false;
      setBusy(false);
    }
  };

  if (window.__ptTest) window.__ptTest.connect = doConnect;

  const refreshPassword = async () => {
    setRefreshing(true);
    try {
      await api.refreshPassword();
      onRefresh();
      toast.success("临时密码已更新");
    } catch (e) {
      toast.error(`更新失败：${String(e)}`);
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <aside className="sidebar" aria-label="设备与连接">
      <div className="sidebar-scroll">
        <Card className="device-card">
          <CardHeader>
            <CardTitle>
              <span className="section-heading">
                <Monitor />
                本机设备
              </span>
            </CardTitle>
            <ServerStatus ov={ov} />
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            <div>
              <div className="credential-label">设备码</div>
              <div className="credential-row">
                {ov?.code ? (
                  <span className="device-code">{formatCode(ov.code)}</span>
                ) : (
                  <span className="credential-placeholder">
                    {ov?.server.state === "none"
                      ? "等待配置服务器"
                      : "正在获取设备码…"}
                  </span>
                )}
                <Button
                  variant="ghost"
                  size="icon-sm"
                  disabled={!ov?.code}
                  aria-label="复制设备码"
                  title="复制设备码"
                  onClick={() => ov?.code && copy(ov.code, "设备码")}
                >
                  {copied === "设备码" ? <Check /> : <Copy />}
                </Button>
              </div>
            </div>
            <div>
              <div className="credential-label">临时密码</div>
              <div className="credential-row">
                <span className="device-password">
                  {ov ? (showPassword ? ov.password : "••••••") : "—"}
                </span>
                <div className="flex shrink-0 items-center">
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={!ov}
                    aria-label={showPassword ? "隐藏临时密码" : "显示临时密码"}
                    title={showPassword ? "隐藏临时密码" : "显示临时密码"}
                    onClick={() => setShowPassword(!showPassword)}
                  >
                    {showPassword ? <EyeOff /> : <Eye />}
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={!ov}
                    aria-label="复制临时密码"
                    title="复制临时密码"
                    onClick={() => ov && copy(ov.password, "临时密码")}
                  >
                    {copied === "临时密码" ? <Check /> : <Copy />}
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    disabled={!ov || refreshing}
                    aria-label="刷新临时密码"
                    title="刷新密码，旧密码立即失效"
                    onClick={refreshPassword}
                  >
                    {refreshing ? (
                      <LoaderCircle className="animate-spin" />
                    ) : (
                      <RefreshCw />
                    )}
                  </Button>
                </div>
              </div>
            </div>
          </CardContent>
          <CardFooter>
            <div className="device-caption">
              <Laptop />
              <span className="truncate">{ov?.name ?? "正在读取本机信息"}</span>
              <span className="shrink-0">
                {ov && platformLabel(ov.platform)}
              </span>
            </div>
          </CardFooter>
        </Card>

        <section className="connect-section" aria-labelledby="connect-title">
          <div className="section-title">
            <h2 id="connect-title">连接远程设备</h2>
            <ArrowUpRight />
          </div>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void doConnect(target, password);
            }}
          >
            <FieldGroup className="gap-3">
              <Field data-disabled={busy}>
                <FieldLabel htmlFor="remote-code">设备码</FieldLabel>
                <Input
                  id="remote-code"
                  placeholder="例如 123 456 789"
                  value={target}
                  onChange={(e) => setTarget(e.target.value)}
                  disabled={busy}
                  spellCheck={false}
                  autoComplete="off"
                />
              </Field>
              <Field data-disabled={busy}>
                <FieldLabel htmlFor="remote-password">临时密码</FieldLabel>
                <Input
                  id="remote-password"
                  ref={pwRef}
                  type="password"
                  placeholder="输入对方的临时密码"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  disabled={busy}
                  spellCheck={false}
                  autoComplete="off"
                />
              </Field>
              {error && (
                <Alert variant="destructive">
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              )}
              <Button
                type="submit"
                className="w-full"
                disabled={busy || !target.trim()}
              >
                {busy ? (
                  <LoaderCircle
                    data-icon="inline-start"
                    className="animate-spin"
                  />
                ) : (
                  <ArrowUpRight data-icon="inline-start" />
                )}
                {busy ? "等待对方同意…" : "连接设备"}
              </Button>
            </FieldGroup>
          </form>
          <div className="connect-note">
            <LockKeyhole />
            连接前需要对方授权
          </div>
        </section>

        <Separator />
        <section className="recent-section" aria-labelledby="recent-title">
          <div className="section-title">
            <h2 id="recent-title">
              <History />
              最近连接
            </h2>
            <span className="item-count">{ov?.recent.length ?? 0}</span>
          </div>
          {ov && ov.recent.length > 0 ? (
            <div className="recent-list">
              {ov.recent.map((r) => (
                <div className="recent-device" key={r.code}>
                  <Button
                    variant="ghost"
                    className="h-auto min-w-0 flex-1 justify-start px-2 py-3"
                    disabled={busy}
                    onClick={() => doConnect(r.code, password)}
                    title={`连接 ${r.name}`}
                  >
                    <Monitor data-icon="inline-start" />
                    <span className="recent-copy">
                      <span className="truncate">{r.name}</span>
                      <span className="recent-meta">
                        {platformLabel(r.platform)} · {formatCode(r.code)}
                      </span>
                    </span>
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`移除最近连接 ${r.name}`}
                    title="从最近连接移除"
                    onClick={() =>
                      api
                        .forgetRecent(r.code)
                        .then(onRefresh)
                        .catch((e) => toast.error(String(e)))
                    }
                  >
                    <Trash2 />
                  </Button>
                </div>
              ))}
            </div>
          ) : (
            <p className="recent-placeholder">连接过的设备会出现在这里。</p>
          )}
        </section>
      </div>
      <div className="sidebar-footer">
        <div className="sidebar-security">
          <ShieldCheck />
          端到端加密 · 点对点连接
        </div>
      </div>
    </aside>
  );
}
