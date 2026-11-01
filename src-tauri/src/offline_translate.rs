//! The "Offline" translation provider, for Windows and Linux (and Macs without Apple Translation):
//! the Firefox Translations models, run on the CPU by the `fxtranslate` crate. No API key or
//! network once a language pair is downloaded, and nothing leaves the computer.
//!
//! Every model translates to or from English, one direction each (20-55 MB). A pair without English
//! (uk → de) goes through English: uk → en, then en → de, so it needs both models.
//!
//! The engine can't move between threads, so it lives on one thread of its own that keeps the last
//! pair loaded between dictations.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use fxtranslate::cache::{Cache, ModelFiles};
use fxtranslate::engine::{Engine, Translation};
use fxtranslate::fetch::{DownloadOutcome, Fetch, FetchError, NetworkFetch, Sink};
use fxtranslate::remote::{fetch_records, pairs, records_url, Record, CDN_ROOT};
use fxtranslate::route::{resolve_route, PREFERRED_HUB};

use crate::apple_translate::{pair_name, to_bcp47, LanguagePack};

/// `translation_provider` value for this provider.
pub const PROVIDER: &str = "Offline";

/// True when `provider` (a `translation_provider` value) is this provider.
pub fn is_offline(provider: &str) -> bool {
    provider.trim().eq_ignore_ascii_case(PROVIDER)
}

fn models_dir() -> PathBuf {
    crate::transcribe::get_models_dir().join("translation")
}

fn cache() -> Cache {
    Cache::with_root(models_dir())
}

/// The downloaded models that translate `src` → `trg`, in the order they run: one, or two through
/// English. `None` while any of them is missing.
fn cached_legs(cache: &Cache, src: &str, trg: &str) -> Option<Vec<ModelFiles>> {
    if let Some(direct) = cache.cached_model(src, trg) {
        return Some(vec![direct]);
    }
    if src == PREFERRED_HUB || trg == PREFERRED_HUB {
        return None;
    }
    Some(vec![cache.cached_model(src, PREFERRED_HUB)?, cache.cached_model(PREFERRED_HUB, trg)?])
}

/// Why this processor can't run the models, if it can't. fxtranslate compiles its math for AVX2
/// (x86-64) or i8mm (ARM) with no runtime check, so without them the whole app would crash.
fn cpu_problem() -> Option<String> {
    #[cfg(target_arch = "x86_64")]
    if !std::arch::is_x86_feature_detected!("avx2") {
        return Some("Offline translation needs a processor with AVX2 (most made since 2013)".to_string());
    }
    #[cfg(target_arch = "aarch64")]
    if !std::arch::is_aarch64_feature_detected!("i8mm") {
        let hint = if cfg!(target_os = "macos") {
            "M1 Macs can use Apple Native translation instead after updating to macOS 26"
        } else {
            "most ARM processors made since 2022 have them"
        };
        return Some(format!("Offline translation needs a processor with ARM's i8mm instructions; {hint}"));
    }
    None
}

fn not_downloaded(src: &str, trg: &str) -> String {
    format!(
        "{} isn't downloaded for offline translation yet. Download it in Settings → Translations → Language packs",
        pair_name(Some(src), trg)
    )
}

/// What keeps a recording from being translated with these languages, as a sentence to show; `None`
/// when nothing is known to be wrong. With the spoken language on Auto, the pair is known only
/// after transcription, so it's not checked here.
pub fn setup_problem(source_lang: &str, target_lang: &str) -> Option<String> {
    if let Some(problem) = cpu_problem() {
        return Some(problem);
    }
    let Some(target) = to_bcp47(target_lang) else {
        return Some(format!("Offline translation can't translate into {target_lang}"));
    };
    let source = to_bcp47(source_lang)?;
    if source == target || cached_legs(&cache(), &source, &target).is_some() {
        return None;
    }
    Some(not_downloaded(&source, &target))
}

// ── The translation thread ──

struct Job {
    src: String,
    trg: String,
    /// Empty to only load the pair (warm-up).
    text: String,
    reply: tokio::sync::oneshot::Sender<Result<String, String>>,
}

fn load(legs: &[ModelFiles]) -> Result<Translation, String> {
    // Memory-mapped weights keep resident memory lower than owned copies.
    let engine = |f: &ModelFiles| Engine::load_mmapped(&f.model, &f.src_vocab, &f.trg_vocab);
    match legs {
        [one] => Ok(Translation::Direct(engine(one)?)),
        [first, second] => Ok(Translation::Pivot { pivot: PREFERRED_HUB.to_string(), first: engine(first)?, second: engine(second)? }),
        _ => Err("unexpected model route".to_string()),
    }
}

fn run(loaded: &mut Option<(String, Translation)>, job: &Job) -> Result<String, String> {
    let key = format!("{}>{}", job.src, job.trg);
    if loaded.as_ref().map(|(k, _)| k) != Some(&key) {
        *loaded = None; // frees the previous pair before loading the next
        if let Some(problem) = cpu_problem() {
            return Err(problem);
        }
        let legs = cached_legs(&cache(), &job.src, &job.trg).ok_or_else(|| not_downloaded(&job.src, &job.trg))?;
        *loaded = Some((key, load(&legs)?));
    }
    let (_, translation) = loaded.as_ref().unwrap();
    Ok(if job.text.is_empty() { String::new() } else { translation.translate_long(&job.text).trim().to_string() })
}

fn worker() -> &'static Sender<Job> {
    static WORKER: OnceLock<Sender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        let spawned = std::thread::Builder::new().name("offline-translate".into()).spawn(move || {
            let mut loaded = None;
            for job in rx {
                let answer = catch_unwind(AssertUnwindSafe(|| run(&mut loaded, &job))).unwrap_or_else(|_| {
                    loaded = None; // reload from disk next time rather than reuse a half-updated engine
                    Err("the translation engine crashed".to_string())
                });
                let _ = job.reply.send(answer);
            }
        });
        if let Err(e) = spawned {
            log::error!("Couldn't start the offline translation thread: {e}");
        }
        tx
    })
}

async fn ask(src: &str, trg: &str, text: &str) -> Result<String, String> {
    let (reply, answer) = tokio::sync::oneshot::channel();
    let job = Job { src: src.to_string(), trg: trg.to_string(), text: text.to_string(), reply };
    worker().send(job).map_err(|_| "the offline translator isn't running".to_string())?;
    answer.await.map_err(|_| "the offline translator stopped".to_string())?
}

/// Translates `text` from `source` into `target` (codes such as "uk", "en").
pub async fn translate(text: &str, source: &str, target: &str, timeout: Duration) -> Result<String, String> {
    let text = tokio::time::timeout(timeout, ask(source, target, text))
        .await
        .map_err(|_| format!("timed out after {} s", timeout.as_secs()))??;
    if text.is_empty() {
        return Err("returned an empty translation".to_string());
    }
    Ok(text)
}

/// Loads the pair about to be translated while the user is still talking. Errors are only logged:
/// the translation reports them.
pub async fn warm_up(source: String, target: String) {
    match ask(&source, &target, "").await {
        Ok(_) => log::info!("Offline translation ready ({})", pair_name(Some(&source), &target)),
        Err(e) => log::warn!("Offline translation warm-up: {e}"),
    }
}

// ── Language packs ──

/// Our copy of Firefox's model list and files (scripts/mirror_translation_models.py). Firefox's own
/// servers aren't meant for other apps and may change, so downloads never go there.
const MIRROR: &str = "https://huggingface.co/Eotsevych/revfly-translation-models/resolve/main";

/// fxtranslate's downloader, sent to the mirror: same paths, our host.
struct MirrorFetch(NetworkFetch);

fn mirrored(url: &str) -> String {
    if url == records_url() {
        return format!("{MIRROR}/records.json");
    }
    match url.strip_prefix(CDN_ROOT) {
        Some(path) => format!("{MIRROR}{path}"),
        None => url.to_string(),
    }
}

impl Fetch for MirrorFetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        self.0.get(&mirrored(url))
    }

    fn get_to(
        &self,
        url: &str,
        range_from: u64,
        sink: &mut dyn Sink,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<DownloadOutcome, FetchError> {
        self.0.get_to(&mirrored(url), range_from, sink, on_progress)
    }
}

fn mirror() -> MirrorFetch {
    MirrorFetch(NetworkFetch::new())
}

/// The mirror's model list, fetched once per launch: only the language list needs it.
async fn records() -> Result<Vec<Record>, String> {
    static RECORDS: Mutex<Option<Vec<Record>>> = Mutex::new(None);
    if let Some(records) = RECORDS.lock().unwrap().clone() {
        return Ok(records);
    }
    let records = tauri::async_runtime::spawn_blocking(|| fetch_records(&mirror()))
        .await
        .map_err(|e| e.to_string())??;
    *RECORDS.lock().unwrap() = Some(records.clone());
    Ok(records)
}

/// Every language the models translate, with its status for translating into `target`:
/// "installed", "supported" (can be downloaded) or "unsupported".
pub async fn languages(target: &str) -> Result<Vec<LanguagePack>, String> {
    if let Some(problem) = cpu_problem() {
        return Err(problem);
    }
    let records = records().await.map_err(|e| format!("Couldn't get the list of languages: {e}"))?;
    let cache = cache();
    let codes: std::collections::BTreeSet<String> = pairs(&records).into_iter().flat_map(|(s, t)| [s, t]).collect();
    let mut packs: Vec<LanguagePack> = codes
        .into_iter()
        .filter(|code| code != target)
        .map(|code| {
            let status = if cached_legs(&cache, &code, target).is_some() {
                "installed"
            } else if resolve_route(&records, &code, target).is_ok() {
                "supported"
            } else {
                "unsupported"
            };
            LanguagePack { name: fxtranslate::lang::display_name(&code).to_string(), code, status: status.to_string() }
        })
        .collect();
    packs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(packs)
}

/// Downloads every model `source` → `target` needs (both legs when it goes through English) and
/// checks each file's hash.
pub async fn download(source: &str, target: &str) -> Result<(), String> {
    let (src, trg) = (source.to_string(), target.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        fxtranslate::loader::ensure_route_files(&mirror(), &cache(), &src, &trg).map(drop)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("Couldn't download {}: {e}", pair_name(Some(source), target)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_pair(root: &std::path::Path, pair: &str) {
        let dir = root.join(pair);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("model.{pair}.intgemm.alphas.bin")), b"").unwrap();
        std::fs::write(dir.join(format!("vocab.{pair}.spm")), b"").unwrap();
    }

    #[test]
    fn finds_direct_and_english_pivot_routes_on_disk() {
        let root = std::env::temp_dir().join(format!("bifaria-offline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cache = Cache::with_root(&root);
        assert!(cached_legs(&cache, "uk", "en").is_none());

        fake_pair(&root, "uk-en");
        assert_eq!(cached_legs(&cache, "uk", "en").map(|l| l.len()), Some(1));
        // uk → de needs en → de too.
        assert!(cached_legs(&cache, "uk", "de").is_none());
        fake_pair(&root, "en-de");
        assert_eq!(cached_legs(&cache, "uk", "de").map(|l| l.len()), Some(2));
        // A missing English leg isn't papered over by the other direction.
        assert!(cached_legs(&cache, "en", "uk").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sends_firefox_urls_to_the_mirror() {
        assert_eq!(mirrored(&records_url()), format!("{MIRROR}/records.json"));
        assert_eq!(mirrored(&format!("{CDN_ROOT}/main-workspace/a.zst")), format!("{MIRROR}/main-workspace/a.zst"));
    }

    /// Downloads real models (about 60 MB) from the mirror into a fresh temp folder:
    /// `cargo test --release --lib -- --ignored offline_translates`.
    #[test]
    #[ignore]
    fn offline_translates_ukrainian_and_english_both_ways() {
        let root = std::env::temp_dir().join(format!("bifaria-offline-models-{}", std::process::id()));
        let cache = Cache::with_root(&root);
        for (src, trg) in [("uk", "en"), ("en", "uk")] {
            fxtranslate::loader::ensure_route_files(&mirror(), &cache, src, trg).unwrap();
        }
        for (src, trg, text) in [
            ("uk", "en", "Привіт, як справи? Сьогодні гарна погода, тому я піду гуляти в парк."),
            ("en", "uk", "Let's meet tomorrow at ten and discuss the release plan."),
        ] {
            let started = std::time::Instant::now();
            let translation = load(&cached_legs(&cache, src, trg).unwrap()).unwrap();
            let loaded = started.elapsed();
            let started = std::time::Instant::now();
            let out = translation.translate_long(text);
            println!("{src}→{trg}: load {loaded:?}, translate {:?}: {out}", started.elapsed());
            assert!(!out.trim().is_empty() && out != text);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recognizes_the_provider_name() {
        assert!(is_offline("Offline"));
        assert!(is_offline(" offline "));
        assert!(!is_offline("Apple"));
    }
}
