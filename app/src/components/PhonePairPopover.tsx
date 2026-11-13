import { useMemo } from "react";
import { Popover } from "@heroui/react";
import { ShieldAlert, Smartphone } from "lucide-react";
import { renderSVG } from "uqr";
import type { Overview } from "../api";
import { buildPairLink } from "@/lib/pairLink";
import { Button } from "@/components/ui/button";
import { t } from "@/i18n";

/**
 * 桌面端标题栏的手机图标：点开显示二维码，手机上的 P2PCLI 扫了就连这台电脑。
 * 二维码里有设备码、配对密码和公钥，所以只在点开时显示；手机连上时这边照样要点同意。
 */
export function PhonePairPopover({ ov }: { ov: Overview | null }) {
  const link = useMemo(
    () => ov && buildPairLink({ code: ov.code, password: ov.password, id: ov.endpoint_id, name: ov.name }),
    [ov?.code, ov?.password, ov?.endpoint_id, ov?.name],
  );
  // 白底黑块，四周留够白边，深色界面里手机也认得出来
  const svg = useMemo(() => link && renderSVG(link, { border: 2, whiteColor: "#ffffff", blackColor: "#000000" }), [link]);

  return (
    <Popover>
      <Button variant="ghost" size="icon-sm" aria-label={t("用手机连接")} title={t("用手机扫码连接这台电脑")}>
        <Smartphone />
      </Button>
      <Popover.Content placement="bottom" offset={8} className="w-72 max-w-[calc(100vw-24px)]">
        <Popover.Arrow />
        <Popover.Dialog aria-label={t("用手机连接")} className="flex flex-col items-center gap-3">
          <Popover.Heading className="self-start">{t("用手机连接")}</Popover.Heading>
          {svg ? (
            <div className="phone-pair-qr" role="img" aria-label={t("连接这台电脑的二维码")}
              dangerouslySetInnerHTML={{ __html: svg }} />
          ) : (
            <div className="phone-pair-qr phone-pair-qr-empty">{t("正在读取本机信息")}</div>
          )}
          <p className="text-center text-xs text-muted-foreground">
            {t("在手机上打开 P2PCLI，点“扫码连接”扫这个二维码。连上时这台电脑还会弹窗问你同不同意。")}
          </p>
          {ov && !ov.code && (
            <p className="text-center text-xs text-warning">
              {t("牵线服务器没连上，手机要和这台电脑在同一个局域网里才连得上。")}
            </p>
          )}
          <div className="flex items-start gap-1.5 text-xs text-muted-foreground">
            <ShieldAlert className="mt-0.5 size-3.5 shrink-0" />
            <span>{t("二维码里有配对密码，别让不认识的人拍到。")}</span>
          </div>
        </Popover.Dialog>
      </Popover.Content>
    </Popover>
  );
}
