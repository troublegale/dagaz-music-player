use dagaz_protocol::Quality;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: dagaz-transcode-probe INPUT OUTPUT original|high|low".into());
    }
    let quality = match args[2].as_str() {
        "original" => Quality::Original,
        "high" => Quality::High,
        "low" => Quality::Low,
        _ => return Err("quality must be original, high or low".into()),
    };
    let result =
        dagaz_transcode_probe::transcode(Path::new(&args[0]), Path::new(&args[1]), quality)?;
    println!("{:?}: {}", result.quality, result.path.display());
    Ok(())
}
