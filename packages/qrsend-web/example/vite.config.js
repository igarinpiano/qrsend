// The examples, built with Vite as a page using the library would be. Here
// the library comes from ../dist (`npm run build` first); in an app it comes
// from node_modules, and needs no configuration at all.
import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

const dist = fileURLToPath(new URL("../dist/", import.meta.url));

export default defineConfig({
  root: fileURLToPath(new URL(".", import.meta.url)),
  resolve: {
    alias: [
      { find: /^@qrsend\/web\/(player|scanner)$/, replacement: `${dist}$1.js` },
      { find: /^@qrsend\/web$/, replacement: `${dist}index.js` },
    ],
  },
  build: {
    rollupOptions: {
      input: {
        index: fileURLToPath(new URL("index.html", import.meta.url)),
        receive: fileURLToPath(new URL("receive.html", import.meta.url)),
        loop: fileURLToPath(new URL("loop.html", import.meta.url)),
      },
    },
  },
});
