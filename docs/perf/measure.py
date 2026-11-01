#!/usr/bin/env python3
"""Dashboard and memory baseline for the bot, in preview mode (sample servers, no Discord).

Starts `target/release/bot preview <port>`, signs in through the preview's dev login, finds
every dashboard page from the sidebar of each sample server, and times each page over a kept-
alive connection (one person clicking around: closed loop, so this is service time, not load).
Reads the process's memory (VmRSS, VmHWM) after boot, and again after every page was served.

    cargo build --release -p bot
    python3 docs/perf/measure.py [--runs 40] [--port 3150] [--gzip] [--json out.json]

Prints a Markdown table; --json also writes the raw samples for later comparison.
"""

import argparse
import gzip
import http.client
import json
import os
import re
import signal
import statistics
import subprocess
import sys
import time

GUILDS = ["1100000000000000001", "1100000000000000002"]


def mem(pid):
    out = {}
    with open(f"/proc/{pid}/status") as f:
        for line in f:
            k, _, v = line.partition(":")
            if k in ("VmRSS", "VmHWM", "Threads"):
                out[k] = v.strip()
    return out


GZIP = False


def get(conn, path, cookie):
    t = time.perf_counter()
    headers = {"Cookie": cookie} if cookie else {}
    if GZIP:
        headers["Accept-Encoding"] = "gzip"
    conn.request("GET", path, headers=headers)
    r = conn.getresponse()
    wire = r.read()
    ms = (time.perf_counter() - t) * 1000
    body = gzip.decompress(wire) if r.getheader("Content-Encoding") == "gzip" else wire
    return r.status, body, ms, r.getheader("Set-Cookie"), len(wire)


def pct(xs, p):
    xs = sorted(xs)
    k = max(0, min(len(xs) - 1, round(p / 100 * (len(xs) - 1))))
    return xs[k]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=40)
    ap.add_argument("--port", type=int, default=3150)
    ap.add_argument("--json")
    ap.add_argument("--bin", default="target/release/bot")
    ap.add_argument("--gzip", action="store_true", help="ask for gzip, as browsers do; sizes are then bytes on the wire")
    a = ap.parse_args()
    global GZIP
    GZIP = a.gzip

    proc = subprocess.Popen([a.bin, "preview", str(a.port)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        conn = None
        for _ in range(100):
            try:
                conn = http.client.HTTPConnection("127.0.0.1", a.port, timeout=10)
                get(conn, "/", None)
                break
            except OSError:
                time.sleep(0.1)
        time.sleep(3)  # let startup work settle
        boot = mem(proc.pid)

        conn = http.client.HTTPConnection("127.0.0.1", a.port, timeout=10)
        set_cookie = get(conn, "/dev/login?to=/", None)[3]
        cookie = set_cookie.split(";")[0] if set_cookie else ""

        pages = []
        for g in GUILDS:
            body = get(conn, f"/g/{g}", cookie)[1]
            for href in re.findall(rb'href="(/g/' + g.encode() + rb'[^"#?]*)"', body):
                p = href.decode()
                if p not in pages:
                    pages.append(p)
        for extra in ["/users", "/activity", "/features"]:
            pages.append(extra)

        rows, raw = [], {}
        for p in pages:
            status, body, _, _, wire = get(conn, p, cookie)
            if status != 200:
                continue
            for _ in range(5):
                get(conn, p, cookie)
            ts = [get(conn, p, cookie)[2] for _ in range(a.runs)]
            raw[p] = ts
            rows.append((p, wire, statistics.median(ts), pct(ts, 95), max(ts)))
        after = mem(proc.pid)
    finally:
        proc.send_signal(signal.SIGTERM)
        proc.wait(timeout=10)

    print(f"Boot (idle 3 s): {boot}\nAfter serving every page {a.runs + 6}x: {after}\n")
    print(f"| Page | {'Sent' if a.gzip else 'HTML'} KB | p50 ms | p95 ms | max ms |\n|---|---:|---:|---:|---:|")
    for p, size, p50, p95, mx in sorted(rows, key=lambda r: -r[3]):
        print(f"| `{p}` | {size / 1024:.0f} | {p50:.2f} | {p95:.2f} | {mx:.2f} |")
    if a.json:
        with open(a.json, "w") as f:
            json.dump({"boot": boot, "after": after, "samples": raw}, f)


if __name__ == "__main__":
    sys.exit(main())
