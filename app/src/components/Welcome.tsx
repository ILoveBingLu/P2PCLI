import { ArrowRight } from "lucide-react";
import appIcon from "../../src-tauri/icons/128x128.png";
import type { Overview } from "../api";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@/components/ui/empty";

export function Welcome({
  ov,
  onSettings,
  onConnect,
}: {
  ov: Overview | null;
  onSettings: () => void;
  onConnect: () => void;
}) {
  return (
    <div className="welcome-scroll">
      <Empty className="welcome">
        <img
          src={appIcon}
          className="size-14 shrink-0 object-contain"
          alt="P2PCLI"
          draggable={false}
        />
        <EmptyHeader>
          <EmptyTitle>
            <h2>连接远程设备</h2>
          </EmptyTitle>
          <EmptyDescription>
            输入设备码和临时密码，开始远程终端会话。
          </EmptyDescription>
        </EmptyHeader>
        <EmptyContent>
          <Button onClick={onConnect}>
            开始连接
            <ArrowRight data-icon="inline-end" />
          </Button>
          {ov?.server.state === "none" && (
            <Alert>
              <AlertTitle>先配置牵线服务器</AlertTitle>
              <AlertDescription>
                配置后即可获取设备码。
                <Button variant="link" size="sm" onClick={onSettings}>
                  打开设置
                  <ArrowRight data-icon="inline-end" />
                </Button>
              </AlertDescription>
            </Alert>
          )}
        </EmptyContent>
      </Empty>
    </div>
  );
}
