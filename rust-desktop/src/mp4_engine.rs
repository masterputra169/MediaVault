//! 100% Pure-Rust ISO-BMFF (MP4 / fMP4 / M4A) Probe, Demuxer, Fast-Start Muxer, and WAV Converter.
//! Replaces FFmpeg & ffprobe completely with zero external binaries and minimal RAM footprint.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::errors::AppError;

const MAX_ATOM_READ: u64 = 64 * 1024 * 1024;
const MAX_SAMPLES_PER_TRACK: usize = 5_000_000;

#[derive(Debug, Clone)]
pub struct Mp4ProbeInfo {
    pub duration_seconds: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    #[allow(dead_code)]
    pub file_size_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrackKind {
    Video,
    Audio,
    Other,
}

#[derive(Debug, Clone)]
struct SampleMeta {
    file_offset: u64,
    size: u32,
    duration: u32,
    cts_offset: i32,
    is_sync: bool,
}

#[derive(Debug, Clone)]
struct ParsedTrack {
    track_id: u32,
    kind: TrackKind,
    timescale: u32,
    duration: u64,
    width_fixed: u32,  // 16.16 fixed-point
    height_fixed: u32, // 16.16 fixed-point
    stsd_body: Vec<u8>, // full stsd box payload (after 8-byte box header)
    codec_fourcc: Option<String>,
    samples: Vec<SampleMeta>,
}

#[derive(Debug, Clone, Copy, Default)]
struct TrexDefaults {
    default_sample_duration: u32,
    default_sample_size: u32,
    default_sample_flags: u32,
}

#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    start: u64,
    header_size: u64,
    size: u64,
    kind: [u8; 4],
}

impl BoxHeader {
    fn payload_start(&self) -> u64 {
        self.start + self.header_size
    }
    fn end(&self) -> u64 {
        self.start + self.size
    }
    fn payload_size(&self) -> u64 {
        self.size.saturating_sub(self.header_size)
    }
}

fn read_be_u32(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off + 4)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_be_i32(b: &[u8], off: usize) -> Option<i32> {
    let s = b.get(off..off + 4)?;
    Some(i32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_be_u64(b: &[u8], off: usize) -> Option<u64> {
    let s = b.get(off..off + 8)?;
    Some(u64::from_be_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

fn read_box_header<R: Read + Seek>(r: &mut R, pos: u64, limit: u64) -> io::Result<Option<BoxHeader>> {
    if pos + 8 > limit {
        return Ok(None);
    }
    r.seek(SeekFrom::Start(pos))?;
    let mut hdr = [0u8; 8];
    r.read_exact(&mut hdr)?;
    let size32 = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
    let kind = [hdr[4], hdr[5], hdr[6], hdr[7]];
    let (size, header_size) = if size32 == 1 {
        if pos + 16 > limit {
            return Ok(None);
        }
        let mut ext = [0u8; 8];
        r.read_exact(&mut ext)?;
        (u64::from_be_bytes(ext), 16)
    } else if size32 == 0 {
        (limit - pos, 8)
    } else {
        (size32, 8)
    };
    if size < header_size || pos.saturating_add(size) > limit {
        return Ok(None);
    }
    Ok(Some(BoxHeader {
        start: pos,
        header_size,
        size,
        kind,
    }))
}

fn read_slice_box<'a>(data: &'a [u8], pos: usize) -> Option<([u8; 4], &'a [u8], usize)> {
    if pos + 8 > data.len() {
        return None;
    }
    let size32 = read_be_u32(data, pos)? as usize;
    let kind = [data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]];
    let (size, hdr) = if size32 == 1 {
        let s64 = read_be_u64(data, pos + 8)? as usize;
        (s64, 16)
    } else if size32 == 0 {
        (data.len() - pos, 8)
    } else {
        (size32, 8)
    };
    if size < hdr || pos.checked_add(size)? > data.len() {
        return None;
    }
    Some((kind, &data[pos + hdr..pos + size], pos + size))
}

fn read_box_bytes<R: Read + Seek>(r: &mut R, bh: &BoxHeader) -> Result<Vec<u8>, AppError> {
    let len = bh.payload_size();
    if len > MAX_ATOM_READ {
        return Err(AppError::new("MUX_FAILED"));
    }
    r.seek(SeekFrom::Start(bh.payload_start()))
        .map_err(|_| AppError::new("MUX_FAILED"))?;
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)
        .map_err(|_| AppError::new("MUX_FAILED"))?;
    Ok(buf)
}

fn parse_tkhd(payload: &[u8]) -> Option<(u32, u32, u32)> {
    let version = *payload.first()?;
    if version == 0 {
        let track_id = read_be_u32(payload, 12)?;
        let width_fixed = read_be_u32(payload, 76)?;
        let height_fixed = read_be_u32(payload, 80)?;
        Some((track_id, width_fixed, height_fixed))
    } else if version == 1 {
        let track_id = read_be_u32(payload, 20)?;
        let width_fixed = read_be_u32(payload, 88)?;
        let height_fixed = read_be_u32(payload, 92)?;
        Some((track_id, width_fixed, height_fixed))
    } else {
        None
    }
}

fn parse_mdhd(payload: &[u8]) -> Option<(u32, u64)> {
    let version = *payload.first()?;
    if version == 0 {
        let timescale = read_be_u32(payload, 12)?;
        let duration = read_be_u32(payload, 16)? as u64;
        Some((timescale, duration))
    } else if version == 1 {
        let timescale = read_be_u32(payload, 20)?;
        let duration = read_be_u64(payload, 24)?;
        Some((timescale, duration))
    } else {
        None
    }
}

fn parse_hdlr(payload: &[u8]) -> TrackKind {
    if payload.len() < 12 {
        return TrackKind::Other;
    }
    match &payload[8..12] {
        b"vide" => TrackKind::Video,
        b"soun" => TrackKind::Audio,
        _ => TrackKind::Other,
    }
}

fn extract_codec_fourcc(stsd_payload: &[u8]) -> Option<String> {
    // stsd: 4 bytes version/flags, 4 bytes entry_count, then sample entry box (4 size, 4 fourcc)
    let entry = stsd_payload.get(8..16)?;
    let s = std::str::from_utf8(&entry[4..8]).ok()?;
    Some(s.trim().to_string())
}

fn parse_stbl_samples(stbl: &[u8], kind: TrackKind) -> Vec<SampleMeta> {
    let mut stts: Vec<(u32, u32)> = Vec::new();
    let mut ctts: Vec<(u32, i32)> = Vec::new();
    let mut stsc: Vec<(u32, u32)> = Vec::new(); // (first_chunk_1based, samples_per_chunk)
    let mut stsz_uniform: u32 = 0;
    let mut stsz_sizes: Vec<u32> = Vec::new();
    let mut stsz_count: usize = 0;
    let mut chunk_offsets: Vec<u64> = Vec::new();
    let mut sync_samples: Vec<u32> = Vec::new();
    let mut has_stss = false;

    let mut pos = 0;
    while let Some((box_kind, payload, next)) = read_slice_box(stbl, pos) {
        pos = next;
        match &box_kind {
            b"stts" if payload.len() >= 8 => {
                let count = read_be_u32(payload, 4).unwrap_or(0) as usize;
                let max_n = ((payload.len() - 8) / 8).min(count);
                for i in 0..max_n {
                    let sc = read_be_u32(payload, 8 + i * 8).unwrap_or(0);
                    let sd = read_be_u32(payload, 12 + i * 8).unwrap_or(0);
                    stts.push((sc, sd));
                }
            }
            b"ctts" if payload.len() >= 8 => {
                let version = payload[0];
                let count = read_be_u32(payload, 4).unwrap_or(0) as usize;
                let max_n = ((payload.len() - 8) / 8).min(count);
                for i in 0..max_n {
                    let sc = read_be_u32(payload, 8 + i * 8).unwrap_or(0);
                    let off = if version == 1 {
                        read_be_i32(payload, 12 + i * 8).unwrap_or(0)
                    } else {
                        read_be_u32(payload, 12 + i * 8).unwrap_or(0) as i32
                    };
                    ctts.push((sc, off));
                }
            }
            b"stsc" if payload.len() >= 8 => {
                let count = read_be_u32(payload, 4).unwrap_or(0) as usize;
                let max_n = ((payload.len() - 8) / 12).min(count);
                for i in 0..max_n {
                    let first_chunk = read_be_u32(payload, 8 + i * 12).unwrap_or(1);
                    let spc = read_be_u32(payload, 12 + i * 12).unwrap_or(0);
                    stsc.push((first_chunk, spc));
                }
            }
            b"stsz" if payload.len() >= 12 => {
                stsz_uniform = read_be_u32(payload, 4).unwrap_or(0);
                stsz_count = (read_be_u32(payload, 8).unwrap_or(0) as usize).min(MAX_SAMPLES_PER_TRACK);
                if stsz_uniform == 0 {
                    let max_n = ((payload.len() - 12) / 4).min(stsz_count);
                    stsz_sizes.reserve(max_n);
                    for i in 0..max_n {
                        stsz_sizes.push(read_be_u32(payload, 12 + i * 4).unwrap_or(0));
                    }
                }
            }
            b"stco" if payload.len() >= 8 => {
                let count = read_be_u32(payload, 4).unwrap_or(0) as usize;
                let max_n = ((payload.len() - 8) / 4).min(count);
                chunk_offsets.reserve(max_n);
                for i in 0..max_n {
                    chunk_offsets.push(read_be_u32(payload, 8 + i * 4).unwrap_or(0) as u64);
                }
            }
            b"co64" if payload.len() >= 8 => {
                let count = read_be_u32(payload, 4).unwrap_or(0) as usize;
                let max_n = ((payload.len() - 8) / 8).min(count);
                chunk_offsets.reserve(max_n);
                for i in 0..max_n {
                    chunk_offsets.push(read_be_u64(payload, 8 + i * 8).unwrap_or(0));
                }
            }
            b"stss" if payload.len() >= 8 => {
                has_stss = true;
                let count = read_be_u32(payload, 4).unwrap_or(0) as usize;
                let max_n = ((payload.len() - 8) / 4).min(count);
                sync_samples.reserve(max_n);
                for i in 0..max_n {
                    sync_samples.push(read_be_u32(payload, 8 + i * 4).unwrap_or(0));
                }
            }
            _ => {}
        }
    }

    let sample_count = if stsz_uniform > 0 {
        stsz_count
    } else {
        stsz_sizes.len()
    };
    if sample_count == 0 || chunk_offsets.is_empty() || stsc.is_empty() {
        return Vec::new();
    }

    // Expand durations
    let mut durations = Vec::with_capacity(sample_count);
    for (cnt, dur) in stts {
        for _ in 0..cnt {
            if durations.len() >= sample_count {
                break;
            }
            durations.push(dur);
        }
    }
    while durations.len() < sample_count {
        durations.push(durations.last().copied().unwrap_or(1));
    }

    // Expand composition offsets
    let mut cts_vec = vec![0i32; sample_count];
    let mut ci = 0usize;
    for (cnt, off) in ctts {
        for _ in 0..cnt {
            if ci >= sample_count {
                break;
            }
            cts_vec[ci] = off;
            ci += 1;
        }
    }

    // Sync sample lookup
    let mut sync_cursor = 0usize;

    // Expand chunk offsets -> sample offsets
    let mut samples = Vec::with_capacity(sample_count);
    let mut sample_idx = 0usize;
    let total_chunks = chunk_offsets.len();
    for (run_idx, &(first_chunk, spc)) in stsc.iter().enumerate() {
        let start_c = first_chunk.saturating_sub(1) as usize;
        let end_c = stsc
            .get(run_idx + 1)
            .map(|&(nc, _)| nc.saturating_sub(1) as usize)
            .unwrap_or(total_chunks)
            .min(total_chunks);
        for c_idx in start_c..end_c {
            let mut offset = chunk_offsets[c_idx];
            for _ in 0..spc {
                if sample_idx >= sample_count {
                    break;
                }
                let size = if stsz_uniform > 0 {
                    stsz_uniform
                } else {
                    stsz_sizes[sample_idx]
                };
                let sample_num_1based = (sample_idx + 1) as u32;
                let is_sync = if kind == TrackKind::Audio || !has_stss {
                    true
                } else {
                    while sync_cursor < sync_samples.len()
                        && sync_samples[sync_cursor] < sample_num_1based
                    {
                        sync_cursor += 1;
                    }
                    sync_cursor < sync_samples.len()
                        && sync_samples[sync_cursor] == sample_num_1based
                };
                samples.push(SampleMeta {
                    file_offset: offset,
                    size,
                    duration: durations[sample_idx],
                    cts_offset: cts_vec[sample_idx],
                    is_sync,
                });
                offset = offset.saturating_add(size as u64);
                sample_idx += 1;
            }
        }
    }
    samples
}

fn parse_trak(trak_payload: &[u8]) -> Option<ParsedTrack> {
    let mut track_id = 0u32;
    let mut width_fixed = 0u32;
    let mut height_fixed = 0u32;
    let mut timescale = 1000u32;
    let mut duration = 0u64;
    let mut kind = TrackKind::Other;
    let mut stsd_body = Vec::new();
    let mut samples = Vec::new();

    let mut pos = 0;
    while let Some((bk, payload, next)) = read_slice_box(trak_payload, pos) {
        pos = next;
        match &bk {
            b"tkhd" => {
                if let Some((tid, wf, hf)) = parse_tkhd(payload) {
                    track_id = tid;
                    width_fixed = wf;
                    height_fixed = hf;
                }
            }
            b"mdia" => {
                let mut mpos = 0;
                while let Some((mk, mpayload, mnext)) = read_slice_box(payload, mpos) {
                    mpos = mnext;
                    match &mk {
                        b"mdhd" => {
                            if let Some((ts, dur)) = parse_mdhd(mpayload) {
                                timescale = ts.max(1);
                                duration = dur;
                            }
                        }
                        b"hdlr" => {
                            kind = parse_hdlr(mpayload);
                        }
                        b"minf" => {
                            let mut ipos = 0;
                            while let Some((ik, ipayload, inext)) = read_slice_box(mpayload, ipos) {
                                ipos = inext;
                                if &ik == b"stbl" {
                                    let mut spos = 0;
                                    while let Some((sk, spayload, snext)) =
                                        read_slice_box(ipayload, spos)
                                    {
                                        spos = snext;
                                        if &sk == b"stsd" {
                                            stsd_body = spayload.to_vec();
                                        }
                                    }
                                    samples = parse_stbl_samples(ipayload, kind);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    if track_id == 0 || stsd_body.is_empty() {
        return None;
    }
    let codec_fourcc = extract_codec_fourcc(&stsd_body);
    Some(ParsedTrack {
        track_id,
        kind,
        timescale,
        duration,
        width_fixed,
        height_fixed,
        stsd_body,
        codec_fourcc,
        samples,
    })
}

fn parse_mvex(mvex_payload: &[u8]) -> HashMap<u32, TrexDefaults> {
    let mut map = HashMap::new();
    let mut pos = 0;
    while let Some((bk, payload, next)) = read_slice_box(mvex_payload, pos) {
        pos = next;
        if &bk == b"trex" && payload.len() >= 24 {
            let tid = read_be_u32(payload, 4).unwrap_or(0);
            let default_sample_duration = read_be_u32(payload, 12).unwrap_or(0);
            let default_sample_size = read_be_u32(payload, 16).unwrap_or(0);
            let default_sample_flags = read_be_u32(payload, 20).unwrap_or(0);
            map.insert(
                tid,
                TrexDefaults {
                    default_sample_duration,
                    default_sample_size,
                    default_sample_flags,
                },
            );
        }
    }
    map
}

fn parse_moof_into_tracks(
    moof_start: u64,
    moof_payload: &[u8],
    next_mdat_payload_start: Option<u64>,
    trex_map: &HashMap<u32, TrexDefaults>,
    tracks: &mut [ParsedTrack],
) {
    let mut pos = 0;
    while let Some((bk, traf_payload, next)) = read_slice_box(moof_payload, pos) {
        pos = next;
        if &bk != b"traf" {
            continue;
        }
        let mut track_id = 0u32;
        let mut base_data_offset: Option<u64> = None;
        let mut default_base_is_moof = false;
        let mut def_dur: Option<u32> = None;
        let mut def_size: Option<u32> = None;
        let mut def_flags: Option<u32> = None;
        let mut truns: Vec<&[u8]> = Vec::new();

        let mut tpos = 0;
        while let Some((tk, tpayload, tnext)) = read_slice_box(traf_payload, tpos) {
            tpos = tnext;
            match &tk {
                b"tfhd" if tpayload.len() >= 8 => {
                    let flags = u32::from_be_bytes([0, tpayload[1], tpayload[2], tpayload[3]]);
                    track_id = read_be_u32(tpayload, 4).unwrap_or(0);
                    default_base_is_moof = (flags & 0x020000) != 0;
                    let mut off = 8usize;
                    if (flags & 0x000001) != 0 {
                        base_data_offset = read_be_u64(tpayload, off);
                        off += 8;
                    }
                    if (flags & 0x000002) != 0 {
                        off += 4; // sample_description_index
                    }
                    if (flags & 0x000008) != 0 {
                        def_dur = read_be_u32(tpayload, off);
                        off += 4;
                    }
                    if (flags & 0x000010) != 0 {
                        def_size = read_be_u32(tpayload, off);
                        off += 4;
                    }
                    if (flags & 0x000020) != 0 {
                        def_flags = read_be_u32(tpayload, off);
                    }
                }
                b"trun" => {
                    truns.push(tpayload);
                }
                _ => {}
            }
        }

        let Some(track) = tracks.iter_mut().find(|t| t.track_id == track_id) else {
            continue;
        };
        let trex = trex_map.get(&track_id).copied().unwrap_or_default();
        let sample_def_dur = def_dur.unwrap_or(trex.default_sample_duration).max(1);
        let sample_def_size = def_size.unwrap_or(trex.default_sample_size);
        let sample_def_flags = def_flags.unwrap_or(trex.default_sample_flags);

        let anchor = if let Some(bdo) = base_data_offset {
            bdo
        } else if default_base_is_moof {
            moof_start
        } else {
            next_mdat_payload_start.unwrap_or(moof_start)
        };

        let mut running_offset = anchor;
        for trun in truns {
            if trun.len() < 8 {
                continue;
            }
            let version = trun[0];
            let flags = u32::from_be_bytes([0, trun[1], trun[2], trun[3]]);
            let sample_count = read_be_u32(trun, 4).unwrap_or(0) as usize;
            let mut off = 8usize;
            if (flags & 0x000001) != 0 {
                let data_offset = read_be_i32(trun, off).unwrap_or(0);
                running_offset = (anchor as i64).saturating_add(data_offset as i64).max(0) as u64;
                off += 4;
            }
            let first_sample_flags = if (flags & 0x000004) != 0 {
                let f = read_be_u32(trun, off);
                off += 4;
                f
            } else {
                None
            };
            let has_dur = (flags & 0x000100) != 0;
            let has_size = (flags & 0x000200) != 0;
            let has_flags = (flags & 0x000400) != 0;
            let has_cts = (flags & 0x000800) != 0;

            for i in 0..sample_count {
                if track.samples.len() >= MAX_SAMPLES_PER_TRACK {
                    break;
                }
                let dur = if has_dur {
                    let Some(v) = read_be_u32(trun, off) else {
                        break;
                    };
                    off += 4;
                    v
                } else {
                    sample_def_dur
                };
                let size = if has_size {
                    let Some(v) = read_be_u32(trun, off) else {
                        break;
                    };
                    off += 4;
                    v
                } else {
                    sample_def_size
                };
                let sflags = if has_flags {
                    let Some(v) = read_be_u32(trun, off) else {
                        break;
                    };
                    off += 4;
                    v
                } else if i == 0 {
                    first_sample_flags.unwrap_or(sample_def_flags)
                } else {
                    sample_def_flags
                };
                let cts_offset = if has_cts {
                    let v = if version == 1 {
                        read_be_i32(trun, off)
                    } else {
                        read_be_u32(trun, off).map(|u| u as i32)
                    };
                    let Some(val) = v else {
                        break;
                    };
                    off += 4;
                    val
                } else {
                    0
                };
                // ISO 14496-12: sample_is_non_sync_sample is bit 16 (0x00010000)
                let is_sync = if track.kind == TrackKind::Audio {
                    true
                } else {
                    (sflags & 0x0001_0000) == 0
                };
                track.samples.push(SampleMeta {
                    file_offset: running_offset,
                    size,
                    duration: dur,
                    cts_offset,
                    is_sync,
                });
                running_offset = running_offset.saturating_add(size as u64);
            }
        }
    }
}

fn parse_mp4_file(path: &Path) -> Result<(u64, u32, u64, Vec<ParsedTrack>), AppError> {
    let file = File::open(path).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    let file_len = file
        .metadata()
        .map_err(|_| AppError::new("FILE_NOT_FOUND"))?
        .len();
    let mut reader = BufReader::new(file);

    let mut top_boxes = Vec::new();
    let mut pos = 0u64;
    while let Some(bh) = read_box_header(&mut reader, pos, file_len)
        .map_err(|_| AppError::new("MUX_FAILED"))?
    {
        pos = bh.end();
        top_boxes.push(bh);
    }

    let mut movie_timescale = 1000u32;
    let mut movie_duration = 0u64;
    let mut tracks = Vec::new();
    let mut trex_map = HashMap::new();

    for bh in &top_boxes {
        if &bh.kind == b"moov" {
            let moov_bytes = read_box_bytes(&mut reader, bh)?;
            let mut mpos = 0;
            while let Some((mk, mpayload, mnext)) = read_slice_box(&moov_bytes, mpos) {
                mpos = mnext;
                match &mk {
                    b"mvhd" if mpayload.len() >= 20 => {
                        let ver = mpayload[0];
                        if ver == 0 {
                            movie_timescale = read_be_u32(mpayload, 12).unwrap_or(1000).max(1);
                            movie_duration = read_be_u32(mpayload, 16).unwrap_or(0) as u64;
                        } else if ver == 1 && mpayload.len() >= 32 {
                            movie_timescale = read_be_u32(mpayload, 20).unwrap_or(1000).max(1);
                            movie_duration = read_be_u64(mpayload, 24).unwrap_or(0);
                        }
                    }
                    b"trak" => {
                        if let Some(t) = parse_trak(mpayload) {
                            tracks.push(t);
                        }
                    }
                    b"mvex" => {
                        trex_map = parse_mvex(mpayload);
                    }
                    _ => {}
                }
            }
        }
    }

    // Parse fragmented MP4 (`moof` + `mdat`) if present
    for (idx, bh) in top_boxes.iter().enumerate() {
        if &bh.kind == b"moof" {
            let moof_bytes = read_box_bytes(&mut reader, bh)?;
            let next_mdat = top_boxes
                .get(idx + 1)
                .filter(|nb| &nb.kind == b"mdat")
                .map(BoxHeader::payload_start);
            parse_moof_into_tracks(bh.start, &moof_bytes, next_mdat, &trex_map, &mut tracks);
        }
    }

    // Recompute track durations from actual samples when fragmented
    for t in &mut tracks {
        if !t.samples.is_empty() {
            let sum_dur: u64 = t.samples.iter().map(|s| s.duration as u64).sum();
            if sum_dur > 0 {
                t.duration = sum_dur;
            }
        }
    }

    if movie_duration == 0 {
        movie_duration = tracks
            .iter()
            .map(|t| (t.duration.saturating_mul(movie_timescale as u64)) / (t.timescale.max(1) as u64))
            .max()
            .unwrap_or(0);
    }

    Ok((file_len, movie_timescale, movie_duration, tracks))
}

/// Fast (<1ms) pure-Rust MP4 / M4A probe replacing `ffprobe`.
pub fn probe_mp4(path: &Path) -> Result<Mp4ProbeInfo, AppError> {
    let (file_len, mvhd_ts, mvhd_dur, tracks) = parse_mp4_file(path)?;
    let duration_seconds = if mvhd_dur > 0 && mvhd_ts > 0 {
        Some(mvhd_dur as f64 / mvhd_ts as f64)
    } else {
        None
    };
    let video = tracks.iter().find(|t| t.kind == TrackKind::Video);
    let audio = tracks.iter().find(|t| t.kind == TrackKind::Audio);
    Ok(Mp4ProbeInfo {
        duration_seconds,
        width: video.map(|v| v.width_fixed >> 16).filter(|&w| w > 0),
        height: video.map(|v| v.height_fixed >> 16).filter(|&h| h > 0),
        video_codec: video.and_then(|v| v.codec_fourcc.clone()),
        audio_codec: audio.and_then(|a| a.codec_fourcc.clone()),
        file_size_bytes: file_len,
    })
}

// ============================================================================
// Pure-Rust Fast-Start MP4 / M4A Writer (Interleaved Chunks + co64)
// ============================================================================

fn wrap_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let total = (8 + body.len()) as u32;
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

fn build_ftyp(audio_only: bool) -> Vec<u8> {
    let mut body = Vec::new();
    if audio_only {
        body.extend_from_slice(b"M4A ");
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(b"M4A ");
        body.extend_from_slice(b"mp42");
        body.extend_from_slice(b"isom");
    } else {
        body.extend_from_slice(b"isom");
        body.extend_from_slice(&0x0000_0200u32.to_be_bytes());
        body.extend_from_slice(b"isom");
        body.extend_from_slice(b"iso2");
        body.extend_from_slice(b"avc1");
        body.extend_from_slice(b"mp41");
    }
    wrap_box(b"ftyp", &body)
}

fn build_mvhd(timescale: u32, duration: u64, next_track_id: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(112);
    // version 1 (64-bit times/duration)
    body.extend_from_slice(&[1, 0, 0, 0]);
    body.extend_from_slice(&0u64.to_be_bytes()); // creation_time
    body.extend_from_slice(&0u64.to_be_bytes()); // modification_time
    body.extend_from_slice(&timescale.to_be_bytes());
    body.extend_from_slice(&duration.to_be_bytes());
    body.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
    body.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
    body.extend_from_slice(&[0u8; 10]); // reserved
    // Unity 3x3 matrix
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        body.extend_from_slice(&v.to_be_bytes());
    }
    body.extend_from_slice(&[0u8; 24]); // pre_defined
    body.extend_from_slice(&next_track_id.to_be_bytes());
    wrap_box(b"mvhd", &body)
}

struct ChunkPlan {
    track_idx: usize,
    sample_start: usize,
    sample_count: usize,
    relative_mdat_offset: u64,
}

fn build_trak_box(
    track: &ParsedTrack,
    out_track_id: u32,
    movie_timescale: u32,
    chunk_runs: &[(usize, u64)], // (samples_in_chunk, absolute_file_offset)
) -> Vec<u8> {
    let track_dur_media: u64 = track.samples.iter().map(|s| s.duration as u64).sum();
    let track_dur_movie: u64 =
        (track_dur_media.saturating_mul(movie_timescale as u64)) / (track.timescale.max(1) as u64);

    // tkhd (version 1)
    let mut tkhd = Vec::with_capacity(96);
    tkhd.extend_from_slice(&[1, 0, 0, 3]); // enabled + in_movie
    tkhd.extend_from_slice(&0u64.to_be_bytes());
    tkhd.extend_from_slice(&0u64.to_be_bytes());
    tkhd.extend_from_slice(&out_track_id.to_be_bytes());
    tkhd.extend_from_slice(&0u32.to_be_bytes());
    tkhd.extend_from_slice(&track_dur_movie.to_be_bytes());
    tkhd.extend_from_slice(&[0u8; 8]); // reserved
    tkhd.extend_from_slice(&0i16.to_be_bytes()); // layer
    tkhd.extend_from_slice(&(if track.kind == TrackKind::Audio { 1i16 } else { 0i16 }).to_be_bytes());
    tkhd.extend_from_slice(&(if track.kind == TrackKind::Audio { 0x0100u16 } else { 0u16 }).to_be_bytes());
    tkhd.extend_from_slice(&0u16.to_be_bytes());
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        tkhd.extend_from_slice(&v.to_be_bytes());
    }
    tkhd.extend_from_slice(&track.width_fixed.to_be_bytes());
    tkhd.extend_from_slice(&track.height_fixed.to_be_bytes());
    let tkhd_box = wrap_box(b"tkhd", &tkhd);

    // mdhd (version 1)
    let mut mdhd = Vec::with_capacity(36);
    mdhd.extend_from_slice(&[1, 0, 0, 0]);
    mdhd.extend_from_slice(&0u64.to_be_bytes());
    mdhd.extend_from_slice(&0u64.to_be_bytes());
    mdhd.extend_from_slice(&track.timescale.to_be_bytes());
    mdhd.extend_from_slice(&track_dur_media.to_be_bytes());
    mdhd.extend_from_slice(&0x55c4u16.to_be_bytes()); // "und"
    mdhd.extend_from_slice(&0u16.to_be_bytes());
    let mdhd_box = wrap_box(b"mdhd", &mdhd);

    // hdlr
    let mut hdlr = Vec::new();
    hdlr.extend_from_slice(&[0, 0, 0, 0]);
    hdlr.extend_from_slice(&0u32.to_be_bytes());
    if track.kind == TrackKind::Video {
        hdlr.extend_from_slice(b"vide");
        hdlr.extend_from_slice(&[0u8; 12]);
        hdlr.extend_from_slice(b"VideoHandler\0");
    } else {
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0u8; 12]);
        hdlr.extend_from_slice(b"SoundHandler\0");
    }
    let hdlr_box = wrap_box(b"hdlr", &hdlr);

    // vmhd / smhd
    let media_hdr_box = if track.kind == TrackKind::Video {
        let mut vmhd = vec![0, 0, 0, 1];
        vmhd.extend_from_slice(&[0u8; 8]);
        wrap_box(b"vmhd", &vmhd)
    } else {
        let mut smhd = vec![0, 0, 0, 0];
        smhd.extend_from_slice(&[0u8; 4]);
        wrap_box(b"smhd", &smhd)
    };

    // dinf -> dref -> url (self-contained flag = 1)
    let url_box = wrap_box(b"url ", &[0, 0, 0, 1]);
    let mut dref = Vec::new();
    dref.extend_from_slice(&[0, 0, 0, 0]);
    dref.extend_from_slice(&1u32.to_be_bytes());
    dref.extend_from_slice(&url_box);
    let dinf_box = wrap_box(b"dinf", &wrap_box(b"dref", &dref));

    // stsd (preserved verbatim from source)
    let stsd_box = wrap_box(b"stsd", &track.stsd_body);

    // stts (RLE)
    let mut stts_runs: Vec<(u32, u32)> = Vec::new();
    for s in &track.samples {
        if let Some(last) = stts_runs.last_mut() {
            if last.1 == s.duration {
                last.0 += 1;
                continue;
            }
        }
        stts_runs.push((1, s.duration));
    }
    let mut stts = Vec::with_capacity(8 + stts_runs.len() * 8);
    stts.extend_from_slice(&[0, 0, 0, 0]);
    stts.extend_from_slice(&(stts_runs.len() as u32).to_be_bytes());
    for (cnt, dur) in stts_runs {
        stts.extend_from_slice(&cnt.to_be_bytes());
        stts.extend_from_slice(&dur.to_be_bytes());
    }
    let stts_box = wrap_box(b"stts", &stts);

    // ctts (only if any non-zero composition offset exists)
    let has_cts = track.samples.iter().any(|s| s.cts_offset != 0);
    let ctts_box = if has_cts {
        let any_neg = track.samples.iter().any(|s| s.cts_offset < 0);
        let mut ctts_runs: Vec<(u32, i32)> = Vec::new();
        for s in &track.samples {
            if let Some(last) = ctts_runs.last_mut() {
                if last.1 == s.cts_offset {
                    last.0 += 1;
                    continue;
                }
            }
            ctts_runs.push((1, s.cts_offset));
        }
        let mut ctts = Vec::with_capacity(8 + ctts_runs.len() * 8);
        ctts.extend_from_slice(&[if any_neg { 1 } else { 0 }, 0, 0, 0]);
        ctts.extend_from_slice(&(ctts_runs.len() as u32).to_be_bytes());
        for (cnt, off) in ctts_runs {
            ctts.extend_from_slice(&cnt.to_be_bytes());
            if any_neg {
                ctts.extend_from_slice(&off.to_be_bytes());
            } else {
                ctts.extend_from_slice(&(off as u32).to_be_bytes());
            }
        }
        Some(wrap_box(b"ctts", &ctts))
    } else {
        None
    };

    // stss (video keyframes)
    let stss_box = if track.kind == TrackKind::Video {
        let mut syncs: Vec<u32> = Vec::new();
        for (idx, s) in track.samples.iter().enumerate() {
            if s.is_sync {
                syncs.push((idx + 1) as u32);
            }
        }
        if !syncs.is_empty() && syncs.len() < track.samples.len() {
            let mut stss = Vec::with_capacity(8 + syncs.len() * 4);
            stss.extend_from_slice(&[0, 0, 0, 0]);
            stss.extend_from_slice(&(syncs.len() as u32).to_be_bytes());
            for n in syncs {
                stss.extend_from_slice(&n.to_be_bytes());
            }
            Some(wrap_box(b"stss", &stss))
        } else {
            None
        }
    } else {
        None
    };

    // stsc (RLE over chunk_runs)
    let mut stsc_entries: Vec<(u32, u32, u32)> = Vec::new();
    let mut prev_spc: Option<u32> = None;
    for (idx, &(spc, _)) in chunk_runs.iter().enumerate() {
        let spc32 = spc as u32;
        if prev_spc != Some(spc32) {
            stsc_entries.push(((idx + 1) as u32, spc32, 1));
            prev_spc = Some(spc32);
        }
    }
    let mut stsc = Vec::with_capacity(8 + stsc_entries.len() * 12);
    stsc.extend_from_slice(&[0, 0, 0, 0]);
    stsc.extend_from_slice(&(stsc_entries.len() as u32).to_be_bytes());
    for (fc, spc, sdi) in stsc_entries {
        stsc.extend_from_slice(&fc.to_be_bytes());
        stsc.extend_from_slice(&spc.to_be_bytes());
        stsc.extend_from_slice(&sdi.to_be_bytes());
    }
    let stsc_box = wrap_box(b"stsc", &stsc);

    // stsz
    let mut stsz = Vec::with_capacity(12 + track.samples.len() * 4);
    stsz.extend_from_slice(&[0, 0, 0, 0]);
    stsz.extend_from_slice(&0u32.to_be_bytes()); // variable sizes
    stsz.extend_from_slice(&(track.samples.len() as u32).to_be_bytes());
    for s in &track.samples {
        stsz.extend_from_slice(&s.size.to_be_bytes());
    }
    let stsz_box = wrap_box(b"stsz", &stsz);

    // co64 (always 64-bit offsets -> safe for >4 GiB and constant size across 2-pass patch!)
    let mut co64 = Vec::with_capacity(8 + chunk_runs.len() * 8);
    co64.extend_from_slice(&[0, 0, 0, 0]);
    co64.extend_from_slice(&(chunk_runs.len() as u32).to_be_bytes());
    for &(_, offset) in chunk_runs {
        co64.extend_from_slice(&offset.to_be_bytes());
    }
    let co64_box = wrap_box(b"co64", &co64);

    let mut stbl = Vec::new();
    stbl.extend_from_slice(&stsd_box);
    stbl.extend_from_slice(&stts_box);
    if let Some(sb) = stss_box {
        stbl.extend_from_slice(&sb);
    }
    if let Some(cb) = ctts_box {
        stbl.extend_from_slice(&cb);
    }
    stbl.extend_from_slice(&stsc_box);
    stbl.extend_from_slice(&stsz_box);
    stbl.extend_from_slice(&co64_box);
    let stbl_box = wrap_box(b"stbl", &stbl);

    let mut minf = Vec::new();
    minf.extend_from_slice(&media_hdr_box);
    minf.extend_from_slice(&dinf_box);
    minf.extend_from_slice(&stbl_box);
    let minf_box = wrap_box(b"minf", &minf);

    let mut mdia = Vec::new();
    mdia.extend_from_slice(&mdhd_box);
    mdia.extend_from_slice(&hdlr_box);
    mdia.extend_from_slice(&minf_box);
    let mdia_box = wrap_box(b"mdia", &mdia);

    let mut trak = Vec::new();
    trak.extend_from_slice(&tkhd_box);
    trak.extend_from_slice(&mdia_box);
    wrap_box(b"trak", &trak)
}

fn write_muxed_mp4(
    inputs: &[(&Path, &ParsedTrack)],
    out_path: &Path,
    audio_only: bool,
) -> Result<(), AppError> {
    if inputs.is_empty() || inputs.iter().any(|(_, t)| t.samples.is_empty()) {
        return Err(AppError::new("MUX_FAILED"));
    }

    // Build interleaved ~1-second chunk plan across all input tracks
    let mut cursors = vec![0usize; inputs.len()];
    let mut track_dts = vec![0.0f64; inputs.len()];
    let mut plan: Vec<ChunkPlan> = Vec::new();
    let mut mdat_payload_len = 0u64;

    loop {
        let next_t = (0..inputs.len())
            .filter(|&i| cursors[i] < inputs[i].1.samples.len())
            .min_by(|&a, &b| track_dts[a].total_cmp(&track_dts[b]));
        let Some(ti) = next_t else {
            break;
        };
        let track = inputs[ti].1;
        let start = cursors[ti];
        let mut count = 0usize;
        let mut chunk_bytes = 0u64;
        let mut chunk_dur = 0u64;
        let target_dur = track.timescale.max(1) as u64; // ~1s per chunk

        while start + count < track.samples.len() {
            let s = &track.samples[start + count];
            chunk_bytes = chunk_bytes.saturating_add(s.size as u64);
            chunk_dur = chunk_dur.saturating_add(s.duration as u64);
            count += 1;
            if chunk_dur >= target_dur || chunk_bytes >= 1024 * 1024 {
                break;
            }
        }
        plan.push(ChunkPlan {
            track_idx: ti,
            sample_start: start,
            sample_count: count,
            relative_mdat_offset: mdat_payload_len,
        });
        mdat_payload_len = mdat_payload_len.saturating_add(chunk_bytes);
        cursors[ti] += count;
        track_dts[ti] += chunk_dur as f64 / track.timescale.max(1) as f64;
    }

    let movie_timescale = 1000u32;
    let movie_duration = inputs
        .iter()
        .map(|(_, t)| {
            let dur: u64 = t.samples.iter().map(|s| s.duration as u64).sum();
            (dur.saturating_mul(movie_timescale as u64)) / (t.timescale.max(1) as u64)
        })
        .max()
        .unwrap_or(0);

    let ftyp = build_ftyp(audio_only);

    // Helper to build moov given mdat_data_start
    let build_moov = |mdat_data_start: u64| -> Vec<u8> {
        let mut moov_body = build_mvhd(movie_timescale, movie_duration, (inputs.len() + 1) as u32);
        for (ti, (_, track)) in inputs.iter().enumerate() {
            let runs: Vec<(usize, u64)> = plan
                .iter()
                .filter(|c| c.track_idx == ti)
                .map(|c| (c.sample_count, mdat_data_start + c.relative_mdat_offset))
                .collect();
            let trak_box = build_trak_box(track, (ti + 1) as u32, movie_timescale, &runs);
            moov_body.extend_from_slice(&trak_box);
        }
        wrap_box(b"moov", &moov_body)
    };

    // Pass 1: measure exact moov size (co64 has fixed width regardless of offset values)
    let moov_pass1 = build_moov(0);
    // 16-byte extended 64-bit mdat header: size32=1, "mdat", size64
    let mdat_header_len = 16u64;
    let mdat_data_start = ftyp.len() as u64 + moov_pass1.len() as u64 + mdat_header_len;
    let moov_final = build_moov(mdat_data_start);
    debug_assert_eq!(moov_pass1.len(), moov_final.len());

    // Stream samples directly from source files into output file (low RAM!)
    let mut src_files: Vec<BufReader<File>> = inputs
        .iter()
        .map(|(p, _)| {
            File::open(p)
                .map(BufReader::new)
                .map_err(|_| AppError::new("FILE_NOT_FOUND"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let out_file = File::create(out_path).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    let mut writer = BufWriter::with_capacity(256 * 1024, out_file);

    let write_res: io::Result<()> = (|| {
        writer.write_all(&ftyp)?;
        writer.write_all(&moov_final)?;
        // 64-bit mdat header
        writer.write_all(&1u32.to_be_bytes())?;
        writer.write_all(b"mdat")?;
        writer.write_all(&(mdat_payload_len + mdat_header_len).to_be_bytes())?;

        let mut scratch = vec![0u8; 256 * 1024];
        for chunk in &plan {
            let track = inputs[chunk.track_idx].1;
            let reader = &mut src_files[chunk.track_idx];
            for s in &track.samples[chunk.sample_start..chunk.sample_start + chunk.sample_count] {
                reader.seek(SeekFrom::Start(s.file_offset))?;
                let mut remaining = s.size as usize;
                while remaining > 0 {
                    let take = remaining.min(scratch.len());
                    reader.read_exact(&mut scratch[..take])?;
                    writer.write_all(&scratch[..take])?;
                    remaining -= take;
                }
            }
        }
        writer.flush()
    })();

    if write_res.is_err() {
        drop(writer);
        let _ = std::fs::remove_file(out_path);
        return Err(AppError::new("STORAGE_UNAVAILABLE"));
    }
    Ok(())
}

/// Lossless pure-Rust MP4 video + audio muxer (handles both standard MP4 and YouTube DASH fMP4).
pub fn mux_video_and_audio_mp4(
    video_path: &Path,
    audio_path: &Path,
    out_path: &Path,
) -> Result<(), AppError> {
    let (_, _, _, v_tracks) = parse_mp4_file(video_path)?;
    let (_, _, _, a_tracks) = parse_mp4_file(audio_path)?;
    let v_track = v_tracks
        .iter()
        .find(|t| t.kind == TrackKind::Video && !t.samples.is_empty())
        .ok_or_else(|| AppError::new("MUX_FAILED"))?;
    let a_track = a_tracks
        .iter()
        .find(|t| t.kind == TrackKind::Audio && !t.samples.is_empty())
        .ok_or_else(|| AppError::new("MUX_FAILED"))?;
    write_muxed_mp4(&[(video_path, v_track), (audio_path, a_track)], out_path, false)
}

/// Lossless pure-Rust audio track extraction into `.m4a` (works on standard MP4 & fMP4).
pub fn extract_audio_m4a(src_path: &Path, out_path: &Path) -> Result<(), AppError> {
    let (_, _, _, tracks) = parse_mp4_file(src_path)?;
    let a_track = tracks
        .iter()
        .find(|t| t.kind == TrackKind::Audio && !t.samples.is_empty())
        .ok_or_else(|| AppError::new("FORMAT_UNAVAILABLE"))?;
    write_muxed_mp4(&[(src_path, a_track)], out_path, true)
}

/// 100% Pure-Rust AAC/MP4/MP3 → 16-bit PCM WAV converter via `symphonia` + `hound`.
pub fn transcode_audio_to_wav(src_path: &Path, out_path: &Path) -> Result<(), AppError> {
    let file = File::open(src_path).map_err(|_| AppError::new("FILE_NOT_FOUND"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = src_path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .ok_or_else(|| AppError::new("FORMAT_UNAVAILABLE"))?;
    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2)
        .clamp(1, 2);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| AppError::new("FORMAT_UNAVAILABLE"))?;

    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav_writer =
        hound::WavWriter::create(out_path, spec).map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    let mut sample_buf: Option<SampleBuffer<i16>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(_) => break,
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let buf = sample_buf.get_or_insert_with(|| {
                    SampleBuffer::<i16>::new(decoded.capacity() as u64, *decoded.spec())
                });
                buf.copy_interleaved_ref(decoded);
                for &s in buf.samples() {
                    if wav_writer.write_sample(s).is_err() {
                        drop(wav_writer);
                        let _ = std::fs::remove_file(out_path);
                        return Err(AppError::new("STORAGE_UNAVAILABLE"));
                    }
                }
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(_) => break,
        }
    }

    wav_writer
        .finalize()
        .map_err(|_| AppError::new("STORAGE_UNAVAILABLE"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_stsd(fourcc: &[u8; 4]) -> Vec<u8> {
        let mut entry = Vec::new();
        entry.extend_from_slice(&[0u8; 6]); // reserved
        entry.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
        entry.extend_from_slice(&[0u8; 16]);
        let entry_box = wrap_box(fourcc, &entry);
        let mut stsd = Vec::new();
        stsd.extend_from_slice(&[0, 0, 0, 0]);
        stsd.extend_from_slice(&1u32.to_be_bytes());
        stsd.extend_from_slice(&entry_box);
        stsd
    }

    fn write_synthetic_fmp4(path: &Path, kind: TrackKind, fourcc: &[u8; 4], chunks: &[&[u8]]) {
        let mut stbl = Vec::new();
        stbl.extend_from_slice(&wrap_box(b"stsd", &synthetic_stsd(fourcc)));
        stbl.extend_from_slice(&wrap_box(b"stts", &[0u8; 8]));
        stbl.extend_from_slice(&wrap_box(b"stsc", &[0u8; 8]));
        stbl.extend_from_slice(&wrap_box(b"stsz", &[0u8; 12]));
        stbl.extend_from_slice(&wrap_box(b"stco", &[0u8; 8]));

        let mut tkhd = vec![0u8; 84];
        tkhd[3] = 3;
        tkhd[12..16].copy_from_slice(&1u32.to_be_bytes());
        if kind == TrackKind::Video {
            tkhd[76..80].copy_from_slice(&(1920u32 << 16).to_be_bytes());
            tkhd[80..84].copy_from_slice(&(1080u32 << 16).to_be_bytes());
        }
        let mut mdhd = vec![0u8; 24];
        mdhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        let mut hdlr = vec![0u8; 25];
        hdlr[8..12].copy_from_slice(if kind == TrackKind::Video { b"vide" } else { b"soun" });

        let minf = wrap_box(b"minf", &wrap_box(b"stbl", &stbl));
        let mut mdia = Vec::new();
        mdia.extend_from_slice(&wrap_box(b"mdhd", &mdhd));
        mdia.extend_from_slice(&wrap_box(b"hdlr", &hdlr));
        mdia.extend_from_slice(&minf);

        let mut trak = Vec::new();
        trak.extend_from_slice(&wrap_box(b"tkhd", &tkhd));
        trak.extend_from_slice(&wrap_box(b"mdia", &mdia));

        let mut trex = vec![0u8; 24];
        trex[4..8].copy_from_slice(&1u32.to_be_bytes());
        trex[12..16].copy_from_slice(&33u32.to_be_bytes()); // default duration
        let mvex = wrap_box(b"mvex", &wrap_box(b"trex", &trex));

        let mut moov = Vec::new();
        moov.extend_from_slice(&build_mvhd(1000, 0, 2));
        moov.extend_from_slice(&wrap_box(b"trak", &trak));
        moov.extend_from_slice(&mvex);

        let mut out = build_ftyp(kind == TrackKind::Audio);
        out.extend_from_slice(&wrap_box(b"moov", &moov));

        // Build moof + mdat fragment with default_base_is_moof + data_offset
        let mut payload = Vec::new();
        let mut trun = Vec::new();
        // flags: 0x000201 (data_offset + sample_size)
        trun.extend_from_slice(&[0, 0x00, 0x02, 0x01]);
        trun.extend_from_slice(&(chunks.len() as u32).to_be_bytes());
        let data_offset_pos = trun.len();
        trun.extend_from_slice(&0i32.to_be_bytes()); // patched below
        for c in chunks {
            trun.extend_from_slice(&(c.len() as u32).to_be_bytes());
            payload.extend_from_slice(c);
        }

        let mut tfhd = Vec::new();
        tfhd.extend_from_slice(&[0, 0x02, 0x00, 0x00]); // default_base_is_moof
        tfhd.extend_from_slice(&1u32.to_be_bytes());

        let tfhd_box = wrap_box(b"tfhd", &tfhd);
        let trun_box_len = 8 + trun.len();
        let traf_box_len = 8 + tfhd_box.len() + trun_box_len;
        let moof_box_len = 8 + traf_box_len;
        let data_offset = (moof_box_len + 8) as i32; // +8 for mdat header
        trun[data_offset_pos..data_offset_pos + 4].copy_from_slice(&data_offset.to_be_bytes());

        let mut traf = Vec::new();
        traf.extend_from_slice(&tfhd_box);
        traf.extend_from_slice(&wrap_box(b"trun", &trun));
        out.extend_from_slice(&wrap_box(b"moof", &wrap_box(b"traf", &traf)));
        out.extend_from_slice(&wrap_box(b"mdat", &payload));

        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn muxes_fragmented_video_and_audio_into_fast_start_mp4_and_extracts_m4a() {
        let dir = std::env::temp_dir().join(format!("mv_mp4_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let vid = dir.join("dash_v.mp4");
        let aud = dir.join("dash_a.m4a");
        let muxed = dir.join("combined.mp4");
        let m4a = dir.join("extracted.m4a");

        write_synthetic_fmp4(&vid, TrackKind::Video, b"avc1", &[b"VID_FRAME_1", b"VID_FRAME_2"]);
        write_synthetic_fmp4(&aud, TrackKind::Audio, b"mp4a", &[b"AUD_PKT_1", b"AUD_PKT_2", b"AUD_PKT_3"]);

        mux_video_and_audio_mp4(&vid, &aud, &muxed).unwrap();
        let info = probe_mp4(&muxed).unwrap();
        assert_eq!(info.width, Some(1920));
        assert_eq!(info.height, Some(1080));
        assert_eq!(info.video_codec.as_deref(), Some("avc1"));
        assert_eq!(info.audio_codec.as_deref(), Some("mp4a"));
        assert!(info.duration_seconds.unwrap_or(0.0) > 0.05);

        // Verify sample bytes survive lossless muxing
        let (_, _, _, tracks) = parse_mp4_file(&muxed).unwrap();
        assert_eq!(tracks.len(), 2);
        let raw_muxed = std::fs::read(&muxed).unwrap();
        let v0 = &tracks[0].samples[0];
        assert_eq!(
            &raw_muxed[v0.file_offset as usize..(v0.file_offset as usize + v0.size as usize)],
            b"VID_FRAME_1"
        );
        let a2 = &tracks[1].samples[2];
        assert_eq!(
            &raw_muxed[a2.file_offset as usize..(a2.file_offset as usize + a2.size as usize)],
            b"AUD_PKT_3"
        );

        // Extract lossless M4A from combined MP4
        extract_audio_m4a(&muxed, &m4a).unwrap();
        let m4a_info = probe_mp4(&m4a).unwrap();
        assert_eq!(m4a_info.video_codec, None);
        assert_eq!(m4a_info.audio_codec.as_deref(), Some("mp4a"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
