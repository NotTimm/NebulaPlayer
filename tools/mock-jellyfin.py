#!/usr/bin/env python3
"""A fake Jellyfin server for trying Nebula Player without a real one.

    python3 tools/mock-jellyfin.py [port]     (default 8096)

Sign in as "demo" with any password (Quick Connect is offered too and
approves itself after a few seconds). There are three movies and a show
with two seasons; images are generated colour gradients. With ffmpeg
installed every item streams a short test video, made once in the temp
folder. Playback reports and watched/favourite changes are printed and
kept in memory.
"""

import json
import os
import struct
import subprocess
import sys
import tempfile
import time
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

SERVER_ID = "mockserver"
USER_ID = "demouser"
TOKEN = "mocktoken"
TICKS = 10_000_000

# ── Library ────────────────────────────────────────────────────────────────

def image_tags(*kinds):
    return {k: f"tag-{k.lower()}" for k in kinds}

LIBRARIES = [
    {"Id": "lib-movies", "Name": "Movies", "Type": "CollectionFolder", "CollectionType": "movies",
     "IsFolder": True, "ImageTags": image_tags("Primary")},
    {"Id": "lib-shows", "Name": "Shows", "Type": "CollectionFolder", "CollectionType": "tvshows",
     "IsFolder": True, "ImageTags": image_tags("Primary")},
]

def movie(i, name, year, rating, minutes, genres, overview):
    return {
        "Id": f"movie-{i}", "Name": name, "Type": "Movie", "IsFolder": False, "ParentId": "lib-movies",
        "ProductionYear": year, "PremiereDate": f"{year}-05-01T00:00:00.0000000Z",
        "OfficialRating": "PG-13", "CommunityRating": rating, "CriticRating": 88,
        "RunTimeTicks": minutes * 60 * TICKS, "Genres": genres, "Overview": overview,
        "Taglines": ["A mock tagline for testing."],
        "ImageTags": image_tags("Primary", "Logo", "Thumb"), "BackdropImageTags": ["tag-backdrop"],
        "People": [
            {"Id": f"person-{i}-1", "Name": "Ada Lovelace", "Role": "The Lead", "Type": "Actor", "PrimaryImageTag": "p"},
            {"Id": f"person-{i}-2", "Name": "Alan Turing", "Role": "Sidekick", "Type": "Actor", "PrimaryImageTag": "p"},
            {"Id": f"person-{i}-3", "Name": "Grace Hopper", "Type": "Director", "PrimaryImageTag": "p"},
        ],
        "Studios": [{"Name": "Mock Pictures", "Id": "studio-1"}],
    }

MOVIES = [
    movie(1, "The Nebula", 2024, 7.9, 112, ["Science Fiction", "Adventure"],
          "A crew of explorers chases a signal into a glowing nebula, where time runs differently."),
    movie(2, "Quiet Harbor", 2019, 6.8, 98, ["Drama"],
          "A lighthouse keeper's last winter on a remote island, told over one long storm."),
    movie(3, "Fast Native", 2022, 8.3, 125, ["Action", "Comedy"],
          "Two rival couriers race across a city that never sleeps, for a package neither should open."),
]

SERIES = {
    "Id": "series-1", "Name": "Orbiters", "Type": "Series", "IsFolder": True, "ParentId": "lib-shows",
    "ProductionYear": 2021, "Status": "Continuing", "OfficialRating": "TV-14", "CommunityRating": 8.6,
    "Genres": ["Science Fiction", "Mystery"],
    "Overview": "Six strangers wake on a space station with no memory of how they got there.",
    "ImageTags": image_tags("Primary", "Logo", "Thumb"), "BackdropImageTags": ["tag-backdrop"],
    "People": [{"Id": "person-s-1", "Name": "Katherine Johnson", "Role": "Commander", "Type": "Actor", "PrimaryImageTag": "p"}],
}

SEASONS = [
    {"Id": f"season-{s}", "Name": f"Season {s}", "Type": "Season", "IsFolder": True, "IndexNumber": s,
     "SeriesId": "series-1", "SeriesName": "Orbiters", "ImageTags": image_tags("Primary")}
    for s in (1, 2)
]

EPISODES = [
    {
        "Id": f"ep-{s}-{e}", "Name": f"Episode Title {s}.{e}", "Type": "Episode", "IsFolder": False,
        "IndexNumber": e, "ParentIndexNumber": s, "SeasonId": f"season-{s}", "SeasonName": f"Season {s}",
        "SeriesId": "series-1", "SeriesName": "Orbiters", "SeriesPrimaryImageTag": "tag-primary",
        "ParentThumbItemId": "series-1", "ParentThumbImageTag": "tag-thumb",
        "ParentBackdropItemId": "series-1", "ParentBackdropImageTags": ["tag-backdrop"],
        "ParentLogoItemId": "series-1", "ParentLogoImageTag": "tag-logo",
        "RunTimeTicks": 45 * 60 * TICKS, "CommunityRating": 7.5 + e / 10,
        "PremiereDate": f"202{s}-0{e}-10T00:00:00.0000000Z",
        "Overview": f"Something strange happens in episode {e} of season {s}.",
        "ImageTags": image_tags("Primary"),
    }
    for s in (1, 2) for e in range(1, 5)
]

ITEMS = {i["Id"]: i for i in MOVIES + [SERIES] + SEASONS + EPISODES + LIBRARIES}

# Per-user state: position, played, favourite.
USER_DATA = {i: {"PlaybackPositionTicks": 0, "Played": False, "IsFavorite": False} for i in ITEMS}
USER_DATA["movie-2"]["PlaybackPositionTicks"] = 40 * 60 * TICKS
USER_DATA["ep-1-1"]["Played"] = True
USER_DATA["ep-1-2"]["PlaybackPositionTicks"] = 20 * 60 * TICKS
LAST_PLAYED = {"movie-2": 2, "ep-1-2": 1}

QUICK = {}

def with_user_data(item):
    out = dict(item)
    ud = dict(USER_DATA.get(item["Id"], {}))
    if item["Type"] in ("Series", "Season"):
        eps = [e for e in EPISODES if item["Id"] in (e["SeriesId"], e["SeasonId"])]
        ud["UnplayedItemCount"] = sum(1 for e in eps if not USER_DATA[e["Id"]]["Played"])
        ud["Played"] = ud["UnplayedItemCount"] == 0
    rt = item.get("RunTimeTicks")
    if rt and ud.get("PlaybackPositionTicks"):
        ud["PlayedPercentage"] = 100.0 * ud["PlaybackPositionTicks"] / rt
    out["UserData"] = ud
    return out

def page(items):
    return {"Items": [with_user_data(i) for i in items], "TotalRecordCount": len(items), "StartIndex": 0}

def next_up(series_id=None):
    for e in EPISODES:
        if series_id and e["SeriesId"] != series_id:
            continue
        if not USER_DATA[e["Id"]]["Played"] and not USER_DATA[e["Id"]]["PlaybackPositionTicks"]:
            return [e]
    return []

# ── Images (no Pillow needed) ──────────────────────────────────────────────

def png(width, height, rgba_rows):
    raw = b"".join(b"\x00" + row for row in rgba_rows)
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))

IMAGE_CACHE = {}

def image(item_id, kind, max_width):
    sizes = {"Primary": (2, 3), "Backdrop": (16, 9), "Thumb": (16, 9), "Logo": (4, 1)}
    if item_id.startswith("ep-") and kind == "Primary" or item_id.startswith("lib-"):
        sizes["Primary"] = (16, 9)
    aw, ah = sizes.get(kind, (2, 3))
    w = min(max_width or 400, 640)
    h = w * ah // aw
    key = (item_id, kind, w)
    if key in IMAGE_CACHE:
        return IMAGE_CACHE[key]
    seed = zlib.crc32(item_id.encode())
    a = ((seed >> 16) & 0xFF, (seed >> 8) & 0xFF, seed & 0xFF)
    b = (0xAA, 0x5C, 0xC3) if seed % 2 else (0x00, 0xA4, 0xDC)
    rows = []
    for y in range(h):
        row = bytearray()
        for x in range(w):
            t = (x / max(w - 1, 1) + y / max(h - 1, 1)) / 2
            r, g, bl = (int(a[i] * (1 - t) + b[i] * t) for i in range(3))
            alpha = 255
            if kind == "Logo":
                # A rounded bar of "text" on a transparent background.
                inside = 0.1 < y / h < 0.9 and (x // max(w // 12, 1)) % 3 != 2
                alpha = 255 if inside else 0
                r, g, bl = 240, 240, 240
            row += bytes((r, g, bl, alpha))
        rows.append(bytes(row))
    data = png(w, h, rows)
    IMAGE_CACHE[key] = data
    return data

# ── Video ──────────────────────────────────────────────────────────────────

VIDEO = os.path.join(tempfile.gettempdir(), "nebula-mock-video.mkv")

def ensure_video():
    if os.path.exists(VIDEO):
        return True
    try:
        subprocess.run(
            ["ffmpeg", "-y", "-loglevel", "error",
             "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30:duration=20",
             "-f", "lavfi", "-i", "sine=frequency=440:duration=20",
             "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", "-shortest", VIDEO],
            check=True)
        return True
    except (OSError, subprocess.CalledProcessError) as e:
        print("no test video (ffmpeg missing?):", e)
        return False

# ── HTTP ───────────────────────────────────────────────────────────────────

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        sys.stderr.write("%s %s\n" % (self.command, self.path))

    def send(self, body, status=200, ctype="application/json"):
        if isinstance(body, (dict, list, bool)):
            body = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def authed(self):
        auth = self.headers.get("Authorization", "") + self.headers.get("X-Emby-Token", "")
        return TOKEN in auth

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(n) or b"{}")

    def do_HEAD(self):
        self.do_GET()

    def do_GET(self):
        u = urlparse(self.path)
        p, q = u.path, {k.lower(): v[0] for k, v in parse_qs(u.query).items()}
        parts = [x for x in p.split("/") if x]

        if p == "/System/Info/Public":
            return self.send({"ServerName": "Mock Jellyfin", "Id": SERVER_ID, "Version": "10.10.0"})
        if p == "/QuickConnect/Enabled":
            return self.send(True)
        if p == "/QuickConnect/Connect":
            s = QUICK.get(q.get("secret"))
            if not s:
                return self.send({}, 404)
            return self.send({"Secret": q["secret"], "Code": s["code"], "Authenticated": time.time() - s["t"] > 6})
        if len(parts) >= 4 and parts[0] == "Items" and parts[2] == "Images":
            return self.send(image(parts[1], parts[3], int(q.get("maxwidth", 400))), ctype="image/png")
        if parts[:1] == ["Videos"] and parts[-1] == "stream":
            if not self.authed():
                return self.send({}, 401)
            if not ensure_video():
                return self.send({}, 404)
            return self.stream_file(VIDEO)
        if not self.authed():
            return self.send({}, 401)

        user = f"/Users/{USER_ID}"
        if p == f"{user}/Views":
            return self.send(page(LIBRARIES))
        if p == f"{user}/Items/Resume":
            items = [ITEMS[i] for i in sorted(LAST_PLAYED, key=LAST_PLAYED.get, reverse=True)
                     if USER_DATA[i]["PlaybackPositionTicks"] and not USER_DATA[i]["Played"]]
            return self.send(page(items))
        if p == "/Shows/NextUp":
            return self.send(page(next_up(q.get("seriesid"))))
        if p == f"{user}/Items/Latest":
            parent = q.get("parentid")
            items = MOVIES[::-1] if parent == "lib-movies" else [SERIES] if parent == "lib-shows" else []
            return self.send([with_user_data(i) for i in items])
        if p == f"{user}/Items":
            if "searchterm" in q:
                term = q["searchterm"].lower()
                return self.send(page([i for i in MOVIES + [SERIES] + EPISODES if term in i["Name"].lower()]))
            parent = q.get("parentid")
            if parent == "lib-movies":
                items = sorted(MOVIES, key=lambda m: m["Name"])
            elif parent == "lib-shows":
                items = [SERIES]
            else:
                items = MOVIES + [SERIES]
            start, limit = int(q.get("startindex", 0)), int(q.get("limit", 100))
            out = page(items[start:start + limit])
            out["TotalRecordCount"] = len(items)
            return self.send(out)
        if len(parts) == 4 and parts[:3] == ["Users", USER_ID, "Items"]:
            item = ITEMS.get(parts[3])
            if not item:
                return self.send({}, 404)
            return self.send(with_user_data(item))
        if len(parts) == 3 and parts[0] == "Items" and parts[2] == "Similar":
            return self.send(page([m for m in MOVIES if m["Id"] != parts[1]]))
        if len(parts) == 3 and parts[0] == "Shows" and parts[2] == "Seasons":
            return self.send(page(SEASONS))
        if len(parts) == 3 and parts[0] == "Shows" and parts[2] == "Episodes":
            eps = EPISODES
            if q.get("seasonid"):
                eps = [e for e in eps if e["SeasonId"] == q["seasonid"]]
            if q.get("startitemid"):
                ids = [e["Id"] for e in eps]
                eps = eps[ids.index(q["startitemid"]):] if q["startitemid"] in ids else []
            if q.get("limit"):
                eps = eps[: int(q["limit"])]
            return self.send(page(eps))
        if len(parts) == 3 and parts[0] == "Items" and parts[2] == "PlaybackInfo":
            return self.send({
                "PlaySessionId": f"session-{int(time.time())}",
                "MediaSources": [{
                    "Id": parts[1], "Container": "mkv",
                    "DefaultAudioStreamIndex": 1, "DefaultSubtitleStreamIndex": -1,
                    "MediaStreams": [
                        {"Type": "Video", "Index": 0, "Codec": "h264"},
                        {"Type": "Audio", "Index": 1, "Codec": "aac", "Language": "eng"},
                    ],
                }],
            })
        return self.send({"error": "not mocked", "path": p}, 404)

    def stream_file(self, path):
        size = os.path.getsize(path)
        start, end = 0, size - 1
        rng = self.headers.get("Range")
        if rng and rng.startswith("bytes="):
            a, _, b = rng[6:].partition("-")
            start = int(a or 0)
            end = int(b) if b else size - 1
        self.send_response(206 if rng else 200)
        self.send_header("Content-Type", "video/x-matroska")
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(end - start + 1))
        if rng:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.end_headers()
        if self.command == "HEAD":
            return
        with open(path, "rb") as f:
            f.seek(start)
            left = end - start + 1
            while left > 0:
                chunk = f.read(min(65536, left))
                if not chunk:
                    break
                try:
                    self.wfile.write(chunk)
                except (BrokenPipeError, ConnectionResetError):
                    return
                left -= len(chunk)

    def do_POST(self):
        p = urlparse(self.path).path
        parts = [x for x in p.split("/") if x]
        auth_reply = {"User": {"Id": USER_ID, "Name": "demo"}, "AccessToken": TOKEN, "ServerId": SERVER_ID}
        if p == "/QuickConnect/Initiate":
            secret = f"secret{int(time.time())}"
            QUICK[secret] = {"code": "123456", "t": time.time()}
            return self.send({"Secret": secret, "Code": "123456", "Authenticated": False})
        if p == "/Users/AuthenticateWithQuickConnect":
            return self.send(auth_reply)
        if p == "/Users/AuthenticateByName":
            body = self.body()
            if body.get("Username") != "demo":
                return self.send({}, 401)
            return self.send(auth_reply)
        if not self.authed():
            return self.send({}, 401)
        if p.startswith("/Sessions/Playing"):
            body = self.body()
            print("report", p, json.dumps(body))
            item = body.get("ItemId")
            if item in USER_DATA:
                ticks = body.get("PositionTicks", 0)
                run = ITEMS[item].get("RunTimeTicks") or 1
                LAST_PLAYED[item] = time.time()
                if p.endswith("Stopped") and ticks >= 0.9 * run:
                    USER_DATA[item].update(Played=True, PlaybackPositionTicks=0)
                else:
                    USER_DATA[item]["PlaybackPositionTicks"] = ticks
            return self.send(b"", 204)
        if p == "/Sessions/Logout":
            return self.send(b"", 204)
        return self.user_flag(parts, True)

    def do_DELETE(self):
        if not self.authed():
            return self.send({}, 401)
        return self.user_flag([x for x in urlparse(self.path).path.split("/") if x], False)

    def user_flag(self, parts, on):
        if len(parts) == 4 and parts[:2] == ["Users", USER_ID] and parts[3] in USER_DATA:
            field = {"PlayedItems": "Played", "FavoriteItems": "IsFavorite"}.get(parts[2])
            if field:
                USER_DATA[parts[3]][field] = on
                # Marking a show or season marks its episodes.
                for e in EPISODES:
                    if field == "Played" and parts[3] in (e["SeriesId"], e["SeasonId"]):
                        USER_DATA[e["Id"]]["Played"] = on
                print("set", field, parts[3], on)
                return self.send(with_user_data(ITEMS[parts[3]]).get("UserData"))
        return self.send({"error": "not mocked"}, 404)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8096
    print(f"Mock Jellyfin on http://localhost:{port} (user: demo, any password)")
    ThreadingHTTPServer(("", port), Handler).serve_forever()
