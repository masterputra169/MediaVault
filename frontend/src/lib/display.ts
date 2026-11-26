import type { Quality, Status } from "./types";

export function niceBytes(value: number | null) {
  if (value === null) return "—";
  if (value < 1024) return `${value} B`;
  const exponent = Math.min(4, Math.floor(Math.log(value) / Math.log(1024)));
  const unit = ["B", "KB", "MB", "GB", "TB"][exponent];
  return `${new Intl.NumberFormat("id-ID", { maximumFractionDigits: 1 }).format(value / 1024 ** exponent)} ${unit}`;
}

export function duration(seconds: number | null) {
  if (seconds === null) return "Tidak diketahui";
  const n = Math.floor(seconds);
  const hours = Math.floor(n / 3600);
  return [hours ? String(hours) : null, String(Math.floor(n / 60) % 60).padStart(2, "0"),
    String(n % 60).padStart(2, "0")].filter(Boolean).join(":");
}

export function isActive(status: Status) {
  return status === "queued" || status === "downloading" || status === "processing";
}

export function qualityLabel(value: Quality) {
  return value === "best" ? "Terbaik" : `${value}p`;
}
