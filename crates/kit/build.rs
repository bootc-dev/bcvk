//! Build script: copy cross-compiled NBD server binary into OUT_DIR for include_bytes!.

fn main() {
    // NBD server binary is needed on non-Linux hosts (macOS/Windows) where
    // bcvk cross-compiles it for the podman machine. On Linux, the file
    // won't exist and that's OK — nbd_macos.rs is cfg-gated.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = std::path::Path::new(&manifest_dir).join("../..");
    let nbd_src = workspace_root.join("target/nbd-server/bcvk-nbd");
    println!("cargo:rerun-if-changed={}", nbd_src.display());
    if nbd_src.exists() {
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let dest = std::path::Path::new(&out_dir).join("bcvk-nbd");
        std::fs::copy(&nbd_src, &dest).expect("failed to copy bcvk-nbd to OUT_DIR");
    }
}
