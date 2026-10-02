// Getting received data out of the browser.
import { zipSync, type Zippable } from "fflate";

export interface FileEntry {
  path: string;
  dir: boolean;
  data?: Uint8Array;
  mtime?: number;
}

export function download(data: Uint8Array | Blob, filename: string): void {
  const blob = data instanceof Blob ? data : new Blob([data as Uint8Array<ArrayBuffer>]);
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

export function basename(path: string): string {
  return path.split("/").pop() || path;
}

export function zip(entries: FileEntry[]): Uint8Array {
  const tree: Zippable = {};
  for (const e of entries) {
    if (e.dir) tree[e.path + "/"] = new Uint8Array();
    else tree[e.path] = [e.data!, { mtime: e.mtime ? new Date(e.mtime) : undefined, level: 0 }];
  }
  return zipSync(tree);
}

export const canPickFolder = typeof window !== "undefined" && "showDirectoryPicker" in window;

type DirHandle = FileSystemDirectoryHandle;

async function subdir(root: DirHandle, parts: string[]): Promise<DirHandle> {
  let dir = root;
  for (const p of parts) dir = await dir.getDirectoryHandle(p, { create: true });
  return dir;
}

async function freeName(dir: DirHandle, name: string): Promise<string> {
  const dot = name.lastIndexOf(".");
  const [stem, ext] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
  for (let n = 0; ; n++) {
    const candidate = n === 0 ? name : `${stem} (${n})${ext}`;
    try {
      await dir.getFileHandle(candidate);
    } catch {
      return candidate;
    }
  }
}

/** Writes entries into a folder the user picks (Chromium). Existing names get " (n)". */
export async function saveToFolder(entries: FileEntry[]): Promise<number> {
  const root: DirHandle = await (window as unknown as { showDirectoryPicker: (o: object) => Promise<DirHandle> })
    .showDirectoryPicker({ mode: "readwrite" });
  let written = 0;
  for (const e of entries) {
    const parts = e.path.split("/");
    if (e.dir) {
      await subdir(root, parts);
      continue;
    }
    const dir = await subdir(root, parts.slice(0, -1));
    const name = await freeName(dir, parts[parts.length - 1]);
    const handle = await dir.getFileHandle(name, { create: true });
    const w = await handle.createWritable();
    await w.write(e.data! as Uint8Array<ArrayBuffer>);
    await w.close();
    written++;
  }
  return written;
}

export async function copyText(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}
