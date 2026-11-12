//! bifaria-asr (Windows, Linux): transcribes speech for Bifaria with transcribe-cpp on the GPU
//! (Vulkan) or the CPU. Same protocol as the Swift helper on the Mac: one JSON request per line on
//! stdin, one JSON answer per line on stdout, in order.
//!
//!   {"op":"load","dir":"<folder with one .gguf>","units":"vulkan"|"cpu"} → {"ok":true,"ms":…,"device":…}
//!   {"op":"transcribe","audio":"<base64 of 16 kHz mono f32 little-endian>"} → {"ok":true,"text":…,"ms":…}
//!   {"op":"ping"} → {"ok":true}
//!
//! Failures are {"ok":false,"error":"not_loaded|bad_request|internal","message":…}.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use base64::Engine as _;
use serde_json::{json, Value};
use transcribe_cpp::{Backend, Model, ModelOptions, RunOptions, Session};

const RATE: usize = 16_000;
/// Longest piece sent to the model at once, cut at the quietest 20 ms in the last 10 s before it
/// (as Bifaria does for Parakeet on the CPU).
const PIECE_SEC: usize = 60;
const CUT_SEARCH_SEC: usize = 10;

fn main() {
    if let Err(e) = transcribe_cpp::init_backends_default() {
        eprintln!("couldn't start the compute backends: {e}");
    }
    let mut loaded: Option<(Model, Session)> = None;
    let stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle(&request, &mut loaded),
            Err(_) => failure("bad_request", "Expected a JSON object with an \"op\""),
        };
        let mut out = stdout.lock();
        let _ = writeln!(out, "{answer}").and_then(|_| out.flush());
    }
}

fn handle(request: &Value, loaded: &mut Option<(Model, Session)>) -> Value {
    match request["op"].as_str() {
        Some("load") => {
            let Some(dir) = request["dir"].as_str() else { return failure("bad_request", "\"load\" needs \"dir\"") };
            let backend = match request["units"].as_str().unwrap_or("vulkan") {
                "vulkan" => Backend::Vulkan,
                "cpu" => Backend::Cpu,
                other => return failure("bad_request", &format!("Unknown \"units\" {other}; use vulkan or cpu")),
            };
            // One model at a time: the old one goes before the new one loads.
            *loaded = None;
            let start = Instant::now();
            match load(Path::new(dir), backend) {
                Ok((model, session)) => {
                    let device = if backend == Backend::Cpu { "CPU".to_string() } else { model.device().map(|d| d.description).unwrap_or_default() };
                    *loaded = Some((model, session));
                    json!({ "ok": true, "ms": start.elapsed().as_millis() as u64, "device": device })
                }
                Err(e) => failure("internal", &e),
            }
        }
        Some("transcribe") => {
            let Some(audio) = request["audio"].as_str().and_then(|a| base64::engine::general_purpose::STANDARD.decode(a).ok()) else {
                return failure("bad_request", "\"transcribe\" needs base64 \"audio\"");
            };
            let Some((_, session)) = loaded.as_mut() else { return failure("not_loaded", "Send \"load\" first") };
            let samples: Vec<f32> = audio.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
            let start = Instant::now();
            match transcribe(session, &samples) {
                Ok(text) => json!({ "ok": true, "text": text, "ms": start.elapsed().as_millis() as u64 }),
                Err(e) => failure("internal", &e),
            }
        }
        Some("ping") => json!({ "ok": true }),
        Some(op) => failure("bad_request", &format!("Unknown op {op}")),
        None => failure("bad_request", "Expected a JSON object with an \"op\""),
    }
}

fn load(dir: &Path, backend: Backend) -> Result<(Model, Session), String> {
    let file = gguf_in(dir)?;
    let model = Model::load_with(&file, &ModelOptions { backend, ..Default::default() }).map_err(|e| format!("couldn't load {}: {e}", file.display()))?;
    let session = model.session().map_err(|e| e.to_string())?;
    Ok((model, session))
}

/// The model file in `dir` (Bifaria keeps each model in its own folder).
fn gguf_in(dir: &Path) -> Result<PathBuf, String> {
    std::fs::read_dir(dir)
        .map_err(|e| format!("couldn't open {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|x| x == "gguf"))
        .ok_or_else(|| format!("no .gguf model in {}", dir.display()))
}

fn transcribe(session: &mut Session, samples: &[f32]) -> Result<String, String> {
    let mut texts = Vec::new();
    for piece in cut_at_pauses(samples, PIECE_SEC) {
        let result = session.run(&samples[piece], &RunOptions::default()).map_err(|e| e.to_string())?;
        let text = result.text.trim();
        if !text.is_empty() {
            texts.push(text.to_string());
        }
    }
    Ok(texts.join(" "))
}

/// Pieces of at most `max_sec`, each cut at the quietest 20 ms shortly before the limit.
fn cut_at_pauses(samples: &[f32], max_sec: usize) -> Vec<std::ops::Range<usize>> {
    let (max_len, search, frame) = (max_sec * RATE, CUT_SEARCH_SEC * RATE, RATE / 50);
    let energy = |f: &[f32]| f.iter().map(|x| x * x).sum::<f32>();
    let (mut pieces, mut start) = (Vec::new(), 0);
    while samples.len() - start > max_len {
        let end = start + max_len;
        let cut = (end - search..end - frame)
            .step_by(frame)
            .min_by(|&a, &b| energy(&samples[a..a + frame]).total_cmp(&energy(&samples[b..b + frame])))
            .map_or(end, |f| f + frame / 2);
        pieces.push(start..cut);
        start = cut;
    }
    if start < samples.len() {
        pieces.push(start..samples.len());
    }
    pieces
}

fn failure(code: &str, message: &str) -> Value {
    json!({ "ok": false, "error": code, "message": message })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_audio_is_cut_into_covering_pieces() {
        let samples: Vec<f32> = (0..RATE * 150).map(|i| if i % (RATE * 7) < RATE / 10 { 0.0 } else { 0.3 }).collect();
        let pieces = cut_at_pauses(&samples, PIECE_SEC);
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0].start, 0);
        assert_eq!(pieces.last().unwrap().end, samples.len());
        assert!(pieces.windows(2).all(|w| w[0].end == w[1].start));
        assert!(pieces.iter().all(|p| p.len() <= PIECE_SEC * RATE));
    }

    #[test]
    fn rejects_bad_requests() {
        let mut loaded = None;
        assert_eq!(handle(&json!({ "op": "transcribe", "audio": "AAAAAA==" }), &mut loaded)["error"], "not_loaded");
        assert_eq!(handle(&json!({ "op": "load", "dir": "/x", "units": "npu" }), &mut loaded)["error"], "bad_request");
        assert_eq!(handle(&json!({ "op": "nope" }), &mut loaded)["error"], "bad_request");
    }
}
