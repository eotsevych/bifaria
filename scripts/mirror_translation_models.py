#!/usr/bin/env python3
"""Copies the Firefox Translations models Bifaria's Offline provider uses into a folder laid out
for our Hugging Face mirror, then prints the upload command.

    python3 scripts/mirror_translation_models.py [out_dir]   # default: ./translation-models-mirror
    hf upload-large-folder <repo> <out_dir> --repo-type model

The folder holds `records.json` (Firefox's model list, cut down to one desktop release build per
file) and each file at its Firefox `location` path, still zstd-compressed, so offline_translate.rs
only swaps the host. Re-run to pick up new model versions; finished files are skipped.
"""
import hashlib
import json
import os
import shutil
import ssl
import sys
import urllib.request

RECORDS_URL = "https://firefox.settings.services.mozilla.com/v1/buckets/main/collections/translations-models-v2/records"
CDN_ROOT = "https://firefox-settings-attachments.cdn.mozilla.net"
# fxtranslate 0.5.0 reads only this major version (remote::SUPPORTED_MAJOR).
SUPPORTED_MAJOR = 3
# Records Firefox shows on desktop release; the others are Android builds or Nightly alphas.
DESKTOP_FILTERS = {"", "env.appinfo.OS != 'Android'"}
# python.org's macOS Python ships without CA certificates; the system bundle works where it exists.
SSL = ssl.create_default_context(cafile="/etc/ssl/cert.pem" if os.path.exists("/etc/ssl/cert.pem") else None)


def version_key(v):
    return [int(p) if p.isdigit() else 0 for p in v.split(".")]


def is_release(r):
    return r.get("version", "").replace(".", "").isdigit()


def select(records):
    """One record per (file type, source, target): the newest desktop release build."""
    best = {}
    for r in records:
        if r.get("filter_expression", "") not in DESKTOP_FILTERS or not is_release(r):
            continue
        if version_key(r["version"])[0] != SUPPORTED_MAJOR or "attachment" not in r:
            continue
        key = (r["fileType"], r["sourceLanguage"], r["targetLanguage"])
        if key not in best or version_key(r["version"]) > version_key(best[key]["version"]):
            best[key] = r
    return sorted(best.values(), key=lambda r: (r["sourceLanguage"], r["targetLanguage"], r["fileType"]))


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fetch(record, out_dir):
    att = record["attachment"]
    dest = os.path.join(out_dir, att["location"])
    if os.path.isfile(dest) and os.path.getsize(dest) == att["size"] and sha256(dest) == att["hash"]:
        return False
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    tmp = dest + ".part"
    with urllib.request.urlopen(f"{CDN_ROOT}/{att['location']}", context=SSL) as resp, open(tmp, "wb") as f:
        shutil.copyfileobj(resp, f)
    if sha256(tmp) != att["hash"]:
        os.remove(tmp)
        raise SystemExit(f"hash mismatch for {record['name']}; nothing written")
    os.replace(tmp, dest)
    return True


def main():
    out_dir = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "translation-models-mirror")
    with urllib.request.urlopen(RECORDS_URL, context=SSL) as resp:
        records = json.load(resp)["data"]
    kept = select(records)
    pairs = {(r["sourceLanguage"], r["targetLanguage"]) for r in kept if r["fileType"] == "model"}
    total = sum(r["attachment"]["size"] for r in kept)
    print(f"{len(kept)} files for {len(pairs)} directions, {total / 1e9:.2f} GB")
    for i, r in enumerate(kept, 1):
        got = fetch(r, out_dir)
        print(f"[{i}/{len(kept)}] {'downloaded' if got else 'have'} {r['name']}", flush=True)
    with open(os.path.join(out_dir, "records.json"), "w") as f:
        json.dump({"data": kept}, f, indent=1)
    print(f"\nDone. Upload with:\n  hf upload-large-folder <repo> {out_dir} --repo-type model")


if __name__ == "__main__":
    main()
