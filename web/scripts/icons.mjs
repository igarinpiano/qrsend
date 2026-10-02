// Renders the PNG app icons (same design as public/icon.svg) without extra tools.
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

function crc32(buf) {
  let c, crc = ~0;
  for (const b of buf) {
    c = (crc ^ b) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = (crc >>> 8) ^ c;
  }
  return ~crc >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}

function png(size) {
  const s = size / 64; // design is on a 64-unit grid
  const px = Buffer.alloc(size * size * 3);
  const fill = (x0, y0, w, h, [r, g, b]) => {
    for (let y = Math.round(y0 * s); y < Math.round((y0 + h) * s); y++)
      for (let x = Math.round(x0 * s); x < Math.round((x0 + w) * s); x++) {
        const i = (y * size + x) * 3;
        px[i] = r; px[i + 1] = g; px[i + 2] = b;
      }
  };
  const bg = [0x0f, 0x17, 0x2a], fg = [0xf8, 0xfa, 0xfc], accent = [0x38, 0xbd, 0xf8];
  fill(0, 0, 64, 64, bg);
  for (const [x, y] of [[12, 12], [36, 12], [12, 36]]) {
    fill(x, y, 16, 16, fg);
    fill(x + 4, y + 4, 8, 8, bg);
  }
  fill(36, 42, 12, 4, accent); // arrow shaft
  for (let i = 0; i < 6; i++) fill(42 + i, 38 + i, 3, 3, accent), fill(42 + i, 47 - i, 3, 3, accent);
  const raw = Buffer.alloc((size * 3 + 1) * size);
  for (let y = 0; y < size; y++) px.copy(raw, y * (size * 3 + 1) + 1, y * size * 3, (y + 1) * size * 3);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; ihdr[9] = 2;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

for (const size of [192, 512]) writeFileSync(new URL(`../public/icon-${size}.png`, import.meta.url), png(size));
