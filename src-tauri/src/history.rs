use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::settings::{get_data_dir, AppSettings};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub id: i64,
    pub timestamp: String,
    pub raw_text: String,
    pub translated_text: String,
    pub source_lang: String,
    pub target_lang: String,
    pub duration: f32,
    pub audio_path: Option<String>,
}

pub struct HistoryManager {
    db_conn: Mutex<Connection>,
    audio_dir: PathBuf,
}

impl HistoryManager {
    pub fn new() -> Result<Self, String> {
        Self::new_in(&get_data_dir())
    }

    fn new_in(data_dir: &Path) -> Result<Self, String> {
        if !data_dir.exists() {
            let _ = fs::create_dir_all(&data_dir);
        }

        let audio_dir = data_dir.join("audio");
        if !audio_dir.exists() {
            let _ = fs::create_dir_all(&audio_dir);
        }

        let db_path = data_dir.join("history.db");
        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                raw_text TEXT NOT NULL,
                translated_text TEXT NOT NULL,
                source_lang TEXT NOT NULL,
                target_lang TEXT NOT NULL,
                duration REAL NOT NULL,
                audio_path TEXT
            )",
            [],
        )
        .map_err(|e| e.to_string())?;

        Ok(Self {
            db_conn: Mutex::new(conn),
            audio_dir,
        })
    }

    pub fn add_entry(
        &self,
        raw_text: &str,
        translated_text: &str,
        source_lang: &str,
        target_lang: &str,
        duration: f32,
        audio: Option<&crate::audio::CapturedAudio>,
        settings: &AppSettings,
    ) -> Result<i64, String> {
        let storage_mode = settings.storage_mode.as_str();
        if crate::settings::is_private(storage_mode) {
            return Ok(0);
        }
        // The app runs for weeks between launches, so old entries are dropped as new ones come in.
        let _ = self.enforce_retention_policy(settings.retention_days);

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let timestamp_str = format!("{}", now.as_secs());

        let mut audio_file_rel: Option<String> = None;

        if storage_mode == "text_audio" || storage_mode == "text_and_audio" {
            if let Some(audio) = audio {
                // Milliseconds, so two recordings that end in the same second keep their own files.
                let filename = format!("rec_{}.wav", now.as_millis());
                let full_path = self.audio_dir.join(&filename);

                // Saved leveled at the microphone's own rate, so playback keeps the full bandwidth.
                let (samples, sample_rate) = audio.playback();
                if let Ok(()) = write_wav_file(&full_path, &samples, sample_rate) {
                    audio_file_rel = Some(filename);
                }

                // Check and enforce storage cap after adding audio
                let _ = self.enforce_storage_cap(settings.storage_cap_mb);
            }
        }

        let conn = self.db_conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO history (timestamp, raw_text, translated_text, source_lang, target_lang, duration, audio_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                timestamp_str,
                raw_text,
                translated_text,
                source_lang,
                target_lang,
                duration,
                audio_file_rel,
            ],
        )
        .map_err(|e| e.to_string())?;

        let row_id = conn.last_insert_rowid();
        Ok(row_id)
    }

    /// Replaces the translated text of a saved entry (used when a failed translation is retried).
    pub fn update_translation(&self, id: i64, translated_text: &str) -> Result<(), String> {
        if id <= 0 {
            return Ok(()); // private mode: nothing was saved
        }
        let conn = self.db_conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE history SET translated_text = ?1 WHERE id = ?2",
            params![translated_text, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Sets the text of a recording whose transcription failed. It wasn't translated, so the
    /// translated text is the same.
    pub fn update_transcription(&self, id: i64, text: &str) -> Result<(), String> {
        if id <= 0 {
            return Ok(()); // private mode: nothing was saved
        }
        let conn = self.db_conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE history SET raw_text = ?1, translated_text = ?1 WHERE id = ?2",
            params![text, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_records(&self, limit: usize) -> Result<Vec<HistoryRecord>, String> {
        let conn = self.db_conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, timestamp, raw_text, translated_text, source_lang, target_lang, duration, audio_path
                 FROM history ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(HistoryRecord {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    raw_text: row.get(2)?,
                    translated_text: row.get(3)?,
                    source_lang: row.get(4)?,
                    target_lang: row.get(5)?,
                    duration: row.get(6)?,
                    audio_path: row.get(7)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut list = Vec::new();
        for r in rows {
            if let Ok(rec) = r {
                list.push(rec);
            }
        }
        Ok(list)
    }

    pub fn clear_history(&self) -> Result<(), String> {
        let conn = self.db_conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM history", [])
            .map_err(|e| e.to_string())?;

        // Delete all audio files in audio directory
        if let Ok(entries) = fs::read_dir(&self.audio_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    let _ = fs::remove_file(p);
                }
            }
        }

        // Also delete latest_recording.wav and latest_vad_trimmed.wav from data_dir
        let data_dir = get_data_dir();
        let latest = data_dir.join("latest_recording.wav");
        if latest.exists() {
            let _ = fs::remove_file(latest);
        }
        let vad = data_dir.join("latest_vad_trimmed.wav");
        if vad.exists() {
            let _ = fs::remove_file(vad);
        }

        // Also clean any stray wav files in data_dir
        if let Ok(entries) = fs::read_dir(&data_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    if let Some(ext) = p.extension() {
                        if ext == "wav" {
                            let _ = fs::remove_file(p);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn enforce_retention_policy(&self, retention_days: u32) -> Result<(), String> {
        if retention_days == 0 {
            return Ok(()); // 0 means Never
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let cutoff = now.saturating_sub((retention_days as u64) * 86400);

        let conn = self.db_conn.lock().map_err(|e| e.to_string())?;

        // Find audio paths to delete
        let mut stmt = conn
            .prepare("SELECT audio_path FROM history WHERE CAST(timestamp AS INTEGER) < ?1 AND audio_path IS NOT NULL")
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![cutoff as i64], |row| {
                row.get::<_, Option<String>>(0)
            })
            .map_err(|e| e.to_string())?;

        for r in rows.flatten().flatten() {
            let full_p = self.audio_dir.join(r);
            if full_p.exists() {
                let _ = fs::remove_file(full_p);
            }
        }

        conn.execute(
            "DELETE FROM history WHERE CAST(timestamp AS INTEGER) < ?1",
            params![cutoff as i64],
        )
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub fn enforce_storage_cap(&self, cap_mb: u64) -> Result<(), String> {
        let max_bytes = cap_mb * 1024 * 1024;
        let mut total_size: u64 = 0;
        let mut files_with_meta: Vec<(PathBuf, SystemTime, u64)> = Vec::new();

        if let Ok(entries) = fs::read_dir(&self.audio_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    if let Ok(meta) = p.metadata() {
                        let size = meta.len();
                        total_size += size;
                        let modified = meta.modified().unwrap_or(UNIX_EPOCH);
                        files_with_meta.push((p, modified, size));
                    }
                }
            }
        }

        if total_size <= max_bytes {
            return Ok(());
        }

        // Sort files by modified time ascending (oldest first - FIFO)
        files_with_meta.sort_by_key(|item| item.1);

        for (path, _, size) in files_with_meta {
            if total_size <= max_bytes {
                break;
            }
            if let Ok(()) = fs::remove_file(&path) {
                total_size = total_size.saturating_sub(size);
                // Nullify audio_path in DB if needed
                if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                    if let Ok(conn) = self.db_conn.lock() {
                        let _ = conn.execute(
                            "UPDATE history SET audio_path = NULL WHERE audio_path = ?1",
                            params![filename],
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Size of the saved recordings, which the storage cap limits.
    pub fn audio_bytes(&self) -> u64 {
        let Ok(entries) = fs::read_dir(&self.audio_dir) else { return 0 };
        entries.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()
    }

    pub fn get_audio_dir(&self) -> PathBuf {
        self.audio_dir.clone()
    }

    pub fn get_audio_file_path(&self, filename: &str) -> PathBuf {
        self.audio_dir.join(filename)
    }
}

pub fn write_wav_file(path: &Path, samples: &[f32], sample_rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for &sample in samples {
        let val = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        writer.write_sample(val).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_drops_only_entries_older_than_the_limit() {
        let dir = std::env::temp_dir().join(format!("revfly_history_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let history = HistoryManager::new_in(&dir).unwrap();
        let mut settings = AppSettings::default();
        settings.storage_mode = "text_audio".to_string();
        settings.retention_days = 0;
        let audio = crate::audio::CapturedAudio::empty();
        let old = history.add_entry("old", "old", "en", "en", 0.1, Some(&audio), &settings).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2)); // audio files are named by the millisecond
        let new = history.add_entry("new", "new", "en", "en", 0.1, Some(&audio), &settings).unwrap();
        let file = |id: i64| history.get_records(10).unwrap().into_iter().find(|r| r.id == id).and_then(|r| r.audio_path);
        let (old_file, new_file) = (file(old).unwrap(), file(new).unwrap());
        assert_ne!(old_file, new_file);
        let ten_days_ago = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() - 10 * 86400;
        history.db_conn.lock().unwrap().execute("UPDATE history SET timestamp = ?1 WHERE id = ?2", params![ten_days_ago.to_string(), old]).unwrap();

        history.enforce_retention_policy(7).unwrap();

        let ids: Vec<i64> = history.get_records(10).unwrap().iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![new]);
        assert!(!history.get_audio_file_path(&old_file).exists());
        assert!(history.get_audio_file_path(&new_file).exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
