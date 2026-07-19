use std::io::{self, Cursor};

use symphonia::core::codecs::audio::well_known::{CODEC_ID_AAC, CODEC_ID_MP3, CODEC_ID_OPUS};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Timestamp;

pub const MAX_TRANSCRIPTION_AUDIO_SECONDS: u64 = 3 * 60 * 60;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidatedAudioFormat {
    Wav,
    Flac,
    Mp3,
    Opus,
    Aac,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidatedAudioMetadata {
    pub format: ValidatedAudioFormat,
    pub sample_rate: u32,
    pub channels: u8,
    pub frames: u64,
    pub duration_seconds_ceil: u64,
}

pub fn validated_audio_metadata(bytes: &[u8]) -> Option<ValidatedAudioMetadata> {
    validated_wav_audio_metadata(bytes)
        .or_else(|| validated_flac_audio_metadata(bytes))
        .or_else(|| validated_packet_audio_metadata(bytes))
}

fn validated_packet_audio_metadata(bytes: &[u8]) -> Option<ValidatedAudioMetadata> {
    if bytes.is_empty() {
        return None;
    }
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            &Hint::new(),
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .ok()?;
    let track = format.default_track(TrackType::Audio)?.clone();
    let params = track.codec_params.as_ref()?.audio()?;
    let audio_format = match params.codec {
        CODEC_ID_MP3 => ValidatedAudioFormat::Mp3,
        CODEC_ID_OPUS => ValidatedAudioFormat::Opus,
        CODEC_ID_AAC => ValidatedAudioFormat::Aac,
        _ => return None,
    };
    let sample_rate = params.sample_rate?;
    let channels = u8::try_from(params.channels.as_ref()?.count()).ok()?;
    if sample_rate == 0 || channels == 0 || channels > 8 {
        return None;
    }
    let track_id = track.id;
    let time_base = track.time_base?;
    let mut last_end: Option<Timestamp> = None;
    loop {
        match format.next_packet() {
            Ok(Some(packet)) => {
                if packet.track_id != track_id || packet.data.is_empty() {
                    continue;
                }
                let end = packet.pts.checked_add(packet.dur)?;
                last_end = Some(last_end.map_or(end, |current| current.max(end)));
            }
            Ok(None) => break,
            Err(_) => return None,
        }
    }
    let duration = last_end?.duration_from(track.start_ts)?;
    let time = time_base.calc_time(duration.timestamp_from(Timestamp::ZERO)?)?;
    if time.is_zero() || time.is_negative() {
        return None;
    }
    let duration_seconds_ceil = u64::try_from(time.as_secs())
        .ok()?
        .saturating_add(u64::from(time.parts().1 > 0));
    if duration_seconds_ceil == 0 || duration_seconds_ceil > MAX_TRANSCRIPTION_AUDIO_SECONDS {
        return None;
    }
    let frame_numerator = u128::try_from(time.as_nanos())
        .ok()?
        .checked_mul(u128::from(sample_rate))?;
    let frames = u64::try_from(frame_numerator.div_ceil(1_000_000_000)).ok()?;
    if frames == 0
        || frames > u64::from(sample_rate).checked_mul(MAX_TRANSCRIPTION_AUDIO_SECONDS)?
    {
        return None;
    }
    Some(ValidatedAudioMetadata {
        format: audio_format,
        sample_rate,
        channels,
        frames,
        duration_seconds_ceil,
    })
}

pub fn validated_wav_audio_metadata(bytes: &[u8]) -> Option<ValidatedAudioMetadata> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let riff_len = usize::try_from(u32::from_le_bytes(bytes[4..8].try_into().ok()?)).ok()?;
    if riff_len.checked_add(8)? != bytes.len() {
        return None;
    }

    let mut offset = 12usize;
    let mut format = None;
    let mut data_len = None;
    while offset < bytes.len() {
        if offset.checked_add(8)? > bytes.len() {
            return None;
        }
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_len = usize::try_from(u32::from_le_bytes(
            bytes[offset + 4..offset + 8].try_into().ok()?,
        ))
        .ok()?;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start.checked_add(chunk_len)?;
        let padded_end = chunk_end.checked_add(chunk_len % 2)?;
        if padded_end > bytes.len() {
            return None;
        }

        if chunk_id == b"fmt " {
            if format.is_some() || chunk_len < 16 {
                return None;
            }
            let format_tag =
                u16::from_le_bytes(bytes[chunk_start..chunk_start + 2].try_into().ok()?);
            let channels =
                u16::from_le_bytes(bytes[chunk_start + 2..chunk_start + 4].try_into().ok()?);
            let sample_rate =
                u32::from_le_bytes(bytes[chunk_start + 4..chunk_start + 8].try_into().ok()?);
            let byte_rate =
                u32::from_le_bytes(bytes[chunk_start + 8..chunk_start + 12].try_into().ok()?);
            let block_align =
                u16::from_le_bytes(bytes[chunk_start + 12..chunk_start + 14].try_into().ok()?);
            let bits_per_sample =
                u16::from_le_bytes(bytes[chunk_start + 14..chunk_start + 16].try_into().ok()?);

            let resolved_tag = if format_tag == 0xfffe {
                if chunk_len < 40
                    || u16::from_le_bytes(
                        bytes[chunk_start + 16..chunk_start + 18].try_into().ok()?,
                    ) < 22
                {
                    return None;
                }
                let guid = bytes.get(chunk_start + 24..chunk_start + 40)?;
                let guid_suffix = [
                    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b,
                    0x71,
                ];
                if guid.get(2..)? != guid_suffix {
                    return None;
                }
                u16::from_le_bytes(guid[0..2].try_into().ok()?)
            } else {
                format_tag
            };
            let valid_encoding = matches!(
                (resolved_tag, bits_per_sample),
                (1, 8 | 16 | 24 | 32) | (3, 32 | 64)
            );
            let bytes_per_sample = u32::from(bits_per_sample).div_ceil(8);
            let expected_block_align = u32::from(channels).checked_mul(bytes_per_sample)?;
            let expected_byte_rate = sample_rate.checked_mul(expected_block_align)?;
            if !valid_encoding
                || channels == 0
                || channels > 8
                || sample_rate == 0
                || expected_block_align != u32::from(block_align)
                || expected_byte_rate != byte_rate
            {
                return None;
            }
            format = Some((
                sample_rate,
                u8::try_from(channels).ok()?,
                u64::from(block_align),
            ));
        } else if chunk_id == b"data" && data_len.replace(u64::try_from(chunk_len).ok()?).is_some()
        {
            return None;
        }
        offset = padded_end;
    }

    let (sample_rate, channels, block_align) = format?;
    let data_len = data_len?;
    if data_len == 0 || data_len % block_align != 0 {
        return None;
    }
    let frames = data_len / block_align;
    let max_frames = u64::from(sample_rate).checked_mul(MAX_TRANSCRIPTION_AUDIO_SECONDS)?;
    if frames == 0 || frames > max_frames {
        return None;
    }
    Some(ValidatedAudioMetadata {
        format: ValidatedAudioFormat::Wav,
        sample_rate,
        channels,
        frames,
        duration_seconds_ceil: frames.div_ceil(u64::from(sample_rate)).max(1),
    })
}

pub fn validated_flac_audio_metadata(bytes: &[u8]) -> Option<ValidatedAudioMetadata> {
    if bytes.len() < 42 || bytes.get(..4)? != b"fLaC" {
        return None;
    }
    let mut reader = claxon::FlacReader::new(io::Cursor::new(bytes)).ok()?;
    let streaminfo = reader.streaminfo();
    let sample_rate = u64::from(streaminfo.sample_rate);
    let channels = u64::from(streaminfo.channels);
    if sample_rate == 0 || channels == 0 || channels > 8 {
        return None;
    }
    let max_samples = sample_rate
        .checked_mul(channels)?
        .checked_mul(MAX_TRANSCRIPTION_AUDIO_SECONDS)?;
    let mut decoded_samples = 0_u64;
    for sample in reader.samples() {
        sample.ok()?;
        decoded_samples = decoded_samples.checked_add(1)?;
        if decoded_samples > max_samples {
            return None;
        }
    }
    if decoded_samples == 0 || decoded_samples % channels != 0 {
        return None;
    }
    let frames = decoded_samples / channels;
    Some(ValidatedAudioMetadata {
        format: ValidatedAudioFormat::Flac,
        sample_rate: u32::try_from(sample_rate).ok()?,
        channels: u8::try_from(channels).ok()?,
        frames,
        duration_seconds_ceil: frames.div_ceil(sample_rate).max(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm16_wav(sample_rate: u32, channels: u16, frames: u32) -> Vec<u8> {
        let block_align = channels * 2;
        let byte_rate = sample_rate * u32::from(block_align);
        let data_len = frames * u32::from(block_align);
        let mut wav = Vec::with_capacity(44 + data_len as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.resize(44 + data_len as usize, 0);
        wav
    }

    #[test]
    fn wav_metadata_uses_validated_frame_geometry() {
        let wav = pcm16_wav(16_000, 2, 32_001);
        assert_eq!(
            validated_wav_audio_metadata(&wav),
            Some(ValidatedAudioMetadata {
                format: ValidatedAudioFormat::Wav,
                sample_rate: 16_000,
                channels: 2,
                frames: 32_001,
                duration_seconds_ceil: 3,
            })
        );
    }

    #[test]
    fn wav_metadata_rejects_forged_rate_and_trailing_bytes() {
        let mut forged_rate = pcm16_wav(16_000, 1, 16_000);
        forged_rate[28..32].copy_from_slice(&1_u32.to_le_bytes());
        assert_eq!(validated_wav_audio_metadata(&forged_rate), None);

        let mut trailing = pcm16_wav(16_000, 1, 16_000);
        trailing.push(0);
        assert_eq!(validated_wav_audio_metadata(&trailing), None);
    }

    #[test]
    fn packet_audio_metadata_validates_supported_music_formats() {
        for (bytes, format) in [
            (
                include_bytes!("../tests/fixtures/audio/tone-1.25s.mp3").as_slice(),
                ValidatedAudioFormat::Mp3,
            ),
            (
                include_bytes!("../tests/fixtures/audio/tone-1.25s.opus").as_slice(),
                ValidatedAudioFormat::Opus,
            ),
            (
                include_bytes!("../tests/fixtures/audio/tone-1.25s.m4a").as_slice(),
                ValidatedAudioFormat::Aac,
            ),
        ] {
            let metadata = validated_audio_metadata(bytes).expect("valid encoded audio");
            assert_eq!(metadata.format, format);
            assert_eq!(metadata.sample_rate, 48_000);
            assert_eq!(metadata.channels, 2);
            assert_eq!(metadata.duration_seconds_ceil, 2);
        }
    }

    #[test]
    fn packet_audio_metadata_rejects_corruption_and_garbage() {
        let mp3 = include_bytes!("../tests/fixtures/audio/tone-1.25s.mp3");
        let corrupt = vec![0_u8; mp3.len()];
        assert!(validated_audio_metadata(&corrupt).is_none());
        assert!(validated_audio_metadata(b"not audio").is_none());
    }
}
