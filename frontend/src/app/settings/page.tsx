"use client";

import { Check, CircleHelp, Cpu, HardDrive, LoaderCircle, RefreshCw, Save, SlidersHorizontal } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { useApp } from "@/components/app-shell";
import { api, message } from "@/lib/api";
import { niceBytes, qualityLabel } from "@/lib/display";
import type { AudioFormat, Health, Preferences, Quality, Settings, Theme } from "@/lib/types";

const dependencyNames: Record<keyof Health["dependencies"], string> = {
  python: "Python", yt_dlp: "yt-dlp", ffmpeg: "FFmpeg", ffprobe: "ffprobe",
  javascript_runtime: "Runtime JavaScript", yt_dlp_ejs: "yt-dlp-ejs",
};

export default function SettingsPage() {
  const { health, checking, reconnect, notify, setTheme } = useApp();
  const [config, setConfig] = useState<Settings | null>(null);
  const [preferences, setPreferences] = useState<Preferences | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const locked = useRef(false);

  useEffect(() => {
    const controller = new AbortController();
    api<Settings>("/settings", { signal: controller.signal }).then((value) => {
      setConfig(value);
      setPreferences(value.preferences);
      setError(null);
    }).catch((reason) => { if (!controller.signal.aborted) setError(message(reason)); });
    return () => controller.abort();
  }, [health]);

  async function save(event: FormEvent) {
    event.preventDefault();
    if (!preferences || locked.current) return;
    locked.current = true;
    setSaving(true);
    try {
      const result = await api<Settings>("/settings", { method: "PATCH", body: JSON.stringify(preferences) });
      setConfig(result);
      setPreferences(result.preferences);
      setTheme(result.preferences.theme);
      notify("Preferensi disimpan.");
    } catch (reason) {
      notify(message(reason), true);
    } finally {
      setSaving(false);
      locked.current = false;
    }
  }

  return <>
    <div className="page-heading">
      <div className="eyebrow"><SlidersHorizontal size={17} /> Konfigurasi lokal</div>
      <h1>Pengaturan<span>.</span></h1>
      <p>Periksa dependency, penyimpanan, dan pilihan default Anda.</p>
    </div>
    <div className="settings-grid">
      <section className="panel settings-panel">
        <div className="section-heading">
          <h2 className="section-label"><Cpu size={19} /> Dependency</h2>
          <button className="button small" onClick={reconnect} disabled={checking}>
            <RefreshCw size={15} className={checking ? "spin" : ""} /> Periksa koneksi
          </button>
        </div>
        {health ? <dl className="details-list">
          {Object.entries(dependencyNames).map(([key, label]) => {
            const dep = health.dependencies[key as keyof Health["dependencies"]];
            return <div key={key}><dt>{label}</dt><dd className={dep.available ? "good-text" : "warning-text"}>
              {dep.available && <Check size={15} />}{dep.name && `${dep.name} `}{dep.version ?? "Tidak tersedia"}
              {!dep.available && dep.version && " · Versi tidak kompatibel"}</dd></div>;
          })}
        </dl> : <p className="muted" role="status">{checking ? "Memeriksa backend…" : "Status dependency belum tersedia."}</p>}
      </section>

      <section className="panel settings-panel">
        <h2 className="section-label"><HardDrive size={19} /> Penyimpanan</h2>
        {health ? <>
          <p className="field-label">Folder unduhan</p>
          <div className="path-value">{health.storage.download_dir}</div>
          <dl className="details-list">
            <div><dt>Akses folder</dt><dd className={health.storage.writable ? "good-text" : "warning-text"}>
              {health.storage.writable ? "Dapat ditulis" : "Gagal menulis"}</dd></div>
            <div><dt>Ruang kosong folder unduhan</dt><dd>{niceBytes(health.storage.free_bytes)}</dd></div>
            <div><dt>Ruang kosong folder sementara</dt><dd>{niceBytes(health.storage.temp_free_bytes)}</dd></div>
            {config && <>
              <div><dt>Unduhan berjalan bersamaan</dt><dd>{config.max_active_downloads}</dd></div>
              <div><dt>Batas antrean</dt><dd>{config.max_queue_length}</dd></div>
              <div><dt>Batas ukuran file</dt><dd>{config.max_file_size_mb} MB</dd></div>
            </>}
          </dl>
          <p className="muted detail">Untuk mengganti folder, ubah <code>DOWNLOAD_DIR</code> di <code>.env</code> lalu mulai ulang backend.</p>
        </> : <p className="muted">Status penyimpanan belum tersedia.</p>}
      </section>

      <section className="panel settings-panel">
        <h2 className="section-label"><SlidersHorizontal size={19} /> Preferensi</h2>
        {error && <p className="field-error" role="alert">{error}</p>}
        {preferences ? <form className="preferences-form" onSubmit={(event) => void save(event)}>
          <label>Kualitas video default<select value={preferences.default_video_quality}
            onChange={(event) => setPreferences({ ...preferences, default_video_quality: event.target.value as Quality })}>
            {(["best", "1080", "720", "480", "360"] as const).map((value) => <option key={value} value={value}>{qualityLabel(value)}</option>)}
          </select></label>
          <label>Format audio default<select value={preferences.default_audio_format}
            onChange={(event) => setPreferences({ ...preferences, default_audio_format: event.target.value as AudioFormat })}>
            <option value="mp3">MP3</option><option value="m4a">M4A</option>
          </select></label>
          <label>Tema<select value={preferences.theme}
            onChange={(event) => setPreferences({ ...preferences, theme: event.target.value as Theme })}>
            <option value="dark">Gelap</option><option value="light">Terang</option><option value="system">Sistem</option>
          </select></label>
          <button className="button primary" disabled={saving || !health?.queue_available}>
            {saving ? <LoaderCircle size={16} className="spin" /> : <Save size={16} />} {saving ? "Menyimpan…" : "Simpan preferensi"}
          </button>
        </form> : !error && <p className="muted" role="status">Memuat preferensi…</p>}
      </section>

      <section className="panel settings-panel">
        <h2 className="section-label"><CircleHelp size={19} /> Pemecahan masalah</h2>
        <dl className="help-list">
          <div><dt>FFmpeg atau ffprobe tidak tersedia</dt><dd>Pasang kedua executable dan tambahkan ke PATH, atau atur <code>FFMPEG_PATH</code>.</dd></div>
          <div><dt>Runtime JavaScript atau EJS belum siap</dt><dd>Gunakan Node.js 22+ atau Deno, lalu pasang <code>yt-dlp[default]</code> sesuai README.</dd></div>
          <div><dt>Metadata tidak tersedia atau HTTP 403</dt><dd>Periksa akses ke postingan video atau foto publik dari browser. Lihat README untuk langkah memperbarui yt-dlp atau gallery-dl secara manual.</dd></div>
          <div><dt>Foto atau carousel tidak terdeteksi</dt><dd>Pastikan tautan mengarah ke satu postingan publik (Instagram, Pinterest, X, TikTok foto, atau Reddit) dan tidak memerlukan login.</dd></div>
          <div><dt>Server offline atau port bentrok</dt><dd>Periksa keluaran skrip dev. Jika port backend diganti, samakan <code>NEXT_PUBLIC_API_URL</code>.</dd></div>
        </dl>
      </section>
    </div>
  </>;
}
