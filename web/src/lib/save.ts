// Getting received data out of the browser. Entries are Blob slices of the
// session's output file in OPFS, so nothing here loads file contents into
// memory: downloads, folder writes and ZIP archives all stream from disk.
import type { OutEntry } from "./engine-types";
import { zipBlob } from "./zip";

export interface FileEntry {
  path: string;
  dir: boolean;
  size: number;
  mtime?: number;
  blob?: Blob;
}

/** Turns the entries of a result into Blob slices of its output file. */
export function sliceEntries(out: Blob, entries: OutEntry[]): FileEntry[] {
  return entries.map((e) => ({
    path: e.path,
    dir: e.dir,
    size: e.size,
    mtime: e.mtime,
    blob: e.dir ? undefined : out.slice(e.offset, e.offset + e.size),
  }));
}

export function download(data: Blob | Uint8Array, filename: string): void {
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

export function zip(entries: FileEntry[], onProgress?: (done: number, total: number) => void): Promise<Blob> {
  return zipBlob(entries, onProgress);
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
    await e.blob!.stream().pipeTo(await handle.createWritable());
    written++;
  }
  return written;
}

export async function copyText(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}
