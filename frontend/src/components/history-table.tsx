"use client";

import { Copy, Download, FolderOpen, History, RotateCcw, Trash2, X } from "lucide-react";
import { useState } from "react";
import { useJobActions } from "@/hooks/use-job-actions";
import { fileUrl } from "@/lib/api";
import { isActive, niceBytes, qualityLabel } from "@/lib/display";
import { platforms, type Job } from "@/lib/types";
import { Dialog } from "./dialog";
import { StatusPill } from "./download-progress";
import { Thumbnail } from "./media-preview";

const dateFormat = new Intl.DateTimeFormat("id-ID", { dateStyle: "medium", timeStyle: "short" });

export function EmptyJobs({ title = "Belum ada unduhan", text, active = false }: {
  title?: string; text?: string; active?: boolean;
}) {
  return <div className={`empty-state ${active ? "compact" : ""}`}>
    <span className="empty-icon"><History size={20} /></span>
    <div><strong>{title}</strong>{text && <p>{text}</p>}</div>
  </div>;
}

export function HistoryTable({ jobs, action, compact = false }: {
  jobs: Job[]; action: ReturnType<typeof useJobActions>; compact?: boolean;
}) {
  const [target, setTarget] = useState<Job | null>(null);
  const [deleteFile, setDeleteFile] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [copyMessage, setCopyMessage] = useState<string | null>(null);
  async function remove() {
    setDeleteError(null);
    if (target && await action.remove(target, deleteFile, setDeleteError)) setTarget(null);
  }
  return <>
    <div className="table-shell">
      <table className="history-table">
        <thead><tr><th scope="col">Media</th><th scope="col">Status</th><th scope="col">Ukuran</th>
          <th scope="col">Waktu</th><th scope="col">Tindakan</th></tr></thead>
        <tbody>{jobs.map((job) => {
          const name = job.title || job.output_filename || platforms[job.platform];
          const pending = action.pending.includes(job.id);
          return <tr key={job.id}>
            <td className="media-cell">
              {!compact && <Thumbnail src={job.thumbnail_url} className="history-thumb" />}
              <div className="min-w-0"><strong title={name}>{name}</strong>
                <span className="muted table-meta">{platforms[job.platform]} · {job.container.toUpperCase()}
                  {job.kind === "video" && ` · ${qualityLabel(job.quality)}`}
                  {job.kind === "image" && job.image_index ? ` · Slide #${job.image_index}` : ""}</span></div>
            </td>
            <td><StatusPill job={job} />
              {job.error && <span className="table-error">{job.error.message} ({job.error.code})</span>}
              {job.status === "completed" && !job.file_available && <span className="table-error">File tidak ditemukan</span>}
            </td>
            <td className="table-size"><span className="sr-only">Ukuran </span>{niceBytes(job.file_size_bytes)}</td>
            <td><time className="table-date" dateTime={job.created_at}>{dateFormat.format(new Date(job.created_at))}</time></td>
            <td className="table-actions">
              <div className="row-actions">
                {job.file_available && <>
                  <a className="icon-button" href={fileUrl(job.id)} aria-label={`Unduh ke browser ${name}`} title="Unduh ke browser"><Download size={17} /></a>
                  <button className="icon-button" aria-label={`Lihat lokasi file ${name}`} title="Lihat lokasi file" disabled={pending}
                    onClick={() => void action.showLocation(job)}><FolderOpen size={17} /></button>
                </>}
                {isActive(job.status) ? <button className="icon-button" aria-label={`Batalkan ${name}`} title="Batalkan" disabled={pending}
                  onClick={() => void action.cancel(job)}><X size={17} /></button> : <>
                  <button className="icon-button" aria-label={`Unduh ulang ${name}`} title="Unduh ulang" disabled={pending}
                    onClick={() => void action.retry(job)}><RotateCcw size={16} /></button>
                  <button className="icon-button danger-icon" aria-label={`Hapus ${name}`} title="Hapus" disabled={pending}
                    onClick={() => { setDeleteError(null); setDeleteFile(false); setTarget(job); }}><Trash2 size={16} /></button>
                </>}
              </div>
            </td>
          </tr>;
        })}</tbody>
      </table>
    </div>
    {target && <Dialog title="Hapus riwayat unduhan" onClose={() => setTarget(null)} locked={action.pending.includes(target.id)}>
      <p className="muted">{target.title ?? target.output_filename ?? platforms[target.platform]}</p>
      {deleteError && <p className="field-error" role="alert">{deleteError}</p>}
      {target.file_available && <label className="checkbox-row">
        <input type="checkbox" checked={deleteFile} onChange={(event) => setDeleteFile(event.target.checked)}
          disabled={action.pending.includes(target.id)} /> Hapus juga file lokal secara permanen
      </label>}
      <div className="modal-actions">
        <button className="button" disabled={action.pending.includes(target.id)} onClick={() => setTarget(null)}>Kembali</button>
        <button className="button danger" disabled={action.pending.includes(target.id)} onClick={() => void remove()}>
          {deleteFile ? "Hapus catatan dan file" : "Hapus catatan"}</button>
      </div>
    </Dialog>}
    {action.location && <Dialog title="Lokasi file" onClose={() => { setCopyMessage(null); action.closeLocation(); }}>
      <div className="path-value">{action.location}</div>
      <p className="muted" role="status">{copyMessage}</p>
      <div className="modal-actions">
        <button className="button" onClick={() => { setCopyMessage(null); action.closeLocation(); }}>Tutup</button>
        <button className="button primary" onClick={async () => {
          try {
            await navigator.clipboard.writeText(action.location ?? "");
            setCopyMessage("Lokasi file disalin.");
          } catch {
            setCopyMessage("Pilih dan salin lokasi file secara manual.");
          }
        }}><Copy size={16} /> Salin lokasi</button>
      </div>
    </Dialog>}
  </>;
}
