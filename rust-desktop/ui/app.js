(() => {
  const invoke = window.__TAURI__.core.invoke;

  const PLATFORMS = {
    youtube: "YouTube", tiktok: "TikTok", instagram: "Instagram", facebook: "Facebook",
    x: "X", pinterest: "Pinterest", threads: "Threads",
  };
  const STATUSES = {
    queued: { label: "Dalam antrean", glyph: "◷" },
    downloading: { label: "Mengunduh", glyph: "↓" },
    processing: { label: "Memproses", glyph: "⚙" },
    completed: { label: "Selesai", glyph: "✓" },
    failed: { label: "Gagal", glyph: "!" },
    cancelled: { label: "Dibatalkan", glyph: "×" },
    interrupted: { label: "Terhenti", glyph: "!" },
  };
  const DATE_FMT = new Intl.DateTimeFormat("id-ID", {
    day: "numeric", month: "long", year: "numeric", hour: "2-digit", minute: "2-digit",
  });
  const SLIDES_PER_PAGE = 4;
  const MAX_PREVIEW_CACHE = 80;
  const REDUCED_MOTION = window.matchMedia("(prefers-reduced-motion: reduce)");
  const HAS_POPOVER = Object.hasOwn(HTMLElement.prototype, "popover");

  let previewReqSeq = 0;
  let refreshListSeq = 0;

  const state = {
    health: null,
    prefs: null,
    tab: "all",
    search: "",
    platform: "",
    page: 0,
    pageSize: window.innerHeight < 760 ? 3 : window.innerHeight < 880 ? 4 : 5,
    customPageSize: false,
    jobs: [],
    total: 0,
    counts: { all: 0, active: 0, video: 0, audio: 0, image: 0 },
    analyzing: false,
    media: null,
    mode: "video",
    selectedSlide: 1,
    slidePage: 0,
    deleteTarget: null,
    viewerData: null,
    viewerSlide: 0,
    previewCache: new Map(),
    pendingToast: null,
    installingDeps: false,
  };

  const $ = (id) => document.getElementById(id);

  function errText(err) {
    if (err && typeof err === "object" && err.message) {
      return err.code ? `${err.message} (${err.code})` : err.message;
    }
    return String(err || "Terjadi kesalahan.");
  }

  function hasOpenModal() {
    return Boolean(document.querySelector("dialog[open]"));
  }

  function announce(text, priority = "normal") {
    // Modern Web Guidance (persistent-toast-notifications.md): prefer ariaNotify() with live-region fallback
    if (typeof document.ariaNotify === "function") {
      try {
        document.ariaNotify(text, { priority });
        return;
      } catch {
        // fallback below
      }
    }
    const region = $("sr-announce");
    if (region) {
      region.textContent = "";
      requestAnimationFrame(() => { region.textContent = text; });
    }
  }

  let toastTimer = null;
  function hideToast() {
    clearTimeout(toastTimer);
    const el = $("toast");
    if (!el) return;
    if (HAS_POPOVER) {
      try {
        if (el.matches(":popover-open")) el.hidePopover();
      } catch {
        el.classList.remove(":popover-open");
      }
    } else {
      el.classList.remove(":popover-open");
    }
  }

  function scheduleToastDismiss() {
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      const el = $("toast");
      if (!el) return;
      // Pause dismissal while hovered or focused (WCAG / persistent-toast-notifications.md)
      if (el.matches(":hover") || el.matches(":focus-within")) {
        scheduleToastDismiss();
        return;
      }
      hideToast();
    }, 4500);
  }

  function notify(text, isError = false) {
    // Per persistent-toast-notifications.md: suppress background toasts while a modal is open
    // unless it's an explicit error from an action inside the modal.
    if (hasOpenModal() && !isError) {
      state.pendingToast = { text, isError };
      return;
    }
    const el = $("toast");
    $("toast-message").textContent = text;
    $("toast-icon").textContent = isError ? "!" : "✓";
    el.classList.toggle("error", isError);
    if (HAS_POPOVER) {
      try {
        if (!el.matches(":popover-open")) el.showPopover();
      } catch {
        el.classList.add(":popover-open");
      }
    } else {
      el.classList.add(":popover-open");
    }
    announce(text, isError ? "high" : "normal");
    scheduleToastDismiss();
  }

  function flushPendingToast() {
    if (state.pendingToast && !hasOpenModal()) {
      const next = state.pendingToast;
      state.pendingToast = null;
      notify(next.text, next.isError);
    }
  }

  function setStatusIcon(kind) {
    const iconBox = $("status-icon");
    if (!iconBox) return;
    iconBox.textContent = "";
    const ns = "http://www.w3.org/2000/svg";
    const svg = document.createElementNS(ns, "svg");
    svg.setAttribute("width", "13");
    svg.setAttribute("height", "13");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("fill", "none");
    svg.setAttribute("stroke", "currentColor");
    svg.setAttribute("stroke-width", "2.4");
    svg.setAttribute("stroke-linecap", "round");
    svg.setAttribute("stroke-linejoin", "round");

    if (kind === "ok") {
      const circle = document.createElementNS(ns, "circle");
      circle.setAttribute("cx", "12");
      circle.setAttribute("cy", "12");
      circle.setAttribute("r", "9");
      const check = document.createElementNS(ns, "path");
      check.setAttribute("d", "m9 12 2 2 4-4");
      svg.append(circle, check);
    } else if (kind === "degraded") {
      const tri = document.createElementNS(ns, "path");
      tri.setAttribute("d", "m10.29 3.86-8.48 14.7A2 2 0 0 0 3.53 21.5h16.94a2 2 0 0 0 1.72-2.94l-8.48-14.7a2 2 0 0 0-3.42 0Z");
      const line = document.createElementNS(ns, "line");
      line.setAttribute("x1", "12"); line.setAttribute("x2", "12"); line.setAttribute("y1", "9"); line.setAttribute("y2", "13");
      const dot = document.createElementNS(ns, "line");
      dot.setAttribute("x1", "12"); dot.setAttribute("x2", "12.01"); dot.setAttribute("y1", "17"); dot.setAttribute("y2", "17");
      svg.append(tri, line, dot);
    } else {
      const circle = document.createElementNS(ns, "circle");
      circle.setAttribute("cx", "12");
      circle.setAttribute("cy", "12");
      circle.setAttribute("r", "9");
      const p1 = document.createElementNS(ns, "path");
      p1.setAttribute("d", "m15 9-6 6");
      const p2 = document.createElementNS(ns, "path");
      p2.setAttribute("d", "m9 9 6 6");
      svg.append(circle, p1, p2);
    }
    iconBox.appendChild(svg);
  }

  function showUrlError(message) {
    const input = $("url-input");
    const box = $("url-error");
    const txt = $("url-error-text");
    if (message) {
      txt.textContent = message;
      box.classList.remove("hidden");
      input.setAttribute("aria-invalid", "true");
    } else {
      txt.textContent = "";
      box.classList.add("hidden");
      input.setAttribute("aria-invalid", "false");
    }
  }

  function niceBytes(n) {
    if (n == null || !Number.isFinite(n)) return "";
    const units = ["B", "KB", "MB", "GB"];
    let v = n;
    let i = 0;
    while (v >= 1024 && i < units.length - 1) { v /= 1024; i += 1; }
    return `${v >= 10 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
  }

  function niceDuration(sec) {
    if (sec == null || !Number.isFinite(sec)) return "Tidak diketahui";
    const s = Math.floor(sec);
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const r = s % 60;
    return h > 0
      ? `${h}:${String(m).padStart(2, "0")}:${String(r).padStart(2, "0")}`
      : `${m}:${String(r).padStart(2, "0")}`;
  }

  function qualityLabel(q) {
    return q === "best" ? "Terbaik" : `${q}p`;
  }

  function kindLabel(job) {
    if (job.kind === "video") return `Video · ${job.container.toUpperCase()} ${qualityLabel(job.quality)}`;
    if (job.kind === "audio") return `Audio · ${job.container.toUpperCase()}`;
    if (job.container === "zip") return "Foto · ZIP Album";
    return `Foto · ${job.container.toUpperCase()}${job.image_index ? ` #${job.image_index}` : ""}`;
  }

  function applyTheme(theme) {
    document.documentElement.dataset.theme = theme || "dark";
  }

  function cachePreview(token, dataUrl) {
    if (state.previewCache.has(token)) {
      state.previewCache.delete(token);
    } else if (state.previewCache.size >= MAX_PREVIEW_CACHE) {
      const oldest = state.previewCache.keys().next().value;
      if (oldest !== undefined) state.previewCache.delete(oldest);
    }
    state.previewCache.set(token, dataUrl);
  }

  async function loadPreviewInto(box, token) {
    const reqId = String(++previewReqSeq);
    box.dataset.reqId = reqId;
    box.textContent = "";
    if (!token) {
      box.textContent = "—";
      return;
    }
    if (state.previewCache.has(token)) {
      const cached = state.previewCache.get(token);
      cachePreview(token, cached);
      const img = document.createElement("img");
      img.src = cached;
      img.alt = "";
      img.decoding = "async";
      box.appendChild(img);
      return;
    }
    box.textContent = "…";
    try {
      const dataUrl = await invoke("get_preview", { token });
      cachePreview(token, dataUrl);
      if (box.dataset.reqId !== reqId) return;
      box.textContent = "";
      const img = document.createElement("img");
      img.src = dataUrl;
      img.alt = "";
      img.decoding = "async";
      box.appendChild(img);
    } catch {
      if (box.dataset.reqId !== reqId) return;
      box.textContent = "—";
    }
  }

  function applyHealthReport(h) {
    state.health = h;
    const badge = $("status-badge");
    const kind = h.status === "ok" ? "ok" : "degraded";
    badge.className = `status-badge ${kind}`;
    setStatusIcon(kind);
    $("status-text").textContent = h.status === "ok" ? "Siap" : "Terbatas";
    $("footer-dl-path").textContent = `Folder: ${h.download_dir}`;
    renderDeps();
  }

  async function refreshHealth() {
    try {
      const h = await invoke("get_health");
      applyHealthReport(h);
    } catch (e) {
      $("status-badge").className = "status-badge error";
      setStatusIcon("error");
      $("status-text").textContent = "Error";
      notify(errText(e), true);
    }
  }

  async function runDepInstall(toolKey) {
    if (state.installingDeps) return;
    state.installingDeps = true;
    const dlg = $("settings-dialog");
    const box = $("dep-install-box");
    const msgEl = $("dep-install-msg");
    const pctEl = $("dep-install-pct");
    const bar = $("dep-install-bar");

    dlg.setAttribute("closedby", "none");
    $("btn-close-settings").disabled = true;
    $("btn-recheck-deps").disabled = true;
    $("btn-install-missing").disabled = true;
    renderDeps();

    box.classList.remove("hidden");
    msgEl.textContent = "Menghubungi rilis resmi GitHub…";
    pctEl.textContent = "…";
    bar.removeAttribute("value");

    const poll = setInterval(async () => {
      try {
        const st = await invoke("get_install_status");
        if (st.message) {
          const bytesInfo = st.downloaded_bytes > 0
            ? (st.total_bytes
              ? ` (${niceBytes(st.downloaded_bytes)} / ${niceBytes(st.total_bytes)})`
              : ` (${niceBytes(st.downloaded_bytes)})`)
            : "";
          msgEl.textContent = `${st.message}${bytesInfo}`;
        }
        if (st.percent != null) {
          bar.value = st.percent;
          pctEl.textContent = `${Math.round(st.percent)}%`;
        } else {
          bar.removeAttribute("value");
          pctEl.textContent = "…";
        }
      } catch {
        // ignore transient poll error
      }
    }, 280);

    try {
      const knownVer = toolKey === "yt_dlp"
        ? (state.health?.yt_dlp?.raw_version || state.health?.yt_dlp?.version || null)
        : toolKey === "gallery_dl"
          ? (state.health?.gallery_dl?.raw_version || state.health?.gallery_dl?.version || null)
          : null;
      const result = await invoke("install_dependency", {
        tool: toolKey,
        currentVersion: knownVer,
      });
      if (result.health) {
        applyHealthReport(result.health);
      }
      bar.value = 100;
      pctEl.textContent = "100%";
      const finalMsg = result.message || "Komponen berhasil dipasang dan siap dipakai.";
      msgEl.textContent = finalMsg;
      notify(finalMsg, false);
      setTimeout(() => {
        if (!state.installingDeps) box.classList.add("hidden");
      }, 2800);
    } catch (err) {
      box.classList.add("hidden");
      notify(errText(err), true);
    } finally {
      clearInterval(poll);
      state.installingDeps = false;
      dlg.setAttribute("closedby", "any");
      $("btn-close-settings").disabled = false;
      $("btn-recheck-deps").disabled = false;
      $("btn-install-missing").disabled = false;
      renderDeps();
    }
  }

  async function loadSettings() {
    try {
      const p = await invoke("get_settings");
      state.prefs = p;
      applyTheme(p.theme);
      $("pref-quality").value = p.default_video_quality;
      $("pref-audio").value = p.default_audio_format;
      $("pref-theme").value = p.theme;
      $("pref-download-dir").value = p.download_dir;
      $("footer-dl-path").textContent = `Folder: ${p.download_dir}`;
    } catch (e) {
      notify(errText(e), true);
    }
  }

  function renderDeps() {
    const dl = $("deps-list");
    dl.textContent = "";
    const h = state.health;
    if (!h) return;

    const missingAny = !h.yt_dlp.available
      || !h.ffmpeg.available
      || !h.ffprobe.available
      || !h.gallery_dl.available;
    const installAllBtn = $("btn-install-missing");
    if (installAllBtn) {
      installAllBtn.classList.toggle("hidden", !missingAny);
      installAllBtn.disabled = state.installingDeps;
    }

    const rows = [
      ["yt-dlp", h.yt_dlp, "yt_dlp", "Pasang", true],
      ["FFmpeg", h.ffmpeg, "ffmpeg", "Pasang FFmpeg", false],
      ["ffprobe", h.ffprobe, "ffmpeg", "Pasang FFmpeg", false],
      ["gallery-dl", h.gallery_dl, "gallery_dl", "Pasang", true],
      ["Node.js", h.node, null, null, false],
    ];
    for (const [label, dep, installKey, btnLabel, allowUpdate] of rows) {
      const wrap = document.createElement("div");
      const dt = document.createElement("dt");
      dt.textContent = label;
      const dd = document.createElement("dd");
      dd.className = "dep-dd-wrap";
      const statusSpan = document.createElement("span");
      statusSpan.className = dep.available ? "good-text" : "warn-text";
      statusSpan.textContent = dep.available ? `✓ ${dep.version || "Tersedia"}` : "⚠ Tidak ditemukan";
      dd.appendChild(statusSpan);
      if (installKey && (!dep.available || allowUpdate)) {
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "button small dep-install-btn";
        btn.textContent = state.installingDeps
          ? "Memasang…"
          : (dep.available ? "Perbarui" : btnLabel);
        btn.disabled = state.installingDeps;
        btn.onclick = () => runDepInstall(installKey);
        dd.appendChild(btn);
      }
      wrap.append(dt, dd);
      dl.appendChild(wrap);
    }
    const dbRow = document.createElement("div");
    const dtDb = document.createElement("dt");
    dtDb.textContent = "Database SQLite";
    const ddDb = document.createElement("dd");
    ddDb.textContent = h.db_path;
    dbRow.append(dtDb, ddDb);
    dl.appendChild(dbRow);
  }

  function withViewTransition(fn, useTransition = false) {
    if (
      useTransition &&
      typeof document.startViewTransition === "function" &&
      !REDUCED_MOTION.matches &&
      !hasOpenModal()
    ) {
      document.startViewTransition(fn);
    } else {
      fn();
    }
  }

  async function refreshList({ animate = false } = {}) {
    const seq = ++refreshListSeq;
    try {
      const params = {
        limit: state.pageSize,
        offset: state.page * state.pageSize,
        search: state.search.trim() || null,
        platform: state.platform || null,
        kind: state.tab !== "all" && state.tab !== "active" ? state.tab : null,
        active: state.tab === "active",
      };
      const res = await invoke("list_downloads", { params });
      if (seq !== refreshListSeq) return;
      state.jobs = res.items;
      state.total = res.total;
      state.counts = res.counts;

      const totalPages = Math.max(1, Math.ceil(state.total / state.pageSize));
      if (state.page >= totalPages) {
        state.page = totalPages - 1;
        return refreshList({ animate: false });
      }
      withViewTransition(() => renderList(), animate);
    } catch (e) {
      if (seq === refreshListSeq) notify(errText(e), true);
    }
  }

  function renderList() {
    $("count-all").textContent = state.counts.all;
    $("count-active").textContent = state.counts.active;
    $("count-video").textContent = state.counts.video;
    $("count-audio").textContent = state.counts.audio;
    $("count-image").textContent = state.counts.image;

    const badge = $("active-badge");
    if (state.counts.active > 0) {
      $("active-badge-text").textContent = `${state.counts.active} berjalan`;
      badge.classList.remove("hidden");
    } else {
      badge.classList.add("hidden");
    }

    const ul = $("dl-rows");
    const empty = $("list-empty");
    if (state.jobs.length === 0) {
      ul.classList.add("hidden");
      empty.classList.remove("hidden");
      const filtered = Boolean(state.search.trim() || state.platform || state.tab !== "all");
      $("empty-title").textContent = filtered ? "Tidak ada hasil yang cocok" : "Belum ada unduhan";
      $("empty-desc").textContent = filtered
        ? "Ubah kata kunci, filter tipe, atau platform untuk melihat item lain."
        : "Tempel tautan video, audio, atau foto di atas lalu klik Analisis URL.";
    } else {
      empty.classList.add("hidden");
      ul.classList.remove("hidden");
      ul.textContent = "";

      for (const job of state.jobs) {
        const li = document.createElement("li");
        li.className = "dl-row";

        const main = document.createElement("div");
        main.className = "dl-row-main";

        const canPreview = job.status === "completed" && job.file_available;
        const thumb = document.createElement(canPreview ? "button" : "div");
        if (canPreview) {
          thumb.type = "button";
          thumb.className = "thumbnail row-thumb row-thumb-btn";
          thumb.setAttribute("aria-label", `Pratinjau ${job.title || job.output_filename || "media"}`);
          thumb.title = "Pratinjau media";
          thumb.onclick = () => openDownloadedPreview(job);
        } else {
          thumb.className = "thumbnail row-thumb";
        }
        loadPreviewInto(thumb, job.thumbnail_preview);

        const info = document.createElement("div");
        info.className = "dl-row-info";

        const top = document.createElement("div");
        top.className = "dl-row-top";
        const name = job.title || job.output_filename || PLATFORMS[job.platform] || job.platform;
        const strong = document.createElement("strong");
        strong.textContent = name;
        strong.title = name;

        const stInfo = STATUSES[job.status] || { label: job.status, glyph: "•" };
        const stBadge = document.createElement("span");
        stBadge.className = `badge badge-${job.status}`;
        const stGlyph = document.createElement("span");
        stGlyph.setAttribute("aria-hidden", "true");
        stGlyph.textContent = stInfo.glyph;
        const stText = document.createElement("span");
        stText.textContent = stInfo.label;
        stBadge.append(stGlyph, stText);

        const typeBadge = document.createElement("span");
        typeBadge.className = "badge badge-type";
        typeBadge.textContent = kindLabel(job);
        top.append(strong, stBadge, typeBadge);
        info.appendChild(top);

        const isActive = job.status === "queued" || job.status === "downloading" || job.status === "processing";
        if (isActive) {
          const prog = document.createElement("div");
          prog.className = "dl-row-progress";
          const bar = document.createElement("progress");
          bar.max = 100;
          bar.setAttribute("aria-label", `Progres unduhan ${name}`);
          const starting = job.downloaded_bytes == null && (job.progress_percent == null || job.progress_percent <= 2);
          if (!starting && job.progress_percent != null) {
            bar.value = job.progress_percent;
          }
          const pct = document.createElement("span");
          pct.textContent = starting
            ? "Menyiapkan…"
            : (job.progress_percent != null ? `${Math.round(job.progress_percent)}%` : "…");
          prog.append(bar, pct);
          if (job.downloaded_bytes != null) {
            const bytesSpan = document.createElement("span");
            bytesSpan.className = "muted";
            bytesSpan.textContent = job.total_bytes
              ? `${niceBytes(job.downloaded_bytes)} / ${niceBytes(job.total_bytes)}`
              : niceBytes(job.downloaded_bytes);
            prog.appendChild(bytesSpan);
          }
          if (job.speed_bytes_per_second != null && job.speed_bytes_per_second > 0) {
            const spdSpan = document.createElement("span");
            spdSpan.className = "muted";
            spdSpan.textContent = `· ${niceBytes(job.speed_bytes_per_second)}/s`;
            prog.appendChild(spdSpan);
          }
          if (job.eta_seconds != null) {
            const etaSpan = document.createElement("span");
            etaSpan.className = "muted";
            etaSpan.textContent = `· ${niceDuration(job.eta_seconds)}`;
            prog.appendChild(etaSpan);
          }
          info.appendChild(prog);
        } else {
          const meta = document.createElement("div");
          meta.className = "dl-row-meta";
          const dt = document.createElement("span");
          dt.textContent = DATE_FMT.format(new Date(job.created_at));
          const plat = document.createElement("span");
          plat.textContent = `· ${PLATFORMS[job.platform] || job.platform}`;
          meta.append(dt, plat);
          if (job.file_size_bytes != null) {
            const sz = document.createElement("span");
            sz.textContent = `· ${niceBytes(job.file_size_bytes)}`;
            meta.appendChild(sz);
          }
          if (job.error_message) {
            const er = document.createElement("span");
            er.className = "row-error";
            er.textContent = `· ${job.error_message}`;
            meta.appendChild(er);
          } else if (job.status === "completed" && !job.file_available) {
            const er = document.createElement("span");
            er.className = "row-error";
            er.textContent = "· File tidak ada di disk";
            meta.appendChild(er);
          }
          info.appendChild(meta);
        }

        main.append(thumb, info);

        const actions = document.createElement("div");
        actions.className = "dl-row-actions";
        if (isActive) {
          const btnCancel = document.createElement("button");
          btnCancel.type = "button";
          btnCancel.className = "chip-btn chip-warn";
          btnCancel.textContent = "Batalkan";
          btnCancel.onclick = async () => {
            try {
              await invoke("cancel_download", { jobId: job.id });
              notify("Unduhan dibatalkan.");
              refreshList();
            } catch (e) { notify(errText(e), true); }
          };
          actions.appendChild(btnCancel);
        } else {
          if (job.file_available) {
            if (job.status === "completed") {
              const btnPreview = document.createElement("button");
              btnPreview.type = "button";
              btnPreview.className = "chip-btn chip-preview";
              btnPreview.textContent = "Pratinjau";
              btnPreview.onclick = () => openDownloadedPreview(job);
              actions.appendChild(btnPreview);
            }
            const btnOpen = document.createElement("button");
            btnOpen.type = "button";
            btnOpen.className = "chip-btn chip-green";
            btnOpen.textContent = "Buka folder";
            btnOpen.onclick = async () => {
              try { await invoke("open_in_explorer", { jobId: job.id }); }
              catch (e) { notify(errText(e), true); }
            };
            const btnMove = document.createElement("button");
            btnMove.type = "button";
            btnMove.className = "chip-btn";
            btnMove.textContent = "Pindahkan";
            btnMove.onclick = async () => {
              btnMove.disabled = true;
              try {
                const movedTo = await invoke("move_download", { jobId: job.id });
                if (movedTo) {
                  notify("File berhasil dipindahkan.");
                  refreshList();
                }
              } catch (e) {
                notify(errText(e), true);
              } finally {
                btnMove.disabled = false;
              }
            };
            actions.append(btnOpen, btnMove);
          }
          const btnRetry = document.createElement("button");
          btnRetry.type = "button";
          btnRetry.className = "chip-btn chip-blue";
          btnRetry.textContent = "Ulangi";
          btnRetry.onclick = async () => {
            try {
              await invoke("retry_download", { jobId: job.id });
              notify("Unduhan diulang.");
              state.page = 0;
              refreshList({ animate: true });
            } catch (e) { notify(errText(e), true); }
          };
          const btnDel = document.createElement("button");
          btnDel.type = "button";
          btnDel.className = "chip-btn chip-red";
          btnDel.textContent = "Hapus";
          btnDel.onclick = () => openDeleteModal(job);
          actions.append(btnRetry, btnDel);
        }

        li.append(main, actions);
        ul.appendChild(li);
      }
    }

    const totalPages = Math.max(1, Math.ceil(state.total / state.pageSize));
    const start = state.total === 0 ? 0 : state.page * state.pageSize + 1;
    const end = Math.min(state.total, (state.page + 1) * state.pageSize);
    const summary = $("footer-summary");
    summary.textContent = "";
    if (state.total > 0) {
      summary.append("Menampilkan ");
      const b1 = document.createElement("strong"); b1.textContent = `${start} - ${end}`;
      const b2 = document.createElement("strong"); b2.textContent = String(state.total);
      summary.append(b1, " dari ", b2, " unduhan");
    } else {
      summary.textContent = "Menampilkan 0 unduhan";
    }
    $("pg-status").textContent = `${state.page + 1} / ${totalPages}`;
    $("pg-first").disabled = state.page === 0;
    $("pg-prev").disabled = state.page === 0;
    $("pg-next").disabled = state.page + 1 >= totalPages;
    $("pg-last").disabled = state.page + 1 >= totalPages;
  }

  function populatePreviewModal(meta) {
    state.media = meta;

    $("preview-platform").textContent = PLATFORMS[meta.platform] || meta.platform;
    const titleEl = $("preview-title");
    titleEl.textContent = meta.title || "Judul tidak tersedia";
    titleEl.title = meta.title || "";
    $("preview-uploader").textContent = meta.uploader || "Pembuat tidak diketahui";
    $("preview-url").textContent = meta.url;

    const details = $("preview-details");
    details.textContent = "";
    const addLi = (t) => { const li = document.createElement("li"); li.textContent = t; details.appendChild(li); };
    if (meta.media_type !== "image") {
      addLi(meta.needs_enrichment && meta.duration_seconds == null ? "Memuat durasi…" : niceDuration(meta.duration_seconds));
    }
    const imgCount = meta.images.length;
    addLi(meta.media_type === "image" ? (imgCount > 1 ? `Album (${imgCount} foto)` : "Foto") : (meta.media_type === "audio" ? "Audio" : "Video"));
    addLi(meta.media_type === "image" ? `${imgCount} foto` : `${meta.format_count} format`);
    if (meta.estimated_size_bytes) addLi(`Estimasi ${niceBytes(meta.estimated_size_bytes)}`);

    // Populate selects (preserve user's current choice during TikTok background enrichment)
    const vSel = $("video-quality");
    const prevQ = vSel.value;
    vSel.textContent = "";
    for (const q of meta.available_qualities) {
      const opt = document.createElement("option");
      opt.value = q;
      opt.textContent = qualityLabel(q);
      vSel.appendChild(opt);
    }
    if (prevQ && meta.available_qualities.includes(prevQ)) {
      vSel.value = prevQ;
    } else if (state.prefs && meta.available_qualities.includes(state.prefs.default_video_quality)) {
      vSel.value = state.prefs.default_video_quality;
    }

    const aSel = $("audio-format");
    const prevA = aSel.value;
    aSel.textContent = "";
    for (const a of meta.available_audio_formats) {
      const opt = document.createElement("option");
      opt.value = a;
      opt.textContent = a.toUpperCase();
      aSel.appendChild(opt);
    }
    if (prevA && meta.available_audio_formats.includes(prevA)) {
      aSel.value = prevA;
    } else if (state.prefs && meta.available_audio_formats.includes(state.prefs.default_audio_format)) {
      aSel.value = state.prefs.default_audio_format;
    }

    const iSel = $("image-format");
    iSel.textContent = "";
    for (const f of meta.available_image_formats) {
      const opt = document.createElement("option");
      opt.value = f;
      opt.textContent = f === "zip" ? `Semua foto (${imgCount} slide · ZIP)` : f.toUpperCase();
      iSel.appendChild(opt);
    }
    $("image-count").value = imgCount > 1 ? `${imgCount} slide` : "1 foto";

    $("mode-video").disabled = meta.available_qualities.length === 0;
    $("mode-audio").disabled = meta.available_audio_formats.length === 0;
    $("mode-image").disabled = meta.available_image_formats.length === 0;

    const warnEl = $("preview-warning");
    if (meta.warnings && meta.warnings.length) {
      warnEl.textContent = `⚠ ${meta.warnings.join(" ")}`;
      warnEl.classList.remove("hidden");
    } else {
      warnEl.classList.add("hidden");
    }

    if (meta.needs_enrichment && !meta.preview_token && !meta.thumbnail_url) {
      $("preview-thumb-box").textContent = "Memuat pratinjau…";
    } else {
      syncPreviewImage();
    }
    renderModeSection();
  }

  function openPreviewModal(meta) {
    state.selectedSlide = 1;
    state.slidePage = 0;
    state.mode = meta.available_qualities.length
      ? "video"
      : meta.available_image_formats.length
        ? "image"
        : "audio";
    $("preview-save-dir").value = state.prefs?.download_dir || "";
    populatePreviewModal(meta);
    $("preview-dialog").showModal();

    if (meta.needs_enrichment) {
      const targetUrl = meta.url;
      invoke("enrich_media", { url: targetUrl })
        .then((full) => {
          const dlg = $("preview-dialog");
          if (dlg.open && state.media && state.media.url === targetUrl) {
            populatePreviewModal(full);
          }
        })
        .catch(() => {
          const dlg = $("preview-dialog");
          if (dlg.open && state.media && state.media.url === targetUrl) {
            $("preview-thumb-box").textContent = "—";
          }
        });
    }
  }

  function syncPreviewImage() {
    const meta = state.media;
    if (!meta) return;
    if (state.mode === "image" && meta.images.length > 0) {
      const item = meta.images[state.selectedSlide - 1] || meta.images[0];
      loadPreviewInto($("preview-thumb-box"), item.preview_token);
    } else {
      loadPreviewInto($("preview-thumb-box"), meta.preview_token);
    }
  }

  function renderModeSection() {
    const meta = state.media;
    if (!meta) return;
    for (const m of ["video", "audio", "image"]) {
      const btn = $(`mode-${m}`);
      const active = state.mode === m;
      btn.classList.toggle("active", active);
      btn.setAttribute("aria-pressed", String(active));
    }

    $("opts-video").classList.toggle("hidden", state.mode !== "video" || !meta.available_qualities.length);
    $("opts-audio").classList.toggle("hidden", state.mode !== "audio" || !meta.available_audio_formats.length);
    $("opts-image").classList.toggle("hidden", state.mode !== "image" || !meta.available_image_formats.length);

    if (state.mode === "audio") {
      $("audio-bitrate-wrap").classList.toggle("hidden", $("audio-format").value !== "mp3");
    }
    if (state.mode === "image") {
      renderSlidePicker();
    }

    const canDownload = (state.mode === "video" && meta.available_qualities.length > 0)
      || (state.mode === "audio" && meta.available_audio_formats.length > 0)
      || (state.mode === "image" && meta.available_image_formats.length > 0);
    $("btn-start-download").disabled = !canDownload;
  }

  function renderSlidePicker() {
    const meta = state.media;
    const isZip = $("image-format").value === "zip";
    const images = meta?.images || [];
    const picker = $("slide-picker");
    const zipNote = $("zip-note");
    if (isZip) {
      picker.classList.add("hidden");
      zipNote.textContent = `Semua ${images.length} foto akan dikemas rapi menjadi satu arsip ZIP.`;
      zipNote.classList.remove("hidden");
      return;
    }
    zipNote.classList.add("hidden");
    if (images.length <= 1) {
      picker.classList.add("hidden");
      return;
    }
    picker.classList.remove("hidden");
    $("slide-label").textContent = `Pilih slide foto (${state.selectedSlide}/${images.length})`;

    const totalPages = Math.max(1, Math.ceil(images.length / SLIDES_PER_PAGE));
    state.slidePage = Math.min(state.slidePage, totalPages - 1);
    const pager = $("slide-pager");
    pager.classList.toggle("hidden", totalPages <= 1);
    $("slide-status").textContent = `${state.slidePage + 1} / ${totalPages}`;
    $("slide-prev").disabled = state.slidePage === 0;
    $("slide-next").disabled = state.slidePage + 1 >= totalPages;

    const grid = $("slide-grid");
    grid.textContent = "";
    const slice = images.slice(state.slidePage * SLIDES_PER_PAGE, (state.slidePage + 1) * SLIDES_PER_PAGE);
    for (const item of slice) {
      const btn = document.createElement("button");
      btn.type = "button";
      const isSelected = state.selectedSlide === item.index;
      btn.className = `slide-tile ${isSelected ? "selected" : ""}`;
      btn.setAttribute("aria-pressed", String(isSelected));
      const thumb = document.createElement("div");
      thumb.className = "thumbnail slide-thumb";
      loadPreviewInto(thumb, item.preview_token);
      const label = document.createElement("span");
      label.textContent = item.width && item.height ? `#${item.index} · ${item.width}×${item.height}` : `#${item.index}`;
      btn.append(thumb, label);
      btn.onclick = () => {
        state.selectedSlide = item.index;
        renderSlidePicker();
        syncPreviewImage();
      };
      grid.appendChild(btn);
    }
  }

  function resetMediaViewer() {
    const vid = $("viewer-video");
    const aud = $("viewer-audio");
    const img = $("viewer-image");
    vid.pause();
    vid.removeAttribute("src");
    vid.load();
    vid.classList.add("hidden");

    aud.pause();
    aud.removeAttribute("src");
    aud.load();
    $("viewer-audio-wrap").classList.add("hidden");

    img.removeAttribute("src");
    img.classList.add("hidden");
    $("viewer-album-bar").classList.add("hidden");
    $("viewer-loading").classList.add("hidden");
    state.viewerData = null;
    state.viewerSlide = 0;
  }

  function renderViewerAlbumSlide() {
    const data = state.viewerData;
    if (!data || data.kind !== "album" || !data.slides.length) return;
    const total = data.slides.length;
    state.viewerSlide = Math.max(0, Math.min(state.viewerSlide, total - 1));
    const slide = data.slides[state.viewerSlide];
    const img = $("viewer-image");
    img.src = slide.url;
    img.alt = `${data.title} — Foto ${slide.index} dari ${total}`;
    img.classList.remove("hidden");
    $("viewer-slide-status").textContent = `${slide.index} / ${total}`;
    $("viewer-prev").disabled = state.viewerSlide === 0;
    $("viewer-next").disabled = state.viewerSlide + 1 >= total;
  }

  async function openDownloadedPreview(job) {
    resetMediaViewer();
    const dlg = $("media-viewer-dialog");
    $("viewer-dialog-title").textContent = job.title || job.output_filename || "Pratinjau Media";
    $("viewer-meta").textContent = "Memuat informasi file…";
    $("viewer-loading").classList.remove("hidden");
    $("viewer-open-folder").onclick = async () => {
      try { await invoke("open_in_explorer", { jobId: job.id }); }
      catch (e) { notify(errText(e), true); }
    };
    dlg.showModal();

    try {
      const data = await invoke("get_downloaded_media_preview", { jobId: job.id });
      if (!dlg.open) return;
      state.viewerData = data;
      state.viewerSlide = 0;
      $("viewer-loading").classList.add("hidden");
      $("viewer-dialog-title").textContent = data.title || data.filename;
      const metaParts = [data.filename, data.container.toUpperCase()];
      if (data.file_size_bytes != null) metaParts.push(niceBytes(data.file_size_bytes));
      if (data.kind === "album") metaParts.push(`${data.slides.length} foto`);
      $("viewer-meta").textContent = metaParts.join(" · ");

      if (data.kind === "video" && data.stream_url) {
        const vid = $("viewer-video");
        vid.src = data.stream_url;
        vid.classList.remove("hidden");
      } else if (data.kind === "audio" && data.stream_url) {
        const aud = $("viewer-audio");
        aud.src = data.stream_url;
        $("viewer-audio-wrap").classList.remove("hidden");
      } else if (data.kind === "image" && data.stream_url) {
        const img = $("viewer-image");
        img.src = data.stream_url;
        img.alt = data.title || data.filename;
        img.classList.remove("hidden");
      } else if (data.kind === "album" && data.slides.length > 0) {
        $("viewer-album-bar").classList.toggle("hidden", data.slides.length <= 1);
        renderViewerAlbumSlide();
      } else {
        dlg.close();
        notify("Format file ini tidak dapat dipratinjau.", true);
      }
    } catch (e) {
      if (dlg.open) dlg.close();
      notify(errText(e), true);
    }
  }

  $("btn-close-viewer").onclick = () => $("media-viewer-dialog").close();
  $("viewer-prev").onclick = () => {
    state.viewerSlide = Math.max(0, state.viewerSlide - 1);
    renderViewerAlbumSlide();
  };
  $("viewer-next").onclick = () => {
    if (!state.viewerData) return;
    state.viewerSlide = Math.min(state.viewerData.slides.length - 1, state.viewerSlide + 1);
    renderViewerAlbumSlide();
  };
  $("media-viewer-dialog").addEventListener("keydown", (e) => {
    if (!state.viewerData || state.viewerData.kind !== "album") return;
    if (e.key === "ArrowLeft" && state.viewerSlide > 0) {
      e.preventDefault();
      state.viewerSlide -= 1;
      renderViewerAlbumSlide();
    } else if (e.key === "ArrowRight" && state.viewerSlide + 1 < state.viewerData.slides.length) {
      e.preventDefault();
      state.viewerSlide += 1;
      renderViewerAlbumSlide();
    }
  });

  function openDeleteModal(job) {
    state.deleteTarget = job;
    $("delete-job-title").textContent = job.title || job.output_filename || PLATFORMS[job.platform] || job.platform;
    $("delete-file-check").checked = false;
    $("delete-file-wrap").classList.toggle("hidden", !job.file_available);
    $("delete-dialog").showModal();
  }

  // Accessible dialog close cleanup (reset returnValue + flush queued notifications)
  for (const id of ["preview-dialog", "settings-dialog", "delete-dialog", "media-viewer-dialog"]) {
    $(id).addEventListener("close", (e) => {
      e.currentTarget.returnValue = "";
      if (id === "delete-dialog") state.deleteTarget = null;
      if (id === "media-viewer-dialog") resetMediaViewer();
      flushPendingToast();
    });
  }

  // Synchronize aria-invalid with :user-invalid (accessible-error-announcement.md)
  const urlInput = $("url-input");
  urlInput.addEventListener("input", () => {
    $("btn-clear-url").classList.toggle("hidden", !urlInput.value);
    showUrlError("");
  });
  urlInput.addEventListener("blur", () => {
    if (urlInput.value.trim() && urlInput.matches(":user-invalid")) {
      showUrlError("Format tautan tidak valid. Pastikan diawali https:// atau http://.");
    }
  });

  $("btn-clear-url").onclick = () => {
    urlInput.value = "";
    $("btn-clear-url").classList.add("hidden");
    showUrlError("");
    urlInput.focus();
  };

  $("btn-paste").onclick = async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (text) {
        urlInput.value = text.trim();
        $("btn-clear-url").classList.remove("hidden");
        showUrlError("");
      }
    } catch {
      notify("Tempel langsung dengan Ctrl+V di kolom tautan.", true);
    }
  };

  $("analyze-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    if (state.analyzing) return;
    const raw = urlInput.value.trim();
    if (!raw) {
      showUrlError("Masukkan tautan media terlebih dahulu.");
      urlInput.focus();
      return;
    }
    state.analyzing = true;
    $("btn-analyze").disabled = true;
    $("btn-analyze-label").textContent = "Menganalisis…";
    showUrlError("");
    try {
      const meta = await invoke("analyze_media", { url: raw });
      openPreviewModal(meta);
    } catch (err) {
      showUrlError(errText(err));
    } finally {
      state.analyzing = false;
      $("btn-analyze").disabled = false;
      $("btn-analyze-label").textContent = "Analisis URL";
    }
  });

  $("mode-video").onclick = () => { state.mode = "video"; renderModeSection(); syncPreviewImage(); };
  $("mode-audio").onclick = () => { state.mode = "audio"; renderModeSection(); syncPreviewImage(); };
  $("mode-image").onclick = () => { state.mode = "image"; renderModeSection(); syncPreviewImage(); };
  $("audio-format").onchange = () => renderModeSection();
  $("image-format").onchange = () => renderModeSection();
  $("slide-prev").onclick = () => { state.slidePage = Math.max(0, state.slidePage - 1); renderSlidePicker(); };
  $("slide-next").onclick = () => { state.slidePage += 1; renderSlidePicker(); };
  $("btn-close-preview").onclick = () => $("preview-dialog").close();

  $("btn-start-download").onclick = async () => {
    const meta = state.media;
    if (!meta) return;
    const btn = $("btn-start-download");
    const dlg = $("preview-dialog");
    btn.disabled = true;
    btn.textContent = "Menambahkan…";
    dlg.setAttribute("closedby", "none");
    try {
      const chosenDir = $("preview-save-dir").value.trim() || null;
      let request;
      if (state.mode === "video") {
        const q = $("video-quality").value;
        request = {
          url: meta.url, kind: "video", quality: q, container: "mp4",
          allow_quality_fallback: q !== "best" && $("video-fallback").checked,
          audio_bitrate_kbps: null, image_index: null, output_dir: chosenDir,
        };
      } else if (state.mode === "audio") {
        const fmt = $("audio-format").value;
        request = {
          url: meta.url, kind: "audio", quality: "best", container: fmt,
          allow_quality_fallback: false,
          audio_bitrate_kbps: fmt === "mp3" ? Number($("audio-bitrate").value) : null,
          image_index: null, output_dir: chosenDir,
        };
      } else {
        const fmt = $("image-format").value;
        request = {
          url: meta.url, kind: "image", quality: "best", container: fmt,
          allow_quality_fallback: false, audio_bitrate_kbps: null,
          image_index: fmt === "zip" ? null : state.selectedSlide, output_dir: chosenDir,
        };
      }
      await invoke("create_download", { request });
      dlg.setAttribute("closedby", "any");
      dlg.close();
      urlInput.value = "";
      $("btn-clear-url").classList.add("hidden");
      state.page = 0;
      notify("Unduhan masuk antrean.");
      refreshList({ animate: true });
    } catch (err) {
      dlg.setAttribute("closedby", "any");
      notify(errText(err), true);
    } finally {
      btn.disabled = false;
      btn.textContent = "Unduh sekarang";
    }
  };

  // Accessible Filter Tabs (tabs.md: roving tabindex + ArrowLeft/ArrowRight/Home/End)
  const filterTabs = Array.from(document.querySelectorAll(".filter-pill"));
  function activateTab(btn, shouldFocus = false) {
    for (const b of filterTabs) {
      const selected = b === btn;
      b.classList.toggle("active", selected);
      b.setAttribute("aria-selected", String(selected));
      b.tabIndex = selected ? 0 : -1;
    }
    if (shouldFocus) btn.focus();
    state.tab = btn.dataset.tab;
    state.page = 0;
    refreshList({ animate: true });
  }

  filterTabs.forEach((btn, idx) => {
    btn.onclick = () => activateTab(btn, false);
    btn.addEventListener("keydown", (e) => {
      if (e.key === "ArrowRight") {
        e.preventDefault();
        activateTab(filterTabs[(idx + 1) % filterTabs.length], true);
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        activateTab(filterTabs[(idx - 1 + filterTabs.length) % filterTabs.length], true);
      } else if (e.key === "Home") {
        e.preventDefault();
        activateTab(filterTabs[0], true);
      } else if (e.key === "End") {
        e.preventDefault();
        activateTab(filterTabs[filterTabs.length - 1], true);
      }
    });
  });

  let searchTimer = null;
  $("search-input").addEventListener("input", () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      state.search = $("search-input").value;
      state.page = 0;
      refreshList({ animate: true });
    }, 220);
  });

  $("platform-select").onchange = () => {
    state.platform = $("platform-select").value;
    state.page = 0;
    refreshList({ animate: true });
  };

  $("page-size-select").value = String(state.pageSize);
  $("page-size-select").onchange = () => {
    state.customPageSize = true;
    state.pageSize = Number($("page-size-select").value);
    state.page = 0;
    refreshList({ animate: true });
  };

  window.addEventListener("resize", () => {
    if (state.customPageSize) return;
    const next = window.innerHeight < 760 ? 3 : window.innerHeight < 880 ? 4 : 5;
    if (next !== state.pageSize) {
      state.pageSize = next;
      $("page-size-select").value = String(next);
      state.page = 0;
      refreshList();
    }
  });

  $("pg-first").onclick = () => { state.page = 0; refreshList({ animate: true }); };
  $("pg-prev").onclick = () => { state.page = Math.max(0, state.page - 1); refreshList({ animate: true }); };
  $("pg-next").onclick = () => { state.page += 1; refreshList({ animate: true }); };
  $("pg-last").onclick = () => {
    state.page = Math.max(0, Math.ceil(state.total / state.pageSize) - 1);
    refreshList({ animate: true });
  };

  // Settings & folder buttons
  const openSettings = () => { refreshHealth(); $("settings-dialog").showModal(); };
  $("btn-settings").onclick = openSettings;
  $("btn-close-settings").onclick = () => { if (!state.installingDeps) $("settings-dialog").close(); };
  $("btn-recheck-deps").onclick = () => refreshHealth();
  $("btn-install-missing").onclick = () => runDepInstall("all_missing");
  $("btn-pick-folder").onclick = async () => {
    const btn = $("btn-pick-folder");
    btn.disabled = true;
    try {
      const picked = await invoke("pick_download_folder", {
        initialDir: $("pref-download-dir").value.trim() || null,
      });
      if (picked) {
        $("pref-download-dir").value = picked;
      }
    } catch (e) {
      notify(errText(e), true);
    } finally {
      btn.disabled = false;
    }
  };
  $("btn-pick-preview-dir").onclick = async () => {
    const btn = $("btn-pick-preview-dir");
    btn.disabled = true;
    try {
      const picked = await invoke("pick_download_folder", {
        initialDir: $("preview-save-dir").value.trim() || null,
      });
      if (picked) {
        $("preview-save-dir").value = picked;
      }
    } catch (e) {
      notify(errText(e), true);
    } finally {
      btn.disabled = false;
    }
  };
  $("btn-open-folder").onclick = async () => {
    try { await invoke("open_downloads_folder"); }
    catch (e) { notify(errText(e), true); }
  };

  // Instant theme preview when changing theme dropdown in settings
  $("pref-theme").addEventListener("change", () => {
    applyTheme($("pref-theme").value);
  });

  $("settings-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const btn = $("btn-save-settings");
    btn.disabled = true;
    try {
      const saved = await invoke("update_settings", {
        prefs: {
          default_video_quality: $("pref-quality").value,
          default_audio_format: $("pref-audio").value,
          theme: $("pref-theme").value,
          download_dir: $("pref-download-dir").value.trim(),
        },
      });
      state.prefs = saved;
      applyTheme(saved.theme);
      $("footer-dl-path").textContent = `Folder: ${saved.download_dir}`;
      $("settings-dialog").close();
      notify("Preferensi disimpan.");
    } catch (err) {
      notify(errText(err), true);
    } finally {
      btn.disabled = false;
    }
  });

  // Delete modal
  $("btn-close-delete").onclick = () => $("delete-dialog").close();
  $("btn-cancel-delete").onclick = () => $("delete-dialog").close();
  $("btn-confirm-delete").onclick = async () => {
    if (!state.deleteTarget) return;
    const btn = $("btn-confirm-delete");
    if (btn.disabled) return;
    btn.disabled = true;
    const targetId = state.deleteTarget.id;
    try {
      await invoke("delete_download", {
        jobId: targetId,
        deleteFile: $("delete-file-check").checked,
      });
      state.deleteTarget = null;
      $("delete-dialog").close();
      notify("Catatan unduhan dihapus.");
      refreshList({ animate: true });
    } catch (e) {
      notify(errText(e), true);
    } finally {
      btn.disabled = false;
    }
  };

  // Close toast button fallback when popover isn't supported
  $("btn-close-toast").addEventListener("click", () => {
    if (!HAS_POPOVER) hideToast();
  });

  // Boot + smart polling (fast when active jobs running, gentle otherwise)
  (async function init() {
    await Promise.all([loadSettings(), refreshHealth(), refreshList()]);
    setInterval(() => {
      if (state.counts.active > 0) refreshList();
    }, 450);
    setInterval(() => {
      if (state.counts.active === 0) refreshList();
    }, 6000);
  })();
})();
