#!/usr/bin/env python3
"""Refreshes src-tauri/src/model_catalog.json: the speech models Bifaria offers through its
transcribe-cpp helper, from Handy's catalog (github.com/cjpais/Handy, MIT), pinned to a commit.

Keeps speech-to-text models, minus Whisper (Bifaria runs it with whisper.cpp) and the Parakeet
models Bifaria defines itself (parakeet.rs), one file each (the catalog's default quantization).

Usage: python3 scripts/sync_model_catalog.py [handy-commit]
"""
import json
import subprocess
import sys
from pathlib import Path

HANDY_COMMIT = sys.argv[1] if len(sys.argv) > 1 else "1472087b7b17d44b6b344658ba7d3a8321214d6f"
URL = f"https://raw.githubusercontent.com/cjpais/Handy/{HANDY_COMMIT}/src-tauri/src/catalog/catalog.json"
# Built into Bifaria already: Whisper (whisper.cpp) and Parakeet v3 / Ultra (parakeet.rs).
SKIP_FAMILIES = {"whisper"}
SKIP_IDS = {"parakeet-tdt-0.6b-v3", "parakeet-ultra"}


def main() -> None:
    # curl, not urllib: it uses the system's certificates (python.org's Python on a Mac has none).
    catalog = json.loads(subprocess.run(["curl", "-sfL", URL], check=True, capture_output=True).stdout)
    models = []
    for m in catalog["models"]:
        if m.get("role") != "asr" or m["family"] in SKIP_FAMILIES or m["id"] in SKIP_IDS:
            continue
        file = m["files"][m["quant"]]
        models.append({
            "id": m["id"],
            "name": m["name"],
            "description": m.get("description", ""),
            "family": m["family"],
            "parameters": m.get("parameters", ""),
            "languages": m["languages"],
            "translate": bool(m["capabilities"].get("translate")),
            "curated": bool(m.get("curated")),
            "url": f"https://huggingface.co/{m['repository']}/resolve/{m['revision']}",
            "file": file["filename"],
            "size": file["size"],
            "sha256": file["sha256"],
        })
    out = {
        "source": f"Handy (MIT), github.com/cjpais/Handy/blob/{HANDY_COMMIT}/src-tauri/src/catalog/catalog.json",
        "transcribeCpp": catalog.get("transcribeCpp"),
        "models": models,
    }
    path = Path(__file__).resolve().parent.parent / "src-tauri/src/model_catalog.json"
    path.write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(f"{len(models)} models → {path}")


if __name__ == "__main__":
    main()
