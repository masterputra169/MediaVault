"use client";

import { Clock3, Film, Image as ImageIcon, ImageOff, Link as LinkIcon } from "lucide-react";
import { useState } from "react";
import { assetUrl } from "@/lib/api";
import { duration, niceBytes } from "@/lib/display";
import { platforms, type Metadata } from "@/lib/types";

export function Thumbnail({ src, className = "" }: { src: string | null; className?: string }) {
  const [broken, setBroken] = useState<string | null>(null);
  const resolved = assetUrl(src);
  const isApi = Boolean(src && src.startsWith("/api/"));
  return <div className={`thumbnail ${className}`}>
    {resolved && broken !== resolved ?
      // eslint-disable-next-line @next/next/no-img-element -- Renders authenticated preview or public thumbnail.
      <img
        src={resolved}
        alt=""
        loading={isApi ? "eager" : "lazy"}
        referrerPolicy={isApi ? undefined : "no-referrer"}
        onError={() => setBroken(resolved)}
      />
      : <ImageOff size={24} aria-hidden />}
  </div>;
}

export function MediaPreview({ media, activeSrc }: { media: Metadata; activeSrc?: string | null }) {
  const imageCount = media.images?.length ?? 0;
  const displaySrc = activeSrc ?? media.preview_url ?? media.thumbnail_url;
  const typeLabel = media.media_type === "video"
    ? "Video"
    : media.media_type === "audio"
      ? "Audio"
      : media.media_type === "image"
        ? (imageCount > 1 ? `Album (${imageCount} foto)` : "Foto")
        : "Tipe tidak diketahui";
  return <section className="panel media-panel" aria-labelledby="preview-title">
    <h2 id="preview-title" className="section-label">
      {media.media_type === "image" ? <ImageIcon size={18} /> : <Film size={18} />} Pratinjau media
    </h2>
    <Thumbnail src={displaySrc} className="preview-image" />
    <div className="preview-copy">
      <span className="pill">{platforms[media.platform]}</span>
      <h3 title={media.title ?? undefined}>{media.title ?? "Judul tidak tersedia"}</h3>
      <p className="muted">{media.uploader ?? "Pembuat tidak diketahui"}</p>
      <ul className="media-details">
        {media.media_type !== "image" && <li><Clock3 size={15} /> <span className="sr-only">Durasi </span>{duration(media.duration_seconds)}</li>}
        <li>{typeLabel}</li>
        <li>{media.media_type === "image" ? `${imageCount} foto` : `${media.format_count} format`}</li>
        {media.estimated_size_bytes !== null && <li>Estimasi {niceBytes(media.estimated_size_bytes)}</li>}
      </ul>
      <a className="source-link" href={media.url} target="_blank" rel="noreferrer"><LinkIcon size={15} /><span>{media.url}</span></a>
    </div>
  </section>;
}
