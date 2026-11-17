fn main() -> Result<(), Box<dyn std::error::Error>> {
    let destination = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "api/openapi.yaml".into());
    let mut output = serde_json::to_string_pretty(&dagaz_protocol::openapi_document())?;
    output.push('\n');
    // JSON is valid YAML; this avoids maintaining a second serialization dependency.
    std::fs::write(destination, output)?;
    Ok(())
}
