"use client";

import { useRef, useState } from "react";
import { useApp } from "@/components/app-shell";
import { api, message } from "@/lib/api";
import type { Job } from "@/lib/types";

export function useJobActions(refresh: () => void) {
  const { notify } = useApp();
  const active = useRef(new Set<string>());
  const [pending, setPending] = useState<string[]>([]);
  const [location, setLocation] = useState<string | null>(null);

  async function run(job: Job, work: () => Promise<void>, success: string, onError?: (text: string) => void) {
    if (active.current.has(job.id)) return false;
    active.current.add(job.id);
    setPending(Array.from(active.current));
    try {
      await work();
      if (success) notify(success);
      return true;
    } catch (error) {
      if (onError) onError(message(error));
      else notify(message(error), true);
      return false;
    } finally {
      active.current.delete(job.id);
      setPending(Array.from(active.current));
      refresh();
    }
  }

  return {
    pending, location, closeLocation: () => setLocation(null),
    cancel: (job: Job) => run(job, async () => {
      await api(`/downloads/${encodeURIComponent(job.id)}/cancel`, { method: "POST" });
    }, "Permintaan pembatalan diterima."),
    retry: (job: Job) => run(job, async () => {
      await api(`/downloads/${encodeURIComponent(job.id)}/retry`, { method: "POST" });
    }, "Unduhan ulang masuk antrean."),
    remove: (job: Job, file: boolean, onError: (text: string) => void) => run(job, async () => {
      await api(`/downloads/${encodeURIComponent(job.id)}?delete_file=${file}`, { method: "DELETE" });
    }, "Catatan unduhan dihapus.", onError),
    showLocation: (job: Job) => run(job, async () => {
      const value = await api<{ path: string }>(`/downloads/${encodeURIComponent(job.id)}/location`);
      setLocation(value.path);
    }, ""),
  };
}
