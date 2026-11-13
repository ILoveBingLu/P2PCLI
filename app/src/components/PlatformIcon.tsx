import appleIcon from "@/assets/platforms/apple.svg";
import windowsIcon from "@/assets/platforms/windows.svg";
import linuxIcon from "@/assets/platforms/linux.svg";
import androidIcon from "@/assets/platforms/android.svg";
import harmonyosIcon from "@/assets/platforms/harmonyos.svg";
import { platformLabel } from "../api";
import { Tooltip, TooltipTrigger } from "@/components/ui/tooltip";

export function PlatformIcon({ platform, decorative = false }: { platform: string; decorative?: boolean }) {
  const icon = ({
    macos: appleIcon,
    ios: appleIcon,
    windows: windowsIcon,
    linux: linuxIcon,
    android: androidIcon,
    harmonyos: harmonyosIcon,
  } as Record<string, string>)[platform];
  const label = platformLabel(platform);

  if (!icon) return <span className="platform-label shrink-0">{label}</span>;

  // Accordion triggers are already buttons; avoid nesting an interactive tooltip trigger.
  if (decorative) return <span className="platform-icon" aria-hidden="true"
    style={{ maskImage: `url("${icon}")`, WebkitMaskImage: `url("${icon}")` }} />;

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
