import { Popover } from "@heroui/react";
import { ArrowUpRight, LoaderCircle, LockKeyhole, Plus } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import type { useConnection } from "./useConnection";

export function ConnectionPopover({ open, onOpenChange, connection }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  connection: ReturnType<typeof useConnection>;
}) {
  const { target, setTarget, password, setPassword, busy, error, pwRef, doConnect } = connection;

  return (
    <Popover isOpen={open} onOpenChange={onOpenChange}>
      <Button variant="ghost" size="icon-sm" aria-label="新建连接" title="新建连接">
        <Plus />
      </Button>
      <Popover.Content
        placement="bottom end"
        offset={8}
        className="w-80 max-w-[calc(100vw-24px)]"
      >
        <Popover.Arrow />
        <Popover.Dialog aria-label="连接远程设备" className="max-h-[calc(100dvh-72px)] overflow-y-auto">
          <section className="connect-section" aria-labelledby="connect-title">
            <div className="section-title">
              <Popover.Heading id="connect-title">连接远程设备</Popover.Heading>
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
                    variant="secondary"
                    autoFocus
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
                    variant="secondary"
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

        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
}
