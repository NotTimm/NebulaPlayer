#!/usr/bin/env python3
"""A fake Jellyfin server for trying Nebula Player without a real one.

    python3 tools/mock-jellyfin.py [port] [--legacy] [--many N]   (default 8096)

With --legacy it answers only the routes Jellyfin 10.8 had
(/Users/{id}/Items...), like an old server; otherwise both those and the
10.9+ ones (/UserViews, /UserItems/Resume, /Items?userId=...).

Sign in as "demo" with any password (Quick Connect is offered too and
approves itself after a few seconds). There are three movies and a show
with two seasons; images are generated colour gradients. With ffmpeg
installed every item streams a short test video, made once in the temp
folder. Playback reports and watched/favourite changes are printed and
kept in memory. --many N adds N more movies, for trying big libraries
(paging, the A-Z bar, search).

SyncPlay works too: the session WebSocket (/socket), /GetUtcTime and the
/SyncPlay/... requests, with a simplified version of the server's group
states (waiting for everyone to be Ready, then playing them all at once).
Run two copies of the app with separate config folders to try it; each
copy is a member named after the end of its device id.
"""

import base64
import hashlib
import json
import os
import struct
import subprocess
import sys
import threading
import uuid
import tempfile
import time
import zlib
from datetime import datetime, timedelta, timezone
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

def sort_name(name):
    """Jellyfin's SortName: lower case, without a leading article."""
    n = name.lower()
    for article in ("the ", "a ", "an "):
        if n.startswith(article):
            return n[len(article):]
    return n

def filler_movies(n):
    first = ["The", "", "A", "Last", "Red", "Silent", "Broken", "Golden", "Lost", "Midnight", "Iron", "Zero",
             "Electric", "Hidden", "Northern", "Paper", "Quantum", "Velvet", "Wild", "Yellow", "Xenon", "Jade",
             "Kings of", "Under", "Oath of", "1984", "300", "12"]
    second = ["Harbor", "Engine", "Signal", "Garden", "Machine", "River", "Empire", "Dream", "Station", "Hours",
              "Summer", "Protocol", "Frontier", "Mirror", "Kingdom", "Echo", "Voyage", "Shadow", "Orchard"]
    out = []
    for i in range(n):
        name = " ".join(w for w in (first[i % len(first)], second[(i // len(first)) % len(second)]) if w)
        if i >= len(first) * len(second):
            name += f" {i // (len(first) * len(second)) + 1}"
        out.append(movie(100 + i, name, 1970 + i % 55, 5 + (i % 40) / 10, 90 + i % 60, ["Drama"],
                         f"Filler movie number {i}."))
    return out

if "--many" in sys.argv:
    MOVIES += filler_movies(int(sys.argv[sys.argv.index("--many") + 1]))

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
LEGACY_ONLY = "--legacy" in sys.argv

def legacy_path(path):
    """Maps a Jellyfin 10.9+ per-user route to the 10.8 one it replaced."""
    parts = [x for x in path.split("/") if x]
    user = f"/Users/{USER_ID}"
    if path == "/UserViews":
        return f"{user}/Views"
    if path == "/UserItems/Resume":
        return f"{user}/Items/Resume"
    if path in ("/Items", "/Items/Latest"):
        return user + path
    if len(parts) == 2 and parts[0] == "Items":
        return f"{user}/Items/{parts[1]}"
    if len(parts) == 2 and parts[0] in ("UserPlayedItems", "UserFavoriteItems"):
        return f"{user}/{parts[0][4:]}/{parts[1]}"
    return None

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

# Trickplay, as a 10.9+ server makes it: a thumbnail a second here (the
# test video is 20 s), 3 × 3 to a sheet so it takes three sheets.
TRICKPLAY = {"Width": 320, "Height": 180, "TileWidth": 3, "TileHeight": 3, "ThumbnailCount": 20,
             "Interval": 1000, "Bandwidth": 100000}
CHAPTERS = [{"StartPositionTicks": 0, "Name": "Opening"}, {"StartPositionTicks": 10 * TICKS, "Name": "The Middle"}]

def trickplay_sheet(index):
    """Sheet `index` of the test video's thumbnails (made once), as JPEG."""
    path = os.path.join(tempfile.gettempdir(), f"nebula-mock-trickplay-{index}.jpg")
    if not os.path.exists(path) and ensure_video():
        pattern = os.path.join(tempfile.gettempdir(), "nebula-mock-trickplay-%d.jpg")
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", VIDEO, "-vf",
                        "fps=1,scale=320:180,tile=3x3", "-start_number", "0", pattern], check=False)
    try:
        with open(path, "rb") as f:
            return f.read()
    except OSError:
        return None

# ── HTTP ───────────────────────────────────────────────────────────────────

# ── SyncPlay ───────────────────────────────────────────────────────────────

SOCKETS = {}  # device id -> Socket
GROUPS = {}  # group id -> group dict
SP_LOCK = threading.RLock()


def utc_now():
    return datetime.now(timezone.utc)


def iso(t):
    """A .NET-style UTC time, 7 fraction digits, as Jellyfin sends."""
    return t.strftime("%Y-%m-%dT%H:%M:%S.") + f"{t.microsecond:06d}0Z"


def parse_iso(s):
    s = s.rstrip("Z")
    if "." in s:
        head, frac = s.split(".")
        s = f"{head}.{frac[:6]:0<6}"
        return datetime.strptime(s, "%Y-%m-%dT%H:%M:%S.%f").replace(tzinfo=timezone.utc)
    return datetime.strptime(s, "%Y-%m-%dT%H:%M:%S").replace(tzinfo=timezone.utc)


class Socket:
    def __init__(self, conn):
        self.conn = conn
        self.lock = threading.Lock()

    def send(self, obj):
        data = json.dumps(obj).encode()
        head = bytes([0x81])
        n = len(data)
        if n < 126:
            head += bytes([n])
        elif n < 65536:
            head += bytes([126]) + struct.pack(">H", n)
        else:
            head += bytes([127]) + struct.pack(">Q", n)
        with self.lock:
            try:
                self.conn.sendall(head + data)
            except OSError:
                pass


def ws_send(device, message_type, data):
    sock = SOCKETS.get(device)
    if sock:
        sock.send({"MessageType": message_type, "MessageId": uuid.uuid4().hex, "Data": data})


def member_name(device):
    return "demo-" + device[-4:]


def group_info(g):
    return {
        "GroupId": g["id"],
        "GroupName": g["name"],
        "State": g["state"],
        "Participants": [member_name(d) for d in g["members"]],
        "LastUpdatedAt": iso(utc_now()),
    }


def group_of(device):
    for g in GROUPS.values():
        if device in g["members"]:
            return g
    return None


def group_update(devices, g, kind, data):
    for d in devices:
        ws_send(d, "SyncPlayGroupUpdate", {"GroupId": g["id"] if g else "", "Type": kind, "Data": data})


def position(g):
    """Where the group is now, in ticks."""
    if g["state"] == "Playing":
        elapsed = (utc_now() - g["since"]).total_seconds()
        return g["ticks"] + max(0, int(elapsed * 10_000_000))
    return g["ticks"]


def current_pid(g):
    if 0 <= g["index"] < len(g["queue"]):
        return g["queue"][g["index"]]["PlaylistItemId"]
    return ""


def command(devices, g, kind, when, ticks):
    data = {
        "GroupId": g["id"],
        "PlaylistItemId": current_pid(g),
        "When": iso(when),
        "PositionTicks": ticks,
        "Command": kind,
        "EmittedAt": iso(utc_now()),
    }
    print("syncplay command", kind, "at", data["When"], "ticks", ticks, "to", [member_name(d) for d in devices])
    for d in devices:
        ws_send(d, "SyncPlayCommand", data)


def play_queue(devices, g, reason):
    data = {
        "Reason": reason,
        "LastUpdate": iso(utc_now()),
        "Playlist": g["queue"],
        "PlayingItemIndex": g["index"],
        "StartPositionTicks": position(g),
        "IsPlaying": g["state"] == "Playing",
        "ShuffleMode": "Sorted",
        "RepeatMode": "RepeatNone",
    }
    print("syncplay queue", reason, "index", g["index"], "of", len(g["queue"]))
    group_update(devices, g, "PlayQueue", data)


def set_state(g, state, reason):
    g["state"] = state
    group_update(g["members"], g, "StateUpdate", {"State": state, "Reason": reason})


def wait_for_all(g, resume, reason):
    """Waiting state: everyone must report Ready before playback goes on."""
    g["ticks"] = position(g)
    g["resume"] = resume
    for m in g["members"].values():
        m["ready"] = False
    set_state(g, "Waiting", reason)


def check_ready(g):
    waiting = [m for m in g["members"].values() if not m["ready"] and not m["ignore"]]
    if g["state"] != "Waiting" or waiting:
        return
    if g["resume"]:
        # Everyone starts together, a little ahead, so the command arrives
        # in time (the server uses twice the highest ping).
        highest = max([m["ping"] for m in g["members"].values()] + [0])
        when = utc_now() + timedelta(milliseconds=max(2 * highest, 500))
        g["since"] = when
        g["state"] = "Playing"
        command(g["members"], g, "Unpause", when, g["ticks"])
        set_state(g, "Playing", "Ready")
    else:
        command(g["members"], g, "Pause", utc_now(), g["ticks"])
        set_state(g, "Paused", "Ready")


def syncplay_post(device, what, body):
    with SP_LOCK:
        g = group_of(device)
        if what == "New":
            if g:
                syncplay_leave(device, g)
            gid = uuid.uuid4().hex
            g = GROUPS[gid] = {
                "id": gid, "name": body.get("GroupName", "Group"), "state": "Idle", "members": {},
                "queue": [], "index": -1, "ticks": 0, "since": utc_now(), "resume": False,
            }
            g["members"][device] = {"ready": True, "ignore": False, "ping": 0}
            print("syncplay new group", g["name"], "by", member_name(device))
            group_update([device], g, "GroupJoined", group_info(g))
            return
        if what == "Join":
            target = GROUPS.get(body.get("GroupId", "").replace("-", ""))
            if not target:
                group_update([device], None, "GroupDoesNotExist", "")
                return
            if g and g is not target:
                syncplay_leave(device, g)
            others = list(target["members"])
            target["members"][device] = {"ready": False, "ignore": False, "ping": 0}
            print("syncplay", member_name(device), "joins", target["name"])
            group_update([device], target, "GroupJoined", group_info(target))
            group_update(others, target, "UserJoined", member_name(device))
            if target["queue"]:
                was_playing = target["state"] == "Playing"
                if was_playing:
                    # Everyone waits for the newcomer to load.
                    target["ticks"] = position(target)
                    command(others, target, "Pause", utc_now(), target["ticks"])
                target["resume"] = was_playing or target["resume"]
                target["state"] = "Waiting"
                play_queue([device], target, "NewPlaylist")
                set_state(target, "Waiting", "Join")
            else:
                target["members"][device]["ready"] = True
            return
        if g is None:
            group_update([device], None, "NotInGroup", "")
            return
        m = g["members"][device]
        if what == "Leave":
            syncplay_leave(device, g)
        elif what == "Ping":
            m["ping"] = body.get("Ping", 0)
        elif what == "SetIgnoreWait":
            m["ignore"] = bool(body.get("IgnoreWait"))
            check_ready(g)
        elif what == "SetNewQueue":
            g["queue"] = [{"ItemId": i, "PlaylistItemId": uuid.uuid4().hex} for i in body.get("PlayingQueue", [])]
            g["index"] = body.get("PlayingItemPosition", 0)
            g["ticks"] = body.get("StartPositionTicks", 0)
            g["state"] = "Idle"
            wait_for_all(g, True, "Play")
            play_queue(g["members"], g, "NewPlaylist")
        elif what in ("NextItem", "PreviousItem"):
            step = 1 if what == "NextItem" else -1
            if body.get("PlaylistItemId") != current_pid(g) or not 0 <= g["index"] + step < len(g["queue"]):
                print("syncplay", what, "ignored")
                return
            g["index"] += step
            g["ticks"] = 0
            g["state"] = "Idle"
            wait_for_all(g, True, what)
            play_queue(g["members"], g, what)
        elif what == "Pause":
            if g["state"] == "Playing":
                g["ticks"] = position(g)
                g["state"] = "Paused"
                command(g["members"], g, "Pause", utc_now(), g["ticks"])
                set_state(g, "Paused", "Pause")
            elif g["state"] == "Waiting":
                g["resume"] = False
        elif what == "Unpause":
            if g["state"] in ("Paused", "Idle") and g["queue"]:
                g["state"] = "Waiting"
                g["resume"] = True
                check_ready(g)
            elif g["state"] == "Waiting":
                g["resume"] = True
                check_ready(g)
        elif what == "Seek":
            resume = g["state"] == "Playing" or (g["state"] == "Waiting" and g["resume"])
            g["state"] = "Idle"
            g["ticks"] = body.get("PositionTicks", 0)
            wait_for_all(g, resume, "Seek")
            command(g["members"], g, "Seek", utc_now(), g["ticks"])
        elif what == "Buffering":
            if g["state"] == "Playing":
                g["ticks"] = position(g)
                g["state"] = "Idle"
                wait_for_all(g, True, "Buffer")
                command([d for d in g["members"] if d != device], g, "Pause", utc_now(), g["ticks"])
            m["ready"] = False
        elif what == "Ready":
            if body.get("PlaylistItemId") != current_pid(g):
                play_queue([device], g, "SetCurrentItem")
                return
            m["ready"] = True
            if g["state"] == "Waiting":
                check_ready(g)
            elif g["state"] == "Playing":
                # Catch up with the group.
                command([device], g, "Unpause", g["since"], g["ticks"])
        elif what == "Stop":
            g["ticks"] = 0
            g["state"] = "Idle"
            command(g["members"], g, "Stop", utc_now(), 0)


def syncplay_leave(device, g):
    del g["members"][device]
    print("syncplay", member_name(device), "leaves", g["name"])
    group_update([device], g, "GroupLeft", g["id"])
    group_update(g["members"], g, "UserLeft", member_name(device))
    if not g["members"]:
        del GROUPS[g["id"]]
    else:
        check_ready(g)


def device_of(headers):
    auth = headers.get("Authorization", "")
    marker = 'DeviceId="'
    if marker in auth:
        return auth.split(marker, 1)[1].split('"', 1)[0]
    return "unknown"


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

    def websocket(self, device):
        """The session WebSocket: handshake, then frames until it closes."""
        key = self.headers.get("Sec-WebSocket-Key", "")
        accept = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        # WebSocket clients want an HTTP/1.1 upgrade.
        self.protocol_version = "HTTP/1.1"
        self.send_response(101)
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.send_header("Sec-WebSocket-Accept", accept)
        self.end_headers()
        self.wfile.flush()
        sock = Socket(self.connection)
        SOCKETS[device] = sock
        print("socket open", member_name(device))
        sock.send({"MessageType": "ForceKeepAlive", "MessageId": uuid.uuid4().hex, "Data": 60})
        try:
            while True:
                head = self.rfile.read(2)
                if len(head) < 2:
                    break
                opcode, n = head[0] & 0x0F, head[1] & 0x7F
                if n == 126:
                    n = struct.unpack(">H", self.rfile.read(2))[0]
                elif n == 127:
                    n = struct.unpack(">Q", self.rfile.read(8))[0]
                mask = self.rfile.read(4) if head[1] & 0x80 else b"\0\0\0\0"
                payload = bytes(b ^ mask[i % 4] for i, b in enumerate(self.rfile.read(n)))
                if opcode == 8:
                    break
                if opcode == 9:
                    with sock.lock:
                        self.connection.sendall(bytes([0x8A, len(payload)]) + payload)
        except OSError:
            pass
        if SOCKETS.get(device) is sock:
            del SOCKETS[device]
        print("socket closed", member_name(device))
        self.close_connection = True

    def do_HEAD(self):
        self.do_GET()

    def route(self):
        """The request's path, with 10.9+ routes mapped to the old ones."""
        p = urlparse(self.path).path
        mapped = legacy_path(p)
        if mapped:
            if LEGACY_ONLY:
                return None
            return mapped
        return p

    def do_GET(self):
        u = urlparse(self.path)
        q = {k.lower(): v[0] for k, v in parse_qs(u.query).items()}
        p = self.route()
        if p is None:
            return self.send({"error": "no such route on this (old) server"}, 404)
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
        if p == "/socket":
            if q.get("api_key") != TOKEN:
                return self.send({}, 401)
            return self.websocket(q.get("deviceid", "unknown"))
        if p == "/GetUtcTime":
            received = iso(utc_now())
            return self.send({"RequestReceptionTime": received, "ResponseTransmissionTime": iso(utc_now())})
        if not self.authed():
            return self.send({}, 401)
        if p == "/SyncPlay/List":
            with SP_LOCK:
                return self.send([group_info(g) for g in GROUPS.values()])
        if len(parts) == 5 and parts[0] == "Videos" and parts[2] == "Trickplay" and parts[4].endswith(".jpg"):
            sheet = trickplay_sheet(int(parts[4][:-4]))
            if sheet is None:
                return self.send({}, 404)
            return self.send(sheet, ctype="image/jpeg")

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
            if "searchterm" in q and "parentid" not in q:
                term = q["searchterm"].lower()
                return self.send(page([i for i in MOVIES + [SERIES] + EPISODES if term in i["Name"].lower()]))
            parent = q.get("parentid")
            if parent == "lib-movies":
                items = sorted(MOVIES, key=lambda m: sort_name(m["Name"]))
            elif parent == "lib-shows":
                items = [SERIES]
            else:
                items = MOVIES + [SERIES]
            if "searchterm" in q:
                items = [i for i in items if q["searchterm"].lower() in i["Name"].lower()]
            if "namelessthan" in q:
                items = [i for i in items if sort_name(i["Name"]) < q["namelessthan"]]
            start, limit = int(q.get("startindex", 0)), int(q.get("limit", 100))
            out = page(items[start:start + limit])
            out["TotalRecordCount"] = len(items)
            return self.send(out)
        if len(parts) == 4 and parts[:3] == ["Users", USER_ID, "Items"]:
            item = ITEMS.get(parts[3])
            if not item:
                return self.send({}, 404)
            out = with_user_data(item)
            fields = q.get("fields", "")
            if "Trickplay" in fields and item["Type"] in ("Movie", "Episode"):
                out["Trickplay"] = {item["Id"]: {"320": TRICKPLAY}}
            if "Chapters" in fields and item["Type"] in ("Movie", "Episode"):
                out["Chapters"] = CHAPTERS
            return self.send(out)
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
        # Skippable parts of every episode (the test video is 20 s long): as
        # Jellyfin 10.10+ media segments, or the old Intro Skipper plugin's
        # endpoint for --legacy.
        if len(parts) == 2 and parts[0] == "MediaSegments" and not LEGACY_ONLY:
            items = []
            if parts[1].startswith("ep-"):
                items = [
                    {"Id": "s1", "ItemId": parts[1], "Type": "Intro", "StartTicks": 2 * TICKS, "EndTicks": 8 * TICKS},
                    {"Id": "s2", "ItemId": parts[1], "Type": "Outro", "StartTicks": 15 * TICKS, "EndTicks": 20 * TICKS},
                ]
            return self.send({"Items": items, "TotalRecordCount": len(items), "StartIndex": 0})
        if len(parts) == 3 and parts[0] == "Episode" and parts[2] == "IntroSkipperSegments":
            if not parts[1].startswith("ep-"):
                return self.send({})
            return self.send({
                "Introduction": {"EpisodeId": parts[1], "Valid": True, "IntroStart": 2.0, "IntroEnd": 8.0,
                                 "ShowSkipPromptAt": 0.0, "HideSkipPromptAt": 10.0},
                "Credits": {"EpisodeId": parts[1], "Valid": True, "IntroStart": 15.0, "IntroEnd": 20.0,
                            "ShowSkipPromptAt": 13.0, "HideSkipPromptAt": 20.0},
            })
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
        p = self.route()
        if p is None:
            return self.send({"error": "no such route on this (old) server"}, 404)
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
        if parts[:1] == ["SyncPlay"] and len(parts) == 2:
            body = self.body()
            device = device_of(self.headers)
            if parts[1] not in ("Ping",):
                print("syncplay request", parts[1], "from", member_name(device), json.dumps(body))
            syncplay_post(device, parts[1], body)
            return self.send(b"", 204)
        return self.user_flag(parts, True)

    def do_DELETE(self):
        if not self.authed():
            return self.send({}, 401)
        p = self.route()
        if p is None:
            return self.send({"error": "no such route on this (old) server"}, 404)
        return self.user_flag([x for x in p.split("/") if x], False)

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
    args = [a for i, a in enumerate(sys.argv[1:], 1) if not a.startswith("--") and sys.argv[i - 1] != "--many"]
    port = int(args[0]) if args else 8096
    kind = "Jellyfin 10.8 routes only" if LEGACY_ONLY else "10.9+ and 10.8 routes"
    print(f"Mock Jellyfin on http://localhost:{port} ({kind}; user: demo, any password)")
    ThreadingHTTPServer(("", port), Handler).serve_forever()
