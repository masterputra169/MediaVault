CREATE TABLE downloads (
    id TEXT PRIMARY KEY,
    source_url TEXT NOT NULL,
    platform TEXT NOT NULL,
    media_id TEXT,
    title TEXT,
    uploader TEXT,
    thumbnail_url TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('video','audio')),
    quality TEXT NOT NULL CHECK (quality IN ('best','1080','720','480','360')),
    container TEXT NOT NULL CHECK (container IN ('mp4','mp3','m4a')),
    audio_bitrate_kbps INTEGER,
    allow_quality_fallback INTEGER NOT NULL,
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
CREATE INDEX downloads_created ON downloads(created_at DESC);
CREATE INDEX downloads_status ON downloads(status);
CREATE INDEX downloads_platform ON downloads(platform);
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
