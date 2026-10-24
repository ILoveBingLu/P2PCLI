import {
  ArrowRight,
  Fingerprint,
  KeyRound,
  Monitor,
  ShieldCheck,
  Terminal,
  TerminalSquare,
} from "lucide-react";
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
}: {
  ov: Overview | null;
  onSettings: () => void;
}) {
  return (
    <div className="welcome-scroll">
      <Empty className="welcome">
        <div className="connection-art" aria-hidden="true">
          <div className="art-device">
            <Monitor />
            <span>本机</span>
          </div>
          <div className="art-link">
            <span />
            <ShieldCheck />
            <span />
          </div>
          <div className="art-terminal">
            <div className="art-titlebar">
              <i />
              <i />
              <i />
              <span>remote terminal</span>
            </div>
            <div className="art-prompt">
              <Terminal />
              <span>连接，即刻开始</span>
              <b />
            </div>
            <div className="art-lines">
              <i />
              <i />
            </div>
          </div>
        </div>
        <EmptyHeader>
          <div className="eyebrow">YOUR TERMINAL, ANYWHERE</div>
          <EmptyTitle>
            <h2 className="welcome-title">你的终端，不止在这台电脑。</h2>
          </EmptyTitle>
          <EmptyDescription>
            安全连接另一台设备，在熟悉的终端里继续工作。
            <br />
            开发环境、命令行工具和文件，都在原来的位置。
          </EmptyDescription>
        </EmptyHeader>
        <EmptyContent>
          <Button
            onClick={() => document.getElementById("remote-code")?.focus()}
          >
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
        <ol className="welcome-steps">
          <li>
            <div className="step-heading">
              <Fingerprint />
              <span>01</span>
            </div>
            <h3>获取设备码</h3>
            <p>
              让对方打开 P2PCLI
              <br />
              分享设备码和临时密码
            </p>
          </li>
          <li>
            <div className="step-heading">
              <KeyRound />
              <span>02</span>
            </div>
            <h3>发起安全连接</h3>
            <p>
              在左侧输入连接信息
              <br />
              等待对方确认授权
            </p>
          </li>
          <li>
            <div className="step-heading">
              <TerminalSquare />
              <span>03</span>
            </div>
            <h3>开始远程工作</h3>
            <p>
              打开终端，自由使用
              <br />
              Claude、Codex 等工具
            </p>
          </li>
        </ol>
      </Empty>
      <div className="welcome-footnote">
        <ShieldCheck />
        IPv6 直连优先，IPv4 打洞与中继自动接续。
      </div>
    </div>
  );
}
