// 用本机 VS Code（或 Cursor）的 Remote-SSH 打开对方电脑上的文件夹：SSH 走 P2PCLI 的管道，不用对方开公网端口
import { useEffect, useState } from "react";
import { Code2, LoaderCircle } from "lucide-react";
import { api } from "../api";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { toast } from "@/components/ui/toast";
import { t, tr } from "@/i18n";

type Editor = "vscode" | "cursor";
const EDITOR_KEY = "p2pcli.ssh.editor";
function loadEditor(): Editor {
  try { return localStorage.getItem(EDITOR_KEY) === "cursor" ? "cursor" : "vscode"; } catch { return "vscode"; }
}

export function OpenInEditorDialog({ open, onOpenChange, connId, deviceName }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  connId: number;
  deviceName: string;
}) {
  const [path, setPath] = useState("~");
  const [editor, setEditor] = useState<Editor>(loadEditor);
  const [busy, setBusy] = useState(false);
  /** 还没在 ~/.ssh/config 里加 Include：先问一句 */
  const [needInclude, setNeedInclude] = useState<string | null>(null);

  useEffect(() => {
    if (open) setNeedInclude(null);
  }, [open]);

  const editorName = editor === "cursor" ? "Cursor" : "VS Code";
  const run = async (addInclude: boolean) => {
    setBusy(true);
    try {
      if (addInclude) await api.sshAddInclude();
      try { localStorage.setItem(EDITOR_KEY, editor); } catch { /* 记不住就算了 */ }
      const opened = await api.openInEditor(connId, path.trim() || "~", editor);
      onOpenChange(false);
      toast.success(t("已在 {{editor}} 里打开 {{alias}}:{{path}}。第一次连会让你确认对方的主机密钥", { editor: editorName, alias: opened.alias, path: opened.path }));
    } catch (e) {
      const message = String(e);
      if (message.includes("ssh_include_missing")) {
        const status = await api.sshStatus().catch(() => null);
        setNeedInclude(status?.config_path ?? "~/.ssh/config");
      } else {
        toast.error(tr(message));
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("用本机编辑器打开 {{name}} 上的文件夹", { name: deviceName })}</DialogTitle>
          <DialogDescription>
            {t("用 VS Code 或 Cursor 的 Remote-SSH 连过去，SSH 走 P2PCLI 的加密通道，不用对方开公网端口。对方是 Mac、Linux 时不用开远程登录、不用密码（两边都要是新版 P2PCLI）；对方是 Windows 时要装好 OpenSSH 服务器。本机编辑器要装 Remote-SSH 扩展。")}
          </DialogDescription>
        </DialogHeader>
        {needInclude ? (
          <div className="flex flex-col gap-2 text-sm">
            <p>{t("第一次用要在 SSH 配置（{{path}}）最前面加一行：", { path: needInclude })}</p>
            <pre className="overflow-x-auto rounded-md bg-muted-surface px-3 py-2 text-xs">Include ~/.ssh/p2pcli_hosts</pre>
            <p className="text-muted-foreground">{t("原有的内容不动。P2PCLI 只维护 ~/.ssh/p2pcli_hosts 这一个文件，里面每台设备一个别名，终端里也能直接 ssh 它。")}</p>
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            <label className="flex flex-col gap-1 text-sm">
              {t("对方电脑上的文件夹")}
              <Input value={path} onChange={(e) => setPath(e.currentTarget.value)} placeholder="~/projects/app" autoFocus
                onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) { e.preventDefault(); void run(false); } }} />
            </label>
            <div className="flex gap-2" role="radiogroup" aria-label={t("编辑器")}>
              {(["vscode", "cursor"] as const).map((id) => (
                <Button key={id} size="sm" variant={editor === id ? "outline" : "ghost"} aria-pressed={editor === id} onPress={() => setEditor(id)}>
                  {id === "cursor" ? "Cursor" : "VS Code"}
                </Button>
              ))}
            </div>
          </div>
        )}
        <DialogFooter>
          <Button variant="ghost" onPress={() => onOpenChange(false)} isDisabled={busy}>{t("取消")}</Button>
          <Button onPress={() => void run(!!needInclude)} isDisabled={busy}>
            {busy ? <LoaderCircle data-icon="inline-start" className="animate-spin" /> : <Code2 data-icon="inline-start" />}
            {needInclude ? t("加上并打开") : t("用 {{editor}} 打开", { editor: editorName })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
