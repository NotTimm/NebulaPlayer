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
- Claude's GitHub token may lack the `workflow` scope: if a push touching
  `.github/workflows/` is refused, park the file in `packaging/pending/`
  and ask the owner to paste it in on github.com.
- The owner's editor is Neovim (use `nvim` in instructions); their Linux
  shell is fish (`set -x VAR value`).
- Flathub's AI policy applies here as for Nebula Reader: Claude doesn't
  prepare or touch a Flathub submission.

## Scope

The owner's priorities: playback and the browsing pages Jellyfin's web
client has, rebuilt natively (home with featured banner, libraries,
details, seasons/episodes, search). No admin/dashboard pages. Local
files are play-only, without scraping; metadata comes from Jellyfin.

## Where things stand

Last updated 2026-10-21.

- **Pending for the owner:** `packaging/pending/build.yml` goes in as
  `.github/workflows/build.yml` (pasted on github.com; this session's
  token can't push workflows). Once it's in, delete the pending copy.
- Testing: `tools/mock-jellyfin.py` (also `--legacy` for 10.8 routes).
  Seen working in Xvfb against it: sign-in (Quick Connect), home, library
  grid, show/movie pages, search, settings, playback with reports,
  autoplay of the next episode, stop on closing mpv, .deb install.
  Not yet seen on a real Jellyfin server, on Wayland or on Windows.

- First alpha code: sign-in (Quick Connect + password), home (featured
  banner, My Media, Continue Watching, Next Up, Recently Added per
  library), library grids (sort, paging), details (movie/show/episode,
  seasons, episodes, cast, similar, watched/favourite), search, local
  folders, settings drawer, Jellyfin theme.
- Playback: mpv in its own window (libmpv client API, `src/mpv.rs`),
  direct play only, Jellyfin's default audio/subtitle tracks mapped to
  mpv ids, external subs added, progress reports, autoplay next episode.
- libmpv is loaded at run time (libloading), not linked; the Windows zip
  ships `libmpv-2.dll`. A file given on the command line plays only once
  the window has opened (starting mpv's renderer while the app's was
  starting crashed Mesa).
- Not yet: embedded video (see docs/DEVELOPING.md "Embedding"), an
  in-app track picker, transcoding fallback, a Windows installer, macOS
  builds, Fedora COPR.
