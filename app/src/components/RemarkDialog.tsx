import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { toast } from "@/components/ui/toast";

export function RemarkDialog({ open, title, description, initialValue, onOpenChange, onSave }: {
  open: boolean;
  title: string;
  description: string;
  initialValue: string;
  onOpenChange: (open: boolean) => void;
  onSave: (remark: string) => Promise<void>;
}) {
  const [value, setValue] = useState(initialValue);
  const [saving, setSaving] = useState(false);
  const pending = useRef(false);

  useEffect(() => {
    if (open) setValue(initialValue);
  }, [initialValue, open]);

  const save = async () => {
    if (pending.current) return;
    pending.current = true;
    setSaving(true);
    try {
      await onSave(value);
      onOpenChange(false);
    } catch (error) {
      toast.error(`保存备注失败：${String(error)}`);
    } finally {
      pending.current = false;
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !saving && onOpenChange(next)}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        <Input
          aria-label="备注"
          value={value}
          onChange={(event) => setValue(event.currentTarget.value)}
          placeholder="留空则使用默认名称"
          maxLength={80}
          autoFocus
          disabled={saving}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.nativeEvent.isComposing) {
              event.preventDefault();
              void save();
            }
          }}
        />
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={saving}>取消</Button>
          <Button onClick={() => void save()} disabled={saving}>{saving ? "保存中…" : "保存"}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
