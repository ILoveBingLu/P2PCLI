import { invoke } from "@tauri-apps/api/core";

/**
 * 手机上选要发的文件。安卓的系统选择框给的是 content:// 地址，Rust 那边拿不到文件路径，
 * 所以用网页自己的文件选择：拿到文件名和内容，交给 Rust 存进缓存目录，再按路径发。
 * 取消选择返回空。
 */
export function pickPhoneFiles(): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.style.display = "none";
    const done = (files: File[]) => {
      input.remove();
      resolve(files);
    };
    input.addEventListener("change", () => done([...(input.files ?? [])]), { once: true });
    input.addEventListener("cancel", () => done([]), { once: true });
    document.body.append(input);
    input.click();
  });
}

/** 把选好的文件交给 Rust 存进缓存目录，返回存好的路径 */
export async function stagePhoneFiles(files: File[]): Promise<string[]> {
  const paths: string[] = [];
  for (const file of files) {
    const bytes = new Uint8Array(await file.arrayBuffer());
    paths.push(await invoke<string>("stage_file", bytes, { headers: { "x-file-name": encodeURIComponent(file.name) } }));
  }
  return paths;
}
