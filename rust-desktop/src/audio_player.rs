use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use rodio::{Decoder, OutputStream, Sink};

pub struct AudioPlayer {
    _stream: Option<OutputStream>,
    sink: Option<Sink>,
    loaded_path: Option<String>,
}

impl AudioPlayer {
    pub fn new() -> Self {
        Self {
            _stream: None,
            sink: None,
            loaded_path: None,
        }
    }

    pub fn play_file(&mut self, path: &Path) -> Result<(), String> {
        self.stop();
        let (stream, handle) =
            OutputStream::try_default().map_err(|_| "Audio output device is unavailable.".to_string())?;
        let sink =
            Sink::try_new(&handle).map_err(|_| "Failed to initialize audio player.".to_string())?;
        let file = File::open(path).map_err(|_| "Audio file was not found.".to_string())?;
        let source = Decoder::new(BufReader::new(file))
            .map_err(|_| "Unable to decode audio format.".to_string())?;
        sink.append(source);
        sink.play();
        self._stream = Some(stream);
        self.sink = Some(sink);
        self.loaded_path = Some(path.display().to_string());
        Ok(())
    }

    pub fn toggle_pause(&mut self) {
        if let Some(sink) = &self.sink {
            if sink.is_paused() {
                sink.play();
            } else {
                sink.pause();
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self._stream = None;
        self.loaded_path = None;
    }

    pub fn is_active(&self) -> bool {
        self.sink.as_ref().is_some_and(|s| !s.empty())
    }

    pub fn is_paused(&self) -> bool {
        self.sink.as_ref().is_some_and(Sink::is_paused)
    }

    pub fn loaded_path(&self) -> Option<&str> {
        self.loaded_path.as_deref()
    }
}
