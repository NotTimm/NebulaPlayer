# Nebula Player

A fast, native video player for your Jellyfin server and your own files,
built with Rust, [libcosmic](https://github.com/pop-os/libcosmic) and
[mpv](https://mpv.io). No web view: every page is a native window, and mpv
does the decoding, so you get its hardware decoding, HDR, subtitle
rendering and audio passthrough.

> **Alpha.** This is an early build. Expect rough edges, and please
> report what breaks.

## What it does

- **Home** like Jellyfin's: a featured banner, My Media, Continue
  Watching, Next Up and Recently Added for each library.
- **Libraries** as poster grids, sortable by name, date added, release
  date or rating.
- **Movie and show pages** with the backdrop, details, cast, seasons,
  episodes and similar titles. You can mark things watched or favorite them.
- **Search** across movies, shows and episodes, and within a library,
  which also has an A–Z bar to jump through it.
- **Playback inside the app**, drawn by mpv, under controls like
  Jellyfin Media Player's: seek bar with trickplay thumbnail previews and
  chapters, back 10 s / forward 30 s, previous/next episode, volume,
  audio, subtitle and speed menus, full screen, and Jellyfin's keys. It
  resumes where you left off, picks your Jellyfin audio and subtitle
  preferences, loads external subtitles, plays the next episode, and
  reports progress back so other Jellyfin apps stay in sync.
- **Skip intros and credits** found by the
  [Intro Skipper](https://github.com/intro-skipper/intro-skipper) plugin
  (or anything else that fills in Jellyfin's media segments): a Skip
  Intro button (or S / Enter), or skipping by itself if you prefer.
  Recaps, previews and ads are covered too.
- **SyncPlay** (watch together), as in Jellyfin's web and desktop apps:
  create or join a group from the people button, and everyone in it
  plays, pauses, seeks and moves to the next episode together, with
  members of the same group on jellyfin-web, Jellyfin Media Player or
  phones.
- **Dashboard** for server administrators: who's watching what and on
  which device (direct play or transcoding, and why), Scan All
  Libraries, and the scheduled tasks with Run and Stop. Nothing else of
  the server's admin settings, on purpose.
- **Local files**: add folders and play anything in them. There's no
  scraping; metadata comes from Jellyfin only.
- Jellyfin's dark look with your desktop's accent colour (or Jellyfin's
  blue), or your desktop's COSMIC theme.

To use mpv's own window instead, with mpv's own controls and keys, turn
on "Play in mpv's own window" in the settings (on macOS it's always mpv's
window). Your own `mpv.conf` goes in the folder the settings open.

## Installing

Builds are on the [Releases](https://github.com/NotTimm/NebulaPlayer/releases)
page, marked as pre-releases while the app is in alpha/beta.

- **Fedora:** `sudo dnf install ./nebula-player-*.rpm` (pulls in `mpv-libs`).
- **Ubuntu / Debian:** `sudo apt install ./nebula-player_*.deb` (pulls in
  `libmpv2`).
- **Any Linux:** the `.flatpak`
  (`flatpak install --user NebulaPlayer-*.flatpak`) or the `.AppImage`,
  which both carry their own mpv.
- **Windows:** run the `-setup.exe`, or unzip the `.zip` and run
  `nebula-player.exe` (keep `libmpv-2.dll` next to it).
- **macOS (Apple Silicon):** open the `.dmg` and drag Nebula Player to
  Applications. It isn't signed by Apple yet, so the first time,
  right-click it and choose Open.

Each file has a minisign signature (`.minisig`) once signing is set up.

Building from source is covered in [docs/DEVELOPING.md](docs/DEVELOPING.md).

## License

GPL-3.0-only.
