use dagaz_protocol::Quality;
use dagaz_transcode_probe::transcode;
use lofty::{
    file::{AudioFile, TaggedFileExt},
    tag::Accessor,
};
use std::path::Path;

#[test]
fn profiles_preserve_duration_and_tags() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let cover = std::fs::read(fixtures.join("artwork/cover.png")).unwrap();
    for name in ["pcm.wav", "float.wav", "tone.flac", "vbr.mp3"] {
        let working = tempfile::tempdir().unwrap();
        let source = working.path().join(name);
        std::fs::copy(fixtures.join("audio").join(name), &source).unwrap();
        dagaz_metadata_probe::retag(&source, &cover).unwrap();
        let before = std::fs::read(&source).unwrap();
        let input = lofty::read_from_path(&source).unwrap();
        for (quality, bitrate) in [(Quality::High, 320), (Quality::Low, 128)] {
            let output = working.path().join(format!("{bitrate}.mp3"));
            let result = transcode(&source, &output, quality).unwrap();
            if name == "vbr.mp3" {
                assert_eq!(result.quality, Quality::Original);
                assert_eq!(result.path, source);
                assert!(!output.exists());
                continue;
            }
            assert_eq!(result.quality, quality);
            let encoded = lofty::read_from_path(&result.path).unwrap();
            let tag = encoded.primary_tag().unwrap();
            assert_eq!(tag.artist().as_deref(), Some("Тестовый исполнитель"));
            assert_eq!(tag.album().as_deref(), Some("Тестовый альбом"));
            assert_eq!(tag.pictures()[0].data(), cover);
            assert!(
                encoded
                    .properties()
                    .duration()
                    .abs_diff(input.properties().duration())
                    .as_millis()
                    <= 500
            );
            assert_eq!(
                dagaz_transcode_probe::audio_bitrate(&result.path).unwrap(),
                bitrate
            );
            let reused = working.path().join(format!("reuse-{bitrate}.mp3"));
            assert_eq!(
                transcode(&output, &reused, quality).unwrap().quality,
                Quality::Original
            );
            assert!(!reused.exists(), "equal-bitrate MP3 must not be re-encoded");
            if quality == Quality::High {
                let low = working.path().join("from-high.mp3");
                assert_eq!(
                    transcode(&output, &low, Quality::Low).unwrap().quality,
                    Quality::Low
                );
            }
        }
        assert_eq!(std::fs::read(&source).unwrap(), before, "original changed");
    }
}

#[test]
fn original_is_reused_and_existing_output_is_never_overwritten() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/audio/pcm.wav");
    let working = tempfile::tempdir().unwrap();
    let output = working.path().join("existing.mp3");
    std::fs::write(&output, b"keep me").unwrap();
    assert_eq!(
        transcode(&source, &output, Quality::Original)
            .unwrap()
            .quality,
        Quality::Original
    );
    assert!(transcode(&source, &output, Quality::High).is_err());
    assert_eq!(std::fs::read(&output).unwrap(), b"keep me");
    let broken = working.path().join("broken.wav");
    std::fs::write(&broken, b"broken").unwrap();
    let new_output = working.path().join("new.mp3");
    assert!(transcode(&broken, &new_output, Quality::High).is_err());
    assert!(!new_output.exists());
}
