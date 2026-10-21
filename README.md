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
- **Search** across movies, shows and episodes.
- **Playback in mpv**: resumes where you left off, picks your Jellyfin
  audio and subtitle preferences, loads external subtitles, plays the
  next episode, and reports progress back so other Jellyfin apps stay in
  sync.
- **Skip intros and credits** found by the
  [Intro Skipper](https://github.com/intro-skipper/intro-skipper) plugin
  (or anything else that fills in Jellyfin's media segments): mpv asks
  "Skip Intro: press S or Enter", or skips by itself if you prefer.
  Recaps, previews and ads are covered too.
- **Local files**: add folders and play anything in them. There's no
  scraping; metadata comes from Jellyfin only.
- Jellyfin's dark look and blue-purple accent by default, or your
  desktop's COSMIC theme.

mpv opens in its own window for now; drawing the video inside the app
window comes later. mpv's usual keys work (Space, arrows, `f` for full
screen, `j`/`#` for subtitle/audio tracks, `q` to close). Your own
`mpv.conf` and `input.conf` go in the folder the settings open.

## Installing

Linux needs libmpv: `sudo dnf install mpv-libs` on Fedora,
`sudo apt install libmpv2` on Debian and Ubuntu. Builds are on the
[Releases](https://github.com/NotTimm/NebulaPlayer/releases) page and are
marked as pre-releases while the app is in alpha/beta.

Building from source is covered in [docs/DEVELOPING.md](docs/DEVELOPING.md).

## License

GPL-3.0-only.
