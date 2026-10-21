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
```

## How it's put together

- `src/mpv.rs`: the `Player`, over one of two backends. On Linux and
  Windows it's libmpv in-process: a small binding to the client API (only
  the calls needed, present since libmpv 1.x, loaded at run time). On
  macOS libmpv can't open its own window, because Cocoa wants the main
  thread, which the app owns. So there the app runs the mpv program and
  drives it over `--input-ipc-server` (JSON lines). Both take the same
  commands and options; `NEBULA_MPV=ipc` uses the IPC backend anywhere,
  for testing, and `NEBULA_MPV_BIN` picks the mpv program. mpv owns its own
  window and on-screen controller; an event thread keeps a `State`
  that the UI polls twice a second.
- `src/jellyfin.rs`: the API client (sign-in, home sections, browsing,
  images, PlaybackInfo, playback reports).
- `src/images.rs`: images are fetched at display size, cached on disk
  (keyed by the server's image tag) and decoded off the UI thread.
- `src/app.rs`: state, messages and navigation. The pages live in
  `src/app/` (`home`, `library`, `details`, `search`, `local`, `signin`,
  `settings_view`, with shared cards in `widgets`).
- `src/theme.rs`: the Jellyfin colours and gradients.

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

### Embedding (later)

Upstream libmpv's render API only does OpenGL and software rendering,
while libcosmic draws with wgpu. So for now mpv keeps its own window. The
planned order is `--wid` embedding on Windows/X11, then a Wayland
subsurface fed by mpv's OpenGL output through dmabuf.

## The libcosmic fork

Nebula Player uses the owner's fork `NotTimm/libcosmic` at the same
commit as Nebula Reader. When Nebula Reader moves its pin after syncing
with upstream, move this one to match.

## Releases

Versions stay pre-release (`0.x.y-alpha.N`, then `-beta.N`) until the
owner says otherwise, and GitHub releases are marked as pre-releases.
Every release gets user-facing notes in
`packaging/release-notes/<version>.md`.
