use prost::Message;

/// Pure-Rust proto compilation: protox parses/compiles the .proto (no protoc on
/// PATH), we persist the FileDescriptorSet to OUT_DIR (main.rs embeds it for the
/// gRPC Server Reflection service), and tonic-prost-build generates client/server
/// code straight from the descriptor set via compile_fds.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/inventory.proto");

    let file_descriptor_set = protox::compile(["proto/inventory.proto"], ["proto"])?;

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(
        out_dir.join("inventory_descriptor.bin"),
        file_descriptor_set.encode_to_vec(),
    )?;

    tonic_prost_build::configure().compile_fds(file_descriptor_set)?;

    Ok(())
}
