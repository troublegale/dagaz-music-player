fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let file = args
        .next()
        .ok_or("usage: metadata-probe WORKING_COPY COVER_PNG")?;
    let cover = args
        .next()
        .ok_or("usage: metadata-probe WORKING_COPY COVER_PNG")?;
    dagaz_metadata_probe::retag(std::path::Path::new(&file), &std::fs::read(cover)?)
}
