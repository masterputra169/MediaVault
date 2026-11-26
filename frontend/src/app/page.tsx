"use client";

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { useApp } from "@/components/app-shell";
import { Dialog } from "@/components/dialog";
import { DownloadListCard, type FilterTab } from "@/components/download-list-card";
import { FormatPicker } from "@/components/format-picker";
import { MediaPreview } from "@/components/media-preview";
import { UrlInput } from "@/components/url-input";
import { useDownloads } from "@/hooks/use-downloads";
import { useJobActions } from "@/hooks/use-job-actions";
import { api, message } from "@/lib/api";
import type { CreateDownload, JobCounts, JobTicket, Metadata, Platform } from "@/lib/types";
import { validateUrl } from "@/lib/url";

const EMPTY_COUNTS: JobCounts = { all: 0, active: 0, video: 0, audio: 0, image: 0 };

function subscribeViewport(callback: () => void) {
  window.addEventListener("resize", callback);
  return () => window.removeEventListener("resize", callback);
}

function getDefaultPageSizeSnapshot() {
  if (window.innerWidth < 700 || window.innerHeight < 790) return 3;
  if (window.innerHeight < 920) return 4;
  return 5;
}

export default function Dashboard() {
  const { health, checking, preferences, notify, openSettings } = useApp();
  const autoPageSize = useSyncExternalStore(subscribeViewport, getDefaultPageSizeSnapshot, () => 4);
  const [url, setUrl] = useState("");
  const [media, setMedia] = useState<Metadata | null>(null);
  const [selectedSlide, setSelectedSlide] = useState(1);
  const [urlError, setUrlError] = useState<string | null>(null);
  const [analyzing, setAnalyzing] = useState(false);
  const [creating, setCreating] = useState(false);

  const [filterTab, setFilterTab] = useState<FilterTab>("all");
  const [search, setSearch] = useState("");
  const [lookup, setLookup] = useState("");
  const [platform, setPlatform] = useState<"" | Platform>("");
  const [page, setPage] = useState(0);
  const [customPageSize, setCustomPageSize] = useState<number | null>(null);
  const pageSize = customPageSize ?? autoPageSize;

  const analysis = useRef<AbortController | null>(null);
  const writing = useRef(false);

  useEffect(() => {
    const timer = window.setTimeout(() => setLookup(search.trim()), 250);
    return () => window.clearTimeout(timer);
  }, [search]);

  const query = new URLSearchParams({ limit: String(pageSize), offset: String(page * pageSize) });
  if (lookup) query.set("search", lookup);
  if (platform) query.set("platform", platform);
  if (filterTab === "active") query.set("active", "true");
  else if (filterTab !== "all") query.set("kind", filterTab);

  const list = useDownloads(query.toString());
  const refresh = list.refresh;
  const action = useJobActions(refresh);

  useEffect(() => () => analysis.current?.abort(), []);
  useEffect(() => { if (health) refresh(); }, [health, refresh]);

  const total = list.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / pageSize));
  const currentPage = Math.min(page, totalPages - 1);

  const counts: JobCounts = list.data?.counts ?? {
    ...EMPTY_COUNTS,
    all: filterTab === "all" && !lookup && !platform ? total : 0,
    active: list.data?.active_count ?? 0,
  };

  function change(next: string) {
    analysis.current?.abort();
    analysis.current = null;
    setAnalyzing(false);
    setUrl(next);
    setUrlError(next.length > 2048 ? validateUrl(next) : null);
  }

  async function analyze() {
    if (analysis.current || !health?.dependencies.yt_dlp.available) return;
    const input = url.trim();
    const invalid = validateUrl(input);
    setUrlError(invalid);
    if (invalid) return;
    const controller = new AbortController();
    analysis.current = controller;
    setAnalyzing(true);
    setMedia(null);
    try {
      const result = await api<Metadata>("/media/analyze", {
        method: "POST", body: JSON.stringify({ url: input }), signal: controller.signal,
      });
      if (!controller.signal.aborted) {
        setSelectedSlide(1);
        setMedia(result);
      }
    } catch (error) {
      if (!controller.signal.aborted) setUrlError(message(error));
    } finally {
      if (analysis.current === controller) {
        analysis.current = null;
        setAnalyzing(false);
      }
    }
  }

  async function download(body: CreateDownload) {
    if (writing.current) return;
    writing.current = true;
    setCreating(true);
    try {
      await api<JobTicket>("/downloads", { method: "POST", body: JSON.stringify(body) });
      notify("Unduhan masuk antrean.");
      setMedia(null);
      setUrl("");
      setPage(0);
      refresh();
    } catch (error) {
      notify(message(error), true);
    } finally {
      writing.current = false;
      setCreating(false);
    }
  }

  return <>
    <div className="single-workspace">
      <UrlInput
        url={url}
        busy={analyzing}
        disabled={checking || !health?.dependencies.yt_dlp.available}
        error={urlError}
        onChange={change}
        onAnalyze={() => void analyze()}
        onClipboardError={() => notify("Akses clipboard tidak tersedia. Tempel langsung di kolom tautan.", true)}
      />

      {health && !health.dependencies.yt_dlp.available && (
        <div className="notice" role="status">
          <span>yt-dlp diperlukan untuk menganalisis tautan.</span>
          <button type="button" className="button small" onClick={openSettings}>Lihat dependency</button>
        </div>
      )}

      <DownloadListCard
        jobs={list.data?.items ?? []}
        total={total}
        counts={counts}
        loading={list.loading}
        error={list.error}
        page={currentPage}
        pageSize={pageSize}
        filterTab={filterTab}
        search={search}
        platform={platform}
        onTabChange={(next) => { setFilterTab(next); setPage(0); }}
        onSearchChange={(next) => { setSearch(next); setPage(0); }}
        onPlatformChange={(next) => { setPlatform(next); setPage(0); }}
        onPageChange={setPage}
        onPageSizeChange={(next) => { setCustomPageSize(next); setPage(0); }}
        onRetryLoad={refresh}
        action={action}
      />
    </div>

    {media && (
      <Dialog title="Pratinjau & Opsi Unduhan" onClose={() => setMedia(null)} locked={creating} wide>
        <div className="result-grid modal-result-grid">
          <MediaPreview
            media={media}
            activeSrc={
              media.images && media.images.length > 0
                ? (media.images[selectedSlide - 1]?.preview_url ?? media.images[selectedSlide - 1]?.url ?? media.preview_url ?? media.thumbnail_url)
                : (media.preview_url ?? media.thumbnail_url)
            }
          />
          <FormatPicker
            key={media.url}
            media={media}
            preferences={preferences}
            onSlideSelect={setSelectedSlide}
            onDownload={(body) => void download(body)}
            busy={creating}
            disabled={!health?.storage.writable || !health?.queue_available}
          />
        </div>
      </Dialog>
    )}
  </>;
}
