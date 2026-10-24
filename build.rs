//! Embeds the app icon and version details (Properties → Details) into the
//! Windows .exe. Linux packages ship the icon through the desktop file and
//! icon theme. Also records the git commit, shown in Settings → About.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    record_commit();
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

/// Sets `NEBULA_COMMIT` for `env!`/`option_env!`: from the environment if a
/// packaging script sets it, else from git, else empty (e.g. a source
/// tarball or an offline build without the .git folder).
fn record_commit() {
    println!("cargo:rerun-if-env-changed=NEBULA_COMMIT");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    let commit = std::env::var("NEBULA_COMMIT").ok().filter(|c| !c.is_empty()).or_else(|| {
        let out = std::process::Command::new("git").args(["rev-parse", "--short", "HEAD"]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    });
    println!("cargo:rustc-env=NEBULA_COMMIT={}", commit.unwrap_or_default());
}
