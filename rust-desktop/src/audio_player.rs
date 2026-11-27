//! Native Pure-Rust In-App Audio Player (`rodio` + `symphonia`) with Seek, Duration & Volume.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rodio::{Decoder, OutputStream, Sink};

pub struct AudioPlayer {
    _stream: Option<OutputStream>,
    sink: Option<Sink>,
    loaded_path: Option<PathBuf>,
    duration_secs: f64,
    base_pos_secs: f64,
    play_started_at: Option<Instant>,
    volume: f32,
    muted: bool,
}

fn probe_audio_duration(path: &Path) -> f64 {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if ext == "wav" {
        if let Ok(reader) = hound::WavReader::open(path) {
            let spec = reader.spec();
            let ch = spec.channels.max(1) as f64;
            let sr = spec.sample_rate.max(1) as f64;
            return (reader.len() as f64) / (ch * sr);
        }
    }
    if let Ok(info) = crate::mp4_engine::probe_mp4(path) {
        if let Some(d) = info.duration_seconds {
            if d.is_finite() && d > 0.0 {
                return d;
            }
        }
    }
    0.0
}

impl AudioPlayer {
    pub fn new() -> Self {
        Self {
            _stream: None,
            sink: None,
            loaded_path: None,
            duration_secs: 0.0,
            base_pos_secs: 0.0,
            play_started_at: None,
            volume: 0.85,
            muted: false,
        }
    }

    pub fn play_file(&mut self, path: &Path) -> Result<(), String> {
        let vol = self.volume;
        let muted = self.muted;
        self.stop();
        let (stream, handle) = OutputStream::try_default()
            .map_err(|_| "Audio output device is unavailable.".to_string())?;
        let sink =
            Sink::try_new(&handle).map_err(|_| "Failed to initialize audio player.".to_string())?;
        let file = File::open(path).map_err(|_| "Audio file was not found.".to_string())?;
        let source = Decoder::new(BufReader::new(file))
            .map_err(|_| "Unable to decode audio format.".to_string())?;
        sink.set_volume(if muted { 0.0 } else { vol });
        sink.append(source);
        sink.play();

        self.duration_secs = probe_audio_duration(path);
        self.base_pos_secs = 0.0;
        self.play_started_at = Some(Instant::now());
        self._stream = Some(stream);
        self.sink = Some(sink);
        self.loaded_path = Some(path.to_path_buf());
        self.volume = vol;
        self.muted = muted;
        Ok(())
    }

    pub fn toggle_pause(&mut self) {
        if let Some(sink) = &self.sink {
            if sink.empty() {
                if let Some(p) = self.loaded_path.clone() {
                    let _ = self.play_file(&p);
                }
                return;
            }
            if sink.is_paused() {
                sink.play();
                self.play_started_at = Some(Instant::now());
            } else {
                if let Some(t0) = self.play_started_at.take() {
                    self.base_pos_secs += t0.elapsed().as_secs_f64();
                }
                sink.pause();
            }
        }
    }

    pub fn seek(&mut self, secs: f64) {
        let target = if self.duration_secs > 0.0 {
            secs.clamp(0.0, (self.duration_secs - 0.05).max(0.0))
        } else {
            secs.max(0.0)
        };
        if let Some(sink) = &self.sink {
            if sink.try_seek(Duration::from_secs_f64(target)).is_ok() {
                self.base_pos_secs = target;
                if !sink.is_paused() {
                    self.play_started_at = Some(Instant::now());
                }
            }
        }
    }

    pub fn set_volume(&mut self, vol: f32) {
        self.volume = vol.clamp(0.0, 1.0);
        if !self.muted {
            if let Some(sink) = &self.sink {
                sink.set_volume(self.volume);
            }
        }
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        if let Some(sink) = &self.sink {
            sink.set_volume(if self.muted { 0.0 } else { self.volume });
        }
    }

    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self._stream = None;
        self.loaded_path = None;
        self.duration_secs = 0.0;
        self.base_pos_secs = 0.0;
        self.play_started_at = None;
    }

    pub fn is_active(&self) -> bool {
        self.sink.as_ref().is_some_and(|s| !s.empty())
    }

    pub fn is_paused(&self) -> bool {
        self.sink.as_ref().is_some_and(Sink::is_paused)
    }

    pub fn loaded_path_str(&self) -> Option<String> {
        self.loaded_path.as_ref().map(|p| p.display().to_string())
    }

    pub fn duration_secs(&self) -> f64 {
        self.duration_secs
    }

    pub fn position_secs(&self) -> f64 {
        if !self.is_active() {
            return self.duration_secs;
        }
        let elapsed = self
            .play_started_at
            .map(|t0| t0.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let pos = self.base_pos_secs + elapsed;
        if self.duration_secs > 0.0 {
            pos.min(self.duration_secs)
        } else {
            pos
        }
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn muted(&self) -> bool {
        self.muted
    }
}
