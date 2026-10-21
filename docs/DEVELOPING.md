# Developing Nebula Player

## Building

Linux build dependencies: Rust (stable), `pkg-config` and the development
packages for libmpv, libxkbcommon, Wayland and OpenSSL.

```fish
# Fedora (mpv is in Fedora itself)
sudo dnf install mpv-libs-devel libxkbcommon-devel wayland-devel openssl-devel pkgconf-pkg-config
# Ubuntu/Debian
sudo apt install libmpv-dev libxkbcommon-dev libwayland-dev libssl-dev pkg-config
```

```fish
cargo build --release
./target/release/nebula-player              # the app
./target/release/nebula-player movie.mkv    # play a file straight away
```

On a laptop, build gently: `nice -n 19 cargo build -j 4`.

libmpv isn't linked: the app loads it when it starts (`src/mpv.rs`), so
it builds without libmpv and, without it, opens and says what to install.
To play anything it needs `libmpv.so.2` (Linux), `libmpv-2.dll` next to
the exe (Windows; from the `mpv-dev-x86_64-*.7z` builds at
https://sourceforge.net/projects/mpv-player-windows/files/libmpv/) or
`brew install mpv` (macOS).

The app is a Wayland client on Wayland desktops (COSMIC, KDE, GNOME) and
uses X11 only in X11 sessions; mpv picks the same.

## Testing without a server

`tools/mock-jellyfin.py` is a fake Jellyfin server with a couple of
movies and a show. It serves generated images and, when `ffmpeg` is
installed, a short test video for every item.

```fish
python3 tools/mock-jellyfin.py 8096
# then connect the app to localhost:8096, user "demo", any password
python3 tools/mock-jellyfin.py 8096 --many 400   # a big movie library
```

## How it's put together

- `src/mpv.rs`: the `Player`, over one of two backends. On Linux and
  Windows it's libmpv in-process: a small binding to the client API (only
  the calls needed, present since libmpv 1.x, loaded at run time). On
  macOS libmpv can't open its own window, because Cocoa wants the main
  thread, which the app owns. So there the app runs the mpv program and
  drives it over `--input-ipc-server` (JSON lines). Both take the same
  commands and options; `NEBULA_MPV=ipc` uses the IPC backend anywhere,
  for testing, and `NEBULA_MPV_BIN` picks the mpv program. An event
  thread keeps a `State` that the UI polls.
- `src/video.rs`: video inside the app's window (see "Embedding").
- `src/input.rs`: browser-like scrolling around every page (wheel steps
  glide, touchpad swipes keep going after the fingers lift) and the
  click-anywhere that closes the sidebar.
- `src/jellyfin.rs`: the API client (sign-in, home sections, browsing,
  images, PlaybackInfo, playback reports).
- `src/images.rs`: images are fetched at display size, cached on disk
  (keyed by the server's image tag) and decoded off the UI thread.
- `src/app.rs`: state, messages and navigation. The pages live in
  `src/app/` (`home`, `library`, `details`, `search`, `local`, `signin`,
  `settings_view`, `player`, with shared cards in `widgets`). A library
  builds only the grid rows on screen and fetches pages wherever the user
  scrolls or jumps (A–Z bar, `nameLessThan`).
- `src/theme.rs`: the Jellyfin colours and gradients. The accent is the
  desktop's (COSMIC's config at start, then the XDG settings portal,
  which KDE and GNOME 47+ also answer), unless set to Jellyfin blue.

### Playback and progress

`Message::Play` asks the server for PlaybackInfo, then loads
`/Videos/{id}/stream?static=true` into mpv, sending the token in an
`X-Emby-Token` header. That header has no commas, which matters because
mpv splits its header list on commas. Nothing is transcoded. The app
reports `/Sessions/Playing` once the file loads, `/Progress` every 10 s
and on pause, and `/Stopped` with the last position. Jellyfin marks an
item watched past 90 %. At the end of an episode the next one loads into
the same mpv window.

### Intros and credits

`jellyfin::segments` reads Jellyfin's media segments (`/MediaSegments/{id}`,
10.10+, where the Intro Skipper plugin puts what it finds). On a 404 it
reads the older plugin's own `/Episode/{id}/IntroSkipperSegments`. During
playback `skip_segments` (src/app.rs) shows mpv's OSD prompt and enables
an input section binding S/Enter to `script-message nebula-skip`. That
arrives as a CLIENT_MESSAGE event and counts in `State::skip_presses`.
The section is disabled again once the segment is over, so S takes
screenshots otherwise. The mock server has an intro (2–8 s) and credits
(15–20 s) on every episode.

### Embedding

Video plays inside the app's window (unless "Play in mpv's own window" is
on, or on macOS). libmpv's render API only does OpenGL and software
rendering, while libcosmic draws with wgpu, so `src/video.rs` has a
render thread with an off-screen EGL context (Mesa's surfaceless
platform, else an EGL device for NVIDIA, else the default display): mpv
draws each frame at the size it's shown into a texture, the thread reads
it back, and a wgpu shader widget uploads and draws it. Without EGL
(Windows, or a failure) mpv's software renderer fills the buffer instead.
mpv gets `vo=libmpv`, `hwdec=auto-copy-safe` (there's no display to share
GPU frames through), no OSC and no key bindings: `src/app/player.rs`
draws the controls and handles jellyfin-web's keys.

The cost is a copy per frame (8 MB at 1080p; frames are capped at 2560
px across). Handing the texture over as a dmabuf (or a Wayland
subsurface, as libcosmic's `sctk_subsurface` example does) would remove
it; worth it only if the copy shows up.

`NEBULA_MPV=ipc` still runs the mpv program in its own window.

## The libcosmic fork

Nebula Player uses the owner's fork `NotTimm/libcosmic` at the same
commit as Nebula Reader. When Nebula Reader moves its pin after syncing
with upstream, move this one to match.

## Packaging

Each package has a script in `packaging/`, and `.github/workflows/build.yml`
runs them on every push to `main` (the files are kept as the run's
artifacts for 14 days):

| Package | Script | Notes |
|---|---|---|
| `.deb` | `build-deb.sh` | Ubuntu 24.04; depends on `libmpv2 \| libmpv1` |
| `.rpm` | `build-rpm.sh` | built in a Fedora container; requires `mpv-libs` |
| AppImage | `build-appimage.sh` | bundles libmpv and what it links |
| `.flatpak` | `build-flatpak.sh` | GNOME runtime; builds libmpv (Celluloid's recipe) |
| Windows `.zip` and setup `.exe` | `build-windows.ps1` | Inno Setup; ships `libmpv-2.dll` |
| macOS `.dmg` | `build-macos.sh` | Homebrew's mpv program and its libraries inside the .app |
| Fedora/COPR | `fedora/nebula-player.spec`, `fedora/make-vendor.sh` | offline build from the release tarballs |

`packaging/build-all.sh` builds whichever of these the machine has tools
for. `packaging/make-icons.sh` regenerates every icon from
`res/icons/source/nebula-player.svg`. Every job then installs or runs what
it built and checks `nebula-player --check-mpv`, which prints which mpv the
app found, or fails.

On the laptop, build gently:
`set -x CARGO_BUILD_JOBS 4; nice -n 19 bash packaging/build-rpm.sh`.

### Fedora and COPR

`packaging/fedora/nebula-player.spec` builds offline from the release's
source tarball and its vendor tarball (`nebula-player-<version>-vendor.tar.xz`,
every crate including the libcosmic fork's git ones), with Fedora's Rust
macros. The `fedora` job builds it the way COPR does, on every push. Keep
its `Version` in step with Cargo.toml, `~` for Cargo's `-`; the workflow
checks.

COPR project: `timbo-slice/nebula-player` (chroots Fedora 43, 44, 45 and
rawhide, x86_64). Install with `sudo dnf copr enable
timbo-slice/nebula-player` then `sudo dnf install nebula-player`. Its
package `nebula-player` builds from SCM (clone URL
`https://github.com/NotTimm/NebulaPlayer`, spec
`packaging/fedora/nebula-player.spec`, build method **make srpm**, which
runs `.copr/Makefile` to download the release's tarballs), with
auto-rebuild off: a build only works once the release exists.

The workflow's `copr` job starts that build after each publish, using the
`COPR_CONFIG` secret (the whole config block from
copr.fedorainfracloud.org/api; the token expires every 180 days, so
renew it there and update the secret). The project comes from the
`COPR_PROJECT` repository variable (Settings → Secrets and variables →
Actions → Variables), `timbo-slice/nebula-player` when unset. Without
the secret the job only warns; start the build by hand (the package's
**Rebuild** button).

### Signing

Every published file gets a minisign signature (`<file>.minisig`) from the
repository secrets `MINISIGN_KEY` (the whole encrypted secret key file) and
`MINISIGN_PASSWORD`, the same key as Nebula Reader's (ID AFE5BA3AD81D4B31;
public key in `packaging/minisign.pub`). The `sign` job checks every
signature against that public key. Without the secrets, builds and
releases go out unsigned, with a warning.

## Releases

Versions stay pre-release (`0.x.y-alpha.N`, then `-beta.N`) until the
owner says otherwise. The workflow refuses to publish anything else.
To release:

1. Bump `version` in Cargo.toml and `Version` in the Fedora spec (with
   `~`), add a `%changelog` entry, and write
   `packaging/release-notes/<version>.md`.
2. Push, then run **Actions → Build → Run workflow** with **publish**
   ticked. It creates the GitHub pre-release `v<version>` with every
   package (and signatures, when the secrets are set), then starts the
   COPR build (or, without `COPR_CONFIG`, click **Rebuild** on COPR).
