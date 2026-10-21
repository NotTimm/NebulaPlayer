// Lets the linker find libmpv where it isn't installed system-wide
// (Windows: the folder holding mpv.lib, made from mpv.def; see
// docs/DEVELOPING.md).
fn main() {
    println!("cargo:rerun-if-env-changed=MPV_LIB_DIR");
    if let Ok(dir) = std::env::var("MPV_LIB_DIR") {
        println!("cargo:rustc-link-search=native={dir}");
    }
}
