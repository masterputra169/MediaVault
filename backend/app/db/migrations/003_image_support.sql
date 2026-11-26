CREATE TABLE downloads_new (
    id TEXT PRIMARY KEY,
    source_url TEXT NOT NULL,
    platform TEXT NOT NULL,
    media_id TEXT,
    title TEXT,
    uploader TEXT,
    thumbnail_url TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('video','audio','image')),
    quality TEXT NOT NULL CHECK (quality IN ('best','1080','720','480','360')),
    container TEXT NOT NULL CHECK (container IN ('mp4','mp3','m4a','jpg','png','webp','zip')),
    audio_bitrate_kbps INTEGER,
    allow_quality_fallback INTEGER NOT NULL,
    image_index INTEGER,
    status TEXT NOT NULL CHECK (status IN ('queued','downloading','processing','completed','failed','cancelled','interrupted')),
    progress_percent REAL,
    downloaded_bytes INTEGER,
    total_bytes INTEGER,
    speed_bytes_per_second REAL,
    eta_seconds INTEGER,
    output_path TEXT,
    output_filename TEXT,
    file_size_bytes INTEGER,
    error_code TEXT,
    error_message TEXT,
    retry_of TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT
);

INSERT INTO downloads_new (
    id, source_url, platform, media_id, title, uploader, thumbnail_url,
    kind, quality, container, audio_bitrate_kbps, allow_quality_fallback, image_index,
    status, progress_percent, downloaded_bytes, total_bytes, speed_bytes_per_second, eta_seconds,
    output_path, output_filename, file_size_bytes, error_code, error_message, retry_of,
    created_at, updated_at, completed_at
)
SELECT
    id, source_url, platform, media_id, title, uploader, thumbnail_url,
    kind, quality, container, audio_bitrate_kbps, allow_quality_fallback, NULL,
    status, progress_percent, downloaded_bytes, total_bytes, speed_bytes_per_second, eta_seconds,
    output_path, output_filename, file_size_bytes, error_code, error_message, retry_of,
    created_at, updated_at, completed_at
FROM downloads;

DROP TABLE downloads;
ALTER TABLE downloads_new RENAME TO downloads;

CREATE INDEX downloads_created ON downloads(created_at DESC);
CREATE INDEX downloads_status ON downloads(status);
CREATE INDEX downloads_platform ON downloads(platform);
