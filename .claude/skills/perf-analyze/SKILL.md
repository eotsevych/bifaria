---
name: perf-analyze
description: Analyze Bifaria's performance log (performance.jsonl) for slowdowns, memory growth and their causes, and its translations for speed and accuracy per service. Use when the user runs /perf-analyze or asks to analyze Bifaria performance, dictation speed, CPU/RAM usage, performance degradation, or how fast and how good translations are.
---

# Analyze Bifaria performance

Bifaria writes one JSON line per dictation (`"type": "dictation"`) and one every 5 minutes
(`"type": "heartbeat"`) to `~/Library/Application Support/bifaria/logs/performance.jsonl`
(rotated to `performance.1.jsonl` at 10 MB). The writer is `src-tauri/src/perf.rs`; the stage laps
are set in `stop_listening_and_process` in `src-tauri/src/app_controller.rs`.

Arguments (optional): a time window such as `today`, `last 3 days`, `since 2026-10-07`, a path
to a different log, or `translation` to look at translations only. Default: all data, with extra
attention to the last 24 hours.

## Steps

1. Run the summary report:
   ```bash
   python3 scripts/perf_report.py
   ```
   (Pass a path as the first argument for a different log.) If it says there are no dictations,
   tell the user and stop.

2. Dig into the raw lines for anything the report flags. Use `python3 -c` or `jq` over the JSONL
   rather than reading the whole file. Useful cuts:
   - speed_factor over time (by hour or by dictation number) to see a trend, not just day medians
   - the slowest 10 dictations with every stage, `usage` and `context`
   - non-`ok` outcomes and the stage they ended in (the latest stage present, in the pipeline
     order below; the keys in `stages_ms` are written alphabetically, not in order). Since 0.1.10
     an early-ending run files its unfinished time under that stage and stops its clock there, so
     the time an error stays on the pill isn't counted. Older runs lack that last stage.
   - heartbeats: `app_rss_mb` against `app_uptime_sec` (memory growth while idle), and app restarts
     (`app_uptime_sec` dropping back near 0)
   - `inference_ms` against `speech_sec` (inference should scale roughly linearly with speech)

3. Translations (skip if the report's Translation table is missing): run
   ```bash
   python3 scripts/translation_samples.py --per 8
   ```
   (`--since YYYY-MM-DD` for a window.) It pairs each translation with what was said, per service,
   and flags `failed`, `unchanged`, `script` (wrong alphabet) and `length` outliers. Then judge the
   samples yourself, as a bilingual reviewer would; see "Translation accuracy" below.

4. For specific slow runs, cross-check `transcription_pipeline.log` in the same folder around the
   same timestamp (`[PERF]`, `[MODEL_LOAD]`, `[IDLE_CLEANUP]`, `[VAD]`, `[PARAKEET_DONE]` lines).

## How to read the fields

- `stages_ms`: laps, in pipeline order: `stop_capture` (recorder flush + denoise), `audio_prep` (leveling +
  WAV), `vad`, `save_audio`, `model_ready`, `inference`, `post_process` (cleanup + language
  detection), `translation`, `paste`, `history_db`, `finalize`. Their sum is `total_ms`.
- `vad_no_speech` (since 1.0.6): Silero found no speech and the whole recording went to the model,
  so `speech_sec` is the full audio. Such runs usually end `empty`; the report counts how many
  still gave text. The pipeline log's `[VAD]` line says "no speech found" for them.
- `input_device`, `input_rate_hz` (since 1.0.6): the microphone that recorded and its rate. A
  Bluetooth headset in call mode records at 16 or 24 kHz. The report's "By input device" table
  counts every outcome per mic, so a mic that often hears nothing shows up as `empty` runs.
- `translation`: the service that translated ("LLM", "Custom API", "Apple", "Apple Intelligence",
  "Offline"; null when skipped). Lines written after 1.0.6 also have `translation_ms` (time in the
  service), `translation_in_chars` / `translation_out_chars` (0 out when it failed) and
  `translation_pair` ("uk → en"; Apple and Offline only before 1.0.8, every service since, with
  "auto" when the spoken language wasn't known). Since 1.0.8 `translation_model` names the LLM, or
  Apple's `fast` / `quality` model. Offline (Firefox Translations models) translates a pair without
  English through English (uk → en → de), so such pairs take about twice as long.
  Apple's on-device translation takes tens of ms per sentence with the Fast model and about half a
  second with Best quality; its first call after launch also starts the `bifaria-translate` helper
  and loads the model (2-3 s), which Bifaria does while the user is still talking.
- `speed_factor`: seconds of speech per second of inference. Higher is better. A falling trend at
  similar `speech_sec` is the clearest sign of real degradation.
- `model_wait_ms`: part of the `inference` lap spent waiting for the model to load. Large values
  mean the model wasn't ready (expected with `model_load_mode: on_stop`, or right after
  `IDLE_CLEANUP` freed it).
- `usage`: sampled every 250 ms while processing. CPU is % of one core (400 = four cores busy).
  `available_mem_min_mb` low or `swap_used_peak_mb` rising means memory pressure. On macOS
  `available_mem_min_mb` comes from the kernel's memory-pressure level; before 0.1.10 it was
  always 0 there, so ignore it in older lines.
- `app_version`: the Bifaria version that wrote the line (since 0.1.12; older lines lack it, so
  tell builds apart by restarts). Compare versions before blaming a release.
- `context`: `thermal_state` (nominal/fair/serious/critical), `low_power_mode`, `load_avg_1m`
  (other apps competing for CPU), `app_uptime_sec`, `dictations_since_launch`.

## What to look for

- **Degradation with uptime or dictation count** (speed falls or RAM grows the longer the app
  runs, recovering after a restart) → a leak or growing state in the app. Name the stage.
- **Slow runs explained by the machine**: `low_power_mode: true`, thermal `serious`/`critical`,
  high `load_avg_1m` or system CPU, low free RAM / swap growth. Say so instead of blaming Bifaria.
- **One stage dominating**: e.g. `vad` in seconds (it should be ~100 ms per 10 s of audio),
  `history_db` > 500 ms (DB size or disk), `paste` > 200 ms, `stop_capture` > 300 ms (denoise).
- **Model load cost**: `model_wait_ms` and `MODEL_LOAD` times; compare `on_start` vs `on_stop`
  runs for both latency and `app_rss_peak_mb`.
- **Failures/cancellations clustering** at a stage or time.

## Translation accuracy

The history has no reference translation, so accuracy is your judgment of the samples. For each
service, rate every sample on:
- **Meaning**: all of it kept, nothing added (4 = all, 3 = a detail off, 2 = part lost or wrong,
  1 = wrong or not translated).
- **Fluency**: reads like a native speaker wrote it (same 1-4 scale).
Note the errors that repeat: names and product words translated or mangled, English words in mixed
speech dropped, tone changed, sentences cut off (often the speech model's fault: compare with
"said"), or a prompt-following service (LLM, Apple Intelligence) answering or adding notes instead
of translating. Errors already in "said" are transcription errors; don't count them against the
translator.

Compare services only on similar text. For a fair head-to-head, ask the user to translate the same
few sentences with each service in Settings › Audio Labs › Translate Again (it runs the chosen
service on any text), then rate those side by side. Report speed and accuracy together: a service
that's 10x faster but loses meaning isn't the better one.

## Report back

Lead with the answer: is there degradation, how big, and the most likely cause (for translations:
which service is fastest and which is most accurate, per language pair), backed by numbers
(medians and counts, with the time range and n). Then list notable findings in order of impact,
each with the evidence and a concrete next step (a code location to look at, a setting to change,
or more data to collect). Say clearly when there's too little data to conclude something. Don't
paste the whole report table; quote only the rows that matter.
