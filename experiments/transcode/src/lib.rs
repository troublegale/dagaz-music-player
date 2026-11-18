use dagaz_protocol::Quality;
use lofty::{
    config::WriteOptions,
    file::{AudioFile, FileType, TaggedFileExt},
    tag::{TagExt, TagType},
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

type Error = Box<dyn std::error::Error>;
static FFMPEG_GATE: Mutex<()> = Mutex::new(());

pub struct Output {
    pub quality: Quality,
    pub path: PathBuf,
}

pub fn audio_bitrate(input: &Path) -> Result<u32, Error> {
    let input = std::path::absolute(input)?;
    let result = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--network",
            "none",
            "--entrypoint",
            "ffprobe",
            "-v",
        ])
        .arg(format!(
            "{}:/input:ro",
            input.parent().ok_or("input parent required")?.display()
        ))
        .args([
            "dagaz-audio-tools:validation",
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=bit_rate",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(format!(
            "/input/{}",
            input
                .file_name()
                .ok_or("input filename required")?
                .to_str()
                .ok_or("UTF-8 filename required")?
        ))
        .output()?;
    if !result.status.success() {
        return Err(format!("FFprobe: {}", String::from_utf8_lossy(&result.stderr)).into());
    }
    Ok(std::str::from_utf8(&result.stdout)?
        .trim()
        .parse::<u32>()?
        .div_ceil(1000))
}

/// Probe only: originals are mounted read-only; output must be a new file.
pub fn transcode(input: &Path, output: &Path, quality: Quality) -> Result<Output, Error> {
    let audio = lofty::read_from_path(input)?;
    if !matches!(
        audio.file_type(),
        FileType::Mpeg | FileType::Wav | FileType::Flac
    ) {
        return Err("unsupported source format".into());
    }
    let bitrate = match quality {
        Quality::Original => None,
        Quality::High => Some(320),
        Quality::Low => Some(128),
    };
    let reuse = bitrate.is_none()
        || (audio.file_type() == FileType::Mpeg && audio_bitrate(input)? <= bitrate.unwrap());
    if reuse {
        return Ok(Output {
            quality: Quality::Original,
            path: input.to_owned(),
        });
    }
    if output.exists() {
        return Err("output already exists".into());
    }
    let input = std::path::absolute(input)?;
    let parent = output.parent().ok_or("output parent required")?.to_owned();
    let parent = std::path::absolute(parent)?;
    let temporary = tempfile::tempdir_in(&parent)?;
    let encoded = temporary.path().join("encoded.mp3");
    let tag = audio
        .tag(TagType::Id3v2)
        .or_else(|| audio.primary_tag())
        .cloned();
    let _guard = FFMPEG_GATE.lock().map_err(|_| "transcode lock poisoned")?;
    let result = Command::new("docker")
        .args(["run", "--rm", "--network", "none", "-v"])
        .arg(format!(
            "{}:/input:ro",
            input.parent().ok_or("input parent required")?.display()
        ))
        .arg("-v")
        .arg(format!("{}:/output", temporary.path().display()))
        .args(["dagaz-audio-tools:validation", "-v", "error", "-n", "-i"])
        .arg(format!(
            "/input/{}",
            input
                .file_name()
                .ok_or("input filename required")?
                .to_str()
                .ok_or("UTF-8 filename required")?
        ))
        .args([
            "-map",
            "0:a:0",
            "-map_metadata",
            "-1",
            "-c:a",
            "libmp3lame",
            "-b:a",
        ])
        .arg(format!("{}k", bitrate.unwrap()))
        .arg("/output/encoded.mp3")
        .output()?;
    if !result.status.success() {
        return Err(format!("FFmpeg: {}", String::from_utf8_lossy(&result.stderr)).into());
    }
    if let Some(mut tag) = tag {
        tag.re_map(TagType::Id3v2);
        tag.save_to_path(&encoded, WriteOptions::default())?;
    }
    let checked = lofty::read_from_path(&encoded)?;
    if checked.file_type() != FileType::Mpeg
        || checked
            .properties()
            .duration()
            .abs_diff(audio.properties().duration())
            .as_millis()
            > 500
    {
        return Err("transcoded audio failed duration/format validation".into());
    }
    // Same filesystem: publish without overwriting a destination created during encoding.
    std::fs::hard_link(&encoded, output)?;
    Ok(Output {
        quality,
        path: output.to_owned(),
    })
}
