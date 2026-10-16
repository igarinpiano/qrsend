// QRSend for web pages: files, folders or a text become a stream of QR codes,
// and the texts a camera reads off them become the files again.
//
// This module does the encoding and decoding only, and runs in browsers and
// in Node. Showing the codes is `@qrsend/web/player`, reading them with a
// camera `@qrsend/web/scanner`.
//
// QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in
// other countries. Pages that show or name QR codes made with this library
// should say so (see README.md, "Trademark"); `QR_CODE_TRADEMARK` holds the
// sentence.

import initWasm, { Decoder as WasmDecoder, Encoder as WasmEncoder, version as wasmVersion } from "./wasm/qrsend_js.js";

/**
 * The trademark notice for QR Code, to show where your page shows or names
 * QR codes (a footer, an about page, the help).
 */
export const QR_CODE_TRADEMARK =
  "QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in other countries.";

/** Where the WebAssembly comes from, when not from next to this module. */
export type WasmSource = string | URL | Request | Response | BufferSource | WebAssembly.Module;

let loading: Promise<void> | undefined;

/**
 * Loads the WebAssembly core. Called by everything that needs it; call it
 * yourself to load it early, or to load it from elsewhere (`source`: its URL
 * or its bytes). In Node, it reads the file next to this module.
 */
export function init(source?: WasmSource): Promise<void> {
  loading ??= (async () => {
    let input: WasmSource | undefined = source;
    if (input === undefined && isNode()) {
      // Not a plain import, so that no bundler for the web tries to follow it.
      const fs = "node:fs/promises";
      const { readFile } = (await import(/* @vite-ignore */ /* webpackIgnore: true */ fs)) as {
        readFile(path: URL): Promise<Uint8Array>;
      };
      input = await readFile(new URL("./wasm/qrsend_js_bg.wasm", import.meta.url));
    }
    await initWasm(input === undefined ? undefined : { module_or_path: input as never });
  })().catch((e) => {
    loading = undefined;
    throw e;
  });
  return loading;
}

function isNode(): boolean {
  const p = (globalThis as { process?: { versions?: { node?: string } } }).process;
  return typeof p?.versions?.node === "string" && typeof (globalThis as { window?: unknown }).window === "undefined";
}

/** The version of the WebAssembly core (that of QRSend it was built with). */
export async function version(): Promise<string> {
  await init();
  return wasmVersion();
}

// ---------------------------------------------------------------- sending

/** One file or folder of a transfer, with its path in it. */
export interface Item {
  /** Path in the transfer, with `/` between folders: `"photos/2024/a.jpg"`. */
  path: string;
  /** The content of a file. Leave out for a folder. A string is sent as UTF-8. */
  data?: Blob | ArrayBuffer | ArrayBufferView | string;
  /** Milliseconds since 1970, as `File.lastModified`. */
  lastModified?: number;
}

/**
 * What to send:
 * - a string: a text (the receiver gets `{ kind: "text" }`),
 * - a `File`: one file with its name,
 * - a `Blob`, `ArrayBuffer` or typed array: one file named `options.name`,
 * - a `FileList` or an array of `File`s: several files, in their folders when
 *   they come from `<input type="file" webkitdirectory>`,
 * - an array of `Item`s: files and folders with the paths you give.
 */
export type Content = string | Blob | ArrayBuffer | ArrayBufferView | FileList | readonly (File | Item)[];

export interface EncodeOptions {
  /**
   * The size of the codes: QR version 1 (small, easy to read) to 40 (large,
   * holds the most). Leave out to choose from the size of the transfer.
   */
  qrVersion?: number;
  /** Error correction level of codes of the `qrVersion` given: "L", "M" (default), "Q" or "H". */
  errorCorrection?: "L" | "M" | "Q" | "H";
  /** Compress the data (default true). Turn off for data that is compressed already. */
  compress?: boolean;
  /** Extra codes in each round, as a fraction of what the data takes (default 0.1). */
  redundancy?: number;
  /** The file name for content without one (a `Blob` or bytes). Default "data". */
  name?: string;
  /** Called while large content is read, with bytes read and in all. */
  onReading?: (done: number, total: number) => void;
}

const CHUNK = 1 << 20;

/**
 * An endless stream of QR codes for one transfer. Show them one after
 * another (`next()` or `nextModules()`), a few per second: the receiver can
 * start reading at any point and miss any of them. A full round
 * (`codesPerRound`) holds everything once, with some to spare.
 *
 * The same content always makes the same transfer, so a receiver that
 * stopped halfway continues where it was when it is sent again.
 */
export class Encoder {
  private constructor(private inner: WasmEncoder) {}

  /** Packs `content` (see `Content`). */
  static async create(content: Content, options: EncodeOptions = {}): Promise<Encoder> {
    await init();
    const inner = new WasmEncoder(
      options.qrVersion ?? 0,
      options.errorCorrection ?? "M",
      options.compress ?? true,
      options.redundancy ?? 0.1,
    );
    try {
      await pack(inner, content, options);
      inner.finish();
    } catch (e) {
      inner.free();
      throw e;
    }
    return new Encoder(inner);
  }

  /** The text of the next code, for a QR code generator of your own (alphanumeric mode, `qrVersion`). */
  next(): string {
    return this.inner.next();
  }

  /** The next code as `size × size` modules, row by row, 1 = dark (no quiet zone). */
  nextModules(): Uint8Array {
    return this.inner.nextModules();
  }

  /** Modules on a side of a code. */
  get size(): number {
    return this.inner.size;
  }

  get qrVersion(): number {
    return this.inner.qrVersion;
  }

  get errorCorrection(): string {
    return this.inner.errorCorrection;
  }

  /** Codes that hold everything once (with some to spare). */
  get codesPerRound(): number {
    return this.inner.codesPerRound;
  }

  /** Codes handed out so far. */
  get count(): number {
    return this.inner.count;
  }

  /** Bytes the codes carry, after compression. */
  get bytes(): number {
    return this.inner.bytes;
  }

  /** The transfer's id, 8 hex digits. */
  get session(): string {
    return this.inner.session;
  }

  /** Frees the memory it holds. It cannot be used afterwards. */
  free(): void {
    this.inner.free();
  }
}

function isItem(x: File | Item): x is Item {
  return typeof (x as Item).path === "string" && !(typeof Blob !== "undefined" && x instanceof Blob);
}

async function pack(e: WasmEncoder, content: Content, options: EncodeOptions): Promise<void> {
  if (typeof content === "string") {
    e.setText(content);
    return;
  }
  const name = options.name ?? "data";
  let items: Item[];
  if (typeof Blob !== "undefined" && content instanceof Blob) {
    const file = content as Blob & { name?: string; lastModified?: number };
    items = [{ path: file.name || name, data: file, lastModified: file.lastModified }];
  } else if (content instanceof ArrayBuffer || ArrayBuffer.isView(content)) {
    items = [{ path: name, data: content }];
  } else {
    items = Array.from(content as ArrayLike<File | Item>, (x) =>
      isItem(x) ? x : { path: x.webkitRelativePath || x.name, data: x, lastModified: x.lastModified },
    );
  }
  const total = items.reduce((n, item) => n + byteLength(item.data), 0);
  let done = 0;
  // Folders that files are in are added for them; empty ones are listed.
  const folders = new Set<string>();
  for (const item of items) {
    const path = item.path.replace(/\\/g, "/").replace(/^\/+|\/+$/g, "");
    if (!path) throw new Error("an item without a path");
    const parts = path.split("/");
    for (let i = 1; i < parts.length; i++) {
      const folder = parts.slice(0, i).join("/");
      if (!folders.has(folder)) {
        folders.add(folder);
        e.addDir(folder, undefined);
      }
    }
    if (item.data === undefined) {
      if (!folders.has(path)) {
        folders.add(path);
        e.addDir(path, item.lastModified);
      }
      continue;
    }
    e.beginFile(path, item.lastModified);
    for await (const chunk of chunks(item.data)) {
      e.write(chunk);
      done += chunk.byteLength;
      options.onReading?.(done, total);
    }
    e.endFile();
  }
}

function byteLength(data: Item["data"]): number {
  if (data === undefined) return 0;
  if (typeof data === "string") return data.length;
  if (typeof Blob !== "undefined" && data instanceof Blob) return data.size;
  return (data as ArrayBuffer | ArrayBufferView).byteLength;
}

async function* chunks(data: NonNullable<Item["data"]>): AsyncGenerator<Uint8Array> {
  if (typeof data === "string") {
    yield new TextEncoder().encode(data);
  } else if (typeof Blob !== "undefined" && data instanceof Blob) {
    for (let at = 0; at < data.size; at += CHUNK) {
      yield new Uint8Array(await data.slice(at, at + CHUNK).arrayBuffer());
    }
  } else {
    const bytes =
      data instanceof ArrayBuffer
        ? new Uint8Array(data)
        : new Uint8Array((data as ArrayBufferView).buffer, (data as ArrayBufferView).byteOffset, (data as ArrayBufferView).byteLength);
    for (let at = 0; at < bytes.length; at += CHUNK) {
      yield bytes.subarray(at, at + CHUNK);
      // Let the page breathe between large pieces.
      if (bytes.length > CHUNK) await new Promise((r) => setTimeout(r, 0));
    }
  }
}

// ---------------------------------------------------------------- receiving

export interface Progress {
  /** "waiting": no code of a transfer read yet. "complete": `result()` can be had. */
  state: "waiting" | "receiving" | "complete";
  /** The transfer's id, 8 hex digits. */
  session: string | null;
  /** Codes of the transfer read, repeats included. */
  codes: number;
  /** Of those, the ones that brought something new. */
  usefulCodes: number;
  /** About how many more codes it takes (at least this many). */
  remainingCodes: number | null;
  receivedBytes: number | null;
  totalBytes: number | null;
  /** 0 to 1. */
  ratio: number;
  /** Once the list of files has arrived. */
  kind: "files" | "text" | null;
  fileCount: number | null;
  /** The size of the files (or the text) once received. */
  size: number | null;
  /** Codes of an encrypted transfer (from the QRSend app or CLI) were read. They cannot be opened here and are passed over. */
  encrypted: boolean;
  /** Codes of another transfer were read and passed over. */
  otherTransfer: boolean;
}

/** An entry of the list of files, as the sender named it. */
export interface Entry {
  path: string;
  directory: boolean;
  size: number;
  lastModified: number | null;
}

export interface ReceivedFile {
  /** Path in the transfer, with `/` between folders. Made safe to save: no `..`, no absolute paths, no names Windows refuses, no two the same. */
  path: string;
  /** The name alone. */
  name: string;
  data: Blob;
  lastModified?: number;
}

export type Received =
  | { kind: "text"; text: string }
  | { kind: "files"; files: ReceivedFile[]; directories: string[] };

/**
 * Puts a transfer together from the texts of its codes, read in any order,
 * with repeats and gaps. It follows the first transfer it reads codes of;
 * codes of others, and texts that are not QRSend codes, are passed over.
 * Everything is checked (checksums of every part and of every file) before
 * `result()` hands it out.
 */
export class Decoder {
  private constructor(private inner: WasmDecoder) {}

  static async create(): Promise<Decoder> {
    await init();
    return new Decoder(new WasmDecoder());
  }

  /** Takes the text of one or more codes. Returns whether `progress` changed. */
  push(texts: string | readonly string[]): boolean {
    let changed = false;
    for (const text of typeof texts === "string" ? [texts] : texts) {
      if (this.inner.push(text)) changed = true;
    }
    return changed;
  }

  get progress(): Progress {
    return this.inner.progress() as Progress;
  }

  get complete(): boolean {
    return this.inner.isComplete();
  }

  /** The list of files as the sender named it, once it has arrived (before the files themselves). */
  entries(): Entry[] {
    return this.inner.entries() as Entry[];
  }

  /** The text or the files, once `complete`. */
  result(): Received {
    let text: string | undefined;
    const files: ReceivedFile[] = [];
    const directories: string[] = [];
    let current: { path: string; lastModified?: number; parts: Uint8Array[] } | undefined;
    const kind = this.inner.extract({
      text: (t: string) => (text = t),
      dir: (path: string) => directories.push(path),
      begin: (path: string, _size: number, lastModified?: number) => (current = { path, lastModified, parts: [] }),
      // The bytes are copied out of WebAssembly memory as they come.
      write: (chunk: Uint8Array) => current!.parts.push(chunk.slice()),
      end: () => {
        const { path, lastModified, parts } = current!;
        files.push({ path, name: path.slice(path.lastIndexOf("/") + 1), data: new Blob(parts as BlobPart[]), lastModified });
        current = undefined;
      },
    });
    return kind === "text" ? { kind: "text", text: text ?? "" } : { kind: "files", files, directories };
  }

  /** Frees the memory it holds. It cannot be used afterwards. */
  free(): void {
    this.inner.free();
  }
}
