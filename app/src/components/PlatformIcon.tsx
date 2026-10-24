import appleIcon from "@/assets/platforms/apple.svg";
import windowsIcon from "@/assets/platforms/windows.svg";
import { platformLabel } from "../api";
import { Tooltip, TooltipTrigger } from "@/components/ui/tooltip";

export function PlatformIcon({ platform }: { platform: string }) {
  const icon =
    platform === "macos"
      ? appleIcon
      : platform === "windows"
        ? windowsIcon
        : undefined;
  const label = platformLabel(platform);

  if (!icon) return <span className="platform-label shrink-0">{label}</span>;

  return (
    <Tooltip content={label}>
      <TooltipTrigger
        className="platform-icon"
        role="img"
        aria-label={label}
        style={{ maskImage: `url("${icon}")`, WebkitMaskImage: `url("${icon}")` }}
      />
    </Tooltip>
  );
}
