import { defineConfig } from "@playwright/test";

// The examples, built by Vite (as an app using the library would build), in
// Chromium. `npm run build` first.
export default defineConfig({
  testDir: "e2e",
  timeout: 120_000,
  workers: 1,
  use: {
    baseURL: "http://localhost:4174/",
    launchOptions: {
      // A Chromium of another Playwright version, when the one this version wants is not installed.
      executablePath: process.env.QRSEND_CHROMIUM || undefined,
      args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
    },
  },
  webServer: {
    command: "npx vite build --config example/vite.config.js && npx vite preview --config example/vite.config.js --port 4174 --strictPort",
    port: 4174,
    reuseExistingServer: !process.env.CI,
  },
});
