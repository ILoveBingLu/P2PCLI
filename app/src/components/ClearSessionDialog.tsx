import { useRef, useState } from "react";
import { LoaderCircle } from "lucide-react";
import { Button } from "./ui/button";
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "./ui/alert-dialog";

export function ClearSessionDialog({ label, needsConnection = false, onConfirm, onCancel }: {
  label: string;
  needsConnection?: boolean;
  onConfirm: () => Promise<void>;
  onCancel: () => void;
}) {
  const pending = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const clear = async () => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setError("");
    try {
      await onConfirm();
      onCancel();
    } catch (error) {
      setError(`清除会话失败：${String(error)}`);
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };
  return <AlertDialog open>
    <AlertDialogContent>
      <AlertDialogHeader>
        <AlertDialogTitle>清除「{label}」？</AlertDialogTitle>
        <AlertDialogDescription>
          {needsConnection && "将先连接这台设备，再清除指定会话。"}
          会话内运行的程序将被结束，未保存的内容可能丢失，对应的终端标签也会关闭。此操作不会删除工作目录中的文件。
        </AlertDialogDescription>
      </AlertDialogHeader>
      {error && <p role="alert" className="text-sm text-destructive break-words">{error}</p>}
      <AlertDialogFooter>
        <Button variant="outline" disabled={busy} onPress={onCancel}>取消</Button>
        <Button variant="destructive" disabled={busy} onPress={() => void clear()}>
          {busy && <LoaderCircle className="size-4 animate-spin" />}
          {busy ? "正在清除" : "清除会话"}
        </Button>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>;
}
