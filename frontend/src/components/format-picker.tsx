"use client";

import { ChevronLeft, ChevronRight, Download, Film, Image as ImageIcon, Info, LoaderCircle, Music2 } from "lucide-react";
import { useState } from "react";
import { qualityLabel } from "@/lib/display";
import type { AudioFormat, Bitrate, CreateDownload, ImageFormat, Kind, Metadata, Preferences, Quality } from "@/lib/types";
import { Thumbnail } from "./media-preview";

const SLIDES_PER_PAGE = 4;

function imageFormatLabel(format: ImageFormat, count: number) {
  if (format === "zip") return `Semua foto (${count} slide · ZIP)`;
  return format.toUpperCase();
}

export function FormatPicker({ media, preferences, onDownload, onSlideSelect, busy, disabled }: {
  media: Metadata; preferences: Preferences | null; onDownload: (value: CreateDownload) => void;
  onSlideSelect?: (index: number) => void; busy: boolean; disabled: boolean;
}) {
  const qualities = media.available_qualities;
  const audioFormats = media.available_audio_formats;
  const imageFormats = media.available_image_formats ?? [];
  const images = media.images ?? [];
  const [kind, setKind] = useState<Kind>(
    qualities.length ? "video" : imageFormats.length ? "image" : "audio",
  );
  const [quality, setQuality] = useState<Quality>(
    preferences && qualities.includes(preferences.default_video_quality) ? preferences.default_video_quality : "best");
  const [audio, setAudio] = useState<AudioFormat>(
    preferences && audioFormats.includes(preferences.default_audio_format) ? preferences.default_audio_format : "mp3");
  const [bitrate, setBitrate] = useState<Bitrate>(192);
  const [fallback, setFallback] = useState(false);
  const [imageFormat, setImageFormat] = useState<ImageFormat>(imageFormats[0] ?? "jpg");
  const [imageIndex, setImageIndex] = useState<number>(1);
  const [slidePage, setSlidePage] = useState<number>(0);
  const slidePages = Math.max(1, Math.ceil(images.length / SLIDES_PER_PAGE));
  const safeSlidePage = Math.min(slidePage, slidePages - 1);
  const visibleSlides = images.slice(safeSlidePage * SLIDES_PER_PAGE, (safeSlidePage + 1) * SLIDES_PER_PAGE);
  const available = kind === "video"
    ? qualities.includes(quality)
    : kind === "audio"
      ? audioFormats.includes(audio)
      : imageFormats.includes(imageFormat);

  function selectSlide(index: number) {
    setImageIndex(index);
    onSlideSelect?.(index);
  }

  return <section className="panel options-panel" aria-labelledby="format-title">
    <h2 id="format-title" className="section-label"><Download size={18} /> Opsi unduhan</h2>
    <div className="segmented" aria-label="Jenis keluaran">
      <button className="button" aria-pressed={kind === "video"} disabled={!qualities.length || busy}
        onClick={() => setKind("video")}><Film size={16} /> Video</button>
      <button className="button" aria-pressed={kind === "audio"} disabled={!audioFormats.length || busy}
        onClick={() => setKind("audio")}><Music2 size={16} /> Audio</button>
      <button className="button" aria-pressed={kind === "image"} disabled={!imageFormats.length || busy}
        onClick={() => setKind("image")}><ImageIcon size={16} /> Foto</button>
    </div>
    {kind === "video" && qualities.length > 0 && <>
      <div className="form-row">
        <label>Kualitas<select value={quality} onChange={(e) => setQuality(e.target.value as Quality)} disabled={busy}>
          {qualities.map((item) => <option key={item} value={item}>{qualityLabel(item)}</option>)}
        </select></label>
        <label>Format<input value="MP4" readOnly /></label>
      </div>
      <label className="checkbox-row"><input type="checkbox" checked={fallback} onChange={(event) => setFallback(event.target.checked)}
        disabled={quality === "best" || busy} /> Izinkan resolusi lebih rendah jika pilihan hilang</label>
    </>}
    {kind === "audio" && audioFormats.length > 0 && <>
      <div className="form-row">
        <label>Format audio<select value={audio} disabled={busy} onChange={(e) => setAudio(e.target.value as AudioFormat)}>
          {audioFormats.map((item) => <option key={item} value={item}>{item.toUpperCase()}</option>)}
        </select></label>
        {audio === "mp3" && <label>Bitrate MP3<select value={bitrate} disabled={busy} onChange={(e) => setBitrate(Number(e.target.value) as Bitrate)}>
          {[128, 192, 256, 320].map((value) => <option key={value} value={value}>{value} kbps</option>)}
        </select></label>}
      </div>
      {audio === "mp3" && <p className="muted detail">Bitrate mengatur proses encoding. Kualitas audio bergantung pada sumber.</p>}
    </>}
    {kind === "image" && imageFormats.length > 0 && <>
      <div className="form-row">
        <label>Format foto<select value={imageFormat} disabled={busy} onChange={(e) => setImageFormat(e.target.value as ImageFormat)}>
          {imageFormats.map((item) => <option key={item} value={item}>{imageFormatLabel(item, images.length)}</option>)}
        </select></label>
        <label>Jumlah foto<input value={images.length > 1 ? `${images.length} slide` : "1 foto"} readOnly /></label>
      </div>
      {images.length > 1 && imageFormat !== "zip" && <div className="slide-picker">
        <div className="slide-picker-header">
          <p className="field-label">Pilih slide foto ({imageIndex}/{images.length})</p>
          {slidePages > 1 && (
            <div className="slide-pager" role="navigation" aria-label="Halaman slide foto">
              <button type="button" className="pager-btn slide-pager-btn" aria-label="Slide sebelumnya"
                disabled={safeSlidePage === 0} onClick={() => setSlidePage((p) => Math.max(0, p - 1))}>
                <ChevronLeft size={14} />
              </button>
              <span className="slide-pager-status">{safeSlidePage + 1} / {slidePages}</span>
              <button type="button" className="pager-btn slide-pager-btn" aria-label="Slide berikutnya"
                disabled={safeSlidePage + 1 >= slidePages} onClick={() => setSlidePage((p) => Math.min(slidePages - 1, p + 1))}>
                <ChevronRight size={14} />
              </button>
            </div>
          )}
        </div>
        <div className="slide-grid" role="radiogroup" aria-label="Pilih slide foto">
          {visibleSlides.map((item) => (
            <button
              key={item.index}
              type="button"
              role="radio"
              aria-checked={imageIndex === item.index}
              className={`slide-tile ${imageIndex === item.index ? "selected" : ""}`}
              disabled={busy}
              onClick={() => selectSlide(item.index)}
            >
              <Thumbnail src={item.preview_url ?? item.url} className="slide-thumb" />
              <span>#{item.index}{item.width && item.height ? ` · ${item.width}×${item.height}` : ""}</span>
            </button>
          ))}
        </div>
      </div>}
      {imageFormat === "zip" && <p className="muted detail">Semua {images.length} foto akan dikemas rapi menjadi satu arsip ZIP.</p>}
    </>}
    {!audioFormats.length && media.has_audio && <p className="muted detail">Ekstraksi audio memerlukan FFmpeg dan ffprobe.</p>}
    {media.warnings.map((warning) => <p className="inline-note" key={warning}><Info size={16} />{warning}</p>)}
    {!qualities.length && !audioFormats.length && !imageFormats.length && <p className="detail muted">Sumber ini belum menyediakan keluaran yang dapat diunduh.</p>}
    <button className="button primary download-button" disabled={!available || busy || disabled} onClick={() => onDownload(
      kind === "video"
        ? { url: media.url, kind, quality, container: "mp4", allow_quality_fallback: quality !== "best" && fallback }
        : kind === "audio"
          ? { url: media.url, kind, quality: "best", container: audio, ...(audio === "mp3" ? { audio_bitrate_kbps: bitrate } : {}) }
          : { url: media.url, kind, quality: "best", container: imageFormat, ...(imageFormat !== "zip" ? { image_index: imageIndex } : {}) },
    )}>{busy ? <LoaderCircle className="spin" size={17} /> : <Download size={17} />}{busy ? "Menambahkan…" : "Unduh sekarang"}</button>
  </section>;
}
