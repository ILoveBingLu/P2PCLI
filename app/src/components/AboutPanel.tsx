import type { ReactNode } from "react";
import { ArrowUpRight, Bug, Download, LoaderCircle, Lock, Package, RefreshCw, Route, Scale } from "lucide-react";
import logo from "@/assets/logo.svg";
import { api } from "../api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { progressText, type useUpdater } from "./useUpdater";

/** 项目主页，和后端 links.rs 里的 REPO_URL 一致；后端只认这个地址和它下面的页面 */
const REPO_URL = "https://github.com/ILoveBingLu/P2PCLI";

/** lucide 没有品牌图标，用 GitHub 官方的 mark */
function GithubMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true" className={className} data-icon="inline-start">
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" />
    </svg>
  );
}

function Feature({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5 rounded-xl border border-border bg-card p-3.5">
      <div className="flex items-center gap-2 text-sm font-medium">
        <span className="flex size-6 items-center justify-center rounded-md bg-muted-surface text-muted-foreground [&>svg]:size-3.5">{icon}</span>
        {title}
      </div>
      <p className="text-xs leading-relaxed text-muted-foreground">{children}</p>
    </div>
  );
}

/** 设置 → 关于：项目入口、特点、更新 */
export function AboutPanel({ updater }: { updater: ReturnType<typeof useUpdater> }) {
  const open = (path = "") => {
    api.openUrl(REPO_URL + path).catch((e) => toast.error(String(e)));
  };
  const status = updater.installing
    ? progressText(updater.progress)
    : updater.checking
      ? "正在检查更新…"
      : updater.error
        ? updater.error
        : updater.update
          ? `发现新版本 v${updater.update.version}`
          : updater.checkedAt
            ? "已经是最新版本"
            : "启动后会自动检查更新，之后每 6 小时检查一次。";

  return (
    <div className="flex flex-col gap-4">
      <section className="relative overflow-hidden rounded-2xl border border-border bg-card p-6">
        {/* 光晕用 logo 里的两个颜色，只在角落淡淡地透出来 */}
        <div aria-hidden="true" className="pointer-events-none absolute -left-20 -top-24 size-64 rounded-full bg-[#34d399]/15 blur-3xl" />
        <div aria-hidden="true" className="pointer-events-none absolute -bottom-28 -right-16 size-72 rounded-full bg-[#60a5fa]/15 blur-3xl" />
        <div className="relative flex flex-col gap-5">
          <div className="flex items-center gap-4">
            <img src={logo} alt="" draggable={false} className="size-16 shrink-0 object-contain" />
            <div className="flex min-w-0 flex-col gap-1">
              <div className="flex items-center gap-2">
                <h2 className="text-xl font-semibold tracking-tight">P2PCLI</h2>
                <Badge className="border border-[#b78c3f]/50 bg-[#17130c] text-[#e7c477]">Beta</Badge>
              </div>
              <p className="text-sm text-muted-foreground">点对点远程终端</p>
              {updater.version && <p className="text-xs tabular-nums text-muted-foreground">v{updater.version}</p>}
            </div>
          </div>
          <p className="max-w-[46ch] text-sm leading-relaxed text-muted-foreground">
            输入对方的设备码，对方点同意，就能在软件里用它的终端。Claude Code、Codex 这类命令行 AI 直接用对方电脑上的账号和环境运行。
            项目开源，觉得好用欢迎点个 Star，遇到问题也可以直接提。
          </p>
          <div className="flex flex-wrap gap-2">
            <Button onClick={() => open()} aria-label="在浏览器中打开 GitHub 项目">
              <GithubMark className="size-4" />
              GitHub 项目
              <ArrowUpRight data-icon="inline-end" />
            </Button>
            <Button variant="outline" onClick={() => open("/issues")}>
              <Bug data-icon="inline-start" />反馈问题
            </Button>
            <Button variant="outline" onClick={() => open("/releases")}>
              <Package data-icon="inline-start" />版本发布
            </Button>
          </div>
        </div>
      </section>

      <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
        <Feature icon={<Lock />} title="端到端加密">连接两端互相认证，中继只转发，看不到内容。</Feature>
        <Feature icon={<Route />} title="直连优先">IPv6 直连、IPv4 打洞，实在不通才走中继兜底。</Feature>
        <Feature icon={<Scale />} title="MIT 开源">代码都在 GitHub 上，随时可以看、可以改。</Feature>
      </div>

      <section className="flex flex-col gap-2 rounded-xl border border-border bg-card p-4">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="text-sm font-medium">软件更新</span>
            <span className="text-xs text-muted-foreground" aria-live="polite">{status}</span>
          </div>
          <div className="flex flex-wrap gap-2">
            {updater.update ? (
              <Button size="sm" onClick={updater.install} disabled={updater.installing}>
                {updater.installing
                  ? <LoaderCircle data-icon="inline-start" className="animate-spin" />
                  : <Download data-icon="inline-start" />}
                更新并重启
              </Button>
            ) : null}
            <Button variant="outline" size="sm" onClick={() => void updater.check()} disabled={updater.checking || updater.installing}>
              {updater.checking
                ? <LoaderCircle data-icon="inline-start" className="animate-spin" />
                : <RefreshCw data-icon="inline-start" />}
              检查更新
            </Button>
          </div>
        </div>
        {updater.update?.notes && !updater.installing && (
          <p className="whitespace-pre-line border-t border-border pt-2 text-xs text-muted-foreground">{updater.update.notes}</p>
        )}
      </section>

      <p className="text-center text-xs text-muted-foreground">© 2026 ILoveBingLu · MIT 开源协议</p>
    </div>
  );
}
