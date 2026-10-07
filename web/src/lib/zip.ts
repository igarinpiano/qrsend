// A store-only (no compression) ZIP writer that never holds file contents in
// memory: the archive is a Blob assembled from small header buffers and the
// file Blobs themselves. Supports ZIP64 (files and archives beyond 4 GiB).

export interface ZipEntry {
  path: string;
  dir: boolean;
  blob?: Blob;
  mtime?: number;
}

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

async function crc32(blob: Blob): Promise<number> {
  let crc = ~0;
  const reader = blob.stream().getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    for (let i = 0; i < value.length; i++) crc = CRC_TABLE[(crc ^ value[i]) & 0xff] ^ (crc >>> 8);
  }
  return ~crc >>> 0;
}

class Buf {
  private view: DataView;
  private pos = 0;
  readonly bytes: Uint8Array<ArrayBuffer>;
  constructor(size: number) {
    this.bytes = new Uint8Array(size);
    this.view = new DataView(this.bytes.buffer);
  }
  u16(v: number): this {
    this.view.setUint16(this.pos, v, true);
    this.pos += 2;
    return this;
  }
  u32(v: number): this {
    this.view.setUint32(this.pos, v >>> 0, true);
    this.pos += 4;
    return this;
  }
  u64(v: number): this {
    this.view.setBigUint64(this.pos, BigInt(v), true);
    this.pos += 8;
    return this;
  }
  raw(b: Uint8Array): this {
    this.bytes.set(b, this.pos);
    this.pos += b.length;
    return this;
  }
}

const MAX32 = 0xffffffff;

function dosDateTime(ms?: number): [number, number] {
  const d = new Date(ms ?? Date.now());
  const year = Math.max(1980, d.getFullYear());
  const time = (d.getHours() << 11) | (d.getMinutes() << 5) | (d.getSeconds() >> 1);
  const date = ((year - 1980) << 9) | ((d.getMonth() + 1) << 5) | d.getDate();
  return [time, date];
}

export async function zipBlob(entries: ZipEntry[], onProgress?: (done: number, total: number) => void): Promise<Blob> {
  const encoder = new TextEncoder();
  const parts: BlobPart[] = [];
  const central: Uint8Array<ArrayBuffer>[] = [];
  let offset = 0;
  let needs64 = false;
  const total = entries.reduce((s, e) => s + (e.blob?.size ?? 0), 0);
  let done = 0;

  for (const e of entries) {
    const name = encoder.encode(e.dir ? `${e.path}/` : e.path);
    const size = e.dir ? 0 : e.blob!.size;
    const crc = e.dir ? 0 : await crc32(e.blob!);
    const [time, date] = dosDateTime(e.mtime);
    const big = size >= MAX32;
    const bigOffset = offset >= MAX32;
    needs64 ||= big || bigOffset;

    const local = new Buf(30 + name.length + (big ? 20 : 0))
      .u32(0x04034b50)
      .u16(big ? 45 : 20)
      .u16(0x0800) // UTF-8 names
      .u16(0) // stored
      .u16(time)
      .u16(date)
      .u32(crc)
      .u32(big ? MAX32 : size)
      .u32(big ? MAX32 : size)
      .u16(name.length)
      .u16(big ? 20 : 0)
      .raw(name);
    if (big) local.u16(0x0001).u16(16).u64(size).u64(size);
    parts.push(local.bytes);
    if (!e.dir) parts.push(e.blob!);

    const extraLen = (big ? 16 : 0) + (bigOffset ? 8 : 0);
    const cd = new Buf(46 + name.length + (extraLen ? 4 + extraLen : 0))
      .u32(0x02014b50)
      .u16(45)
      .u16(big || bigOffset ? 45 : 20)
      .u16(0x0800)
      .u16(0)
      .u16(time)
      .u16(date)
      .u32(crc)
      .u32(big ? MAX32 : size)
      .u32(big ? MAX32 : size)
      .u16(name.length)
      .u16(extraLen ? 4 + extraLen : 0)
      .u16(0) // comment
      .u16(0) // disk
      .u16(0) // internal attributes
      .u32(e.dir ? 0x10 : 0)
      .u32(bigOffset ? MAX32 : offset)
      .raw(name);
    if (extraLen) {
      cd.u16(0x0001).u16(extraLen);
      if (big) cd.u64(size).u64(size);
      if (bigOffset) cd.u64(offset);
    }
    central.push(cd.bytes);

    offset += local.bytes.length + size;
    done += size;
    onProgress?.(done, total);
  }

  const cdOffset = offset;
  const cdSize = central.reduce((s, c) => s + c.length, 0);
  parts.push(...central);
  needs64 ||= entries.length >= 0xffff || cdOffset >= MAX32 || cdSize >= MAX32;
  if (needs64) {
    parts.push(
      new Buf(56)
        .u32(0x06064b50)
        .u64(44)
        .u16(45)
        .u16(45)
        .u32(0)
        .u32(0)
        .u64(entries.length)
        .u64(entries.length)
        .u64(cdSize)
        .u64(cdOffset).bytes,
      new Buf(20).u32(0x07064b50).u32(0).u64(cdOffset + cdSize).u32(1).bytes,
    );
  }
  const count = Math.min(entries.length, 0xffff);
  parts.push(
    new Buf(22)
      .u32(0x06054b50)
      .u16(0)
      .u16(0)
      .u16(count)
      .u16(count)
      .u32(Math.min(cdSize, MAX32))
      .u32(Math.min(cdOffset, MAX32))
      .u16(0).bytes,
  );
  return new Blob(parts, { type: "application/zip" });
}
