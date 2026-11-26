import type { NextConfig } from "next";
import { existsSync } from "node:fs";
import { resolve } from "node:path";

const rootEnv = resolve(process.cwd(), "../.env");
if (existsSync(rootEnv)) process.loadEnvFile(rootEnv);

const publicMode = Boolean(
  (process.env.FRONTEND_ORIGINS || "").includes("https://") || process.env.NEXT_PUBLIC_API_URL === "/api",
);
const apiOrigin = `http://127.0.0.1:${process.env.BACKEND_PORT || "8000"}`;

const csp = publicMode
  ? "default-src 'self'; base-uri 'self'; connect-src 'self'; font-src 'self' data:; form-action 'self'; frame-ancestors 'none'; img-src 'self' data: https:; object-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; upgrade-insecure-requests"
  : `default-src 'self'; base-uri 'self'; connect-src 'self' ${apiOrigin} http://localhost:${process.env.BACKEND_PORT || "8000"}; font-src 'self' data:; form-action 'self'; frame-ancestors 'none'; img-src 'self' data: https: ${apiOrigin} http://localhost:${process.env.BACKEND_PORT || "8000"}; object-src 'none'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'`;

const config: NextConfig = {
  poweredByHeader: false,
  async headers() {
    const headers = [
      { key: "X-Frame-Options", value: "DENY" },
      { key: "X-Content-Type-Options", value: "nosniff" },
      { key: "Referrer-Policy", value: "no-referrer" },
      { key: "Permissions-Policy", value: "camera=(), microphone=(), geolocation=()" },
      { key: "Content-Security-Policy", value: csp },
    ];
    if (publicMode) {
      headers.push({ key: "Strict-Transport-Security", value: "max-age=31536000; includeSubDomains" });
    }
    return [{ source: "/:path*", headers }];
  },
};

export default config;
