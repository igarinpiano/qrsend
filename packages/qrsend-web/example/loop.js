import { QrPlayer } from "@qrsend/web/player";
import { QrScanner } from "@qrsend/web/scanner";
import zxingWasmUrl from "zxing-wasm/reader/zxing_reader.wasm?url";

const params = new URLSearchParams(location.search);
const size = Number(params.get("size") ?? 20000);
const grid = params.get("grid") === "fill" ? "fill" : Number(params.get("grid") ?? 1);

// Content the test can check, which does not compress.
const bytes = new Uint8Array(size);
let x = 0x2545f491;
for (let i = 0; i < size; i++) {
  x ^= x << 13;
  x ^= x >>> 17;
  x ^= x << 5;
  bytes[i] = x & 0xff;
}

const canvas = document.getElementById("codes");
const player = new QrPlayer(canvas, { interval: Number(params.get("interval") ?? 66), grid, minModulePixels: 3 });
await player.play([
  { path: "folder/data.bin", data: bytes },
  { path: "hello.txt", data: "hello" },
]);

const scanner = new QrScanner(document.getElementById("camera"), {
  stream: canvas.captureStream(30),
  worker: params.get("worker") !== "0",
  zxingWasmUrl,
});
let updates = 0;
scanner.onprogress = () => updates++;
const started = performance.now();
const received = await scanner.receive();
const shown = { codes: player.current.count, version: player.current.qrVersion, perRound: player.current.codesPerRound };
player.stop();
const data = new Uint8Array(await received.files.find((f) => f.path === "folder/data.bin").data.arrayBuffer());
window.result = {
  kind: received.kind,
  paths: received.files.map((f) => f.path).sort(),
  same: data.length === bytes.length && data.every((b, i) => b === bytes[i]),
  hello: await received.files.find((f) => f.path === "hello.txt").data.text(),
  engine: scanner.engine,
  ...shown,
  updates,
  seconds: (performance.now() - started) / 1000,
};
document.getElementById("status").textContent = "done";
