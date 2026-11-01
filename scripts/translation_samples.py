#!/usr/bin/env python3
"""Pairs Bifaria's translations with what was said, per service, and flags likely bad ones.

  python3 scripts/translation_samples.py               # default locations on macOS
  python3 scripts/translation_samples.py --per 10      # samples per service (default 8)
  python3 scripts/translation_samples.py --since 2026-11-01
  python3 scripts/translation_samples.py --test        # self-check

The performance log (logs/performance.jsonl) knows which service translated and how fast; the
history database (history.db) keeps the text. A translation is matched to the history row saved
within 15 s of it. Flags are hints for a reader, not a verdict:
  failed      the service returned nothing
  unchanged   the output is the input (not translated)
  script      the output isn't in the target language's alphabet (Cyrillic for uk/ru/be/bg/sr...)
  length      the output is under half or over twice the input's length
"""
import json
import os
import re
import sqlite3
import sys
import time
from collections import defaultdict

APP_DIR = os.path.expanduser("~/Library/Application Support/bifaria")
CYRILLIC_TARGETS = {"uk", "ru", "be", "bg", "sr", "mk", "kk"}
TARGET_CODES = {"English": "en", "Ukrainian": "uk", "Russian": "ru", "Spanish": "es", "French": "fr",
                "German": "de", "Italian": "it", "Polish": "pl", "Japanese": "ja", "Chinese": "zh"}


def flags(source, output, target):
    """Hints that a translation of `source` into language code `target` went wrong."""
    if not output.strip():
        return ["failed"]
    found = []
    norm = lambda s: re.sub(r"\W+", " ", s.lower()).strip()
    if norm(source) == norm(output):
        found.append("unchanged")
    letters = [c for c in output if c.isalpha()]
    if letters and target and target not in ("ja", "zh"):
        cyrillic = sum("Ѐ" <= c <= "ӿ" for c in letters) / len(letters)
        expected = cyrillic if target in CYRILLIC_TARGETS else 1 - cyrillic
        if expected < 0.6:
            found.append("script")
    ratio = len(output) / max(len(source), 1)
    if ratio < 0.5 or ratio > 2.0:
        found.append("length")
    return found


def load_translations(log_dir, since):
    rows = []
    for name in ("performance.1.jsonl", "performance.jsonl"):
        path = os.path.join(log_dir, name)
        if os.path.exists(path):
            with open(path) as f:
                rows += [json.loads(line) for line in f if line.strip()]
    return [r for r in rows if r.get("type") == "dictation" and r.get("translation_ms") is not None
            and r["ts"][:10] >= since]


def match_history(db_path, runs):
    """Each run with the history row closest in time (within 15 s), as (run, row) pairs."""
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    history = db.execute("SELECT timestamp, raw_text, translated_text, target_lang FROM history").fetchall()
    db.close()
    pairs = []
    for run in runs:
        at = time.mktime(time.strptime(run["ts"], "%Y-%m-%d %H:%M:%S"))
        best = min(history, key=lambda h: abs(float(h[0]) - at), default=None)
        if best and abs(float(best[0]) - at) <= 15:
            pairs.append((run, best))
    return pairs


def target_of(run, row):
    pair = run.get("translation_pair") or ""
    return pair.split("→")[-1].strip() if "→" in pair else TARGET_CODES.get(row[3])


def main(args):
    per = int(args[args.index("--per") + 1]) if "--per" in args else 8
    since = args[args.index("--since") + 1] if "--since" in args else "0000"
    runs = load_translations(os.path.join(APP_DIR, "logs"), since)
    if not runs:
        print("No translations in the performance log yet.")
        return
    by_service = defaultdict(list)
    for run, row in match_history(os.path.join(APP_DIR, "history.db"), runs):
        out = row[2] if run.get("translation_out_chars") else ""
        by_service[(run["translation"], run.get("translation_model") or "-")].append(
            (run, row[1], out, flags(row[1], out, target_of(run, row))))

    print(f"{len(runs)} translations in the log, {sum(map(len, by_service.values()))} matched to history\n")
    for (service, model), items in sorted(by_service.items(), key=lambda kv: -len(kv[1])):
        counts = defaultdict(int)
        for *_, found in items:
            for flag in found:
                counts[flag] += 1
        summary = ", ".join(f"{k} {v}" for k, v in sorted(counts.items())) or "no flags"
        print(f"## {service} ({model}): {len(items)} translations, {summary}\n")
        # Flagged ones first, then the rest; the most recent first in each.
        shown = sorted(items, key=lambda i: (bool(i[3]), i[0]["ts"]), reverse=True)
        for run, source, out, found in shown[:per]:
            pair = run.get("translation_pair") or "?"
            print(f"- {run['ts']} · {pair} · {run['translation_ms']} ms" + (f" · FLAGS: {', '.join(found)}" if found else ""))
            print(f"  said: {source[:300]}")
            print(f"  got:  {out[:300]}")
        print()


def self_test():
    assert flags("Привіт, як справи?", "Hello, how are you?", "en") == []
    assert flags("Привіт, як справи?", "", "en") == ["failed"]
    assert flags("Hello there", "Hello there.", "en") == ["unchanged"]
    assert "script" in flags("Hello, how are you today?", "Hello, how are you today, friend?", "uk")
    assert flags("Hello, how are you today?", "Привіт, як ти сьогодні?", "uk") == []
    assert "length" in flags("Привіт, як справи? Що нового в тебе сьогодні?", "Hello.", "en")
    print("ok")


if __name__ == "__main__":
    if sys.argv[1:] == ["--test"]:
        self_test()
    else:
        main(sys.argv[1:])
