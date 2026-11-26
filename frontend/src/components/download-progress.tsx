"use client";

import { Clock3, DownloadCloud, LoaderCircle, X } from "lucide-react";
import { duration, niceBytes } from "@/lib/display";
import { platforms, statuses, type Job } from "@/lib/types";

export function StatusPill({ job }: { job: Job }) {
  return <span className={`status-pill status-${job.status}`}><span />{statuses[job.status]}</span>;
}

export function DownloadProgress({ job, pending, onCancel }: {
  job: Job; pending: boolean; onCancel: () => void;
}) {
  const name = job.title || platforms[job.platform];
  const value = job.progress_percent;
  const showProgress = job.status !== "queued";
  return <li className="job-card">
    <div className="job-icon"><DownloadCloud size={20} /></div>
    <div className="job-data">
      <div className="job-top"><strong>{name}</strong><span aria-live="polite"><StatusPill job={job} /></span></div>
      {showProgress && <>
        <div className="progress-row">
          <progress max={100} value={value ?? undefined} aria-label={`Progres unduhan ${name}`} />
          <span>{value === null ? "—" : `${value}%`}</span>
          <span className="sr-only" aria-live="polite">{value !== null && Math.floor(value / 10) * 10 + "%"}</span>
        </div>
        <p className="job-stats">
          {job.downloaded_bytes !== null && <span>{niceBytes(job.downloaded_bytes)}
            {job.total_bytes !== null && ` / ${niceBytes(job.total_bytes)}`}</span>}
          {job.speed_bytes_per_second !== null && <span>{niceBytes(job.speed_bytes_per_second)}/d</span>}
          {job.eta_seconds !== null && <span><Clock3 size={14} /> {duration(job.eta_seconds)}</span>}
        </p>
      </>}
    </div>
    <button className="button small" type="button" onClick={onCancel} disabled={pending}>
      {pending ? <LoaderCircle className="spin" size={15} /> : <X size={15} />}
      Batalkan
    </button>
  </li>;
}
