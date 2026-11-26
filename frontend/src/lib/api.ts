import type { AppError } from "./types";

const base = (process.env.NEXT_PUBLIC_API_URL || "http://127.0.0.1:8000/api").replace(/\/$/, "");
let unauthorizedHandler: (() => void) | null = null;

export function setUnauthorizedHandler(handler: (() => void) | null) {
  unauthorizedHandler = handler;
}

export class ApiError extends Error {
  constructor(public readonly data: AppError) {
    super(data.message);
  }
}

export async function api<T>(path: string, options: RequestInit = {}): Promise<T> {
  const request: RequestInit = {
    ...options,
    cache: "no-store" as const,
    credentials: base.startsWith("/") ? "same-origin" : "include",
  };
  if (options.method === "POST" || options.method === "PATCH") {
    request.headers = { "Content-Type": "application/json" };
  }
  let response: Response;
  try {
    response = await fetch(base + path, request);
  } catch (error) {
    if (options.signal?.aborted) throw error;
    throw new ApiError({ code: "SERVER_OFFLINE", message: "Backend lokal tidak dapat dihubungi.", retryable: true, details: null });
  }
  if (!response.ok) {
    const body = await response.json().catch(() => null) as { error?: AppError } | null;
    if (response.status === 401 && path !== "/auth/login" && path !== "/auth/session") {
      unauthorizedHandler?.();
    }
    throw new ApiError(body?.error ?? {
      code: "INTERNAL_ERROR", message: "Permintaan gagal diproses.", retryable: true, details: null,
    });
  }
  return response.status === 204 ? undefined as T : response.json();
}

export function message(error: unknown) {
  if (error instanceof ApiError) return `${error.message} (${error.data.code})`;
  return "Terjadi kesalahan. Coba lagi.";
}

export function fileUrl(id: string) {
  return `${base}/downloads/${encodeURIComponent(id)}/file`;
}

export function assetUrl(path: string | null | undefined) {
  if (!path) return null;
  return path.startsWith("/api/") ? `${base}${path.slice(4)}` : path;
}
