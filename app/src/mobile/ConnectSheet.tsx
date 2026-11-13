import { Fragment } from "react";
import { InputOTP, REGEXP_ONLY_DIGITS } from "@heroui/react";
import { ArrowUpRight, LoaderCircle, LockKeyhole, ScanLine } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { PlatformIcon } from "../components/PlatformIcon";
import type { useConnection } from "../components/useConnection";
import { Sheet } from "./Sheet";
import { t } from "@/i18n";

/** 连接新设备：扫电脑上的二维码最省事，也可以手输设备码和配对密码 */
export function ConnectSheet({ open, onClose, connection }: {
  open: boolean;
  onClose: () => void;
  connection: ReturnType<typeof useConnection>;
}) {
  const { target, setTarget, password, setPassword, busy, error, setError, pwRef, doConnect, lanTarget, setLanTarget } = connection;
  const canSubmit = lanTarget !== null || /^\d{9}$/.test(target);
  return (
    <Sheet open={open} title={lanTarget ? t("连接 {{name}}", { name: lanTarget.name }) : t("连接新设备")} onClose={onClose}>
      <form
        className="m-connect"
        onSubmit={(e) => {
          e.preventDefault();
          if (!canSubmit || busy) return;
          void (lanTarget ? doConnect(lanTarget.endpoint_id, password, lanTarget) : doConnect(target, password));
        }}
      >
        <FieldGroup className="gap-4">
          {!lanTarget && (
            <>
              <Button type="button" size="lg" className="w-full" disabled={busy} onPress={() => void connection.scanAndConnect()}>
                <ScanLine data-icon="inline-start" />
                {t("扫码连接")}
              </Button>
              <p className="m-connect-hint">{t("在电脑上点标题栏的手机图标，会显示二维码")}</p>
              <div className="m-connect-or"><span>{t("或者输入设备码")}</span></div>
            </>
          )}
          {lanTarget ? (
            <div className="m-connect-lan">
              <span className="m-device-tile"><PlatformIcon platform={lanTarget.platform} decorative /></span>
              <span className="min-w-0 flex-1 truncate">{lanTarget.name}</span>
              <Button variant="link" size="sm" disabled={busy} onPress={() => { setLanTarget(null); setError(null); }}>
                {t("改用设备码")}
              </Button>
            </div>
          ) : (
            <Field data-disabled={busy}>
              <FieldLabel htmlFor="m-remote-code">{t("设备码")}</FieldLabel>
              <InputOTP
                id="m-remote-code"
                className="m-otp"
                variant="secondary"
                pushPasswordManagerStrategy="none"
                aria-label={t("设备码，9 位数字")}
                maxLength={9}
                minLength={9}
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
                  <Fragment key={start}>
                    {start > 0 && <InputOTP.Separator aria-hidden="true" className="bg-muted-foreground/60" />}
                    <InputOTP.Group className="flex-1">
                      {[0, 1, 2].map((offset) => <InputOTP.Slot key={offset} index={start + offset} />)}
                    </InputOTP.Group>
                  </Fragment>
                ))}
              </InputOTP>
            </Field>
          )}
          <Field data-disabled={busy}>
            <FieldLabel htmlFor="m-remote-password">{t("配对密码")}</FieldLabel>
            <Input
              id="m-remote-password"
              variant="secondary"
              ref={pwRef}
              type="password"
              className="m-input"
              placeholder={t("输入对方的配对密码")}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              disabled={busy}
              spellCheck={false}
              autoComplete="off"
              enterKeyHint="go"
            />
          </Field>
          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <Button type="submit" size="lg" variant={lanTarget ? "default" : "secondary"} className="w-full" disabled={busy || !canSubmit}>
            {busy ? <LoaderCircle data-icon="inline-start" className="animate-spin" /> : <ArrowUpRight data-icon="inline-start" />}
            {busy ? t("等待对方同意…") : t("连接")}
          </Button>
          <p className="m-connect-note">
            <LockKeyhole aria-hidden="true" />
            {t("连接时对方电脑上会弹窗问要不要同意")}
          </p>
        </FieldGroup>
      </form>
    </Sheet>
  );
}
