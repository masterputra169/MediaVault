"use client";

import { Check, Cpu, HardDrive, LoaderCircle, RefreshCw, Save, SlidersHorizontal } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { api, message } from "@/lib/api";
import { niceBytes, qualityLabel } from "@/lib/display";
import type { AudioFormat, Health, Preferences, Quality, Settings, Theme } from "@/lib/types";
import { Dialog } from "./dialog";

const dependencyNames: Record<keyof Health["dependencies"], string> = {
  python: "Python", yt_dlp: "yt-dlp", ffmpeg: "FFmpeg", ffprobe: "ffprobe",
  javascript_runtime: "Runtime JS", yt_dlp_ejs: "yt-dlp-ejs",
};

export function SettingsModal({
  health, checking, reconnect, notify, setTheme, onClose, onSaved,
}: {
  health: Health | null;
  checking: boolean;
  reconnect: () => void;
  notify: (text: string, error?: boolean) => void;
  setTheme: (theme: Theme) => void;
  onClose: () => void;
  onSaved?: (prefs: Preferences) => void;
}) {
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
      onSaved?.(result.preferences);
      notify("Preferensi disimpan.");
    } catch (reason) {
      notify(message(reason), true);
    } finally {
      setSaving(false);
      locked.current = false;
    }
  }

  return (
    <Dialog title="Pengaturan & Status Server" onClose={onClose} locked={saving} wide>
      <div className="settings-modal-grid">
        <section className="panel settings-subpanel">
          <div className="section-heading">
            <h3 className="section-label"><SlidersHorizontal size={16} /> Preferensi</h3>
          </div>
          {error && <p className="field-error" role="alert">{error}</p>}
          {preferences ? (
            <form className="preferences-form" onSubmit={(event) => void save(event)}>
              <label>Kualitas video default
                <select value={preferences.default_video_quality}
                  onChange={(event) => setPreferences({ ...preferences, default_video_quality: event.target.value as Quality })}>
                  {(["best", "1080", "720", "480", "360"] as const).map((v) => <option key={v} value={v}>{qualityLabel(v)}</option>)}
                </select>
              </label>
              <label>Format audio default
                <select value={preferences.default_audio_format}
                  onChange={(event) => setPreferences({ ...preferences, default_audio_format: event.target.value as AudioFormat })}>
                  <option value="mp3">MP3</option><option value="m4a">M4A</option>
                </select>
              </label>
              <label>Tema
                <select value={preferences.theme}
                  onChange={(event) => setPreferences({ ...preferences, theme: event.target.value as Theme })}>
                  <option value="dark">Gelap</option><option value="light">Terang</option><option value="system">Sistem</option>
                </select>
              </label>
              <button className="button primary small" disabled={saving || !health?.queue_available}>
                {saving ? <LoaderCircle size={15} className="spin" /> : <Save size={15} />}
                {saving ? "Menyimpan…" : "Simpan preferensi"}
              </button>
            </form>
          ) : !error && <p className="muted" role="status">Memuat preferensi…</p>}
        </section>

        <section className="panel settings-subpanel">
          <div className="section-heading">
            <h3 className="section-label"><Cpu size={16} /> Dependency &amp; Storage</h3>
            <button type="button" className="button small" onClick={reconnect} disabled={checking}>
              <RefreshCw size={14} className={checking ? "spin" : ""} /> Cek ulang
            </button>
          </div>
          {health ? (
            <dl className="details-list compact-details">
              {Object.entries(dependencyNames).map(([key, label]) => {
                const dep = health.dependencies[key as keyof Health["dependencies"]];
                return (
                  <div key={key}>
                    <dt>{label}</dt>
                    <dd className={dep.available ? "good-text" : "warning-text"}>
                      {dep.available && <Check size={14} />}{dep.name && `${dep.name} `}{dep.version ?? "Tidak tersedia"}
                    </dd>
                  </div>
                );
              })}
              <div>
                <dt><HardDrive size={13} style={{ display: "inline", verticalAlign: "-2px" }} /> Ruang kosong</dt>
                <dd>{niceBytes(health.storage.free_bytes)} {config ? `· maks ${config.max_file_size_mb} MB` : ""}</dd>
              </div>
              <div>
                <dt>Folder simpan</dt>
                <dd><code>{health.storage.download_dir}</code></dd>
              </div>
            </dl>
          ) : <p className="muted">{checking ? "Memeriksa backend…" : "Backend tidak terhubung."}</p>}
        </section>
      </div>
    </Dialog>
  );
}
