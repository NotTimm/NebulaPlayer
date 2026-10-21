//! Embeds the app icon and version details (Properties → Details) into the
//! Windows .exe. Linux packages ship the icon through the desktop file and
//! icon theme.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=res/icons/io.github.NotTimm.NebulaPlayer.ico");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("res/icons/io.github.NotTimm.NebulaPlayer.ico")
            .set("ProductName", "Nebula Player")
            .set("FileDescription", "Nebula Player")
            .set("CompanyName", "Tim Singel")
            .set("LegalCopyright", "Copyright © Tim Singel. Licensed under GPL-3.0-only.")
            .set("OriginalFilename", "nebula-player.exe")
            .set("InternalName", "nebula-player");
        res.compile().expect("failed to embed Windows resources (icon, version info)");
    }
}
