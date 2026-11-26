from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator

Quality = Literal["best", "1080", "720", "480", "360"]
AudioFormat = Literal["mp3", "m4a"]
Status = Literal["queued", "downloading", "processing", "completed", "failed", "cancelled", "interrupted"]
ACTIVE = ("queued", "downloading", "processing")


class Input(BaseModel):
    model_config = ConfigDict(extra="forbid")


class Analyze(Input):
    url: str = Field(max_length=2048)


class Create(Analyze):
    kind: Literal["video", "audio", "image"]
    quality: Quality = "best"
    container: Literal["mp4", "mp3", "m4a", "jpg", "png", "webp", "zip"]
    audio_bitrate_kbps: Literal[128, 192, 256, 320] | None = None
    allow_quality_fallback: bool = False
    image_index: int | None = Field(default=None, ge=1, le=30)

    @model_validator(mode="after")
    def validate_output(self):
        if self.kind == "video":
            if self.container != "mp4" or self.audio_bitrate_kbps is not None or self.image_index is not None:
                raise ValueError("Video")
        elif self.kind == "audio":
            if self.container not in {"mp3", "m4a"} or self.quality != "best" or self.allow_quality_fallback or self.image_index is not None:
                raise ValueError("Audio")
            if self.container == "m4a" and self.audio_bitrate_kbps is not None:
                raise ValueError("M4A")
            if self.container == "mp3" and self.audio_bitrate_kbps is None:
                self.audio_bitrate_kbps = 192
        else:
            if (self.container not in {"jpg", "png", "webp", "zip"} or self.quality != "best"
                    or self.audio_bitrate_kbps is not None or self.allow_quality_fallback):
                raise ValueError("Image")
            if self.container == "zip" and self.image_index is not None:
                raise ValueError("Zip")
        return self


class Preferences(Input):
    default_video_quality: Quality = "best"
    default_audio_format: AudioFormat = "mp3"
    theme: Literal["dark", "light", "system"] = "dark"


class Login(Input):
    username: str = Field(min_length=1, max_length=64)
    password: str = Field(min_length=1, max_length=256)
