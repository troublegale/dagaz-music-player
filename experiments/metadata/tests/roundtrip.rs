use lofty::{
    file::TaggedFileExt,
    tag::{Accessor, TagType},
};
use std::{path::Path, process::Command};

fn pcm(dir: &Path, name: &str) -> Vec<u8> {
    let output = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-v",
            &format!("{}:/data:ro", dir.display()),
            "dagaz-audio-tools:validation",
            "-v",
            "error",
            "-i",
            &format!("/data/{name}"),
            "-f",
            "f64le",
            "-acodec",
            "pcm_f64le",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty());
    output.stdout
}

#[test]
fn tag_roundtrip_preserves_audio_and_unmodified_fields() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .canonicalize()
        .unwrap();
    let cover = std::fs::read(root.join("artwork/cover.png")).unwrap();
    let large_cover = std::fs::read(root.join("artwork/large-cover.png")).unwrap();
    for name in [
        "pcm.wav",
        "float.wav",
        "untagged.wav",
        "unknown-chunk.wav",
        "vbr.mp3",
        "tone.flac",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(name);
        std::fs::copy(root.join("audio").join(name), &path).unwrap();
        let before = lofty::read_from_path(&path).unwrap();
        let title = before
            .primary_tag()
            .and_then(|t| t.title())
            .map(|s| s.into_owned());
        let comment = before
            .primary_tag()
            .and_then(|t| t.comment())
            .map(|s| s.into_owned());
        let audio = pcm(temp.path(), name);
        for picture in [&cover, &large_cover] {
            dagaz_metadata_probe::retag(&path, picture).unwrap();
            let after = lofty::read_from_path(&path).unwrap();
            let tag_type = if name.ends_with(".flac") {
                TagType::VorbisComments
            } else {
                TagType::Id3v2
            };
            let tag = after.tag(tag_type).unwrap();
            assert_eq!(
                tag.artist().as_deref(),
                Some("Тестовый исполнитель"),
                "{name}"
            );
            assert_eq!(tag.album().as_deref(), Some("Тестовый альбом"), "{name}");
            assert_eq!(
                tag.pictures().first().unwrap().data(),
                picture.as_slice(),
                "{name}"
            );
            assert_eq!(
                after
                    .primary_tag()
                    .and_then(|t| t.title())
                    .map(|s| s.into_owned()),
                title,
                "{name}"
            );
            assert_eq!(
                after
                    .primary_tag()
                    .and_then(|t| t.comment())
                    .map(|s| s.into_owned()),
                comment,
                "{name}"
            );
            assert_eq!(pcm(temp.path(), name), audio, "audio changed: {name}");
            if name == "unknown-chunk.wav" {
                assert!(
                    std::fs::read(&path)
                        .unwrap()
                        .windows(9)
                        .any(|chunk| chunk == b"DoNotLose")
                );
            }
        }
    }
}

#[test]
fn damaged_audio_is_rejected_without_modifying_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("broken.wav");
    std::fs::write(&path, b"RIFFbroken").unwrap();
    let cover = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/artwork/cover.png"),
    )
    .unwrap();
    assert!(dagaz_metadata_probe::retag(&path, &cover).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"RIFFbroken");
}

#[test]
fn invalid_cover_is_rejected_without_touching_valid_audio() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("valid.flac");
    let bytes =
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/audio/tone.flac"))
            .unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(dagaz_metadata_probe::retag(&path, b"invalid picture").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}
