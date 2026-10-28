# Notes for Claude

**Nebula Player** ("Nebula Player" in launchers), app ID
`io.github.NotTimm.NebulaPlayer`, binary `nebula-player`, repo
`NotTimm/NebulaPlayer`. A native Jellyfin + local video player: Rust,
libcosmic (the owner's fork, same pin as Nebula Reader) and libmpv. Sister
app of Nebula Reader (`NotTimm/NebulaReader`), which it must not touch:
no book support here, and no shared crates for now (code is copied).

## Workflow

- **Commit as the owner, every time.** Before the first commit in any
  session run `git config user.name "Tim Singel"` and
  `git config user.email "gamer.tdogtim@gmail.com"` (cloud sessions default
  to a Claude identity), and check `git log -1 --format='%an <%ae>'` before
  pushing. Claude goes only in the `Co-Authored-By` trailer.
- Commit small, logical steps and push often; the owner follows progress
  on the repo page. Commits that build and pass `cargo test` go to `main`
  (fast-forward or merge, never squash). Keep the session branch in sync.
- **Releases stay alpha/beta**: versions like `0.1.0-alpha.2`, GitHub
  releases marked pre-release, until the owner says otherwise. Don't
  release unless asked. Every release gets user-facing notes in
  `packaging/release-notes/<version>.md`; give the owner the same notes
  in chat.
- On the owner's laptop `origin` is SSH with the owner's key, which can
  push `.github/workflows/` changes. Elsewhere Claude's GitHub token may
  lack the `workflow` scope: if such a push is refused, park the file in
  `packaging/pending/` and ask the owner to paste it in on github.com.
  Don't push to `main` while a publish run is going: pushes cancel the
  running build.
- The owner's editor is Neovim (use `nvim` in instructions); their Linux
  shell is fish (`set -x VAR value`).
- Flathub's AI policy applies here as for Nebula Reader: Claude doesn't
  prepare or touch a Flathub submission.

## Scope

The owner's priorities: playback and the browsing pages Jellyfin's web
client has, rebuilt natively (home with featured banner, libraries,
details, seasons/episodes, search). Local files are play-only, without
scraping; metadata comes from Jellyfin.

Admin: only the Dashboard the owner asked for (2026-10-23), for admins:
Scan All Libraries, active sessions, scheduled tasks with Run/Stop. All
admin requests live in `src/admin.rs` and must stay that small: no
settings, users, libraries, plugins, schedules, restart or shutdown.
Scan and Run ask first.

## Where things stand

Last updated 2026-10-24.

- CI: `.github/workflows/build.yml` is the full workflow modelled on
  Nebula Reader's minus COPR and winget (deb, rpm, AppImage, flatpak,
  Fedora spec + vendor, Windows zip + setup, macOS dmg, minisign,
  publish). Every job calls a script in packaging/, so fixes there need no
  paste. First full run (2026-10-21): all green except Windows (libmpv-2.dll
  imports vulkan-1.dll, missing without a GPU driver: the Vulkan loader now
  ships in `vulkan\`, loaded only when Windows has none) and flatpak
  (1024px icon refused; left out). Run 39651963465 with both fixes: all
  green. Signing was skipped there (secrets not visible to it yet).
- Released v0.1.0-alpha.1 (2026-10-21, pre-release, every file signed
  with the reader's minisign key). Its GitHub text was published before
  the notes file's Windows line mentioned the `vulkan` folder.
- alpha.3 was never published (both publish runs cancelled while the
  .deb job crawled). Released v0.1.0-alpha.4 (2026-10-23) instead, with
  SyncPlay, the Windows title bar fixes, the Dashboard and no
  click-to-pause.
- alpha.3's .deb job once sat 20+ min in `apt-get install` on a slow
  Azure mirror (recommended packages: yt-dlp, a 27 MB pocketsphinx
  model). Transient: later runs take ~20 s. Hardening (no recommends in
  test installs, `timeout-minutes`) was offered and declined; it's in
  git history (fd415dc) if wanted.
- COPR: project `timbo-slice/nebula-player` (owner's account), package
  `nebula-player` from SCM + make srpm, chroots Fedora 43/44/45/rawhide
  x86_64, no auto-rebuild; first build 11184764 (alpha.1) started by
  hand. The workflow's `copr` job (secret COPR_CONFIG, project from
  the COPR_PROJECT variable, default timbo-slice/nebula-player) starts it
  after each publish (COPR_CONFIG is set: alpha.4's build started on its own). Plan (owner, "this weekend"): one
  `timbo-slice/nebula-apps` project holding both nebula-player and
  nebula-reader: here that's just setting COPR_PROJECT; the Reader's
  workflow `copr` job needs its project name changed (a workflow paste).
- Testing: `tools/mock-jellyfin.py` (also `--legacy` for 10.8 routes,
  `--many N` for a big library). The owner's laptop has no Xvfb package:
  `dnf download xorg-x11-server-Xvfb xdotool libxdo` and unpack them
  with rpm2cpio into the scratchpad; run the app with XDG_CONFIG_HOME
  etc. pointed there (never the owner's real config), and give the
  window focus with `xdotool windowfocus` or keys go nowhere (no WM).
  Seen working in Xvfb against it: sign-in (Quick Connect), home, library
  grid, show/movie pages, search, settings, playback with reports,
  autoplay of the next episode, stop on closing mpv, .deb install.
  Not yet seen on a real Jellyfin server, on Wayland or on Windows.

- First alpha code: sign-in (Quick Connect + password), home (featured
  banner, My Media, Continue Watching, Next Up, Recently Added per
  library), library grids (sort, paging), details (movie/show/episode,
  seasons, episodes, cast, similar, watched/favourite), search, local
  folders, settings drawer, Jellyfin theme.
- Playback: inside the app's window since 2026-10-21 (mpv render API,
  `src/video.rs`, controls in `src/app/player.rs` modelled on Jellyfin
  Media Player: seek bar, ±10/30 s, prev/next episode, volume, audio,
  subtitle and speed menus, full screen, Skip Intro button, auto-hiding,
  jellyfin-web keys). "Play in mpv's own window" setting keeps the old
  way. Direct play only, Jellyfin's default tracks mapped to mpv ids,
  external subs, progress reports, autoplay next episode.
  Trickplay previews and chapter names on the seek bar, fetched as
  jellyfin-web does (see docs/DEVELOPING.md "Trickplay"); tested against
  the mock only.
- 2026-10-21 (owner's requests from using the alpha on a laptop): smooth
  and kinetic scrolling (`src/input.rs`), desktop accent colour with a
  Jellyfin-blue override, sidebar closed by default and closed by any
  click on the page, library grid filling the width and built only where
  visible (big libraries stuttered), A–Z bar, library search.
- Intro Skipper: media segments (10.10+) or the old plugin endpoint;
  mpv OSD prompt "Skip Intro: press S or Enter" (setting: ask / auto /
  off). Tested against the mock in both server modes.
- macOS plays through the mpv program over JSON IPC (`mpv::ipc`), since
  libmpv can't own a window there; `NEBULA_MPV=ipc` tests it on Linux
  (seen working: prompt, skip, reports, next episode).
- libmpv is loaded at run time (libloading), not linked; the Windows zip
  ships `libmpv-2.dll`. A file given on the command line plays only once
  the window has opened (starting mpv's renderer while the app's was
  starting crashed Mesa).
- SyncPlay (2026-10-22): watch together the way jellyfin-web does it,
  see docs/DEVELOPING.md "SyncPlay". Tested with two copies of the app
  against the mock (`tools/mock-jellyfin.py` has a SyncPlay group
  server and WebSocket): create/join, start together, pause, seek, Skip
  Intro as a group seek, autoplay through a show's queue, a late joiner
  (both within ~±80 ms in Xvfb). Not yet tried against a real Jellyfin
  server or with jellyfin-web in the same group (demo.jellyfin.org is
  blocked from cloud sessions).
- Windows window (2026-10-22): native title bar only (libcosmic's window
  buttons hidden on Windows/macOS/KDE, as in Nebula Reader), opaque
  window there, the exe's icon set with WM_SETICON and the title bar's
  colours from the theme (`src/app/window_frame.rs`). The owner saw the
  page flicker while scrolling on Windows (against demo.jellyfin.org);
  it doesn't happen in Xvfb. The transparent window was the suspect
  (the Reader, opaque with a native frame, doesn't flicker); unconfirmed
  until the owner tries a build.
- Dashboard (2026-10-23, `src/admin.rs`, `src/app/dashboard.rs`): sidebar
  entry for administrators (`/Users/Me` policy), sessions with what's
  playing, progress, direct play/transcode and reasons, Scan All
  Libraries and scheduled tasks with Run (confirm in the row) and Stop;
  refreshes every 5 s (2 s while a task runs). Tested against the mock
  (`--not-admin` hides it); not yet against a real server.
- While a video plays in the window the title bar stays (setting "Show
  the title bar while playing", on by default; only offered where the
  app draws its own title bar, so COSMIC/GNOME): with it hidden there
  was no way to move or close the window without the Super key. Full
  screen hides it; the header has no page buttons while playing.
- A click on the video no longer pauses (owner's choice); only the pause
  button and keys do.
- 2026-10-24: pages reach the window's edges (libcosmic's content
  container off: it inset everything 7-8 px); scroll reports throttled in
  `src/input.rs` (each rebuilt the whole page every frame: the laptop's
  stutter); in-app updates ported from Nebula Reader (`src/updates.rs`,
  `src/updater.rs`, `src/app/update_view.rs`; pre-releases included, the
  COPR build doesn't check: NEBULA_DISTRO); "Play in mpv's own window"
  moved to an Experimental section. The owner's laptop had it switched on,
  which is why its video played outside the app.
- **Don't build on the owner's laptop** (it eats their CPU): push and let
  GitHub CI and COPR build and test.
- Dependabot (owner turned it on 2026-10-24; `.github/dependabot.yml`):
  weekly grouped PRs for crates and GitHub Actions, plus security fixes.
  CI runs on each PR; merge green ones with `gh pr merge N --merge`
  (never squash). libcosmic (pinned with the Reader) and ashpd minor/major
  are ignored. Its commits are dependabot's, not the owner's: that's fine.
- Not yet: transcoding fallback, zero-copy video
  (dmabuf). Embedded video seen working only in Xvfb (OpenGL through
  llvmpipe); not yet on the owner's GPU, Wayland, NVIDIA or Windows
  (software renderer there).
