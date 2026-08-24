fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schema = "proto/vibemux_plugin_v1.proto";
    println!("cargo:rerun-if-changed={schema}");
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config.compile_protos(&[schema], &["proto"])?;
    Ok(())
}
