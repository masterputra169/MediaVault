"use client";

import { ChevronLeft, ChevronRight, History, LoaderCircle, Search } from "lucide-react";
import { useEffect, useState } from "react";
import { useApp } from "@/components/app-shell";
import { EmptyJobs, HistoryTable } from "@/components/history-table";
import { useDownloads } from "@/hooks/use-downloads";
import { useJobActions } from "@/hooks/use-job-actions";
import { platforms, statuses } from "@/lib/types";

const pageSize = 25;

export default function HistoryPage() {
  const [search, setSearch] = useState("");
  const [status, setStatus] = useState("");
  const [platform, setPlatform] = useState("");
  const [page, setPage] = useState(0);
  const [lookup, setLookup] = useState("");
  const { health } = useApp();

  useEffect(() => {
    const timer = window.setTimeout(() => setLookup(search.trim()), 250);
    return () => window.clearTimeout(timer);
  }, [search]);

  const query = new URLSearchParams({ limit: String(pageSize), offset: String(page * pageSize) });
  if (lookup) query.set("search", lookup);
  if (status) query.set("status", status);
  if (platform) query.set("platform", platform);
  const { data, error, loading, refresh } = useDownloads(query.toString());
  const action = useJobActions(refresh);

  useEffect(() => { if (health) refresh(); }, [health, refresh]);

  return <>
    <div className="page-heading">
      <div className="eyebrow"><History size={17} /> Pustaka lokal</div>
      <h1>Riwayat unduhan<span>.</span></h1>
      <p>Cari unduhan, ulangi pekerjaan, dan kelola file yang tersimpan.</p>
    </div>
    <section aria-label="Riwayat unduhan">
      <div className="panel filter-bar">
        <label className="filter-search">Cari judul
          <div className="search-field"><Search size={17} /><input type="search" value={search}
            maxLength={240} placeholder="Judul media" onChange={(event) => { setSearch(event.target.value); setPage(0); }} /></div>
        </label>
        <label>Status<select value={status} onChange={(event) => { setStatus(event.target.value); setPage(0); }}>
          <option value="">Semua status</option>
          {Object.entries(statuses).map(([key, label]) => <option key={key} value={key}>{label}</option>)}
        </select></label>
        <label>Platform<select value={platform} onChange={(event) => { setPlatform(event.target.value); setPage(0); }}>
          <option value="">Semua platform</option>
          {Object.entries(platforms).map(([key, label]) => <option key={key} value={key}>{label}</option>)}
        </select></label>
      </div>
      <div className="section-heading history-heading"><h2>Unduhan</h2>
        {data && <span className="muted">{data.total} hasil · Terbaru dulu</span>}
      </div>
      {loading && <p className="panel loading-text" role="status"><LoaderCircle size={17} className="spin" /> Memuat riwayat…</p>}
      {error && <div className="notice notice-error" role="alert"><span>{error}</span>
        <button className="button small" onClick={refresh}>Coba lagi</button></div>}
      {data && (data.items.length ? <HistoryTable jobs={data.items} action={action} />
        : <EmptyJobs title={lookup || status || platform ? "Tidak ada hasil" : "Belum ada unduhan"}
          text={lookup || status || platform ? "Ubah pencarian atau filter untuk melihat hasil lain." : "Unduhan akan tercatat di sini."} />)}
      {data && (page > 0 || data.total > pageSize) && <nav className="pagination" aria-label="Halaman riwayat">
        <button className="button small" disabled={!page} onClick={() => setPage(page - 1)}><ChevronLeft size={16} /> Sebelumnya</button>
        <span className="muted">Halaman {page + 1}</span>
        <button className="button small" disabled={(page + 1) * pageSize >= data.total}
          onClick={() => setPage(page + 1)}>Berikutnya <ChevronRight size={16} /></button>
      </nav>}
    </section>
  </>;
}
