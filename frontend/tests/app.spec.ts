import { expect, test, type Page } from "@playwright/test";
import type { Dependency, Job, Metadata } from "../src/lib/types";

const publicUrl = "https://youtu.be/fixture";
const sample: Metadata = {
  url: publicUrl, platform: "youtube", title: "Video fixture", uploader: "Pustaka", duration_seconds: null,
  thumbnail_url: null, media_id: "fixture", media_type: "video", format_count: 2, available_qualities: ["best", "720"],
  available_audio_formats: ["mp3"], has_audio: true, supports_mp4: true, estimated_size_bytes: null, warnings: [],
};
const dependency: Dependency = { available: true, version: "1.0" };
const date = "2026-01-01T01:00:00Z";
const fixtureJob: Job = {
  id: "11111111-1111-4111-8111-111111111111", url: publicUrl, title: "Video fixture", platform: "youtube",
  kind: "video", quality: "720", container: "mp4", status: "queued", progress_percent: null,
  downloaded_bytes: null, total_bytes: null, speed_bytes_per_second: null, eta_seconds: null,
  file_size_bytes: null, output_filename: null, thumbnail_url: null, error: null, file_available: false,
  created_at: date, updated_at: date, completed_at: null, retry_of: null,
};

async function mockApi(page: Page) {
  const jobs: Job[] = [];
  const requests = { analyzed: 0, created: 0, retried: 0 };
  let reject = false;
  let offline = false;
  let delay: Promise<void> | undefined;
  await page.route("http://127.0.0.1:8000/api/**", async (route) => {
    const request = route.request();
    const { pathname, searchParams } = new URL(request.url());
    const method = request.method();
    if (offline) return route.abort("connectionrefused");
    const headers = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Headers": "Content-Type",
      "Access-Control-Allow-Methods": "GET,POST,PATCH,DELETE,OPTIONS" };
    const respond = (json: unknown, status = 200) => route.fulfill({ json, status, headers });
    if (method === "OPTIONS") return route.fulfill({ status: 204, headers });
    if (pathname.endsWith("/health")) return respond({
      status: "ok", queue_available: true, storage: { writable: true, download_dir: "/fixture" },
      dependencies: { python: dependency, yt_dlp: dependency, yt_dlp_ejs: dependency,
        ffmpeg: dependency, ffprobe: dependency, javascript_runtime: dependency },
    });
    if (pathname.endsWith("/settings")) return respond({
      preferences: { default_video_quality: "best", default_audio_format: "mp3", theme: "dark" },
      download_dir: "/fixture", max_active_downloads: 2, max_queue_length: 20, max_file_size_mb: 2048,
    });
    if (pathname.endsWith("/analyze")) {
      requests.analyzed++;
      await delay;
      return reject ? respond({ error: { code: "UNAVAILABLE_MEDIA", message: "Media tidak tersedia.", retryable: false } }, 422)
        : respond(sample);
    }
    if (pathname.endsWith("/cancel")) {
      jobs[0].status = "cancelled";
      return respond({}, 202);
    }
    if (pathname.endsWith("/retry")) {
      requests.retried++;
      jobs.push({ ...fixtureJob, id: "22222222-2222-4222-8222-222222222222", retry_of: jobs[0].id });
      return respond(jobs[1], 202);
    }
    if (pathname.endsWith("/downloads") && method === "POST") {
      requests.created++;
      expect(request.postDataJSON()).toMatchObject({ url: publicUrl, quality: "720", container: "mp4" });
      jobs.push({ ...fixtureJob });
      return respond(fixtureJob, 202);
    }
    if (pathname.endsWith("/downloads")) {
      const active = jobs.filter((job) => ["queued", "downloading", "processing"].includes(job.status));
      const matches = (searchParams.get("active") ? active : jobs).filter((job) =>
        !searchParams.has("search") || job.title?.toLowerCase().includes(searchParams.get("search")!.toLowerCase()));
      return respond({ items: matches, total: matches.length, active_count: active.length, limit: 25, offset: 0 });
    }
    return respond({}, 404);
  });
  return { jobs, requests, reject: () => { reject = true; }, setOffline: (value: boolean) => { offline = value; }, pause: () => {
    let release = () => {};
    delay = new Promise<void>((resolve) => { release = resolve; });
    return release;
  } };
}

test("input, loading, API error, null metadata, and truthful options", async ({ page }) => {
  const mock = await mockApi(page);
  await page.goto("/");
  const input = page.getByRole("textbox", { name: "Tautan media" });
  const analyze = page.getByRole("button", { name: "Analisis URL" });
  const urlError = page.locator("#url-error");
  await expect(analyze).toBeEnabled();
  await input.fill("https:youtu.be/fixture");
  await input.press("Enter");
  await expect(urlError).toContainText("HTTP atau HTTPS");
  await input.fill("https://example.com/video");
  await input.press("Enter");
  await expect(urlError).toContainText("platform yang didukung");
  expect(mock.requests.analyzed).toBe(0);
  const release = mock.pause();
  await input.fill(publicUrl);
  await analyze.click();
  await expect(page.getByRole("button", { name: "Menganalisis…" })).toBeDisabled();
  release();
  await expect(page.getByRole("heading", { name: "Video fixture" })).toBeVisible();
  await expect(page.getByText("Tidak diketahui")).toBeVisible();
  await expect(page.getByLabel("Kualitas").locator("option")).toHaveText(["Terbaik", "720p"]);
  await page.getByRole("button", { name: "Audio", exact: true }).click();
  await expect(page.getByLabel("Format audio").locator("option")).toHaveText(["MP3"]);
  mock.reject();
  await analyze.click();
  await expect(urlError).toContainText("UNAVAILABLE_MEDIA");
});

test("polling, progress, cancel, retry, and history after refresh", async ({ page }) => {
  const mock = await mockApi(page);
  await page.goto("/");
  await page.getByRole("textbox", { name: "Tautan media" }).fill(publicUrl);
  await page.getByRole("button", { name: "Analisis URL" }).click();
  await page.getByLabel("Kualitas").selectOption("720");
  await page.getByRole("button", { name: "Unduh sekarang" }).click();
  const queue = page.getByRole("region", { name: "Antrean unduhan" });
  await expect(queue.getByText("Dalam antrean")).toBeVisible();
  expect(mock.requests.created).toBe(1);
  mock.setOffline(true);
  await expect(page.getByText("Server offline")).toBeVisible();
  await expect(queue.getByText("Dalam antrean")).toBeVisible();
  mock.setOffline(false);
  await page.getByRole("button", { name: "Hubungkan ulang" }).click();
  mock.jobs[0].status = "downloading";
  mock.jobs[0].progress_percent = 46;
  await expect(queue.getByRole("progressbar")).toHaveAttribute("value", "46");
  mock.jobs[0].status = "processing";
  await expect(queue.getByText("Memproses")).toBeVisible();
  await expect(queue.getByRole("progressbar")).toHaveAttribute("value", "46");
  await queue.getByRole("button", { name: "Batalkan" }).click();
  await expect(queue.getByText("Antrean kosong")).toBeVisible();
  await page.getByRole("link", { name: "Lihat riwayat" }).click();
  await page.reload();
  const history = page.getByRole("table");
  await expect(history.getByText("Dibatalkan")).toBeVisible();
  await page.getByRole("searchbox", { name: "Cari judul" }).fill("tidak cocok");
  await expect(page.getByText("Tidak ada hasil")).toBeVisible();
  await page.getByRole("searchbox", { name: "Cari judul" }).fill("fixture");
  await page.getByRole("button", { name: "Unduh ulang Video fixture" }).click();
  await expect(page.getByText("Unduhan ulang masuk antrean.")).toBeVisible();
  await expect(history.getByText("Dalam antrean")).toHaveCount(1);
  expect(mock.requests.retried).toBe(1);
});

for (const width of [360, 768, 1280]) {
  test(`navigation fits and is keyboard accessible at ${width}px`, async ({ page }) => {
    await mockApi(page);
    await page.setViewportSize({ width, height: 850 });
    await page.goto("/");
    await page.keyboard.press("Tab");
    await expect(page.getByRole("link", { name: "Lewati navigasi" })).toBeFocused();
    await expect(page.getByRole("link", { name: "Pengaturan" })).toBeVisible();
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    await page.getByRole("link", { name: "Pengaturan" }).click();
    await expect(page.getByRole("heading", { name: "Pengaturan." })).toBeVisible();
    await expect(page.getByText("Folder unduhan", { exact: true })).toBeVisible();
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
  });
}
