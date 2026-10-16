import { QrScanner, QR_CODE_TRADEMARK } from "@qrsend/web/scanner";
// ZXing reads the codes where the browser has no detector of its own
// (Safari, Firefox). Served from this site instead of a CDN:
import zxingWasmUrl from "zxing-wasm/reader/zxing_reader.wasm?url";

document.getElementById("trademark").textContent = QR_CODE_TRADEMARK;

const scanner = new QrScanner(document.getElementById("camera"), { zxingWasmUrl });
const bar = document.getElementById("bar");
const status = document.getElementById("status");
scanner.onprogress = (p) => {
  bar.value = p.ratio;
  status.textContent = `${Math.round(p.ratio * 100)}% · ${p.usefulCodes} codes`;
};

document.getElementById("start").onclick = async () => {
  const received = await scanner.receive();
  status.textContent = "Received.";
  if (received.kind === "text") {
    document.getElementById("text").textContent = received.text;
    return;
  }
  for (const file of received.files) {
    const a = document.createElement("a");
    a.href = URL.createObjectURL(file.data);
    a.download = file.name;
    a.textContent = `${file.path} (${file.data.size} bytes)`;
    const li = document.createElement("li");
    li.append(a);
    document.getElementById("received").append(li);
  }
};
