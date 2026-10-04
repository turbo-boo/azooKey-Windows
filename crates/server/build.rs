use std::env;

fn main() {
    let project_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    // The Swift FFI import library is only required by the executable.
    // Keeping it off the library target allows pure Rust unit tests to link
    // without requiring the Swift build artifact.
    println!("cargo:rustc-link-search=native={}/target/", project_dir);
    println!("cargo:rustc-link-arg-bin=azookey-server=azookey-server.lib");
}
