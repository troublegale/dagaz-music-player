use rodio::Source;
use std::{path::Path, time::Duration};

#[test]
fn encoded_profiles_decode_and_seek_without_excessive_padding_error() {
    for name in ["high.mp3", "low.mp3"] {
        let file = std::fs::File::open(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/audio/profiles")
                .join(name),
        )
        .unwrap();
        let mut decoder = rodio::Decoder::try_from(file).unwrap();
        assert!(
            decoder
                .total_duration()
                .unwrap()
                .abs_diff(Duration::from_secs(30))
                .as_millis()
                <= 500
        );
        let rate = u64::from(decoder.sample_rate().get()) * u64::from(decoder.channels().get());
        decoder.try_seek(Duration::from_secs(25)).unwrap();
        let remaining = decoder.count() as u64;
        assert!(
            remaining.abs_diff(5 * rate) <= rate / 2,
            "{name}: seek error exceeds 500ms"
        );
    }
}
