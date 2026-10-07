// Random-access files for the engine worker. Backed by OPFS synchronous
// access handles (data stays on disk, reads and writes are synchronous, which
// is what the WebAssembly core needs); falls back to memory where OPFS is
// unavailable.

export interface RandomFile {
  read(offset: number, length: number): Uint8Array;
  write(offset: number, data: Uint8Array): void;
  size(): number;
  close(): void;
}

export interface FileStore {
  /** Whether data survives a reload (false for the in-memory fallback). */
  readonly persistent: boolean;
  open(path: string[]): Promise<RandomFile>;
  /** The file's content as a Blob; the file must not be open. */
  blob(path: string[]): Promise<Blob>;
  remove(path: string[]): Promise<void>;
}

type SyncHandle = {
  read(buffer: Uint8Array, options: { at: number }): number;
  write(buffer: Uint8Array, options: { at: number }): number;
  getSize(): number;
  flush(): void;
  close(): void;
};

class OpfsFile implements RandomFile {
  constructor(private handle: SyncHandle) {}
  read(offset: number, length: number): Uint8Array {
    const out = new Uint8Array(length);
    let got = 0;
    while (got < length) {
      const n = this.handle.read(out.subarray(got), { at: offset + got });
      if (n === 0) break;
      got += n;
    }
    if (got !== length) throw new Error(`short read at ${offset}: ${got} of ${length} bytes`);
    return out;
  }
  write(offset: number, data: Uint8Array): void {
    let put = 0;
    while (put < data.length) put += this.handle.write(data.subarray(put), { at: offset + put });
  }
  size(): number {
    return this.handle.getSize();
  }
  close(): void {
    this.handle.flush();
    this.handle.close();
  }
}

class OpfsStore implements FileStore {
  readonly persistent = true;
  constructor(private root: FileSystemDirectoryHandle) {}

  private async dir(path: string[], create: boolean): Promise<FileSystemDirectoryHandle> {
    let d = this.root;
    for (const part of path) d = await d.getDirectoryHandle(part, { create });
    return d;
  }

  async open(path: string[]): Promise<RandomFile> {
    const dir = await this.dir(path.slice(0, -1), true);
    const file = await dir.getFileHandle(path[path.length - 1], { create: true });
    const handle = await (file as unknown as { createSyncAccessHandle(): Promise<SyncHandle> }).createSyncAccessHandle();
    return new OpfsFile(handle);
  }

  async blob(path: string[]): Promise<Blob> {
    const dir = await this.dir(path.slice(0, -1), false);
    return (await dir.getFileHandle(path[path.length - 1])).getFile();
  }

  async remove(path: string[]): Promise<void> {
    try {
      const dir = await this.dir(path.slice(0, -1), false);
      await dir.removeEntry(path[path.length - 1], { recursive: true });
    } catch {
      /* already gone */
    }
  }
}

class MemoryFile implements RandomFile {
  data = new Uint8Array(0);
  length = 0;
  read(offset: number, length: number): Uint8Array {
    if (offset + length > this.length) throw new Error(`short read at ${offset}`);
    return this.data.slice(offset, offset + length);
  }
  write(offset: number, chunk: Uint8Array): void {
    const end = offset + chunk.length;
    if (end > this.data.length) {
      const grown = new Uint8Array(Math.max(end, this.data.length * 2, 1 << 16));
      grown.set(this.data.subarray(0, this.length));
      this.data = grown;
    }
    this.data.set(chunk, offset);
    this.length = Math.max(this.length, end);
  }
  size(): number {
    return this.length;
  }
  close(): void {}
}

class MemoryStore implements FileStore {
  readonly persistent = false;
  private files = new Map<string, MemoryFile>();

  async open(path: string[]): Promise<RandomFile> {
    const key = path.join("/");
    let f = this.files.get(key);
    if (!f) this.files.set(key, (f = new MemoryFile()));
    return f;
  }

  async blob(path: string[]): Promise<Blob> {
    const f = this.files.get(path.join("/"));
    if (!f) throw new Error("no such file");
    return new Blob([f.data.slice(0, f.length)]);
  }

  async remove(path: string[]): Promise<void> {
    const prefix = path.join("/");
    for (const key of [...this.files.keys()]) {
      if (key === prefix || key.startsWith(prefix + "/")) this.files.delete(key);
    }
  }
}

let store: Promise<FileStore> | undefined;

export function fileStore(): Promise<FileStore> {
  store ??= (async () => {
    try {
      const root = await navigator.storage.getDirectory();
      // Probe: synchronous access handles exist only in workers, and some
      // private-browsing modes reject OPFS altogether.
      const probe = await root.getFileHandle(".probe", { create: true });
      const handle = await (probe as unknown as { createSyncAccessHandle(): Promise<SyncHandle> }).createSyncAccessHandle();
      handle.close();
      await root.removeEntry(".probe");
      return new OpfsStore(root);
    } catch {
      return new MemoryStore();
    }
  })();
  return store;
}
