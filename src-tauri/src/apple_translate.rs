//! The "Apple" translation provider: Apple's on-device Translation framework, the engine behind the
//! Translate app. No API key or network, and nothing leaves the Mac.
//!
//! The framework is Swift-only, so Bifaria talks to `bifaria-translate` (src-tauri/apple-translate),
//! a helper bundled next to the Bifaria executable. One helper process is started on first use and
//! kept running, so the language model stays loaded between dictations; it gets one JSON request
//! per line and answers each with one JSON line. Language downloads need Apple's own prompt, which
//! the helper shows in a small window of its own (`bifaria-translate download <source> <target>`).
//!
//! Needs macOS 26 or later: before that, Apple's framework translates only from inside a SwiftUI
//! view. Bifaria itself still runs on older macOS; there the provider is offered but disabled.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::translate::{format_lang_name, normalize_lang};

/// `translation_provider` value for this provider.
pub const PROVIDER: &str = "Apple";
/// First macOS version where the helper can translate.
pub const MIN_MACOS: u32 = 26;
const HELPER_NAME: &str = "bifaria-translate";
/// Longest a status or language-list request may take; translations use the caller's timeout.
const QUERY_TIMEOUT: Duration = Duration::from_secs(15);
/// Longest the download window may stay open.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Where Apple's language downloads are managed when the in-app download can't be used.
const SETTINGS_PATH: &str = "System Settings › General › Language & Region › Translation Languages";

/// True when `provider` (a `translation_provider` value) is this provider.
pub fn is_apple(provider: &str) -> bool {
    provider.trim().eq_ignore_ascii_case(PROVIDER)
}

/// BCP-47 code Apple's framework takes for a Bifaria language name or code: "Ukrainian" → "uk",
/// "Chinese" → "zh-Hans". `None` for Auto, unknown and "none". A code Bifaria has no name for
/// (e.g. Whisper's "ko") is passed through.
pub fn to_bcp47(lang: &str) -> Option<String> {
    let norm = normalize_lang(lang);
    let code = match norm.as_str() {
        "" | "auto" | "unknown" | "none" => return None,
        "english" => "en",
        "ukrainian" => "uk",
        "russian" => "ru",
        "german" => "de",
        "spanish" => "es",
        "french" => "fr",
        "polish" => "pl",
        "japanese" => "ja",
        "italian" => "it",
        "chinese" | "zh" => "zh-Hans",
        "portuguese" => "pt",
        "korean" => "ko",
        "dutch" => "nl",
        "turkish" => "tr",
        "arabic" => "ar",
        "hindi" => "hi",
        "thai" => "th",
        "vietnamese" => "vi",
        "indonesian" => "id",
        "swedish" => "sv",
        "danish" => "da",
        "finnish" => "fi",
        "hebrew" => "he",
        "malay" => "ms",
        "norwegian" => "nb",
        other => {
            // Already a code: "ko", "pt-BR", "zh-Hant".
            let mut parts = other.split('-');
            let base = parts.next().unwrap_or("");
            let base_ok = (2..=3).contains(&base.len()) && base.chars().all(|c| c.is_ascii_alphabetic());
            let rest_ok = parts.all(|p| (2..=4).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()));
            if !(base_ok && rest_ok) {
                return None;
            }
            return Some(fix_case(other));
        }
    };
    Some(code.to_string())
}

/// "pt-br" → "pt-BR", "zh-hant" → "zh-Hant".
fn fix_case(code: &str) -> String {
    code.split('-')
        .enumerate()
        .map(|(i, part)| match (i, part.len()) {
            (0, _) => part.to_lowercase(),
            (_, 4) => {
                let mut c = part.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
            }
            _ => part.to_uppercase(),
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// The helper's name for the model the `apple_translation_model` setting asks for.
pub fn model_name(setting: &str) -> &'static str {
    if setting.trim().eq_ignore_ascii_case("quality") {
        "highFidelity"
    } else {
        "lowLatency"
    }
}

/// The spoken language to translate from: the configured one, else the detected one. `None` lets
/// the helper detect it from the text.
pub fn source_code(configured: &str, detected: &str) -> Option<String> {
    to_bcp47(configured).or_else(|| to_bcp47(detected))
}

/// macOS version as (major, minor), e.g. (26, 1). `None` off macOS or when it can't be read.
pub fn macos_version() -> Option<(u32, u32)> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    parse_version(&sysinfo::System::os_version()?)
}

fn parse_version(v: &str) -> Option<(u32, u32)> {
    let mut parts = v.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

/// Why the provider can't be used on this Mac with an `os` version, if it can't. `helper_found`
/// says whether the helper is bundled.
fn unavailable_reason_for(os: Option<(u32, u32)>, helper_found: bool) -> Option<String> {
    match os {
        None => Some("Apple Translation is available only on macOS.".to_string()),
        Some((major, minor)) if major < MIN_MACOS => Some(format!(
            "Apple Translation needs macOS {MIN_MACOS} or later; this Mac runs macOS {major}.{minor}."
        )),
        _ if !helper_found => Some(format!(
            "This Bifaria build doesn't include the Apple Translation helper ({HELPER_NAME}). Reinstall Bifaria."
        )),
        _ => None,
    }
}

/// Why the provider can't be used on this Mac, or `None` when it can.
pub fn unavailable_reason() -> Option<String> {
    unavailable_reason_for(macos_version(), helper_path().is_some())
}

pub fn is_available() -> bool {
    unavailable_reason().is_none()
}

/// The helper next to the Bifaria executable (Contents/MacOS in the app, target/<profile> in dev).
/// Unit tests use the one build.rs made.
fn helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join(HELPER_NAME);
    #[cfg(test)]
    let path = if path.is_file() {
        path
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/bifaria-translate-universal-apple-darwin")
    };
    path.is_file().then_some(path)
}

/// A failed request, with the reason to show.
#[derive(Debug, Clone, PartialEq)]
pub enum AppleError {
    /// The language pair isn't downloaded yet.
    NotInstalled,
    /// Apple doesn't translate between these languages.
    Unsupported,
    /// The provider can't run here (old macOS, missing helper).
    Unavailable(String),
    Other(String),
}

impl AppleError {
    fn from_response(code: &str, message: &str) -> Self {
        match code {
            "not_installed" => AppleError::NotInstalled,
            "unsupported" => AppleError::Unsupported,
            _ => AppleError::Other(message.to_string()),
        }
    }

    /// The reason in words, naming the languages (e.g. "Ukrainian → English") and how to fix it.
    pub fn describe(&self, pair: &str) -> String {
        match self {
            AppleError::NotInstalled => format!(
                "{pair} isn't downloaded for Apple Translation yet. Download it in Settings → Translations → Language packs, or in {SETTINGS_PATH}"
            ),
            AppleError::Unsupported => format!("Apple Translation doesn't support {pair}"),
            AppleError::Unavailable(reason) => reason.clone(),
            AppleError::Other(message) => format!("Apple Translation failed: {message}"),
        }
    }
}

/// "Ukrainian → English" for codes "uk", "en"; a missing source reads "Detected language".
pub fn pair_name(source: Option<&str>, target: &str) -> String {
    let from = source.map(lang_name).unwrap_or_else(|| "Detected language".to_string());
    format!("{} → {}", from, lang_name(target))
}

fn lang_name(code: &str) -> String {
    match code {
        "zh-Hans" => "Chinese".to_string(),
        "zh-Hant" => "Chinese (Traditional)".to_string(),
        _ => format_lang_name(code),
    }
}

// ── Status cache ──
//
// Statuses seen in answers, by "source>target". Starting a recording reads it to warn about a pair
// that isn't downloaded without waiting on the helper.

fn status_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remember_status(source: &str, target: &str, status: &str) {
    status_cache().lock().unwrap().insert(format!("{source}>{target}"), status.to_string());
}

/// Last status seen for a pair: "installed", "supported" (downloadable) or "unsupported".
pub fn cached_status(source: &str, target: &str) -> Option<String> {
    status_cache().lock().unwrap().get(&format!("{source}>{target}")).cloned()
}

/// The source language of the last translation to each target, used to warm up the model.
fn last_source() -> &'static Mutex<HashMap<String, String>> {
    static LAST: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What keeps a recording from being translated with these languages, as a sentence to show; `None`
/// when nothing is known to be wrong. It doesn't wait on the helper: a pair is judged by the last
/// status seen. Only "unsupported" counts: a pair seen as not downloaded may have been downloaded
/// since (or was misreported right after an update), so it's tried, and a translation that fails
/// still pastes the spoken text with how to download the pair.
pub fn setup_problem(source_lang: &str, target_lang: &str) -> Option<String> {
    if let Some(reason) = unavailable_reason() {
        return Some(reason);
    }
    let Some(target) = to_bcp47(target_lang) else {
        return Some(format!("Apple Translation can't translate into {}", target_lang));
    };
    let source = to_bcp47(source_lang)?;
    let pair = pair_name(Some(&source), &target);
    match cached_status(&source, &target).as_deref() {
        Some("unsupported") => Some(AppleError::Unsupported.describe(&pair)),
        _ => None,
    }
}

// ── The helper process ──

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

fn worker() -> &'static tokio::sync::Mutex<Option<Worker>> {
    static WORKER: OnceLock<tokio::sync::Mutex<Option<Worker>>> = OnceLock::new();
    WORKER.get_or_init(|| tokio::sync::Mutex::new(None))
}

fn spawn_worker() -> Result<Worker, AppleError> {
    if let Some(reason) = unavailable_reason() {
        return Err(AppleError::Unavailable(reason));
    }
    let path = helper_path().ok_or_else(|| AppleError::Unavailable("The Apple Translation helper is missing".to_string()))?;
    let mut child = Command::new(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppleError::Other(format!("couldn't start {}: {}", HELPER_NAME, e)))?;
    let stdin = child.stdin.take().ok_or_else(|| AppleError::Other("helper has no stdin".to_string()))?;
    let stdout = child.stdout.take().ok_or_else(|| AppleError::Other("helper has no stdout".to_string()))?;
    log::info!("Started the Apple Translation helper (pid {:?})", child.id());
    Ok(Worker { child, stdin, stdout: BufReader::new(stdout).lines() })
}

/// Sends one request to the helper and returns its answer (`ok` true). Requests take turns. A
/// helper that stops answering in `timeout` is stopped; the next request starts a new one. A helper
/// whose pipe broke (it quit, or the async runtime it was started on is gone) is replaced and the
/// request sent again once; requests only read, so repeating one is harmless.
async fn request(req: Value, timeout: Duration) -> Result<Value, AppleError> {
    let mut line = req.to_string();
    line.push('\n');
    let mut slot = worker().lock().await;
    let mut attempt = 0;
    let answer = loop {
        attempt += 1;
        if slot.is_none() {
            *slot = Some(spawn_worker()?);
        }
        let w = slot.as_mut().unwrap();
        let exchange = async {
            w.stdin.write_all(line.as_bytes()).await?;
            w.stdin.flush().await?;
            w.stdout.next_line().await
        };
        let broken = match tokio::time::timeout(timeout, exchange).await {
            Ok(Ok(Some(answer))) => break answer,
            Ok(Ok(None)) => "the helper quit unexpectedly".to_string(),
            Ok(Err(e)) => format!("couldn't talk to the helper: {}", e),
            Err(_) => {
                if let Some(mut w) = slot.take() {
                    let _ = w.child.start_kill();
                }
                return Err(AppleError::Other(format!("timed out after {} s", timeout.as_secs())));
            }
        };
        if let Some(mut w) = slot.take() {
            let _ = w.child.start_kill();
        }
        if attempt >= 2 {
            return Err(AppleError::Other(broken));
        }
        log::warn!("Apple Translation: {}; starting the helper again", broken);
    };
    drop(slot);
    parse_answer(&answer)
}

fn parse_answer(answer: &str) -> Result<Value, AppleError> {
    let value: Value = serde_json::from_str(answer)
        .map_err(|_| AppleError::Other(format!("unexpected answer from the helper: {}", answer)))?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(value);
    }
    let code = value.get("error").and_then(Value::as_str).unwrap_or("internal");
    let message = value.get("message").and_then(Value::as_str).unwrap_or("unknown error");
    Err(AppleError::from_response(code, message))
}

/// A finished translation.
pub struct AppleTranslation {
    pub text: String,
    /// The language translated from: the one asked for, or the one the helper detected.
    pub source: String,
}

/// Translates `text` into `target`. With no `source`, the helper detects it.
/// `model` is the helper's model name (see `model_name`).
pub async fn translate(text: &str, source: Option<&str>, target: &str, model: &str, timeout: Duration) -> Result<AppleTranslation, AppleError> {
    let req = json!({ "op": "translate", "text": text, "source": source, "target": target, "model": model });
    let answer = request(req, timeout).await;
    if let Some(source) = source {
        match &answer {
            Ok(_) => remember_status(source, target, "installed"),
            Err(AppleError::NotInstalled) => remember_status(source, target, "supported"),
            Err(AppleError::Unsupported) => remember_status(source, target, "unsupported"),
            Err(_) => {}
        }
    }
    let answer = answer?;
    let text = answer.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let used = answer.get("source").and_then(Value::as_str).or(source).unwrap_or("").to_string();
    if text.is_empty() {
        return Err(AppleError::Other("returned an empty translation".to_string()));
    }
    last_source().lock().unwrap().insert(target.to_string(), used.clone());
    Ok(AppleTranslation { text, source: used })
}

/// Starts the helper and loads the model for the pair about to be translated, while the user is
/// still talking. With no `source` it uses the language last translated into `target`; without
/// one either, it only starts the helper. Errors are only logged: the translation reports them.
pub async fn warm_up(source: Option<String>, target: String, model: &'static str) {
    let source = source.or_else(|| last_source().lock().unwrap().get(&target).cloned());
    let req = match &source {
        Some(source) => json!({ "op": "warm", "source": source, "target": target, "model": model }),
        None => json!({ "op": "ping" }),
    };
    match request(req, QUERY_TIMEOUT).await {
        Ok(answer) => {
            if let Some(source) = &source {
                remember_status(source, &target, "installed");
            }
            log::info!("Apple Translation ready ({}): {}", pair_name(source.as_deref(), &target), answer);
        }
        Err(e) => {
            if let Some(source) = &source {
                match e {
                    AppleError::NotInstalled => remember_status(source, &target, "supported"),
                    AppleError::Unsupported => remember_status(source, &target, "unsupported"),
                    _ => {}
                }
            }
            log::warn!("Apple Translation warm-up: {}", e.describe(&pair_name(source.as_deref(), &target)));
        }
    }
}

/// Whether a pair is "installed", "supported" (downloadable) or "unsupported".
pub async fn status(source: &str, target: &str) -> Result<String, AppleError> {
    // "installed" whichever model is downloaded, so the preference doesn't matter here.
    let answer = request(json!({ "op": "status", "source": source, "target": target }), QUERY_TIMEOUT).await?;
    let status = answer.get("status").and_then(Value::as_str).unwrap_or("unsupported").to_string();
    remember_status(source, target, &status);
    Ok(status)
}

/// A language Apple can translate from, with its status for one target.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct LanguagePack {
    pub code: String,
    pub name: String,
    /// "installed", "supported" (can be downloaded) or "unsupported".
    pub status: String,
}

/// Every language Apple translates, with its status for translating into `target`.
pub async fn languages(target: &str) -> Result<Vec<LanguagePack>, AppleError> {
    let answer = request(json!({ "op": "languages", "target": target }), QUERY_TIMEOUT).await?;
    let packs: Vec<LanguagePack> = serde_json::from_value(answer.get("languages").cloned().unwrap_or(Value::Null))
        .map_err(|e| AppleError::Other(format!("unexpected language list: {}", e)))?;
    for pack in &packs {
        remember_status(&pack.code, target, &pack.status);
    }
    Ok(packs)
}

/// Shows Apple's download prompt for a pair (in the helper's own small window) and waits until it
/// is downloaded, cancelled or fails. `model` is the one to download when neither is.
pub async fn download(source: &str, target: &str, model: &str) -> Result<(), AppleError> {
    if let Some(reason) = unavailable_reason() {
        return Err(AppleError::Unavailable(reason));
    }
    let path = helper_path().ok_or_else(|| AppleError::Unavailable("The Apple Translation helper is missing".to_string()))?;
    let run = Command::new(path)
        .args(["download", source, target, model])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(DOWNLOAD_TIMEOUT, run)
        .await
        .map_err(|_| AppleError::Other("the download window was open too long".to_string()))?
        .map_err(|e| AppleError::Other(format!("couldn't open the download window: {}", e)))?;
    let answer = String::from_utf8_lossy(&output.stdout);
    let result = parse_answer(answer.lines().last().unwrap_or("")).map(|_| ());
    match &result {
        Ok(()) => remember_status(source, target, "installed"),
        Err(AppleError::Unsupported) => remember_status(source, target, "unsupported"),
        Err(_) => {}
    }
    result
}

// ── Apple Intelligence ──
//
// The same helper answers prompts with Apple Intelligence's on-device language model
// (FoundationModels): free, offline, no API key. It is an LLM, so unlike Apple Translation it
// follows the prompt template ("translate and fix grammar", "make it polite").

/// `translation_provider` value for Apple Intelligence.
pub const INTELLIGENCE_PROVIDER: &str = "Apple Intelligence";

pub fn is_apple_intelligence(provider: &str) -> bool {
    provider.trim().eq_ignore_ascii_case(INTELLIGENCE_PROVIDER)
}

/// Why Apple Intelligence can't be used on this Mac (old macOS, not turned on, no Apple silicon),
/// or `None` when it can.
pub async fn intelligence_unavailable_reason() -> Option<String> {
    if let Some(reason) = unavailable_reason() {
        return Some(reason.replace("Apple Translation", "Apple Intelligence"));
    }
    match request(json!({ "op": "llm_status" }), QUERY_TIMEOUT).await {
        Ok(answer) if answer.get("status").and_then(Value::as_str) == Some("available") => None,
        Ok(answer) => Some(answer.get("message").and_then(Value::as_str).unwrap_or("Apple Intelligence isn't available on this Mac.").to_string()),
        Err(e) => Some(format!("Couldn't ask Apple Intelligence: {}", e.describe(""))),
    }
}

/// Loads Apple Intelligence's model while the user is still talking. Errors are only logged: the
/// translation reports them.
pub async fn warm_up_intelligence() {
    if let Err(e) = request(json!({ "op": "llm_warm" }), QUERY_TIMEOUT).await {
        log::warn!("Apple Intelligence warm-up: {}", e.describe(""));
    }
}

/// Apple Intelligence's answer to `prompt` (a rendered prompt template).
pub async fn generate(prompt: &str, timeout: Duration) -> Result<String, String> {
    let answer = request(json!({ "op": "generate", "text": prompt }), timeout).await.map_err(|e| match e {
        AppleError::Unavailable(reason) => reason.replace("Apple Translation", "Apple Intelligence"),
        AppleError::Other(message) => message,
        other => other.describe(""),
    })?;
    let text = answer.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if text.is_empty() {
        return Err("Apple Intelligence returned an empty answer".to_string());
    }
    Ok(text)
}

/// Opens System Settings at Language & Region, where Translation Languages are managed.
pub fn open_system_settings() -> Result<(), String> {
    std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.Localization-Settings.extension")
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't open System Settings: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_language_names_and_codes_to_bcp47() {
        assert_eq!(to_bcp47("Ukrainian").as_deref(), Some("uk"));
        assert_eq!(to_bcp47("uk").as_deref(), Some("uk"));
        assert_eq!(to_bcp47("ua").as_deref(), Some("uk"));
        assert_eq!(to_bcp47("English").as_deref(), Some("en"));
        assert_eq!(to_bcp47("Chinese").as_deref(), Some("zh-Hans"));
        assert_eq!(to_bcp47("zh").as_deref(), Some("zh-Hans"));
        assert_eq!(to_bcp47("Japanese").as_deref(), Some("ja"));
        // Whisper codes Bifaria has no name for pass through.
        assert_eq!(to_bcp47("ko").as_deref(), Some("ko"));
        assert_eq!(to_bcp47("pt-br").as_deref(), Some("pt-BR"));
        assert_eq!(to_bcp47("zh-hant").as_deref(), Some("zh-Hant"));
        for none in ["Auto", "auto", "unknown", "", "No Translation", "Klingon language"] {
            assert_eq!(to_bcp47(none), None, "{none}");
        }
    }

    #[test]
    fn model_setting_maps_to_the_helpers_names() {
        assert_eq!(model_name("fast"), "lowLatency");
        assert_eq!(model_name(""), "lowLatency");
        assert_eq!(model_name("quality"), "highFidelity");
    }

    #[test]
    fn source_is_the_configured_language_else_the_detected_one() {
        assert_eq!(source_code("Ukrainian", "english").as_deref(), Some("uk"));
        assert_eq!(source_code("Auto", "ukrainian").as_deref(), Some("uk"));
        assert_eq!(source_code("Auto", "it").as_deref(), Some("it"));
        assert_eq!(source_code("Auto", "unknown"), None);
    }

    #[test]
    fn reports_why_the_provider_is_unavailable() {
        assert!(unavailable_reason_for(Some((26, 0)), true).is_none());
        assert!(unavailable_reason_for(Some((27, 1)), true).is_none());
        let old = unavailable_reason_for(Some((15, 5)), true).unwrap();
        assert!(old.contains("macOS 26 or later") && old.contains("15.5"), "{old}");
        assert!(unavailable_reason_for(Some((26, 0)), false).unwrap().contains("helper"));
        assert!(unavailable_reason_for(None, true).unwrap().contains("only on macOS"));
        assert_eq!(parse_version("26.0.1"), Some((26, 0)));
        assert_eq!(parse_version("11"), Some((11, 0)));
    }

    #[test]
    fn helper_answers_become_errors_with_reasons() {
        assert!(parse_answer(r#"{"ok":true,"text":"Hi"}"#).is_ok());
        assert_eq!(
            parse_answer(r#"{"ok":false,"error":"not_installed","message":"x"}"#),
            Err(AppleError::NotInstalled)
        );
        assert_eq!(parse_answer(r#"{"ok":false,"error":"unsupported","message":"x"}"#), Err(AppleError::Unsupported));
        assert_eq!(parse_answer(r#"{"ok":false,"error":"internal","message":"boom"}"#), Err(AppleError::Other("boom".into())));
        assert!(matches!(parse_answer("not json"), Err(AppleError::Other(_))));

        let pair = pair_name(Some("uk"), "en");
        assert_eq!(pair, "Ukrainian → English");
        let not_installed = AppleError::NotInstalled.describe(&pair);
        assert!(not_installed.contains("Ukrainian → English isn't downloaded") && not_installed.contains("Language packs"));
        assert_eq!(AppleError::Unsupported.describe(&pair), "Apple Translation doesn't support Ukrainian → English");
        assert_eq!(pair_name(None, "zh-Hans"), "Detected language → Chinese");
    }

    /// Through the real helper and Apple's framework, when this Mac can run them.
    #[tokio::test]
    async fn translates_ukrainian_with_the_helper() {
        if !is_available() {
            return;
        }
        match status("uk", "en").await.expect("status") .as_str() {
            "installed" => {
                let done = translate("Привіт, як справи?", Some("uk"), "en", "lowLatency", Duration::from_secs(20))
                    .await
                    .expect("uk → en is installed");
                assert!(done.text.to_lowercase().contains("hello"), "{}", done.text);
                assert_eq!(done.source, "uk");
                // No source: the helper detects Ukrainian.
                let detected = translate("Привіт, як справи?", None, "en", "lowLatency", Duration::from_secs(20)).await.unwrap();
                assert_eq!(detected.source, "uk");
            }
            "supported" => {
                let err = translate("Привіт", Some("uk"), "en", "lowLatency", Duration::from_secs(20)).await.err();
                assert_eq!(err, Some(AppleError::NotInstalled));
            }
            other => panic!("uk → en should be supported, got {other}"),
        }
        let err = translate("Hi", Some("be"), "en", "lowLatency", Duration::from_secs(20)).await.err();
        assert_eq!(err, Some(AppleError::Unsupported));
        let packs = languages("en").await.unwrap();
        assert!(packs.iter().any(|p| p.code == "uk"));
        assert!(packs.iter().all(|p| p.code != "en"));
    }

    /// Through the real helper and Apple Intelligence, when this Mac has it turned on.
    #[tokio::test]
    async fn apple_intelligence_answers_a_translation_prompt() {
        assert!(is_apple_intelligence("apple intelligence") && !is_apple_intelligence("Apple"));
        if intelligence_unavailable_reason().await.is_some() {
            return;
        }
        let prompt = crate::translate::render_prompt_template("", "Привіт, як справи?", "Ukrainian", "English");
        let text = generate(&prompt, Duration::from_secs(60)).await.expect("Apple Intelligence answers");
        assert!(text.to_lowercase().contains("hello") || text.to_lowercase().contains("hi"), "{text}");
    }

    #[test]
    fn only_an_unsupported_pair_blocks_the_recording() {
        if !is_available() {
            // Off macOS 26 the reason is the OS, checked above.
            assert!(setup_problem("Polish", "Japanese").is_some());
            return;
        }
        remember_status("pl", "ja", "installed");
        assert_eq!(setup_problem("Polish", "Japanese"), None);
        // Not downloaded is tried anyway; the translation reports it.
        remember_status("pl", "ja", "supported");
        assert_eq!(setup_problem("Polish", "Japanese"), None);
        remember_status("pl", "ja", "unsupported");
        assert!(setup_problem("Polish", "Japanese").unwrap().contains("doesn't support"));
        // An unknown spoken language can't be judged before the recording.
        assert_eq!(setup_problem("Auto", "Japanese"), None);
        assert!(setup_problem("Auto", "Klingon language").is_some());
    }
}
