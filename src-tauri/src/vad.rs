use crate::chunking::{self, Chunk};
use silero_vad_crs::{
    get_timestamps_from_probs_with_config, SileroVad, TimestampConfig, DEFAULT_CHUNK_SAMPLES,
};
use std::ops::Range;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct VadResult {
    pub samples: Vec<f32>,
    pub original_samples: usize,
    pub trimmed_samples: usize,
    pub original_duration_sec: f32,
    pub trimmed_duration_sec: f32,
    pub silence_removed_sec: f32,
    pub duration_ms: u64,
    /// Silero found no speech, so the whole recording was kept. False when VAD couldn't run.
    pub no_speech: bool,
    /// Where `samples` is cut for chunked transcription: one chunk when it is 40 s or less, or
    /// when VAD couldn't run.
    pub chunks: Vec<Chunk>,
}

impl VadResult {
    /// The parts of `samples` to transcribe one by one: the chunks with chunking on, else all of it.
    pub fn ranges(&self, chunking: bool) -> Vec<Range<usize>> {
        if chunking {
            self.chunks.iter().map(|c| c.range.clone()).collect()
        } else {
            vec![0..self.samples.len()]
        }
    }
}

/// Trims silence from 16 kHz mono audio using Silero VAD.
/// Keeps 400ms padding before voice start and after voice end.
/// If no speech is detected, falls back to the full original buffer.
pub fn trim_silence(samples: &[f32], sample_rate: u32) -> VadResult {
    let t0 = Instant::now();
    let original_len = samples.len();
    let original_dur = original_len as f32 / sample_rate as f32;

    if samples.is_empty() {
        return VadResult {
            samples: Vec::new(),
            original_samples: 0,
            trimmed_samples: 0,
            original_duration_sec: 0.0,
            trimmed_duration_sec: 0.0,
            silence_removed_sec: 0.0,
            duration_ms: t0.elapsed().as_millis() as u64,
            no_speech: false,
            chunks: Vec::new(),
        };
    }

    let mut vad = match SileroVad::new() {
        Ok(v) => v,
        Err(e) => {
            log::warn!("Silero VAD init error: {:?}. Returning full buffer.", e);
            return VadResult { duration_ms: t0.elapsed().as_millis() as u64, ..untrimmed(samples, sample_rate) };
        }
    };

    let probs = match vad.forward_audio(samples) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Silero VAD forward error: {:?}. Returning full buffer.", e);
            return VadResult { duration_ms: t0.elapsed().as_millis() as u64, ..untrimmed(samples, sample_rate) };
        }
    };

    let segments = get_timestamps_from_probs_with_config(&probs, samples.len(), timestamp_config(sample_rate));

    let no_speech = segments.is_empty();
    let start = if no_speech {
        log::info!("Silero VAD detected no active speech, preserving full buffer as fallback.");
        0
    } else {
        // Head-only trimming:
        // Trim silence before the first word (saving model inference time).
        // NEVER trim the ending of user-triggered audio recordings to avoid cutting off trailing syllables or words.
        let start = segments[0].start;
        if start < samples.len() { start } else { 0 }
    };
    let trimmed = samples[start..].to_vec();
    // Planned on the whole recording's scores, then moved to positions in `trimmed`.
    let chunks = chunking::plan(&probs, DEFAULT_CHUNK_SAMPLES, samples.len(), start)
        .into_iter()
        .map(|c| Chunk { range: c.range.start - start..c.range.end - start, ending: c.ending })
        .collect();

    let trimmed_len = trimmed.len();
    let trimmed_dur = trimmed_len as f32 / sample_rate as f32;
    let silence_removed = (original_dur - trimmed_dur).max(0.0);
    let ms = t0.elapsed().as_millis() as u64;

    VadResult {
        samples: trimmed,
        original_samples: original_len,
        trimmed_samples: trimmed_len,
        original_duration_sec: original_dur,
        trimmed_duration_sec: trimmed_dur,
        silence_removed_sec: silence_removed,
        duration_ms: ms,
        no_speech,
        chunks,
    }
}

/// The whole recording as one chunk, without running VAD: what `trim_silence` returns when VAD
/// can't run, and what a model that needs no trimming or chunking gets.
pub fn untrimmed(samples: &[f32], sample_rate: u32) -> VadResult {
    let duration = samples.len() as f32 / sample_rate as f32;
    VadResult {
        samples: samples.to_vec(),
        original_samples: samples.len(),
        trimmed_samples: samples.len(),
        original_duration_sec: duration,
        trimmed_duration_sec: duration,
        silence_removed_sec: 0.0,
        duration_ms: 0,
        no_speech: false,
        chunks: chunking::whole(0..samples.len()),
    }
}

/// Chunks of 16 kHz mono audio that didn't go through `trim_silence`. Audio of 40 s or less, or
/// that VAD can't score, is one chunk.
pub fn plan_chunks(samples: &[f32]) -> Vec<Chunk> {
    let whole = chunking::whole(0..samples.len());
    // Short audio is never cut, so it needs no VAD.
    if samples.len() <= chunking::MAX_CHUNK_SAMPLES {
        return whole;
    }
    let probs = SileroVad::new().map_err(|e| format!("{:?}", e)).and_then(|mut vad| vad.forward_audio(samples).map_err(|e| format!("{:?}", e)));
    match probs {
        Ok(probs) => chunking::plan(&probs, DEFAULT_CHUNK_SAMPLES, samples.len(), 0),
        Err(e) => {
            log::warn!("Silero VAD error: {}. Transcribing the audio in one piece.", e);
            whole
        }
    }
}

fn timestamp_config(sample_rate: u32) -> TimestampConfig {
    let mut config = TimestampConfig::default();
    config.sampling_rate = sample_rate as usize;
    config.speech_pad_ms = 400; // Keep 400ms margin before voice start and after voice end
    config.threshold = 0.25;    // Sensitive threshold to capture soft speech and whispers
    config.min_speech_duration_ms = 80;
    config.min_silence_duration_ms = 400;
    config
}

/// Whether 16 kHz mono audio contains any speech, judged like `trim_silence`. Says yes when the
/// VAD can't run, so a failure never raises a false "no speech" alarm.
pub fn has_speech(samples: &[f32]) -> bool {
    if samples.is_empty() {
        return false;
    }
    let Ok(mut vad) = SileroVad::new() else {
        return true;
    };
    let Ok(probs) = vad.forward_audio(samples) else {
        return true;
    };
    !get_timestamps_from_probs_with_config(&probs, samples.len(), timestamp_config(16000)).is_empty()
}

#[cfg(test)]
mod tests {
    /// Plans chunks for a real recording: `BIFARIA_WAV=path cargo test --lib real_recording -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_recording_chunks_cover_it_in_pieces_of_at_most_40_s() {
        let path = std::env::var("BIFARIA_WAV").expect("set BIFARIA_WAV to a recording");
        let (samples, rate) = crate::lab::read_wav_file(std::path::Path::new(&path)).unwrap();
        let samples = crate::audio::resample_to_16k(&samples, rate);
        let vad = super::trim_silence(&samples, 16000);
        assert_eq!(vad.chunks.first().unwrap().range.start, 0);
        assert_eq!(vad.chunks.last().unwrap().range.end, vad.samples.len());
        for pair in vad.chunks.windows(2) {
            assert_eq!(pair[0].range.end, pair[1].range.start);
        }
        for c in &vad.chunks {
            println!("{:7.2}s - {:7.2}s  {:?}", c.range.start as f32 / 16000.0, c.range.end as f32 / 16000.0, c.ending);
            assert!(c.range.len() <= crate::chunking::MAX_CHUNK_SAMPLES);
        }
    }
}
