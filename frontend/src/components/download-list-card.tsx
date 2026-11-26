"use client";

import {
  ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight,
  Copy, Download, FolderOpen, History, LoaderCircle, RotateCcw, Search, Trash2, X,
} from "lucide-react";
import { useState } from "react";
import { useJobActions } from "@/hooks/use-job-actions";
import { fileUrl } from "@/lib/api";
import { duration, isActive, niceBytes, qualityLabel } from "@/lib/display";
import { platforms, statuses, type Job, type JobCounts, type Platform } from "@/lib/types";
import { Dialog } from "./dialog";
import { Thumbnail } from "./media-preview";

export type FilterTab = "all" | "active" | "video" | "audio" | "image";

const dateFormat = new Intl.DateTimeFormat("id-ID", { day: "numeric", month: "long", year: "numeric", hour: "2-digit", minute: "2-digit" });

function kindLabel(job: Job) {
  if (job.kind === "video") return `Video · ${job.container.toUpperCase()} ${qualityLabel(job.quality)}`;
  if (job.kind === "audio") return `Audio · ${job.container.toUpperCase()}`;
  if (job.container === "zip") return "Foto · ZIP Album";
  return `Foto · ${job.container.toUpperCase()}${job.image_index ? ` #${job.image_index}` : ""}`;
}

export function DownloadListCard({
  jobs, total, counts, loading, error, page, pageSize, filterTab, search, platform,
  onTabChange, onSearchChange, onPlatformChange, onPageChange, onPageSizeChange, onRetryLoad, action,
}: {
  jobs: Job[];
  total: number;
  counts: JobCounts;
  loading: boolean;
  error: string | null;
  page: number;
  pageSize: number;
  filterTab: FilterTab;
  search: string;
  platform: "" | Platform;
  onTabChange: (tab: FilterTab) => void;
  onSearchChange: (value: string) => void;
  onPlatformChange: (value: "" | Platform) => void;
  onPageChange: (page: number) => void;
  onPageSizeChange: (size: number) => void;
  onRetryLoad: () => void;
  action: ReturnType<typeof useJobActions>;
}) {
  const [target, setTarget] = useState<Job | null>(null);
  const [deleteFile, setDeleteFile] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [copyMessage, setCopyMessage] = useState<string | null>(null);

  const totalPages = Math.max(1, Math.ceil(total / pageSize));
  const safePage = Math.min(page, totalPages - 1);
  const rangeStart = total === 0 ? 0 : safePage * pageSize + 1;
  const rangeEnd = Math.min(total, (safePage + 1) * pageSize);

  const tabs: { id: FilterTab; label: string; count: number }[] = [
    { id: "all", label: "Semua", count: counts.all },
    { id: "active", label: "Aktif", count: counts.active },
    { id: "video", label: "Video", count: counts.video },
    { id: "audio", label: "Audio", count: counts.audio },
    { id: "image", label: "Foto", count: counts.image },
  ];

  async function confirmRemove() {
    setDeleteError(null);
    if (target && await action.remove(target, deleteFile, setDeleteError)) setTarget(null);
  }

  return <>
    <section className="panel list-card" aria-labelledby="list-card-title">
      <div className="list-card-header">
        <div className="list-title-row">
          <h2 id="list-card-title">Daftar unduhan</h2>
          {counts.active > 0 && <span className="pill accent-pill">{counts.active} berjalan</span>}
        </div>
        <div className="list-toolbar">
          <div className="filter-pills" role="tablist" aria-label="Filter tipe unduhan">
            <span className="filter-prefix">Tipe:</span>
            {tabs.map((tab) => (
              <button
                key={tab.id}
                type="button"
                role="tab"
                aria-selected={filterTab === tab.id}
                className={`filter-pill ${filterTab === tab.id ? "active" : ""}`}
                onClick={() => onTabChange(tab.id)}
              >
                <span>{tab.label}</span>
                <span className="pill-count">{tab.count}</span>
              </button>
            ))}
          </div>
          <div className="toolbar-controls">
            <div className="search-field compact-search">
              <Search size={15} aria-hidden />
              <input
                type="search"
                aria-label="Cari judul media"
                placeholder="Cari judul media…"
                maxLength={240}
                value={search}
                onChange={(event) => onSearchChange(event.target.value)}
              />
            </div>
            <select
              aria-label="Filter platform"
              className="compact-select"
              value={platform}
              onChange={(event) => onPlatformChange(event.target.value as "" | Platform)}
            >
              <option value="">Semua platform</option>
              {Object.entries(platforms).map(([key, label]) => (
                <option key={key} value={key}>{label}</option>
              ))}
            </select>
          </div>
        </div>
      </div>

      <div className="list-card-body">
        {error && (
          <div className="list-notice">
            <div className="notice notice-error" role="alert">
              <span>{error}</span>
              <button type="button" className="button small" onClick={onRetryLoad}>Coba lagi</button>
            </div>
          </div>
        )}
        {loading && jobs.length === 0 ? (
          <p className="loading-text" role="status"><LoaderCircle size={17} className="spin" /> Memuat daftar unduhan…</p>
        ) : jobs.length === 0 ? (
          <div className="list-empty">
            <span className="empty-icon"><History size={19} /></span>
            <div>
              <strong>{search || platform || filterTab !== "all" ? "Tidak ada hasil yang cocok" : "Belum ada unduhan"}</strong>
              <p>{search || platform || filterTab !== "all"
                ? "Ubah kata kunci, filter tipe, atau platform untuk melihat item lain."
                : "Tempel tautan video, audio, atau foto di atas lalu klik Analisis URL."}</p>
            </div>
          </div>
        ) : (
          <ul className="dl-rows">
            {jobs.map((job) => {
              const name = job.title || job.output_filename || platforms[job.platform];
              const pending = action.pending.includes(job.id);
              const active = isActive(job.status);
              return (
                <li key={job.id} className="dl-row">
                  <div className="dl-row-main">
                    <Thumbnail src={job.thumbnail_preview_url ?? job.thumbnail_url} className="row-thumb" />
                    <div className="dl-row-info">
                      <div className="dl-row-top">
                        <strong title={name}>{name}</strong>
                        <span className={`badge badge-${job.status}`}>{statuses[job.status]}</span>
                        <span className="badge badge-type">{kindLabel(job)}</span>
                      </div>
                      {active ? (
                        <div className="dl-row-progress">
                          <progress max={100} value={job.progress_percent ?? undefined} aria-label={`Progres ${name}`} />
                          <span>{job.progress_percent === null ? "…" : `${job.progress_percent}%`}</span>
                          {job.downloaded_bytes !== null && (
                            <span className="muted">
                              {niceBytes(job.downloaded_bytes)}
                              {job.total_bytes !== null ? ` / ${niceBytes(job.total_bytes)}` : ""}
                            </span>
                          )}
                          {job.eta_seconds !== null && <span className="muted">· {duration(job.eta_seconds)}</span>}
                        </div>
                      ) : (
                        <div className="dl-row-meta">
                          <time dateTime={job.created_at}>{dateFormat.format(new Date(job.created_at))}</time>
                          <span>· {platforms[job.platform]}</span>
                          {job.file_size_bytes !== null && <span>· {niceBytes(job.file_size_bytes)}</span>}
                          {job.error && <span className="row-error">· {job.error.message}</span>}
                          {job.status === "completed" && !job.file_available && <span className="row-error">· File hilang</span>}
                        </div>
                      )}
                    </div>
                  </div>
                  <div className="dl-row-actions">
                    {active ? (
                      <button type="button" className="chip-btn chip-warn" disabled={pending}
                        onClick={() => void action.cancel(job)}>
                        {pending ? <LoaderCircle size={14} className="spin" /> : <X size={14} />}
                        <span>Batalkan</span>
                      </button>
                    ) : (
                      <>
                        {job.file_available && (
                          <>
                            <a className="chip-btn chip-blue" href={fileUrl(job.id)} title="Unduh ke perangkat">
                              <Download size={14} /><span>Unduh</span>
                            </a>
                            <button type="button" className="chip-btn chip-green" disabled={pending}
                              onClick={() => void action.showLocation(job)} title="Lihat lokasi file">
                              <FolderOpen size={14} /><span>Lokasi</span>
                            </button>
                          </>
                        )}
                        <button type="button" className="chip-btn chip-purple" disabled={pending}
                          onClick={() => void action.retry(job)} title="Unduh ulang">
                          <RotateCcw size={14} /><span>Ulangi</span>
                        </button>
                        <button type="button" className="chip-btn chip-red icon-only" disabled={pending}
                          aria-label={`Hapus ${name}`} title="Hapus"
                          onClick={() => { setDeleteError(null); setDeleteFile(false); setTarget(job); }}>
                          <Trash2 size={14} />
                        </button>
                      </>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>

      <div className="list-card-footer">
        <span className="footer-summary">
          {total > 0 ? <>Menampilkan <strong>{rangeStart} - {rangeEnd}</strong> dari <strong>{total}</strong> unduhan</>
            : "Menampilkan 0 unduhan"}
        </span>
        <div className="pagination-controls">
          <label className="page-size-picker">
            <span>Tampilkan:</span>
            <select value={pageSize} onChange={(event) => onPageSizeChange(Number(event.target.value))}>
              {[3, 4, 5, 10].map((size) => <option key={size} value={size}>{size} data</option>)}
            </select>
          </label>
          <div className="pager-buttons" role="navigation" aria-label="Navigasi halaman unduhan">
            <button type="button" className="pager-btn" aria-label="Halaman pertama"
              disabled={safePage === 0} onClick={() => onPageChange(0)}>
              <ChevronsLeft size={15} />
            </button>
            <button type="button" className="pager-btn" aria-label="Halaman sebelumnya"
              disabled={safePage === 0} onClick={() => onPageChange(safePage - 1)}>
              <ChevronLeft size={15} />
            </button>
            <span className="pager-status" aria-live="polite">{safePage + 1} / {totalPages}</span>
            <button type="button" className="pager-btn" aria-label="Halaman berikutnya"
              disabled={safePage + 1 >= totalPages} onClick={() => onPageChange(safePage + 1)}>
              <ChevronRight size={15} />
            </button>
            <button type="button" className="pager-btn" aria-label="Halaman terakhir"
              disabled={safePage + 1 >= totalPages} onClick={() => onPageChange(totalPages - 1)}>
              <ChevronsRight size={15} />
            </button>
          </div>
        </div>
      </div>
    </section>

    {target && (
      <Dialog title="Hapus riwayat unduhan" onClose={() => setTarget(null)} locked={action.pending.includes(target.id)}>
        <p className="muted">{target.title ?? target.output_filename ?? platforms[target.platform]}</p>
        {deleteError && <p className="field-error" role="alert">{deleteError}</p>}
        {target.file_available && (
          <label className="checkbox-row">
            <input type="checkbox" checked={deleteFile} onChange={(event) => setDeleteFile(event.target.checked)}
              disabled={action.pending.includes(target.id)} /> Hapus juga file di server secara permanen
          </label>
        )}
        <div className="modal-actions">
          <button type="button" className="button" disabled={action.pending.includes(target.id)} onClick={() => setTarget(null)}>Kembali</button>
          <button type="button" className="button danger" disabled={action.pending.includes(target.id)} onClick={() => void confirmRemove()}>
            {deleteFile ? "Hapus catatan dan file" : "Hapus catatan"}
          </button>
        </div>
      </Dialog>
    )}

    {action.location && (
      <Dialog title="Lokasi file" onClose={() => { setCopyMessage(null); action.closeLocation(); }}>
        <div className="path-value">{action.location}</div>
        <p className="muted" role="status">{copyMessage}</p>
        <div className="modal-actions">
          <button type="button" className="button" onClick={() => { setCopyMessage(null); action.closeLocation(); }}>Tutup</button>
          <button type="button" className="button primary" onClick={async () => {
            try {
              await navigator.clipboard.writeText(action.location ?? "");
              setCopyMessage("Lokasi file disalin.");
            } catch {
              setCopyMessage("Pilih dan salin lokasi file secara manual.");
            }
          }}><Copy size={16} /> Salin lokasi</button>
        </div>
      </Dialog>
    )}
  </>;
}
