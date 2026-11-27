//! Ultra-Lightweight Native In-App MP4 Video + Audio Player (Zero External Dependencies).
//!
//! Architecture designed for minimal CPU/GPU usage and ZERO audio stuttering:
//! - Audio runs on its OWN dedicated Media Foundation audio-only thread -> `rodio::Sink`
//!   (keeping a smooth ~250ms PCM cushion completely decoupled from video decoding).
//! - Video runs on its OWN dedicated Media Foundation video-only thread with precomputed
//!   integer scaling LUTs (854px cap), 30fps cap, smart late-frame skip, and zero-clone
//!   `egui::ColorImage` handoff directly to the GPU.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use egui::{Color32, ColorImage};

pub struct VideoFrame {
    pub image: ColorImage,
    pub seq: u64,
}

#[derive(Clone)]
pub struct VideoPlayerSnapshot {
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    pub position_secs: f64,
    pub playing: bool,
    pub ended: bool,
    pub muted: bool,
    pub volume: f32,
    pub has_audio: bool,
    pub error: Option<String>,
    pub frame_seq: u64,
}

impl Default for VideoPlayerSnapshot {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            duration_secs: 0.0,
            position_secs: 0.0,
            playing: false,
            ended: false,
            muted: false,
            volume: 0.85,
            has_audio: false,
            error: None,
            frame_seq: 0,
        }
    }
}

enum PlayerCmd {
    TogglePlay,
    Seek(f64),
    SetVolume(f32),
    SetMuted(bool),
    Stop,
}

struct SharedState {
    snap: VideoPlayerSnapshot,
    pending_frame: Option<VideoFrame>,
}

pub struct VideoPlayer {
    cmd_tx: Option<mpsc::Sender<PlayerCmd>>,
    shared: Arc<Mutex<SharedState>>,
    worker: Option<JoinHandle<()>>,
    loaded_path: Option<PathBuf>,
}

impl VideoPlayer {
    pub fn new() -> Self {
        Self {
            cmd_tx: None,
            shared: Arc::new(Mutex::new(SharedState {
                snap: VideoPlayerSnapshot::default(),
                pending_frame: None,
            })),
            worker: None,
            loaded_path: None,
        }
    }

    pub fn open(&mut self, path: &Path, repaint_cb: impl Fn() + Send + 'static) {
        self.stop();

        let shared = Arc::new(Mutex::new(SharedState {
            snap: VideoPlayerSnapshot {
                playing: true,
                volume: 0.85,
                ..Default::default()
            },
            pending_frame: None,
        }));
        let (tx, rx) = mpsc::channel();
        let path_owned = path.to_path_buf();
        let shared_worker = Arc::clone(&shared);

        let handle = std::thread::Builder::new()
            .name("mediavault-video-decoder".into())
            .spawn(move || {
                #[cfg(windows)]
                win_mf::run_decoder_loop(&path_owned, rx, shared_worker, Box::new(repaint_cb));

                #[cfg(not(windows))]
                {
                    let _ = (&path_owned, rx, repaint_cb);
                    if let Ok(mut st) = shared_worker.lock() {
                        st.snap.playing = false;
                        st.snap.error = Some(
                            "In-app hardware video playback is available on Windows; use External."
                                .into(),
                        );
                    }
                }
            })
            .ok();

        self.cmd_tx = Some(tx);
        self.shared = shared;
        self.worker = handle;
        self.loaded_path = Some(path.to_path_buf());
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(PlayerCmd::Stop);
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
        self.loaded_path = None;
        if let Ok(mut st) = self.shared.lock() {
            st.snap = VideoPlayerSnapshot::default();
            st.pending_frame = None;
        }
    }

    pub fn toggle_play(&self) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(PlayerCmd::TogglePlay);
        }
    }

    pub fn seek(&self, secs: f64) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(PlayerCmd::Seek(secs.max(0.0)));
        }
    }

    pub fn set_volume(&self, vol: f32) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(PlayerCmd::SetVolume(vol.clamp(0.0, 1.0)));
        }
    }

    pub fn set_muted(&self, muted: bool) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(PlayerCmd::SetMuted(muted));
        }
    }

    pub fn snapshot(&self) -> VideoPlayerSnapshot {
        self.shared
            .lock()
            .map(|st| st.snap.clone())
            .unwrap_or_default()
    }

    /// Moves the newest decoded `ColorImage` out with ZERO cloning on the UI thread.
    pub fn take_new_frame(&self, last_seq: u64) -> Option<VideoFrame> {
        let mut st = self.shared.lock().ok()?;
        if st.pending_frame.as_ref().is_some_and(|f| f.seq > last_seq) {
            st.pending_frame.take()
        } else {
            None
        }
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

// ============================================================================
// Decoupled Dual-Thread Windows Media Foundation Video + Audio Engine
// ============================================================================

#[cfg(windows)]
mod win_mf {
    use super::*;
    use rodio::{buffer::SamplesBuffer, OutputStream, OutputStreamHandle, Sink};
    use windows::core::{GUID, HSTRING, PROPVARIANT};
    use windows::Win32::Media::MediaFoundation::{
        IMFAttributes, IMFMediaBuffer, IMFMediaType, IMFSourceReader, MFCreateAttributes,
        MFCreateMediaType, MFCreateSourceReaderFromURL, MFShutdown, MFStartup, MFAudioFormat_PCM,
        MFMediaType_Audio, MFMediaType_Video, MFVideoFormat_RGB32, MF_MT_AUDIO_BITS_PER_SAMPLE,
        MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_DEFAULT_STRIDE,
        MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_MT_VIDEO_ROTATION, MF_PD_DURATION,
        MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED, MF_SOURCE_READERF_ENDOFSTREAM,
        MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
        MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
        MF_SOURCE_READER_MEDIASOURCE, MF_VERSION, MFSTARTUP_NOSOCKET,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    /// 854px max dimension keeps GPU upload tiny (~1.5 MB) while looking sharp with bilinear filtering.
    const MAX_PREVIEW_DIM: u32 = 854;
    /// Cap preview presentation at 30 fps so 60fps videos use half the CPU & GPU.
    const MIN_FRAME_INTERVAL_SECS: f64 = 1.0 / 30.0;

    struct ComMfGuard;
    impl Drop for ComMfGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = MFShutdown();
                CoUninitialize();
            }
        }
    }

    #[derive(Clone, Copy)]
    struct VideoFormatInfo {
        raw_w: u32,
        raw_h: u32,
        stride: i32,
        rotation_deg: u32,
    }

    impl VideoFormatInfo {
        fn display_dims(&self) -> (u32, u32) {
            if self.rotation_deg == 90 || self.rotation_deg == 270 {
                (self.raw_h, self.raw_w)
            } else {
                (self.raw_w, self.raw_h)
            }
        }
    }

    /// Precomputed integer lookup tables so BGRA -> Color32 conversion does ZERO float math per frame.
    struct FastScaler {
        fmt: VideoFormatInfo,
        out_w: usize,
        out_h: usize,
        row_stride: usize,
        bottom_up: bool,
        direct_1to1: bool,
        ox_lut: Vec<usize>,
        oy_lut: Vec<usize>,
    }

    impl FastScaler {
        fn new(fmt: VideoFormatInfo) -> Self {
            let (disp_w, disp_h) = fmt.display_dims();
            let max_side = disp_w.max(disp_h);
            let (out_w, out_h) = if max_side > MAX_PREVIEW_DIM {
                let scale = MAX_PREVIEW_DIM as f64 / max_side as f64;
                (
                    ((disp_w as f64 * scale).round() as usize).max(2),
                    ((disp_h as f64 * scale).round() as usize).max(2),
                )
            } else {
                (disp_w as usize, disp_h as usize)
            };

            let rw = fmt.raw_w as usize;
            let row_stride = (fmt.stride.unsigned_abs() as usize).max(rw * 4);
            let bottom_up = fmt.stride < 0;
            let direct_1to1 = fmt.rotation_deg == 0
                && out_w == fmt.raw_w as usize
                && out_h == fmt.raw_h as usize;

            let (ox_lut, oy_lut) = if direct_1to1 {
                (Vec::new(), Vec::new())
            } else {
                let ox: Vec<usize> = (0..out_w)
                    .map(|dx| ((dx * disp_w as usize) / out_w).min(disp_w as usize - 1))
                    .collect();
                let oy: Vec<usize> = (0..out_h)
                    .map(|dy| ((dy * disp_h as usize) / out_h).min(disp_h as usize - 1))
                    .collect();
                (ox, oy)
            };

            Self {
                fmt,
                out_w,
                out_h,
                row_stride,
                bottom_up,
                direct_1to1,
                ox_lut,
                oy_lut,
            }
        }

        fn convert(&self, src: &[u8]) -> Option<ColorImage> {
            let rw = self.fmt.raw_w as usize;
            let rh = self.fmt.raw_h as usize;
            if src.len() < self.row_stride * rh {
                return None;
            }

            let mut pixels = vec![Color32::BLACK; self.out_w * self.out_h];

            if self.direct_1to1 {
                for y in 0..self.out_h {
                    let sy = if self.bottom_up { rh - 1 - y } else { y };
                    let srow = &src[sy * self.row_stride..sy * self.row_stride + rw * 4];
                    let drow = &mut pixels[y * self.out_w..(y + 1) * self.out_w];
                    for (dst, chunk) in drow.iter_mut().zip(srow.chunks_exact(4)) {
                        *dst = Color32::from_rgb(chunk[2], chunk[1], chunk[0]);
                    }
                }
            } else if self.fmt.rotation_deg == 0 {
                for (dy, &oy) in self.oy_lut.iter().enumerate() {
                    let sy = if self.bottom_up { rh - 1 - oy } else { oy };
                    let srow = &src[sy * self.row_stride..sy * self.row_stride + rw * 4];
                    let drow = &mut pixels[dy * self.out_w..(dy + 1) * self.out_w];
                    for (dst, &ox) in drow.iter_mut().zip(self.ox_lut.iter()) {
                        let si = ox * 4;
                        *dst = Color32::from_rgb(srow[si + 2], srow[si + 1], srow[si]);
                    }
                }
            } else {
                for (dy, &oy) in self.oy_lut.iter().enumerate() {
                    let drow = &mut pixels[dy * self.out_w..(dy + 1) * self.out_w];
                    for (dst, &ox) in drow.iter_mut().zip(self.ox_lut.iter()) {
                        let (rx, ry) = match self.fmt.rotation_deg {
                            90 => (oy, rh - 1 - ox),
                            180 => (rw - 1 - ox, rh - 1 - oy),
                            270 => (rw - 1 - oy, ox),
                            _ => (ox, oy),
                        };
                        let sy = if self.bottom_up { rh - 1 - ry } else { ry };
                        let si = sy * self.row_stride + rx * 4;
                        *dst = Color32::from_rgb(src[si + 2], src[si + 1], src[si]);
                    }
                }
            }

            Some(ColorImage {
                size: [self.out_w, self.out_h],
                pixels,
            })
        }
    }

    // ========================================================================
    // Dedicated Audio-Only Thread (Zero Stutter, Completely Decoupled!)
    // ========================================================================

    enum AudioCmd {
        SetPlaying(bool),
        Seek(f64, bool),
        SetVolume(f32),
        SetMuted(bool),
        Stop,
    }

    struct DedicatedAudio {
        tx: mpsc::Sender<AudioCmd>,
        handle: Option<JoinHandle<()>>,
        ready: Arc<AtomicBool>,
        active: Arc<AtomicBool>,
        clock_bits: Arc<AtomicU64>,
    }

    impl DedicatedAudio {
        fn spawn(path: &Path, volume: f32, muted: bool) -> Self {
            let (tx, rx) = mpsc::channel();
            let ready = Arc::new(AtomicBool::new(false));
            let active = Arc::new(AtomicBool::new(false));
            let clock_bits = Arc::new(AtomicU64::new(0f64.to_bits()));
            let init_done = Arc::new(AtomicBool::new(false));

            let p = path.to_path_buf();
            let r_c = Arc::clone(&ready);
            let a_c = Arc::clone(&active);
            let clk_c = Arc::clone(&clock_bits);
            let done_c = Arc::clone(&init_done);

            let handle = std::thread::Builder::new()
                .name("mediavault-audio-decoder".into())
                .spawn(move || {
                    run_audio_only_loop(&p, rx, volume, muted, r_c, a_c, clk_c, done_c);
                })
                .ok();

            // Wait cleanly for MF + WASAPI audio initialization (exits immediately once ready)
            let t0 = Instant::now();
            while !init_done.load(Ordering::Acquire) && t0.elapsed() < Duration::from_millis(1500) {
                std::thread::sleep(Duration::from_millis(3));
            }

            Self {
                tx,
                handle,
                ready,
                active,
                clock_bits,
            }
        }

        fn has_audio(&self) -> bool {
            self.ready.load(Ordering::Acquire)
        }

        fn is_active(&self) -> bool {
            self.active.load(Ordering::Acquire)
        }

        fn clock_secs(&self) -> f64 {
            f64::from_bits(self.clock_bits.load(Ordering::Acquire))
        }

        fn send(&self, cmd: AudioCmd) {
            let _ = self.tx.send(cmd);
        }

        fn stop(mut self) {
            let _ = self.tx.send(AudioCmd::Stop);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    fn build_sink(
        handle: Option<&OutputStreamHandle>,
        volume: f32,
        muted: bool,
        paused: bool,
    ) -> Option<Sink> {
        let h = handle?;
        let sink = Sink::try_new(h).ok()?;
        sink.set_volume(if muted { 0.0 } else { volume });
        if paused {
            sink.pause();
        }
        Some(sink)
    }

    fn run_audio_only_loop(
        path: &Path,
        rx: mpsc::Receiver<AudioCmd>,
        mut volume: f32,
        mut muted: bool,
        ready: Arc<AtomicBool>,
        active: Arc<AtomicBool>,
        clock_bits: Arc<AtomicU64>,
        init_done: Arc<AtomicBool>,
    ) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            if MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).is_err() {
                CoUninitialize();
                init_done.store(true, Ordering::Release);
                return;
            }
            let _guard = ComMfGuard;

            let url = HSTRING::from(path.as_os_str());
            let Ok(reader): Result<IMFSourceReader, _> = MFCreateSourceReaderFromURL(&url, None)
            else {
                init_done.store(true, Ordering::Release);
                return;
            };

            let Ok(audio_out) = MFCreateMediaType() else {
                init_done.store(true, Ordering::Release);
                return;
            };
            let _ = audio_out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio);
            let _ = audio_out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM);
            let _ = audio_out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16);
            if reader
                .SetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, None, &audio_out)
                .is_err()
            {
                init_done.store(true, Ordering::Release);
                return;
            }

            let Ok(cur_mt) = reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32)
            else {
                init_done.store(true, Ordering::Release);
                return;
            };
            let mut channels = cur_mt.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2) as u16;
            let mut sample_rate = cur_mt
                .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                .unwrap_or(48000);
            if channels == 0 || sample_rate == 0 {
                init_done.store(true, Ordering::Release);
                return;
            }

            let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
            let _ = reader.SetStreamSelection(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, true);

            let Ok((_stream, handle)) = OutputStream::try_default() else {
                init_done.store(true, Ordering::Release);
                return;
            };
            let mut sink = build_sink(Some(&handle), volume, muted, false);
            ready.store(true, Ordering::Release);
            active.store(true, Ordering::Release);
            init_done.store(true, Ordering::Release);

            let mut playing = true;
            let mut eof = false;
            let mut clock_base = 0.0f64;
            let mut clock_origin = Instant::now();
            let mut seek_floor: Option<f64> = None;

            loop {
                // Drain commands cleanly
                let mut stop_now = false;
                while let Ok(cmd) = rx.try_recv() {
                    match cmd {
                        AudioCmd::Stop => {
                            stop_now = true;
                            break;
                        }
                        AudioCmd::SetPlaying(p) => {
                            if p != playing {
                                if p {
                                    clock_origin = Instant::now();
                                    if let Some(s) = &sink {
                                        s.play();
                                    }
                                } else {
                                    clock_base += clock_origin.elapsed().as_secs_f64();
                                    if let Some(s) = &sink {
                                        s.pause();
                                    }
                                }
                                playing = p;
                            }
                        }
                        AudioCmd::Seek(target, resume) => {
                            let ticks = (target.max(0.0) * 10_000_000.0) as i64;
                            let pv = PROPVARIANT::from(ticks);
                            let _ = reader.SetCurrentPosition(&GUID::zeroed(), &pv);
                            if let Some(old) = sink.take() {
                                old.stop();
                            }
                            sink = build_sink(Some(&handle), volume, muted, !resume);
                            playing = resume;
                            eof = false;
                            clock_base = target.max(0.0);
                            clock_origin = Instant::now();
                            seek_floor = Some(target.max(0.0));
                            clock_bits.store(clock_base.to_bits(), Ordering::Release);
                            active.store(true, Ordering::Release);
                        }
                        AudioCmd::SetVolume(v) => {
                            volume = v;
                            if !muted {
                                if let Some(s) = &sink {
                                    s.set_volume(volume);
                                }
                            }
                        }
                        AudioCmd::SetMuted(m) => {
                            muted = m;
                            if let Some(s) = &sink {
                                s.set_volume(if muted { 0.0 } else { volume });
                            }
                        }
                    }
                }
                if stop_now {
                    break;
                }

                // Publish live audio playback clock
                if playing {
                    let now_pos = clock_base + clock_origin.elapsed().as_secs_f64();
                    clock_bits.store(now_pos.to_bits(), Ordering::Release);
                }

                // Keep a healthy buffer cushion (~16 PCM chunks ≈ ~350ms) without over-reading!
                let queued_chunks = sink.as_ref().map(|s| s.len()).unwrap_or(0);
                if eof {
                    if queued_chunks == 0 {
                        active.store(false, Ordering::Release);
                    }
                    match rx.recv_timeout(Duration::from_millis(25)) {
                        Ok(AudioCmd::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(AudioCmd::Seek(t, resume)) => {
                            let ticks = (t.max(0.0) * 10_000_000.0) as i64;
                            let pv = PROPVARIANT::from(ticks);
                            let _ = reader.SetCurrentPosition(&GUID::zeroed(), &pv);
                            if let Some(old) = sink.take() {
                                old.stop();
                            }
                            sink = build_sink(Some(&handle), volume, muted, !resume);
                            playing = resume;
                            eof = false;
                            clock_base = t.max(0.0);
                            clock_origin = Instant::now();
                            seek_floor = Some(t.max(0.0));
                            clock_bits.store(clock_base.to_bits(), Ordering::Release);
                            active.store(true, Ordering::Release);
                        }
                        Ok(AudioCmd::SetVolume(v)) => {
                            volume = v;
                            if !muted {
                                if let Some(s) = &sink {
                                    s.set_volume(volume);
                                }
                            }
                        }
                        Ok(AudioCmd::SetMuted(m)) => {
                            muted = m;
                            if let Some(s) = &sink {
                                s.set_volume(if muted { 0.0 } else { volume });
                            }
                        }
                        Ok(AudioCmd::SetPlaying(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    continue;
                }

                if !playing || queued_chunks >= 16 {
                    std::thread::sleep(Duration::from_millis(8));
                    continue;
                }

                let mut flags: u32 = 0;
                let mut timestamp: i64 = 0;
                let mut sample = None;
                let read_res = reader.ReadSample(
                    MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                );
                if read_res.is_err() {
                    eof = true;
                    continue;
                }
                if (flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32) != 0 {
                    if let Ok(mt) =
                        reader.GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32)
                    {
                        channels = mt.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(channels as u32) as u16;
                        sample_rate = mt
                            .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                            .unwrap_or(sample_rate);
                    }
                }
                if (flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                    eof = true;
                    continue;
                }
                let Some(sample) = sample else {
                    continue;
                };
                let pts = (timestamp.max(0) as f64) / 10_000_000.0;
                if let Some(floor) = seek_floor {
                    if pts + 0.03 < floor {
                        continue;
                    }
                    seek_floor = None;
                    clock_base = pts;
                    clock_origin = Instant::now();
                }

                if let Ok(buf) = sample.ConvertToContiguousBuffer() {
                    let mut ptr: *mut u8 = std::ptr::null_mut();
                    let mut len: u32 = 0;
                    if buf.Lock(&mut ptr, None, Some(&mut len)).is_ok() {
                        let samples: Vec<i16> = if !ptr.is_null() && len >= 2 {
                            std::slice::from_raw_parts(ptr, len as usize)
                                .chunks_exact(2)
                                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        let _ = buf.Unlock();
                        if !samples.is_empty() {
                            if let Some(s) = &sink {
                                s.append(SamplesBuffer::new(channels, sample_rate, samples));
                            }
                        }
                    }
                }
            }

            if let Some(old) = sink.take() {
                old.stop();
            }
        }
    }

    // ========================================================================
    // Dedicated Video-Only Thread
    // ========================================================================

    fn unpack_u64_pair(packed: u64) -> (u32, u32) {
        ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32)
    }

    unsafe fn query_video_format(
        reader: &IMFSourceReader,
        fallback_rot: u32,
    ) -> Result<VideoFormatInfo, String> {
        let mt: IMFMediaType = reader
            .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
            .map_err(|_| "Failed to query video stream format.".to_string())?;
        let packed = mt
            .GetUINT64(&MF_MT_FRAME_SIZE)
            .map_err(|_| "Failed to read video frame size.".to_string())?;
        let (raw_w, raw_h) = unpack_u64_pair(packed);
        if raw_w == 0 || raw_h == 0 || raw_w > 8192 || raw_h > 8192 {
            return Err("Invalid video frame dimensions.".into());
        }
        let stride = mt
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map(|v| v as i32)
            .unwrap_or((raw_w * 4) as i32);
        let rot = mt.GetUINT32(&MF_MT_VIDEO_ROTATION).unwrap_or(fallback_rot);
        Ok(VideoFormatInfo {
            raw_w,
            raw_h,
            stride,
            rotation_deg: rot % 360,
        })
    }

    fn set_error(shared: &Arc<Mutex<SharedState>>, msg: impl Into<String>, repaint: &dyn Fn()) {
        if let Ok(mut st) = shared.lock() {
            st.snap.playing = false;
            st.snap.error = Some(msg.into());
        }
        repaint();
    }

    pub fn run_decoder_loop(
        path: &Path,
        cmd_rx: mpsc::Receiver<PlayerCmd>,
        shared: Arc<Mutex<SharedState>>,
        repaint: Box<dyn Fn() + Send>,
    ) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            if MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).is_err() {
                CoUninitialize();
                set_error(
                    &shared,
                    "Windows Media Foundation failed to initialize.",
                    &*repaint,
                );
                return;
            }
            let _guard = ComMfGuard;

            let mut attrs: Option<IMFAttributes> = None;
            if MFCreateAttributes(&mut attrs, 2).is_err() {
                set_error(&shared, "Failed to configure video reader attributes.", &*repaint);
                return;
            }
            let Some(attrs) = attrs else {
                set_error(&shared, "Video reader attributes unavailable.", &*repaint);
                return;
            };
            let _ = attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1);

            let url = HSTRING::from(path.as_os_str());
            let reader: IMFSourceReader = match MFCreateSourceReaderFromURL(&url, &attrs) {
                Ok(r) => r,
                Err(_) => {
                    set_error(
                        &shared,
                        "Could not open video stream with Media Foundation.",
                        &*repaint,
                    );
                    return;
                }
            };

            let native_rot = reader
                .GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0)
                .ok()
                .and_then(|mt| mt.GetUINT32(&MF_MT_VIDEO_ROTATION).ok())
                .unwrap_or(0);

            let Ok(video_out) = MFCreateMediaType() else {
                set_error(&shared, "Failed to allocate video media type.", &*repaint);
                return;
            };
            let _ = video_out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
            let _ = video_out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32);
            if reader
                .SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    None,
                    &video_out,
                )
                .is_err()
            {
                set_error(
                    &shared,
                    "Video codec is not supported by Windows Media Foundation.",
                    &*repaint,
                );
                return;
            }

            let video_fmt = match query_video_format(&reader, native_rot) {
                Ok(f) => f,
                Err(e) => {
                    set_error(&shared, e, &*repaint);
                    return;
                }
            };
            let mut scaler = FastScaler::new(video_fmt);

            let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
            let _ = reader.SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true);

            let duration_secs = reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .ok()
                .and_then(|pv| u64::try_from(&pv).ok())
                .map(|ticks| ticks as f64 / 10_000_000.0)
                .unwrap_or(0.0);

            // Spawn dedicated audio-only thread so audio NEVER stutters regardless of video load!
            let audio = DedicatedAudio::spawn(path, 0.85, false);
            let has_audio = audio.has_audio();

            let (disp_w, disp_h) = video_fmt.display_dims();
            let mut playing = true;
            let mut ended = false;

            if let Ok(mut st) = shared.lock() {
                st.snap.width = disp_w;
                st.snap.height = disp_h;
                st.snap.duration_secs = duration_secs;
                st.snap.has_audio = has_audio;
                st.snap.playing = true;
                st.snap.volume = 0.85;
                st.snap.muted = false;
            }
            repaint();

            let mut frame_seq: u64 = 0;
            let mut video_eof = false;
            let mut clock_origin = Instant::now();
            let mut clock_base_pts = 0.0f64;
            let mut last_presented_pts = -1.0f64;
            let mut seek_preview_one_frame = false;
            let mut seek_target_pts: Option<f64> = None;

            let current_clock = |clock_base: f64, origin: Instant| -> f64 {
                if has_audio && audio.is_active() {
                    audio.clock_secs()
                } else {
                    clock_base + origin.elapsed().as_secs_f64()
                }
            };

            let do_seek = |target: f64,
                           reader: &IMFSourceReader,
                           video_eof: &mut bool,
                           ended: &mut bool,
                           clock_origin: &mut Instant,
                           clock_base_pts: &mut f64,
                           last_presented_pts: &mut f64,
                           seek_preview_one_frame: &mut bool,
                           seek_target_pts: &mut Option<f64>,
                           playing: bool| {
                let clamped = if duration_secs > 0.0 {
                    target.clamp(0.0, (duration_secs - 0.05).max(0.0))
                } else {
                    target.max(0.0)
                };
                let ticks = (clamped * 10_000_000.0) as i64;
                let pv = PROPVARIANT::from(ticks);
                if reader.SetCurrentPosition(&GUID::zeroed(), &pv).is_ok() {
                    if has_audio {
                        audio.send(AudioCmd::Seek(clamped, playing));
                    }
                    *video_eof = false;
                    *ended = false;
                    *clock_base_pts = clamped;
                    *clock_origin = Instant::now();
                    *last_presented_pts = -1.0;
                    *seek_preview_one_frame = !playing;
                    *seek_target_pts = Some(clamped);
                    if let Ok(mut st) = shared.lock() {
                        st.snap.position_secs = clamped;
                        st.snap.ended = false;
                    }
                    repaint();
                }
            };

            let handle_cmd = |cmd: PlayerCmd,
                              playing: &mut bool,
                              ended: &mut bool,
                              video_eof: &mut bool,
                              clock_origin: &mut Instant,
                              clock_base_pts: &mut f64,
                              last_presented_pts: &mut f64,
                              seek_preview_one_frame: &mut bool,
                              seek_target_pts: &mut Option<f64>|
             -> bool {
                match cmd {
                    PlayerCmd::Stop => return true,
                    PlayerCmd::TogglePlay => {
                        if *playing {
                            *clock_base_pts = current_clock(*clock_base_pts, *clock_origin);
                            *playing = false;
                            if has_audio {
                                audio.send(AudioCmd::SetPlaying(false));
                            }
                        } else if *ended {
                            *playing = true;
                            do_seek(
                                0.0,
                                &reader,
                                video_eof,
                                ended,
                                clock_origin,
                                clock_base_pts,
                                last_presented_pts,
                                seek_preview_one_frame,
                                seek_target_pts,
                                true,
                            );
                        } else {
                            *playing = true;
                            *clock_origin = Instant::now();
                            if has_audio {
                                audio.send(AudioCmd::SetPlaying(true));
                            }
                        }
                        if let Ok(mut st) = shared.lock() {
                            st.snap.playing = *playing;
                            st.snap.ended = *ended;
                            st.snap.position_secs = *clock_base_pts;
                        }
                        repaint();
                    }
                    PlayerCmd::Seek(t) => {
                        do_seek(
                            t,
                            &reader,
                            video_eof,
                            ended,
                            clock_origin,
                            clock_base_pts,
                            last_presented_pts,
                            seek_preview_one_frame,
                            seek_target_pts,
                            *playing,
                        );
                    }
                    PlayerCmd::SetVolume(v) => {
                        if has_audio {
                            audio.send(AudioCmd::SetVolume(v));
                        }
                        if let Ok(mut st) = shared.lock() {
                            st.snap.volume = v;
                        }
                    }
                    PlayerCmd::SetMuted(m) => {
                        if has_audio {
                            audio.send(AudioCmd::SetMuted(m));
                        }
                        if let Ok(mut st) = shared.lock() {
                            st.snap.muted = m;
                        }
                    }
                }
                false
            };

            loop {
                let mut should_stop = false;
                while let Ok(cmd) = cmd_rx.try_recv() {
                    if handle_cmd(
                        cmd,
                        &mut playing,
                        &mut ended,
                        &mut video_eof,
                        &mut clock_origin,
                        &mut clock_base_pts,
                        &mut last_presented_pts,
                        &mut seek_preview_one_frame,
                        &mut seek_target_pts,
                    ) {
                        should_stop = true;
                        break;
                    }
                }
                if should_stop {
                    break;
                }

                if (!playing && !seek_preview_one_frame) || video_eof {
                    if video_eof && playing {
                        if !has_audio || !audio.is_active() {
                            playing = false;
                            ended = true;
                            if let Ok(mut st) = shared.lock() {
                                st.snap.playing = false;
                                st.snap.ended = true;
                                if duration_secs > 0.0 {
                                    st.snap.position_secs = duration_secs;
                                }
                            }
                            repaint();
                        } else {
                            let cur = current_clock(clock_base_pts, clock_origin);
                            if let Ok(mut st) = shared.lock() {
                                st.snap.position_secs = cur.min(duration_secs.max(cur));
                            }
                            repaint();
                            std::thread::sleep(Duration::from_millis(50));
                            continue;
                        }
                    }
                    match cmd_rx.recv_timeout(Duration::from_millis(60)) {
                        Ok(cmd) => {
                            if handle_cmd(
                                cmd,
                                &mut playing,
                                &mut ended,
                                &mut video_eof,
                                &mut clock_origin,
                                &mut clock_base_pts,
                                &mut last_presented_pts,
                                &mut seek_preview_one_frame,
                                &mut seek_target_pts,
                            ) {
                                break;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    continue;
                }

                let mut flags: u32 = 0;
                let mut timestamp: i64 = 0;
                let mut sample = None;
                let read_res = reader.ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                );
                if read_res.is_err() {
                    video_eof = true;
                    continue;
                }

                if (flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32) != 0 {
                    if let Ok(new_fmt) = query_video_format(&reader, native_rot) {
                        scaler = FastScaler::new(new_fmt);
                        let (dw, dh) = new_fmt.display_dims();
                        if let Ok(mut st) = shared.lock() {
                            st.snap.width = dw;
                            st.snap.height = dh;
                        }
                    }
                }

                if (flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                    video_eof = true;
                    continue;
                }

                let Some(sample) = sample else {
                    continue;
                };
                let pts_secs = (timestamp.max(0) as f64) / 10_000_000.0;

                if let Some(target) = seek_target_pts {
                    if pts_secs + 0.04 < target {
                        continue;
                    }
                }

                // 30fps cap: skip intermediate frames on 60fps videos BEFORE converting pixels
                if !seek_preview_one_frame
                    && last_presented_pts >= 0.0
                    && pts_secs - last_presented_pts < MIN_FRAME_INTERVAL_SECS
                {
                    continue;
                }

                // Smart late-frame drop: stay locked to audio clock without wasting CPU
                if playing && !seek_preview_one_frame && last_presented_pts >= 0.0 {
                    let now_pts = current_clock(clock_base_pts, clock_origin);
                    if now_pts - pts_secs > 0.055 {
                        continue;
                    }
                }

                if playing && !seek_preview_one_frame {
                    let mut aborted = false;
                    loop {
                        let now_pts = current_clock(clock_base_pts, clock_origin);
                        let wait = pts_secs - now_pts;
                        if wait <= 0.005 {
                            break;
                        }
                        let sleep_ms = ((wait * 1000.0).min(12.0)) as u64;
                        match cmd_rx.recv_timeout(Duration::from_millis(sleep_ms.max(2))) {
                            Ok(cmd) => {
                                if handle_cmd(
                                    cmd,
                                    &mut playing,
                                    &mut ended,
                                    &mut video_eof,
                                    &mut clock_origin,
                                    &mut clock_base_pts,
                                    &mut last_presented_pts,
                                    &mut seek_preview_one_frame,
                                    &mut seek_target_pts,
                                ) {
                                    audio.stop();
                                    return;
                                }
                                aborted = true;
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                audio.stop();
                                return;
                            }
                        }
                    }
                    if aborted {
                        continue;
                    }
                }

                let Ok(buf): Result<IMFMediaBuffer, _> = sample.ConvertToContiguousBuffer() else {
                    continue;
                };

                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut cur_len: u32 = 0;
                if buf.Lock(&mut ptr, None, Some(&mut cur_len)).is_ok() {
                    let converted = if !ptr.is_null() && cur_len > 0 {
                        let slice = std::slice::from_raw_parts(ptr, cur_len as usize);
                        scaler.convert(slice)
                    } else {
                        None
                    };
                    let _ = buf.Unlock();

                    if let Some(image) = converted {
                        frame_seq += 1;
                        last_presented_pts = pts_secs;
                        seek_target_pts = None;
                        if seek_preview_one_frame {
                            seek_preview_one_frame = false;
                        }
                        if let Ok(mut st) = shared.lock() {
                            st.snap.position_secs = pts_secs;
                            st.snap.frame_seq = frame_seq;
                            st.pending_frame = Some(VideoFrame {
                                image,
                                seq: frame_seq,
                            });
                        }
                        repaint();
                    }
                }
            }

            audio.stop();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn fast_scaler_converts_landscape_bottom_up_and_rotated_portrait() {
            let src = vec![
                10, 20, 30, 0,  40, 50, 60, 0,
                70, 80, 90, 0,  100, 110, 120, 0,
            ];
            let s0 = FastScaler::new(VideoFormatInfo {
                raw_w: 2,
                raw_h: 2,
                stride: 8,
                rotation_deg: 0,
            });
            let img = s0.convert(&src).unwrap();
            assert_eq!(img.size, [2, 2]);
            assert_eq!(img.pixels[0], Color32::from_rgb(30, 20, 10));
            assert_eq!(img.pixels[3], Color32::from_rgb(120, 110, 100));

            let s_bu = FastScaler::new(VideoFormatInfo {
                raw_w: 2,
                raw_h: 2,
                stride: -8,
                rotation_deg: 0,
            });
            let img_bu = s_bu.convert(&src).unwrap();
            assert_eq!(img_bu.pixels[0], Color32::from_rgb(90, 80, 70));

            let src_rect = vec![0u8; 4 * 2 * 4];
            let s_rot = FastScaler::new(VideoFormatInfo {
                raw_w: 4,
                raw_h: 2,
                stride: 16,
                rotation_deg: 90,
            });
            let img_rot = s_rot.convert(&src_rect).unwrap();
            assert_eq!(img_rot.size, [2, 4]);
        }
    }
}
