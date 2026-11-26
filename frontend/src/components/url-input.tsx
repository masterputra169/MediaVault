"use client";

import { Clipboard, Link2, LoaderCircle, ScanLine, X } from "lucide-react";
import { useRef, type FormEvent } from "react";

export function UrlInput({ url, busy, disabled, error, onChange, onAnalyze, onClipboardError }: {
  url: string; busy: boolean; disabled: boolean; error: string | null;
  onChange: (value: string) => void; onAnalyze: () => void; onClipboardError: () => void;
}) {
  const input = useRef<HTMLInputElement>(null);

  async function paste() {
    try {
      const text = await navigator.clipboard.readText();
      onChange(text.trim());
    } catch {
      onClipboardError();
    }
    input.current?.focus();
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    onAnalyze();
  }

  return <form className="panel url-panel" onSubmit={submit} noValidate aria-busy={busy}>
    <label className="field-label" htmlFor="media-url">Tautan media</label>
    <div className="url-row">
      <div className="url-field">
        <Link2 size={19} aria-hidden />
        <input ref={input} id="media-url" name="url" type="url" spellCheck={false} autoComplete="off"
          placeholder="https://…" value={url} aria-invalid={!!error}
          aria-describedby={error ? "url-error" : "url-help"} onChange={(event) => onChange(event.target.value)} />
        {url && <button type="button" className="icon-button" aria-label="Kosongkan tautan"
          onClick={() => { onChange(""); input.current?.focus(); }}><X size={17} /></button>}
        <button type="button" className="button small paste-button" onClick={() => void paste()}><Clipboard size={15} /> Tempel</button>
      </div>
      <button className="button primary analyze-button" disabled={busy || disabled}>
        {busy ? <LoaderCircle className="spin" size={18} /> : <ScanLine size={18} />} {busy ? "Menganalisis…" : "Analisis URL"}
      </button>
    </div>
    {error ? <p id="url-error" className="field-error" role="alert">{error}</p>
      : <p id="url-help" className="muted field-help">Gunakan tautan langsung ke satu video, foto, atau carousel publik (MP4, MP3/M4A, JPG/PNG/WebP/ZIP).</p>}
  </form>;
}
