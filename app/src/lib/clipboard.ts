import { isTauri } from "@tauri-apps/api/core";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";

// Desktop clipboard access goes through native IPC; browser previews keep the web API.
export async function readClipboardText(): Promise<string> {
  return isTauri() ? readText() : navigator.clipboard.readText();
}

export async function writeClipboardText(text: string): Promise<void> {
  if (isTauri()) await writeText(text);
  else await navigator.clipboard.writeText(text);
}
