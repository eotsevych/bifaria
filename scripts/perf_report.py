#!/usr/bin/env python3
"""Summarizes Bifaria's performance log (logs/performance.jsonl) to spot slowdowns.

  python3 scripts/perf_report.py              # default log location on macOS
  python3 scripts/perf_report.py path/to/performance.jsonl

Shows per-day medians of each stage, how speed changes with app uptime, thermal state and Low Power
Mode, and how the app's memory moves between dictations (heartbeats).
"""
import json
import os
import statistics
import sys
from collections import defaultdict

DEFAULT_LOG = os.path.expanduser("~/Library/Application Support/bifaria/logs/performance.jsonl")


def median(values):
    values = [v for v in values if v is not None]
    return round(statistics.median(values), 1) if values else None


def load(path):
    rows = []
    for name in (path.replace(".jsonl", ".1.jsonl"), path):  # rotated file first, oldest data
        if os.path.exists(name):
            with open(name) as f:
                rows += [json.loads(line) for line in f if line.strip()]
    return rows


def table(title, headers, rows):
    print(f"\n{title}")
    widths = [max(len(str(x)) for x in col) for col in zip(headers, *rows)] if rows else [len(h) for h in headers]
    for line in [headers, ["-" * w for w in widths], *rows]:
        print("  " + "  ".join(str(x if x is not None else "-").rjust(w) for x, w in zip(line, widths)))


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_LOG
    rows = load(path)
    runs = [r for r in rows if r.get("type") == "dictation"]
    beats = [r for r in rows if r.get("type") == "heartbeat"]
    if not runs:
        print(f"No dictations in {path} yet.")
        return

    outcomes = defaultdict(int)
    for r in runs:
        outcomes[r["outcome"]] += 1
    print(f"{len(runs)} dictations, {len(beats)} heartbeats  |  " + ", ".join(f"{k}: {v}" for k, v in sorted(outcomes.items())))
    no_speech = [r for r in runs if r.get("vad_no_speech")]
    if no_speech:
        texted = sum(1 for r in no_speech if r["outcome"] == "ok")
        print(f"VAD found no speech in {len(no_speech)} (kept the whole recording); {texted} of them still gave text")

    ok = [r for r in runs if r["outcome"] in ("ok", "translation_failed")]
    # serde_json writes stages_ms keys sorted, so restore the pipeline order.
    order = ["stop_capture", "audio_prep", "vad", "save_audio", "model_ready", "inference",
             "post_process", "translation", "paste", "history_db", "finalize"]
    stages = sorted({s for r in ok for s in r["stages_ms"]}, key=lambda s: order.index(s) if s in order else len(order))

    by_day = defaultdict(list)
    for r in ok:
        by_day[r["ts"][:10]].append(r)
    table(
        "Median per day (ms; speed = seconds of speech per second of inference, higher is better)",
        ["day", "n", "total", "speed", "wait"] + stages + ["cpu avg%", "ram peak MB"],
        [
            [day, len(rs), median(r["total_ms"] for r in rs), median(r["speed_factor"] for r in rs),
             median(r["model_wait_ms"] for r in rs)]
            + [median(r["stages_ms"].get(s) for r in rs) for s in stages]
            + [median(r["usage"]["app_cpu_avg_pct"] for r in rs), median(r["usage"]["app_rss_peak_mb"] for r in rs)]
            for day, rs in sorted(by_day.items())
        ],
    )

    def grouped(title, key):
        groups = defaultdict(list)
        for r in ok:
            groups[key(r)].append(r)
        table(title, ["group", "n", "total ms", "speed", "inference ms"], [
            [g, len(rs), median(r["total_ms"] for r in rs), median(r["speed_factor"] for r in rs),
             median(r["inference_ms"] for r in rs)]
            for g, rs in sorted(groups.items(), key=lambda kv: str(kv[0]))
        ])

    def uptime_bucket(r):
        h = r["app_uptime_sec"] / 3600
        return "00-01h" if h < 1 else "01-06h" if h < 6 else "06-24h" if h < 24 else "24h+"

    grouped("By app uptime (slower with uptime = something degrades while the app runs)", uptime_bucket)
    grouped("By thermal state", lambda r: r["context"].get("thermal_state"))
    grouped("By Low Power Mode", lambda r: r["context"].get("low_power_mode"))
    grouped("By model load mode", lambda r: r.get("model_load_mode") or "-")

    # All outcomes, not only ok runs: a mic that hears nothing shows up as empty runs.
    by_device = defaultdict(list)
    for r in runs:
        if r.get("input_device"):
            by_device[r["input_device"]].append(r)
    if by_device:
        rows = []
        for name, rs in sorted(by_device.items(), key=lambda kv: -len(kv[1])):
            fine = [r for r in rs if r["outcome"] in ("ok", "translation_failed")]
            rates = sorted({r.get("input_rate_hz") for r in rs if r.get("input_rate_hz")})
            rows.append([name, len(rs), sum(r["outcome"] == "ok" for r in rs),
                         sum(r["outcome"] == "empty" for r in rs),
                         median(r["speed_factor"] for r in fine if r.get("speed_factor")) if fine else None,
                         "/".join(str(x) for x in rates) or None])
        table("By input device (since 1.0.6; all outcomes)", ["device", "n", "ok", "empty", "speed", "rate Hz"], rows)

    # Translation speed per service, model and language pair. Failed runs count, so they're shown.
    translated = [r for r in runs if r.get("translation_ms") is not None]
    if translated:
        groups = defaultdict(list)
        for r in translated:
            groups[(r["translation"], r.get("translation_model") or "-", r.get("translation_pair") or "-")].append(r)
        rows = []
        for (service, model, pair), rs in sorted(groups.items(), key=lambda kv: -len(kv[1])):
            done = [r for r in rs if r.get("translation_out_chars")]
            ms = sorted(r["translation_ms"] for r in done)
            rows.append([service, model, pair, len(rs), len(rs) - len(done),
                         median(ms), ms[int(len(ms) * 0.9)] if ms else None,
                         median(r["translation_ms"] * 100 / max(r["translation_in_chars"], 1) for r in done),
                         median(r["translation_out_chars"] / max(r["translation_in_chars"], 1) for r in done)])
        table("Translation (since 1.0.6; model and pair for every service since 1.0.8)",
              ["service", "model", "pair", "n", "failed", "median ms", "p90 ms", "ms/100 chars", "out/in chars"], rows)

    if beats:
        table("App memory between dictations (heartbeats, first / last / max)", ["", "first", "last", "max"], [
            ["app RAM MB", beats[0]["app_rss_mb"], beats[-1]["app_rss_mb"], max(b["app_rss_mb"] for b in beats)],
            ["swap MB", beats[0]["swap_used_mb"], beats[-1]["swap_used_mb"], max(b["swap_used_mb"] for b in beats)],
        ])

    slowest = sorted(ok, key=lambda r: r["total_ms"], reverse=True)[:5]
    table("Slowest dictations", ["time", "total", "speech s", "slowest stage", "thermal", "low power", "uptime h"], [
        [r["ts"], r["total_ms"], r["speech_sec"], max(r["stages_ms"].items(), key=lambda kv: kv[1])[0],
         r["context"].get("thermal_state"), r["context"].get("low_power_mode"), round(r["app_uptime_sec"] / 3600, 1)]
        for r in slowest
    ])


if __name__ == "__main__":
    main()
