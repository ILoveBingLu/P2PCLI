import { InputOTP, Popover, REGEXP_ONLY_DIGITS } from "@heroui/react";
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
  const isDeviceCodeComplete = /^\d{9}$/.test(target);

  return (
    <Popover isOpen={open} onOpenChange={onOpenChange}>
      <Button variant="ghost" size="icon-sm" aria-label="新建连接" title="新建连接">
        <Plus />
      </Button>
      <Popover.Content
        placement="bottom end"
        offset={8}
        className="w-[26rem] max-w-[calc(100vw-24px)]"
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
                if (!isDeviceCodeComplete) return;
                void doConnect(target, password);
              }}
            >
              <FieldGroup className="gap-3">
                <Field data-disabled={busy}>
                  <FieldLabel htmlFor="remote-code">设备码</FieldLabel>
                  <InputOTP
                    id="remote-code"
                    className="gap-3"
                    variant="secondary"
                    autoFocus
                    aria-label="设备码，9 位数字"
                    maxLength={9}
                    minLength={9}
                    required
                    pattern={REGEXP_ONLY_DIGITS}
                    inputMode="numeric"
                    value={target}
                    onChange={setTarget}
                    pasteTransformer={(text) => text.replace(/[\s-]/g, "")}
                    isDisabled={busy}
                    spellCheck={false}
                    autoComplete="off"
                  >
                    {[0, 3, 6].map((start) => (
                      <InputOTP.Group key={start} className="flex-1">
                        {[0, 1, 2].map((offset) => (
                          <InputOTP.Slot key={offset} index={start + offset} />
                        ))}
                      </InputOTP.Group>
                    ))}
                  </InputOTP>
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
                  disabled={busy || !isDeviceCodeComplete}
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
