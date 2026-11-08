#!/usr/bin/env python3
"""Download the pinned Minecraft version (research-lock.json) from Mojang into an
official-launcher-layout folder (versions/, libraries/, assets/) plus the server jar,
for horizon-prepare on a Mac without the official launcher. Dev tool only: the files
land in .local/ and are never committed.

Usage: python3 tools/ios/fetch_minecraft.py [--out .local/mojang] [--no-sounds]
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
import sys
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
MANIFEST = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
OBJECTS = "https://resources.download.minecraft.net"


def fetch(url):
    with urllib.request.urlopen(url, timeout=120) as r:
        return r.read()


def sha1(data):
    return hashlib.sha1(data).hexdigest()


def get(url, path, want_sha1=None):
    if os.path.exists(path) and want_sha1:
        with open(path, "rb") as f:
            if sha1(f.read()) == want_sha1:
                return
    data = fetch(url)
    if want_sha1 and sha1(data) != want_sha1:
        raise SystemExit(f"sha1 mismatch for {url}")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".part"
    with open(tmp, "wb") as f:
        f.write(data)
    os.replace(tmp, path)


def allowed(lib):
    rules = lib.get("rules")
    if not rules:
        return True
    ok = False
    for rule in rules:
        os_rule = rule.get("os")
        if os_rule is None or os_rule.get("name") == "osx":
            ok = rule["action"] == "allow"
    return ok


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(ROOT, ".local", "mojang"))
    ap.add_argument("--no-sounds", action="store_true")
    args = ap.parse_args()
    lock = json.load(open(os.path.join(ROOT, "research-lock.json")))["minecraft"]
    version = lock["version"]
    accepted = {lock["manifest_sha1"], *lock.get("accepted_manifest_sha1", [])}
    out = args.out

    entry = next(v for v in json.loads(fetch(MANIFEST))["versions"] if v["id"] == version)
    if entry["sha1"] not in accepted:
        print(f"warning: {version}.json sha1 {entry['sha1']} not in research-lock.json", file=sys.stderr)
    vdir = os.path.join(out, "versions", version)
    get(entry["url"], os.path.join(vdir, f"{version}.json"), entry["sha1"])
    vjson = json.load(open(os.path.join(vdir, f"{version}.json")))

    jobs = []
    c = vjson["downloads"]["client"]
    jobs.append((c["url"], os.path.join(vdir, f"{version}.jar"), c["sha1"]))
    s = vjson["downloads"]["server"]
    jobs.append((s["url"], os.path.join(out, "server", f"server-{version}.jar"), s["sha1"]))
    for lib in vjson["libraries"]:
        if not allowed(lib):
            continue
        art = lib.get("downloads", {}).get("artifact")
        if art:
            jobs.append((art["url"], os.path.join(out, "libraries", art["path"]), art["sha1"]))
    ai = vjson["assetIndex"]
    index_path = os.path.join(out, "assets", "indexes", f"{ai['id']}.json")
    get(ai["url"], index_path, ai["sha1"])
    for name, obj in json.load(open(index_path))["objects"].items():
        if args.no_sounds and name.endswith(".ogg"):
            continue
        h = obj["hash"]
        jobs.append((f"{OBJECTS}/{h[:2]}/{h}", os.path.join(out, "assets", "objects", h[:2], h), h))

    print(f"{len(jobs)} files")
    done = 0
    with concurrent.futures.ThreadPoolExecutor(16) as pool:
        for fut in concurrent.futures.as_completed([pool.submit(get, *j) for j in jobs]):
            fut.result()
            done += 1
            if done % 500 == 0:
                print(f"{done}/{len(jobs)}", flush=True)
    print(f"done: {out}")
    print(f"server jar: {os.path.join(out, 'server', f'server-{version}.jar')}")


if __name__ == "__main__":
    main()
