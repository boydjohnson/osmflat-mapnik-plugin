use std::path::PathBuf;

fn main() {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    // Generate the C++ header next to the crate so CMake can add
    // `rust/osmflat-capi/include` to its include path. This runs as part of
    // `cargo build`, which Corrosion drives before the C++ module compiles, so
    // the header is always in sync with the FFI in `lib.rs`.
    let out = PathBuf::from(&crate_dir).join("include/osmflat_capi.hpp");

    cbindgen::generate(&crate_dir)
        .expect("cbindgen failed to generate bindings")
        .write_to_file(&out);

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
