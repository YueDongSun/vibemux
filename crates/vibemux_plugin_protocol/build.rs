fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schema = "proto/vibemux_plugin_v1.proto";
    println!("cargo:rerun-if-changed={schema}");
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let mut config = prost_build::Config::new();
    for name in ["TerminalPane", "TerminalInventory"] {
        config.type_attribute(name, "#[derive(serde::Serialize, serde::Deserialize)]");
    }
    config.protoc_executable(protoc);
    config.compile_protos(&[schema], &["proto"])?;
    Ok(())
}
