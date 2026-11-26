"use client";

import { useCallback, useEffect, useState } from "react";
import { useApp } from "@/components/app-shell";
import { ApiError, api, message } from "@/lib/api";
import type { JobPage } from "@/lib/types";

export function useDownloads(query: string) {
  const { markOffline, reconnect } = useApp();
  const [revision, setRevision] = useState(0);
  const [snapshot, setSnapshot] = useState<{ query: string; data: JobPage | null; error: string | null }>();
  const refresh = useCallback(() => setRevision((value) => value + 1), []);

  useEffect(() => {
    let controller: AbortController;
    let timer: number | undefined;
    let activeBeforeFailure = false;
    let offline = false;

    async function fetchJobs() {
      controller?.abort();
      const request = new AbortController();
      controller = request;
      try {
        const data = await api<JobPage>("/downloads?" + query, { signal: request.signal });
        if (request.signal.aborted) return;
        setSnapshot({ query, data, error: null });
        activeBeforeFailure = data.active_count > 0;
        if (offline) {
          offline = false;
          reconnect();
        }
        if (data.active_count > 0 && !document.hidden) {
          timer = window.setTimeout(() => void fetchJobs(), 1500);
        }
      } catch (error) {
        if (request.signal.aborted) return;
        setSnapshot((previous) => ({ query, data: previous?.query === query ? previous.data : null, error: message(error) }));
        if (error instanceof ApiError && error.data.code === "SERVER_OFFLINE") {
          offline = true;
          markOffline();
          if (activeBeforeFailure && !document.hidden) timer = window.setTimeout(() => void fetchJobs(), 5000);
        }
      }
    }

    function visibility() {
      window.clearTimeout(timer);
      if (document.hidden) controller?.abort();
      else void fetchJobs();
    }

    document.addEventListener("visibilitychange", visibility);
    if (!document.hidden) void fetchJobs();
    return () => {
      document.removeEventListener("visibilitychange", visibility);
      window.clearTimeout(timer);
      controller?.abort();
    };
  }, [query, revision, markOffline, reconnect]);

  const visible = snapshot?.query === query ? snapshot : undefined;
  return { data: visible?.data ?? null, error: visible?.error ?? null, loading: !visible, refresh };
}
