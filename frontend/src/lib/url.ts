const domains = [
  "youtube.com", "tiktok.com", "instagram.com", "facebook.com", "x.com",
  "twitter.com", "reddit.com", "pinterest.com", "vimeo.com", "threads.net", "threads.com",
];
const aliases = ["youtu.be", "fb.watch", "fb.com", "t.co", "redd.it", "i.redd.it", "pin.it"];

export function validateUrl(raw: string): string | null {
  if (raw.length > 2048) return "Tautan harus berisi maksimal 2.048 karakter.";
  if (!/^https?:\/\/[^/]/i.test(raw) || /[\s\\]/.test(raw)) return "Masukkan tautan HTTP atau HTTPS yang valid.";
  try {
    const url = new URL(raw);
    if (!["http:", "https:"].includes(url.protocol) || url.username || url.password || url.port) {
      return "Masukkan tautan HTTP atau HTTPS yang valid.";
    }
    if (!aliases.includes(url.hostname) && !domains.some((domain) =>
      url.hostname === domain || url.hostname.endsWith("." + domain))) {
      return "Gunakan tautan dari platform yang didukung.";
    }
    return null;
  } catch {
    return "Masukkan tautan HTTP atau HTTPS yang valid.";
  }
}
