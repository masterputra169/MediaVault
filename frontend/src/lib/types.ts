export const platforms = {
  youtube: "YouTube", tiktok: "TikTok", instagram: "Instagram", facebook: "Facebook",
  x: "X", reddit: "Reddit", pinterest: "Pinterest", vimeo: "Vimeo", threads: "Threads",
} as const;
export type Platform = keyof typeof platforms;
export const statuses = {
  queued: "Dalam antrean", downloading: "Mengunduh", processing: "Memproses",
  completed: "Selesai", failed: "Gagal", cancelled: "Dibatalkan", interrupted: "Terhenti",
} as const;
export type Status = keyof typeof statuses;
export type Quality = "best" | "1080" | "720" | "480" | "360";
export type AudioFormat = "mp3" | "m4a";
export type ImageFormat = "jpg" | "png" | "webp" | "zip";
export type Kind = "video" | "audio" | "image";
export type ImageItem = { index: number; url: string; preview_url?: string | null; width: number | null; height: number | null };
export type Bitrate = 128 | 192 | 256 | 320;
export type Theme = "dark" | "light" | "system";
export type AppError = { code: string; message: string; retryable: boolean; details: null };
export type Dependency = { available: boolean; version: string | null; name?: string | null };
export type Health = {
  status: "ok" | "degraded";
  app_version: string;
  dependencies: Record<"python" | "yt_dlp" | "yt_dlp_ejs" | "ffmpeg" | "ffprobe" | "javascript_runtime", Dependency>;
  storage: { writable: boolean; download_dir: string; free_bytes: number | null; temp_free_bytes: number | null };
  queue_available: boolean;
};
export type Preferences = { default_video_quality: Quality; default_audio_format: AudioFormat; theme: Theme };
export type Settings = {
  preferences: Preferences;
  download_dir: string;
  max_active_downloads: number;
  max_queue_length: number;
  max_file_size_mb: number;
};
export type Metadata = {
  url: string;
  platform: Platform;
  title: string | null;
  uploader: string | null;
  duration_seconds: number | null;
  thumbnail_url: string | null;
  preview_url?: string | null;
  media_id: string | null;
  media_type: Kind | null;
  format_count: number;
  available_qualities: Quality[];
  available_audio_formats: AudioFormat[];
  available_image_formats?: ImageFormat[];
  images?: ImageItem[];
  has_audio: boolean;
  supports_mp4: boolean;
  estimated_size_bytes: number | null;
  warnings: string[];
};
export type CreateDownload = {
  url: string; kind: Kind; quality: Quality; container: "mp4" | AudioFormat | ImageFormat;
  allow_quality_fallback?: boolean; audio_bitrate_kbps?: Bitrate; image_index?: number;
};
export type Job = {
  id: string; url: string; title: string | null; platform: Platform; kind: Kind; quality: Quality;
  container: "mp4" | AudioFormat | ImageFormat; image_index?: number | null; status: Status; progress_percent: number | null;
  downloaded_bytes: number | null; total_bytes: number | null;
  speed_bytes_per_second: number | null; eta_seconds: number | null; file_size_bytes: number | null;
  output_filename: string | null; thumbnail_url: string | null; thumbnail_preview_url?: string | null; error: AppError | null;
  file_available: boolean; created_at: string; updated_at: string; completed_at: string | null; retry_of: string | null;
};
export type JobCounts = { all: number; active: number; video: number; audio: number; image: number };
export type JobPage = { items: Job[]; limit: number; offset: number; total: number; active_count: number; counts?: JobCounts };
export type JobTicket = Pick<Job, "id" | "status" | "created_at">;
export type AuthSession = { auth_required: boolean; authenticated: boolean; username: string | null };
