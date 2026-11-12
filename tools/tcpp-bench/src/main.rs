//! tcpp-bench: transcribes an audio file with transcribe-cpp the way Bifaria's Audio Lab would
//! (16 kHz mono, cut at pauses into pieces of at most 60 s) and reports the speed.
//!
//!     tcpp-bench <model.gguf> <audio file> [--backend auto|vulkan|cpu] [--max-minutes N]
//!
//! Prints the device used, model load time, and inference speed as × real time, and writes the
//! transcript next to the audio file.

use std::path::{Path, PathBuf};
use std::time::Instant;

use transcribe_cpp::{devices, init_backends_default, Backend, Model, ModelOptions, RunOptions};

const RATE: usize = 16_000;
/// As Bifaria's Audio Lab: pieces of at most 60 s, cut at the quietest 20 ms in the last 10 s.
const PIECE_SEC: usize = 60;
const CUT_SEARCH_SEC: usize = 10;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let (Some(model_path), Some(audio_path)) = (args.next(), args.next()) else {
        return Err("usage: tcpp-bench <model.gguf> <audio file> [--backend auto|vulkan|cpu] [--max-minutes N]".into());
    };
    let (mut backend, mut max_minutes) = (Backend::Auto, None::<f64>);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--backend" => {
                backend = match value.as_str() {
                    "auto" => Backend::Auto,
                    "vulkan" => Backend::Vulkan,
                    "cpu" => Backend::Cpu,
                    other => return Err(format!("unknown backend {other}")),
                }
            }
            "--max-minutes" => max_minutes = Some(value.parse().map_err(|_| "--max-minutes takes a number")?),
            other => return Err(format!("unknown option {other}")),
        }
    }

    init_backends_default().map_err(|e| format!("couldn't start the compute backends: {e}"))?;
    println!("transcribe-cpp {}", transcribe_cpp::version());
    println!("devices:");
    for d in devices() {
        println!("  {} [{}] {}", d.name, d.kind, d.description);
    }

    let t = Instant::now();
    let (samples, rate) = decode(Path::new(&audio_path))?;
    let mut pcm = resample_to_16k(&samples, rate);
    drop(samples);
    if let Some(m) = max_minutes {
        pcm.truncate(((m * 60.0) as usize * RATE).min(pcm.len()));
    }
    let audio_sec = pcm.len() as f64 / RATE as f64;
    println!("audio: {:.1} min, decoded and resampled in {} ms", audio_sec / 60.0, t.elapsed().as_millis());

    let t = Instant::now();
    let model = Model::load_with(&model_path, &ModelOptions { backend, ..Default::default() })
        .map_err(|e| format!("couldn't load {model_path} with {backend:?}: {e}"))?;
    let device = model.device().map(|d| format!("{} ({})", d.name, d.description)).unwrap_or_default();
    println!("model: {} {} on {} [{}], loaded in {} ms", model.arch(), model.variant(), model.backend(), device, t.elapsed().as_millis());
    let mut session = model.session().map_err(|e| e.to_string())?;

    let pieces = cut_at_pauses(&pcm, PIECE_SEC);
    let (mut texts, mut done) = (Vec::new(), 0usize);
    let t = Instant::now();
    for (i, piece) in pieces.iter().enumerate() {
        let result = session.run(&pcm[piece.clone()], &RunOptions::default()).map_err(|e| e.to_string())?;
        texts.push(result.text.trim().to_string());
        done += piece.len();
        if (i + 1) % 10 == 0 || i + 1 == pieces.len() {
            let spent = t.elapsed().as_secs_f64();
            println!("  {:>4}/{} pieces, {:5.1}% , {:.1}x real time so far", i + 1, pieces.len(), 100.0 * done as f64 / pcm.len() as f64, done as f64 / RATE as f64 / spent);
        }
    }
    let spent = t.elapsed().as_secs_f64();
    println!(
        "RESULT {} on {}: {:.1} min of audio in {:.1} s = {:.1}x real time",
        model.variant(),
        model.backend(),
        audio_sec / 60.0,
        spent,
        audio_sec / spent
    );

    let out = transcript_path(Path::new(&audio_path), Path::new(&model_path), &model.backend());
    std::fs::write(&out, texts.join(" ")).map_err(|e| format!("couldn't write {}: {e}", out.display()))?;
    println!("transcript: {}", out.display());
    Ok(())
}

fn transcript_path(audio: &Path, model: &Path, backend: &str) -> PathBuf {
    let stem = |p: &Path| p.file_stem().and_then(|s| s.to_str()).unwrap_or("x").to_string();
    audio.with_file_name(format!("{}.{}.{}.txt", stem(audio), stem(model), backend))
}

/// Any file symphonia reads, mixed down to mono at its own rate (as Bifaria's Audio Lab).
fn decode(path: &Path) -> Result<(Vec<f32>, u32), String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|e| format!("couldn't open {}: {e}", path.display()))?;
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, MediaSourceStream::new(Box::new(file), Default::default()), &Default::default(), &Default::default())
        .map_err(|e| format!("unsupported audio file: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .ok_or("the file has no audio track")?;
    let (track_id, rate) = (track.id, track.codec_params.sample_rate.unwrap_or(16_000));
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("can't decode this audio: {e}"))?;
    let mut samples = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(format!("couldn't read the audio: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("couldn't decode the audio: {e}")),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        samples.extend(buf.samples().chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32));
    }
    Ok((samples, rate))
}

/// Windowed-sinc resampling to 16 kHz with a 7.2 kHz low-pass, like Bifaria's (32 taps, Hann).
fn resample_to_16k(x: &[f32], rate: u32) -> Vec<f32> {
    if rate as usize == RATE {
        return x.to_vec();
    }
    let ratio = rate as f64 / RATE as f64;
    let fc = (7200.0 / rate as f64).min(0.48);
    const RADIUS: i64 = 16;
    let n_out = (x.len() as f64 / ratio) as usize;
    (0..n_out)
        .map(|i| {
            let pos = i as f64 * ratio;
            let center = pos.round() as i64;
            let (mut sum, mut weights) = (0.0f64, 0.0f64);
            for k in (center - RADIUS)..=(center + RADIUS) {
                if k < 0 || k as usize >= x.len() {
                    continue;
                }
                let d = k as f64 - pos;
                let sinc = if d.abs() < 1e-9 { 2.0 * fc } else { (2.0 * std::f64::consts::PI * fc * d).sin() / (std::f64::consts::PI * d) };
                let window = 0.5 + 0.5 * (std::f64::consts::PI * d / (RADIUS as f64 + 1.0)).cos();
                let w = sinc * window;
                sum += x[k as usize] as f64 * w;
                weights += w;
            }
            if weights.abs() > 1e-12 { (sum / weights) as f32 } else { 0.0 }
        })
        .collect()
}

/// Pieces of at most `max_sec`, each cut at the quietest 20 ms in the CUT_SEARCH_SEC before the limit.
fn cut_at_pauses(x: &[f32], max_sec: usize) -> Vec<std::ops::Range<usize>> {
    let (max_len, search, frame) = (max_sec * RATE, CUT_SEARCH_SEC * RATE, RATE / 50);
    let energy = |f: &[f32]| f.iter().map(|v| v * v).sum::<f32>();
    let (mut pieces, mut start) = (Vec::new(), 0);
    while x.len() - start > max_len {
        let end = start + max_len;
        let quietest = (end - search..end - frame)
            .step_by(frame)
            .min_by(|&a, &b| energy(&x[a..a + frame]).total_cmp(&energy(&x[b..b + frame])))
            .map_or(end, |f| f + frame / 2);
        pieces.push(start..quietest);
        start = quietest;
    }
    if start < x.len() {
        pieces.push(start..x.len());
    }
    pieces
}

#[cfg(test)]
mod tests {
    #[test]
    fn pieces_cover_the_audio_within_the_limit() {
        let x: Vec<f32> = (0..150 * super::RATE).map(|i| (i as f32 * 0.01).sin()).collect();
        let pieces = super::cut_at_pauses(&x, 60);
        assert_eq!(pieces.first().unwrap().start, 0);
        assert_eq!(pieces.last().unwrap().end, x.len());
        assert!(pieces.windows(2).all(|p| p[0].end == p[1].start));
        assert!(pieces.iter().all(|p| p.len() <= 60 * super::RATE));
    }

    #[test]
    fn resampling_48k_to_16k_keeps_a_tone() {
        let x: Vec<f32> = (0..48_000).map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin()).collect();
        let y = super::resample_to_16k(&x, 48_000);
        assert!((y.len() as i64 - 16_000).abs() <= 1);
        let rms = (y[1000..15000].iter().map(|v| v * v).sum::<f32>() / 14000.0).sqrt();
        assert!((rms - 0.707).abs() < 0.05, "rms {rms}");
    }
}
