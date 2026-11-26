import { defineConfig } from "@playwright/test";

const realBackend = process.env.MEDIAVAULT_LOCAL_SMOKE === "1";
const backendPort = process.env.MEDIAVAULT_SMOKE_BACKEND_PORT || "8000";
const backendPython = process.platform === "win32" ? ".\\.venv\\Scripts\\python.exe" : ".venv/bin/python";

export default defineConfig({
  testDir: "./tests",
  workers: 2,
  use: { baseURL: "http://127.0.0.1:3010", channel: process.env.PLAYWRIGHT_CHANNEL },
  webServer: [{
    command: "node scripts/next.mjs start --hostname 127.0.0.1 --port 3010",
    url: "http://127.0.0.1:3010",
    reuseExistingServer: false,
    timeout: 120_000,
  }, ...(realBackend ? [{
    command: backendPython + " -m app.serve",
    cwd: "../backend",
    url: "http://127.0.0.1:" + backendPort + "/api/health",
    reuseExistingServer: false,
    timeout: 90_000,
    env: {
      FRONTEND_ORIGINS: "http://127.0.0.1:3010",
      BACKEND_PORT: backendPort,
      BACKEND_HOST: "127.0.0.1",
      DATABASE_PATH: "storage/smoke/app.db",
      DOWNLOAD_DIR: "storage/smoke/downloads",
      TEMP_DIR: "storage/smoke/tmp",
    },
  }] : [])],
});
