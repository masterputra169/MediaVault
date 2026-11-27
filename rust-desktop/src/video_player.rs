//! Native In-App Real-Time MP4 Video + Audio Player (Zero External Dependencies).
//! Uses built-in Windows Media Foundation (`IMFSourceReader`) for hardware-accelerated
//! H.264/HEVC/AAC decoding + `rodio` for synchronized audio playback.

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    #[allow(dead_code)]
    pub pts_secs: f64,
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
    #[allow(dead_code)]
    Play,
    #[allow(dead_code)]
    Pause,
    TogglePlay,
    Seek(f64),
    SetVolume(f32),
    SetMuted(bool),
    Stop,
}

struct SharedState {
    snap: VideoPlayerSnapshot,
    latest_frame: Option<VideoFrame>,
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
                latest_frame: None,
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
            latest_frame: None,
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
                            "In-app hardware video playback is available on Windows; use Open External."
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
            st.latest_frame = None;
        }
    }

    pub fn is_open(&self) -> bool {
        self.loaded_path.is_some()
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

    /// Returns the newest video frame only when its sequence number exceeds `last_seq`.
    pub fn take_new_frame(&self, last_seq: u64) -> Option<VideoFrame> {
        let st = self.shared.lock().ok()?;
        let frame = st.latest_frame.as_ref()?;
        if frame.seq > last_seq {
            Some(frame.clone())
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
// Windows Media Foundation Hardware-Accelerated Decoder + Rodio Synced Audio
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
        MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_ANY_STREAM,
        MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_AUDIO_STREAM,
        MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READER_MEDIASOURCE, MF_VERSION,
        MFSTARTUP_NOSOCKET,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    const MAX_PREVIEW_DIM: u32 = 1280;

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

    #[derive(Clone, Copy)]
    struct AudioFormatInfo {
        channels: u16,
        sample_rate: u32,
    }

    fn unpack_u64_pair(packed: u64) -> (u32, u32) {
        ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32)
    }

    unsafe fn query_video_format(reader: &IMFSourceReader, fallback_rot: u32) -> Result<VideoFormatInfo, String> {
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

    unsafe fn query_audio_format(reader: &IMFSourceReader) -> Option<AudioFormatInfo> {
        let mt: IMFMediaType = reader
            .GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32)
            .ok()?;
        let channels = mt.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).ok()? as u16;
        let sample_rate = mt.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).ok()?;
        if channels == 0 || sample_rate == 0 {
            return None;
        }
        Some(AudioFormatInfo {
            channels,
            sample_rate,
        })
    }

    fn set_error(shared: &Arc<Mutex<SharedState>>, msg: impl Into<String>, repaint: &dyn Fn()) {
        if let Ok(mut st) = shared.lock() {
            st.snap.playing = false;
            st.snap.error = Some(msg.into());
        }
        repaint();
    }

    /// Converts MF RGB32 (BGRA, potentially bottom-up negative stride, with optional 90/180/270 rotation)
    /// into tightly packed top-down RGBA, cleanly downscaling if > MAX_PREVIEW_DIM for silky GPU upload.
    fn convert_bgra_to_rgba(src: &[u8], fmt: VideoFormatInfo) -> Option<(u32, u32, Vec<u8>)> {
        let rw = fmt.raw_w as usize;
        let rh = fmt.raw_h as usize;
        let abs_stride = fmt.stride.unsigned_abs() as usize;
        let row_stride = abs_stride.max(rw * 4);
        if src.len() < row_stride * rh {
            return None;
        }
        let bottom_up = fmt.stride < 0;

        let (disp_w, disp_h) = fmt.display_dims();
        let max_side = disp_w.max(disp_h);
        let (out_w, out_h) = if max_side > MAX_PREVIEW_DIM {
            let scale = MAX_PREVIEW_DIM as f64 / max_side as f64;
            (
                ((disp_w as f64 * scale).round() as u32).max(2),
                ((disp_h as f64 * scale).round() as u32).max(2),
            )
        } else {
            (disp_w, disp_h)
        };

        let ow = out_w as usize;
        let oh = out_h as usize;
        let mut out = vec![255u8; ow * oh * 4];

        // Fast direct path: no rotation and no scaling (most common 720p/1080p->720p or <=1280px)
        if fmt.rotation_deg == 0 && out_w == fmt.raw_w && out_h == fmt.raw_h {
            for y in 0..oh {
                let sy = if bottom_up { rh - 1 - y } else { y };
                let srow = &src[sy * row_stride..sy * row_stride + rw * 4];
                let drow = &mut out[y * ow * 4..(y + 1) * ow * 4];
                for x in 0..ow {
                    let si = x * 4;
                    drow[si] = srow[si + 2];     // R
                    drow[si + 1] = srow[si + 1]; // G
                    drow[si + 2] = srow[si];     // B
                    drow[si + 3] = 255;          // A
                }
            }
            return Some((out_w, out_h, out));
        }

        // General path: handles scaling + 90/180/270 MP4 rotation cleanly
        let scale_x = disp_w as f64 / out_w as f64;
        let scale_y = disp_h as f64 / out_h as f64;
        for dy in 0..oh {
            let oy = ((dy as f64 + 0.5) * scale_y) as usize;
            let oy = oy.min(disp_h as usize - 1);
            let drow = &mut out[dy * ow * 4..(dy + 1) * ow * 4];
            for dx in 0..ow {
                let ox = ((dx as f64 + 0.5) * scale_x) as usize;
                let ox = ox.min(disp_w as usize - 1);
                let (rx, ry) = match fmt.rotation_deg {
                    90 => (oy, rh - 1 - ox),
                    180 => (rw - 1 - ox, rh - 1 - oy),
                    270 => (rw - 1 - oy, ox),
                    _ => (ox, oy),
                };
                let sy = if bottom_up { rh - 1 - ry } else { ry };
                let si = sy * row_stride + rx * 4;
                let di = dx * 4;
                drow[di] = src[si + 2];
                drow[di + 1] = src[si + 1];
                drow[di + 2] = src[si];
                drow[di + 3] = 255;
            }
        }
        Some((out_w, out_h, out))
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
                set_error(&shared, "Windows Media Foundation failed to initialize.", &*repaint);
                return;
            }
            let _guard = ComMfGuard;

            // Create SourceReader with video processing enabled (auto YUV -> RGB32)
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
                    set_error(&shared, "Could not open video stream with Media Foundation.", &*repaint);
                    return;
                }
            };

            // Read native rotation before setting RGB32 output type (RGB32 type sometimes drops rotation attr)
            let native_rot = reader
                .GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0)
                .ok()
                .and_then(|mt| mt.GetUINT32(&MF_MT_VIDEO_ROTATION).ok())
                .unwrap_or(0);

            // Configure Video stream -> RGB32
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
                set_error(&shared, "Video codec is not supported by Windows Media Foundation.", &*repaint);
                return;
            }

            let mut video_fmt = match query_video_format(&reader, native_rot) {
                Ok(f) => f,
                Err(e) => {
                    set_error(&shared, e, &*repaint);
                    return;
                }
            };

            // Configure Audio stream -> 16-bit PCM (optional: video might be silent)
            let mut audio_fmt: Option<AudioFormatInfo> = None;
            if let Ok(audio_out) = MFCreateMediaType() {
                let _ = audio_out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio);
                let _ = audio_out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM);
                let _ = audio_out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16);
                if reader
                    .SetCurrentMediaType(
                        MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                        None,
                        &audio_out,
                    )
                    .is_ok()
                {
                    audio_fmt = query_audio_format(&reader);
                }
            }

            // Select streams explicitly
            let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
            let _ = reader.SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true);
            if audio_fmt.is_some() {
                let _ = reader.SetStreamSelection(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, true);
            }

            // Query duration (100-ns units -> seconds)
            let duration_secs = reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .ok()
                .and_then(|pv| u64::try_from(&pv).ok())
                .map(|ticks| ticks as f64 / 10_000_000.0)
                .unwrap_or(0.0);

            let (disp_w, disp_h) = video_fmt.display_dims();
            let audio_output = if audio_fmt.is_some() {
                OutputStream::try_default().ok()
            } else {
                None
            };
            let audio_handle = audio_output.as_ref().map(|(_, h)| h);

            let mut playing = true;
            let mut ended = false;
            let mut volume = 0.85f32;
            let mut muted = false;
            let mut sink = build_sink(audio_handle, volume, muted, false);

            if let Ok(mut st) = shared.lock() {
                st.snap.width = disp_w;
                st.snap.height = disp_h;
                st.snap.duration_secs = duration_secs;
                st.snap.has_audio = audio_fmt.is_some() && audio_handle.is_some();
                st.snap.playing = true;
            }
            repaint();

            let mut frame_seq: u64 = 0;
            let has_audio_stream = audio_fmt.is_some();
            let mut video_eof = false;
            let mut audio_eof = !has_audio_stream;
            let mut clock_origin = Instant::now();
            let mut clock_base_pts = 0.0f64;
            let mut clock_started = false;
            let mut seek_preview_one_frame = false;
            let mut seek_target_pts: Option<f64> = None;

            // Helper closure for seeking both video & audio cleanly
            let do_seek = |target: f64,
                           reader: &IMFSourceReader,
                           sink: &mut Option<Sink>,
                           video_eof: &mut bool,
                           audio_eof: &mut bool,
                           ended: &mut bool,
                           clock_origin: &mut Instant,
                           clock_base_pts: &mut f64,
                           clock_started: &mut bool,
                           seek_preview_one_frame: &mut bool,
                           seek_target_pts: &mut Option<f64>,
                           playing: bool,
                           volume: f32,
                           muted: bool| {
                let clamped = if duration_secs > 0.0 {
                    target.clamp(0.0, (duration_secs - 0.05).max(0.0))
                } else {
                    target.max(0.0)
                };
                let ticks = (clamped * 10_000_000.0) as i64;
                let pv = PROPVARIANT::from(ticks);
                if reader.SetCurrentPosition(&GUID::zeroed(), &pv).is_ok() {
                    if let Some(old) = sink.take() {
                        old.stop();
                    }
                    *sink = build_sink(audio_handle, volume, muted, !playing);
                    *video_eof = false;
                    *audio_eof = !has_audio_stream;
                    *ended = false;
                    *clock_base_pts = clamped;
                    *clock_origin = Instant::now();
                    *clock_started = false;
                    *seek_preview_one_frame = !playing;
                    *seek_target_pts = Some(clamped);
                    if let Ok(mut st) = shared.lock() {
                        st.snap.position_secs = clamped;
                        st.snap.ended = false;
                    }
                    repaint();
                }
            };

            loop {
                // Drain pending UI commands
                let mut should_stop = false;
                while let Ok(cmd) = cmd_rx.try_recv() {
                    match cmd {
                        PlayerCmd::Stop => {
                            should_stop = true;
                            break;
                        }
                        PlayerCmd::Play => {
                            if ended {
                                playing = true;
                                do_seek(
                                    0.0,
                                    &reader,
                                    &mut sink,
                                    &mut video_eof,
                                    &mut audio_eof,
                                    &mut ended,
                                    &mut clock_origin,
                                    &mut clock_base_pts,
                                    &mut clock_started,
                                    &mut seek_preview_one_frame,
                                    &mut seek_target_pts,
                                    true,
                                    volume,
                                    muted,
                                );
                            } else if !playing {
                                playing = true;
                                clock_origin = Instant::now();
                                if let Some(s) = &sink {
                                    s.play();
                                }
                            }
                            if let Ok(mut st) = shared.lock() {
                                st.snap.playing = playing;
                                st.snap.ended = ended;
                            }
                            repaint();
                        }
                        PlayerCmd::Pause => {
                            if playing {
                                if clock_started {
                                    clock_base_pts += clock_origin.elapsed().as_secs_f64();
                                }
                                playing = false;
                                if let Some(s) = &sink {
                                    s.pause();
                                }
                                if let Ok(mut st) = shared.lock() {
                                    st.snap.playing = false;
                                    st.snap.position_secs = clock_base_pts;
                                }
                                repaint();
                            }
                        }
                        PlayerCmd::TogglePlay => {
                            if playing {
                                if clock_started {
                                    clock_base_pts += clock_origin.elapsed().as_secs_f64();
                                }
                                playing = false;
                                if let Some(s) = &sink {
                                    s.pause();
                                }
                            } else if ended {
                                playing = true;
                                do_seek(
                                    0.0,
                                    &reader,
                                    &mut sink,
                                    &mut video_eof,
                                    &mut audio_eof,
                                    &mut ended,
                                    &mut clock_origin,
                                    &mut clock_base_pts,
                                    &mut clock_started,
                                    &mut seek_preview_one_frame,
                                    &mut seek_target_pts,
                                    true,
                                    volume,
                                    muted,
                                );
                            } else {
                                playing = true;
                                clock_origin = Instant::now();
                                if let Some(s) = &sink {
                                    s.play();
                                }
                            }
                            if let Ok(mut st) = shared.lock() {
                                st.snap.playing = playing;
                                st.snap.ended = ended;
                            }
                            repaint();
                        }
                        PlayerCmd::Seek(t) => {
                            do_seek(
                                t,
                                &reader,
                                &mut sink,
                                &mut video_eof,
                                &mut audio_eof,
                                &mut ended,
                                &mut clock_origin,
                                &mut clock_base_pts,
                                &mut clock_started,
                                &mut seek_preview_one_frame,
                                &mut seek_target_pts,
                                playing,
                                volume,
                                muted,
                            );
                        }
                        PlayerCmd::SetVolume(v) => {
                            volume = v;
                            if !muted {
                                if let Some(s) = &sink {
                                    s.set_volume(volume);
                                }
                            }
                            if let Ok(mut st) = shared.lock() {
                                st.snap.volume = volume;
                            }
                        }
                        PlayerCmd::SetMuted(m) => {
                            muted = m;
                            if let Some(s) = &sink {
                                s.set_volume(if muted { 0.0 } else { volume });
                            }
                            if let Ok(mut st) = shared.lock() {
                                st.snap.muted = muted;
                            }
                        }
                    }
                }
                if should_stop {
                    break;
                }

                // Idle wait when paused or ended (zero CPU usage!)
                if (!playing && !seek_preview_one_frame) || (video_eof && audio_eof) {
                    if video_eof && audio_eof && playing {
                        let audio_done = sink.as_ref().map(|s| s.empty()).unwrap_or(true);
                        if audio_done {
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
                            let cur = clock_base_pts + clock_origin.elapsed().as_secs_f64();
                            if let Ok(mut st) = shared.lock() {
                                st.snap.position_secs = cur.min(duration_secs.max(cur));
                            }
                            repaint();
                            std::thread::sleep(Duration::from_millis(16));
                            continue;
                        }
                    }
                    match cmd_rx.recv_timeout(Duration::from_millis(40)) {
                        Ok(cmd) => {
                            // Re-dispatch immediately by handling a single command inline
                            match cmd {
                                PlayerCmd::Stop => break,
                                PlayerCmd::Play | PlayerCmd::TogglePlay => {
                                    if ended {
                                        playing = true;
                                        do_seek(
                                            0.0,
                                            &reader,
                                            &mut sink,
                                            &mut video_eof,
                                            &mut audio_eof,
                                            &mut ended,
                                            &mut clock_origin,
                                            &mut clock_base_pts,
                                            &mut clock_started,
                                            &mut seek_preview_one_frame,
                                            &mut seek_target_pts,
                                            true,
                                            volume,
                                            muted,
                                        );
                                    } else if !playing {
                                        playing = true;
                                        clock_origin = Instant::now();
                                        if let Some(s) = &sink {
                                            s.play();
                                        }
                                    }
                                    if let Ok(mut st) = shared.lock() {
                                        st.snap.playing = playing;
                                        st.snap.ended = ended;
                                    }
                                    repaint();
                                }
                                PlayerCmd::Pause => {}
                                PlayerCmd::Seek(t) => {
                                    do_seek(
                                        t,
                                        &reader,
                                        &mut sink,
                                        &mut video_eof,
                                        &mut audio_eof,
                                        &mut ended,
                                        &mut clock_origin,
                                        &mut clock_base_pts,
                                        &mut clock_started,
                                        &mut seek_preview_one_frame,
                                        &mut seek_target_pts,
                                        playing,
                                        volume,
                                        muted,
                                    );
                                }
                                PlayerCmd::SetVolume(v) => {
                                    volume = v;
                                    if !muted {
                                        if let Some(s) = &sink {
                                            s.set_volume(volume);
                                        }
                                    }
                                    if let Ok(mut st) = shared.lock() {
                                        st.snap.volume = volume;
                                    }
                                }
                                PlayerCmd::SetMuted(m) => {
                                    muted = m;
                                    if let Some(s) = &sink {
                                        s.set_volume(if muted { 0.0 } else { volume });
                                    }
                                    if let Ok(mut st) = shared.lock() {
                                        st.snap.muted = muted;
                                    }
                                }
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    continue;
                }

                // Read next interleaved sample (video OR audio) from Media Foundation
                let mut stream_idx: u32 = 0;
                let mut flags: u32 = 0;
                let mut timestamp: i64 = 0;
                let mut sample = None;
                let read_res = reader.ReadSample(
                    MF_SOURCE_READER_ANY_STREAM.0 as u32,
                    0,
                    Some(&mut stream_idx),
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                );
                if read_res.is_err() {
                    video_eof = true;
                    audio_eof = true;
                    continue;
                }

                // Identify whether this sample belongs to video or audio
                let is_video_stream = reader
                    .GetCurrentMediaType(stream_idx)
                    .ok()
                    .and_then(|mt| mt.GetGUID(&MF_MT_MAJOR_TYPE).ok())
                    .is_some_and(|g| g == MFMediaType_Video);

                if (flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32) != 0 {
                    if is_video_stream {
                        if let Ok(new_fmt) = query_video_format(&reader, native_rot) {
                            video_fmt = new_fmt;
                            let (dw, dh) = video_fmt.display_dims();
                            if let Ok(mut st) = shared.lock() {
                                st.snap.width = dw;
                                st.snap.height = dh;
                            }
                        }
                    } else if let Some(af) = query_audio_format(&reader) {
                        audio_fmt = Some(af);
                    }
                }

                if (flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                    if is_video_stream {
                        video_eof = true;
                    } else {
                        audio_eof = true;
                    }
                    continue;
                }

                let Some(sample) = sample else {
                    continue;
                };
                let pts_secs = (timestamp.max(0) as f64) / 10_000_000.0;

                // Skip pre-roll frames before an exact seek target
                if let Some(target) = seek_target_pts {
                    if pts_secs + 0.04 < target {
                        continue;
                    }
                }

                let Ok(buf): Result<IMFMediaBuffer, _> = sample.ConvertToContiguousBuffer() else {
                    continue;
                };

                if is_video_stream {
                    // Pace video presentation smoothly against playback clock
                    if playing && !seek_preview_one_frame {
                        if !clock_started {
                            clock_base_pts = pts_secs;
                            clock_origin = Instant::now();
                            clock_started = true;
                        } else {
                            loop {
                                let now_pts = clock_base_pts + clock_origin.elapsed().as_secs_f64();
                                let wait = pts_secs - now_pts;
                                if wait <= 0.004 {
                                    break;
                                }
                                let sleep_ms = ((wait * 1000.0).min(10.0)) as u64;
                                std::thread::sleep(Duration::from_millis(sleep_ms.max(2)));
                                // Check for instant UI interrupt while pacing
                                if let Ok(cmd) = cmd_rx.try_recv() {
                                    match cmd {
                                        PlayerCmd::Stop => {
                                            if let Some(old) = sink.take() {
                                                old.stop();
                                            }
                                            return;
                                        }
                                        PlayerCmd::Pause | PlayerCmd::TogglePlay => {
                                            clock_base_pts += clock_origin.elapsed().as_secs_f64();
                                            playing = false;
                                            if let Some(s) = &sink {
                                                s.pause();
                                            }
                                            if let Ok(mut st) = shared.lock() {
                                                st.snap.playing = false;
                                                st.snap.position_secs = clock_base_pts;
                                            }
                                            repaint();
                                            break;
                                        }
                                        PlayerCmd::Seek(t) => {
                                            do_seek(
                                                t,
                                                &reader,
                                                &mut sink,
                                                &mut video_eof,
                                                &mut audio_eof,
                                                &mut ended,
                                                &mut clock_origin,
                                                &mut clock_base_pts,
                                                &mut clock_started,
                                                &mut seek_preview_one_frame,
                                                &mut seek_target_pts,
                                                playing,
                                                volume,
                                                muted,
                                            );
                                            break;
                                        }
                                        PlayerCmd::SetVolume(v) => {
                                            volume = v;
                                            if !muted {
                                                if let Some(s) = &sink {
                                                    s.set_volume(volume);
                                                }
                                            }
                                        }
                                        PlayerCmd::SetMuted(m) => {
                                            muted = m;
                                            if let Some(s) = &sink {
                                                s.set_volume(if muted { 0.0 } else { volume });
                                            }
                                        }
                                        PlayerCmd::Play => {}
                                    }
                                }
                            }
                        }
                    }

                    // Lock buffer & convert BGRA -> RGBA
                    let mut ptr: *mut u8 = std::ptr::null_mut();
                    let mut cur_len: u32 = 0;
                    if buf.Lock(&mut ptr, None, Some(&mut cur_len)).is_ok() {
                        let converted = if !ptr.is_null() && cur_len > 0 {
                            let slice = std::slice::from_raw_parts(ptr, cur_len as usize);
                            convert_bgra_to_rgba(slice, video_fmt)
                        } else {
                            None
                        };
                        let _ = buf.Unlock();

                        if let Some((w, h, rgba)) = converted {
                            frame_seq += 1;
                            seek_target_pts = None;
                            if seek_preview_one_frame {
                                seek_preview_one_frame = false;
                            }
                            if let Ok(mut st) = shared.lock() {
                                st.snap.position_secs = pts_secs;
                                st.snap.frame_seq = frame_seq;
                                st.latest_frame = Some(VideoFrame {
                                    width: w,
                                    height: h,
                                    rgba,
                                    pts_secs,
                                    seq: frame_seq,
                                });
                            }
                            repaint();
                        }
                    }
                } else if let (Some(af), Some(s)) = (audio_fmt, sink.as_ref()) {
                    // Decode 16-bit PCM audio chunk and append to synced Rodio sink
                    let mut ptr: *mut u8 = std::ptr::null_mut();
                    let mut cur_len: u32 = 0;
                    if buf.Lock(&mut ptr, None, Some(&mut cur_len)).is_ok() {
                        let pcm_samples: Vec<i16> = if !ptr.is_null() && cur_len >= 2 {
                            let slice = std::slice::from_raw_parts(ptr, cur_len as usize);
                            slice
                                .chunks_exact(2)
                                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        let _ = buf.Unlock();

                        if !pcm_samples.is_empty() {
                            if !clock_started && playing {
                                clock_base_pts = pts_secs;
                                clock_origin = Instant::now();
                                clock_started = true;
                            }
                            s.append(SamplesBuffer::new(af.channels, af.sample_rate, pcm_samples));
                        }
                    }
                }
            }

            if let Some(old) = sink.take() {
                old.stop();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn converts_bgra_landscape_and_bottom_up_and_rotated_portrait() {
            // 2x2 BGRA top-down: pixel (0,0) = B=10, G=20, R=30, X=0
            let src = vec![
                10, 20, 30, 0,  40, 50, 60, 0,
                70, 80, 90, 0,  100, 110, 120, 0,
            ];
            let fmt = VideoFormatInfo {
                raw_w: 2,
                raw_h: 2,
                stride: 8,
                rotation_deg: 0,
            };
            let (w, h, rgba) = convert_bgra_to_rgba(&src, fmt).unwrap();
            assert_eq!((w, h), (2, 2));
            assert_eq!(&rgba[0..4], &[30, 20, 10, 255]);
            assert_eq!(&rgba[12..16], &[120, 110, 100, 255]);

            // Bottom-up negative stride flips rows vertically
            let fmt_bu = VideoFormatInfo {
                raw_w: 2,
                raw_h: 2,
                stride: -8,
                rotation_deg: 0,
            };
            let (_, _, rgba_bu) = convert_bgra_to_rgba(&src, fmt_bu).unwrap();
            assert_eq!(&rgba_bu[0..4], &[90, 80, 70, 255]);

            // 90-degree rotation swaps display dimensions (4x2 -> 2x4)
            let src_rect = vec![0u8; 4 * 2 * 4];
            let fmt_rot = VideoFormatInfo {
                raw_w: 4,
                raw_h: 2,
                stride: 16,
                rotation_deg: 90,
            };
            let (rw, rh, _) = convert_bgra_to_rgba(&src_rect, fmt_rot).unwrap();
            assert_eq!((rw, rh), (2, 4));
        }
    }
}
