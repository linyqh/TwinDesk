// Compiles schema/ with protox (pure Rust), so building needs no `protoc`.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schema_root = "../../schema";
    let proto = "../../schema/osm/v1/osm.proto";
    println!("cargo:rerun-if-changed={proto}");

    let fds = protox::compile([proto], [schema_root])?;
    prost_build::Config::new().compile_fds(fds)?;
    Ok(())
}
