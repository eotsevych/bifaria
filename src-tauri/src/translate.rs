use serde_json::json;
use std::collections::HashSet;

pub fn normalize_lang(name: &str) -> String {
    let n = name.trim().to_lowercase();
    match n.as_str() {
        "none" | "no translation" | "no_translation" | "no-translation" | "off" | "disabled" => {
            "none".to_string()
        }
        "uk" | "ukrainian" | "ua" | "українська" | "укр" => "ukrainian".to_string(),
        "en" | "english" | "us" | "англійська" => "english".to_string(),
        "ru" | "russian" | "російська" | "русский" => "russian".to_string(),
        "de" | "german" | "deutsch" | "німецька" => "german".to_string(),
        "es" | "spanish" | "іспанська" => "spanish".to_string(),
        "fr" | "french" | "французька" => "french".to_string(),
        "pl" | "polish" | "польська" => "polish".to_string(),
        "ja" | "japanese" | "японська" => "japanese".to_string(),
        _ => n,
    }
}

pub fn format_lang_name(name: &str) -> String {
    let norm = normalize_lang(name);
    match norm.as_str() {
        "ukrainian" => "Ukrainian".to_string(),
        "english" => "English".to_string(),
        "spanish" => "Spanish".to_string(),
        "french" => "French".to_string(),
        "german" => "German".to_string(),
        "italian" => "Italian".to_string(),
        "polish" => "Polish".to_string(),
        "japanese" => "Japanese".to_string(),
        "chinese" => "Chinese".to_string(),
        "russian" => "Russian".to_string(),
        other if !other.is_empty() => {
            let mut c = other.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        }
        _ => "Auto".to_string(),
    }
}

pub fn detect_text_language(text: &str) -> String {
    let clean = text.to_lowercase();

    // Fast path: uniquely Ukrainian vs uniquely Russian glyphs — biggest confusion source.
    // Order matters: check uk-specific first so "ї/є/ґ/і" beats ru "ы/э/ъ/ё" on mixed strings.
    let has_uk_specific = clean.chars().any(|c| matches!(c, 'ї' | 'є' | 'ґ' | 'і'));
    let has_ru_specific = clean.chars().any(|c| matches!(c, 'ы' | 'э' | 'ъ' | 'ё'));
    if has_uk_specific && !has_ru_specific {
        return "ukrainian".to_string();
    }
    if has_ru_specific && !has_uk_specific {
        return "russian".to_string();
    }
    if clean.chars().any(|c| "іїєґ".contains(c)) {
        return "ukrainian".to_string();
    }
    if clean.chars().any(|c| "ыэъё".contains(c)) {
        return "russian".to_string();
    }
    if clean.chars().any(|c| "ąćęłńóśźż".contains(c)) {
        return "polish".to_string();
    }
    if clean.chars().any(|c| "äöüß".contains(c)) {
        return "german".to_string();
    }
    if clean.chars().any(|c| "ñ¿¡".contains(c)) {
        return "spanish".to_string();
    }

    let words: HashSet<&str> = clean
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();

    // Expanded Ukrainian lexicon — includes high-frequency particles that survive bad ASR
    let uk_words: HashSet<&str> = [
        "і", "в", "на", "не", "що", "я", "з", "до", "по", "як", "це", "ти", "ми", "та", "ще",
        "би", "хотів", "але", "щоб", "для", "його", "так", "вони", "тут", "мене", "тебе",
        "було", "добре", "взагалі", "мов", "потрібно", "є", "їх", "її", "моя", "твоя", "наш",
        "будь", "ласка", "дуже", "дякую", "привіт", "так", "ні", "коли", "де", "чому", "хто",
        "зараз", "сьогодні", "вчора", "завтра", "років", "раз", "будь-який",
    ]
    .iter()
    .cloned()
    .collect();

    let ru_words: HashSet<&str> = [
        "и", "в", "не", "на", "что", "с", "по", "как", "это", "он", "к", "но", "они", "мы",
        "вы", "еще", "бы", "хотел", "было", "хорошо", "вообще", "которые", "нужно", "есть",
        "их", "её", "мой", "твой", "наш", "пожалуйста", "спасибо", "привет", "когда", "где",
        "почему", "кто", "сейчас", "сегодня", "вчера", "завтра", "лет", "раз",
    ]
    .iter()
    .cloned()
    .collect();

    let en_words: HashSet<&str> = [
        "the", "is", "and", "to", "of", "in", "that", "it", "for", "you", "with", "on", "as",
        "have", "but", "be", "at", "this", "from", "or", "by", "we", "are", "not", "can",
        "test", "okay", "let", "lets", "all", "hello", "good", "morning",
    ]
    .iter()
    .cloned()
    .collect();

    let uk_score = words.intersection(&uk_words).count();
    let ru_score = words.intersection(&ru_words).count();
    let en_score = words.intersection(&en_words).count();

    let cyrillic_chars = text.chars().filter(|c| ('\u{0400}'..='\u{04FF}').contains(c)).count();
    let latin_chars = text.chars().filter(|c| c.is_ascii_alphabetic()).count();

    if cyrillic_chars > latin_chars {
        if uk_score > ru_score {
            return "ukrainian".to_string();
        }
        if ru_score > uk_score {
            return "russian".to_string();
        }
        // Tie-break: uk-specific glyphs already returned above; remaining tie favors the
        // script majority without guessing — keep as ukrainian when both scores zero but
        // cyrillic dominates and ru-specific already ruled out (already returned).
        if uk_score == 0 && ru_score == 0 && cyrillic_chars > 0 {
            // Last resort: look at character-level n-gram-ish signal — count "о"/"а" vs nothing
            // Better than flipping to russian by default on short Ukrainian phrases.
            return "ukrainian".to_string();
        }
        if uk_score >= ru_score {
            return "ukrainian".to_string();
        }
        return "russian".to_string();
    }

    if latin_chars > 0 {
        if en_score > 0 || latin_chars > 3 {
            return "english".to_string();
        }
    }

    "unknown".to_string()
}

/// Instant local skip check (0.0001s, 0 network calls).
///
/// Rules:
/// - If target language is "none" or empty -> skip.
/// - If detected language matches target language -> skip (already in target language).
/// - If source language is configured as a specific language (not "auto"):
///   Only translate when detected language matches the configured source language!
///   If the user speaks any other language, skip translation.
/// - If source language is "auto":
///   Translate whenever detected language does not match target language.
pub fn should_skip(text: &str, whisper_lang: &str, source_lang: &str, target_lang: &str) -> bool {
    let target_norm = normalize_lang(target_lang);
    if target_norm == "none" || target_norm.is_empty() {
        return true;
    }

    let mut detected = normalize_lang(whisper_lang);
    if detected == "unknown" || detected == "auto" {
        detected = detect_text_language(text);
    }

    if detected == "unknown" {
        let source_norm = normalize_lang(source_lang);
        if source_norm != "auto" && source_norm != "unknown" && !source_norm.is_empty() {
            return true;
        }
        return false;
    }

    // Already in target language
    if detected == target_norm {
        return true;
    }

    let source_norm = normalize_lang(source_lang);
    if source_norm != "auto" && source_norm != "unknown" && !source_norm.is_empty() {
        // User configured a specific spoken language (e.g. Ukrainian).
        // Translation only launches when the detected language matches this spoken language.
        if detected != source_norm {
            return true;
        }
    }

    false
}

/// Checks whether a detected language or the recognized text matches an excluded language.
pub fn is_language_excluded(detected_lang: &str, text: &str, excluded_languages: &str) -> bool {
    let excluded_items: Vec<String> = excluded_languages
        .split(|c| c == ',' || c == ';' || c == '|' || c == '\n')
        .map(|s| normalize_lang(s.trim()))
        .filter(|s| !s.is_empty() && s != "none" && s != "auto")
        .collect();

    if excluded_items.is_empty() {
        return false;
    }

    let norm_detected = normalize_lang(detected_lang);
    if excluded_items.contains(&norm_detected) {
        return true;
    }

    let text_lang = detect_text_language(text);
    if excluded_items.contains(&text_lang) {
        return true;
    }

    // Explicit check for Russian-specific glyphs if Russian is in excluded list
    if excluded_items.contains(&"russian".to_string()) {
        let clean = text.to_lowercase();
        let has_ru_glyphs = clean.chars().any(|c| matches!(c, 'ы' | 'э' | 'ъ' | 'ё'));
        let has_uk_glyphs = clean.chars().any(|c| matches!(c, 'ї' | 'є' | 'ґ' | 'і'));
        if has_ru_glyphs && !has_uk_glyphs {
            return true;
        }
    }

    false
}

/// Picks the best candidate language to re-run transcription with when an excluded language is detected.
pub fn get_fallback_language_for_excluded(
    source_lang: &str,
    target_lang: &str,
    text: &str,
    excluded_languages: &str,
) -> String {
    let source_norm = normalize_lang(source_lang);
    let excluded_items: Vec<String> = excluded_languages
        .split(|c| c == ',' || c == ';' || c == '|' || c == '\n')
        .map(|s| normalize_lang(s.trim()))
        .filter(|s| !s.is_empty())
        .collect();

    // 1. If user configured a specific spoken language (e.g. Ukrainian), use it!
    if source_norm != "auto" && source_norm != "unknown" && !excluded_items.contains(&source_norm) {
        return source_norm;
    }

    // 2. If the text has Cyrillic characters and Russian was excluded, the natural intended language is Ukrainian
    let cyrillic_count = text.chars().filter(|c| ('\u{0400}'..='\u{04FF}').contains(c)).count();
    let latin_count = text.chars().filter(|c| c.is_ascii_alphabetic()).count();

    if cyrillic_count > latin_count {
        if !excluded_items.contains(&"ukrainian".to_string()) {
            return "ukrainian".to_string();
        }
    }

    // 3. If target language is non-empty and not excluded, check it
    let target_norm = normalize_lang(target_lang);
    if target_norm != "none" && target_norm != "unknown" && !excluded_items.contains(&target_norm) {
        return target_norm;
    }

    // 4. Default to English or Ukrainian
    if !excluded_items.contains(&"english".to_string()) {
        "english".to_string()
    } else {
        "ukrainian".to_string()
    }
}

/// Renders a translation prompt using template tags like {text}, {source_lang}, and {target_lang}.
pub fn render_prompt_template(
    template: &str,
    text: &str,
    source_lang: &str,
    target_lang: &str,
) -> String {
    let default_tmpl = "You are a strict translation engine. Translate the following text from {source_lang} to {target_lang}. Do not refuse. Do not explain. Do not add conversational text or notes. Output ONLY the exact translation using the native alphabet:\n\n{text}";
    let tmpl = if template.trim().is_empty() {
        default_tmpl
    } else {
        template.trim()
    };

    let s_lang = if source_lang.trim().is_empty() || source_lang.eq_ignore_ascii_case("auto") {
        "the detected spoken language"
    } else {
        source_lang.trim()
    };

    let t_lang = if target_lang.trim().is_empty() {
        "English"
    } else {
        target_lang.trim()
    };

    let mut rendered = tmpl
        .replace("{source_lang}", s_lang)
        .replace("{{source_lang}}", s_lang)
        .replace("{spoken_lang}", s_lang)
        .replace("{{spoken_lang}}", s_lang)
        .replace("{target_lang}", t_lang)
        .replace("{{target_lang}}", t_lang);

    if rendered.contains("{text}") || rendered.contains("{{text}}") || rendered.contains("{{input}}") {
        rendered = rendered
            .replace("{text}", text)
            .replace("{{text}}", text)
            .replace("{{input}}", text);
    } else {
        rendered.push_str("\n\nText:\n");
        rendered.push_str(text);
    }

    rendered
}

/// Translates text using any OpenAI-compatible provider (Ollama, LM Studio, Groq, Mistral, etc.)
pub async fn translate_with_openai_compatible(
    text: &str,
    endpoint_url: &str,
    api_key: &str,
    source_lang: &str,
    target_lang: &str,
    model: &str,
    prompt_template: &str,
) -> Result<String, String> {
    let mut url = endpoint_url.trim().to_string();
    if url.is_empty() {
        return Err("Translation endpoint URL is missing. Add URL in Bifaria settings.".to_string());
    }

    // Normalize URL
    if url.ends_with('/') {
        url.pop();
    }
    if !url.ends_with("/chat/completions") {
        if url.ends_with("/v1") {
            url.push_str("/chat/completions");
        } else {
            url.push_str("/v1/chat/completions");
        }
    }

    let prompt = render_prompt_template(prompt_template, text, source_lang, target_lang);
    let model_name = if model.trim().is_empty() {
        "llama3.2"
    } else {
        model.trim()
    };

    let payload = json!({
        "model": model_name,
        "messages": [
            { "role": "user", "content": prompt }
        ],
        "temperature": 0.1
    });

    let client = reqwest::Client::new();
    let mut req = client.post(&url).header("Content-Type", "application/json");

    if !api_key.trim().is_empty() {
        let key = api_key.trim();
        let auth_val = if key.to_lowercase().starts_with("bearer ") {
            key.to_string()
        } else {
            format!("Bearer {}", key)
        };
        req = req.header("Authorization", auth_val);
    }

    let resp = req
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Translation request failed: {}", e))?;

    let status = resp.status();
    let resp_bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read translation response: {}", e))?;

    let resp_val: serde_json::Value = serde_json::from_slice(&resp_bytes)
        .map_err(|e| format!("Invalid JSON from translation provider: {}", e))?;

    if !status.is_success() {
        let err_msg = resp_val
            .get("error")
            .and_then(|e| {
                if let Some(msg) = e.get("message").and_then(|m| m.as_str()) {
                    Some(msg)
                } else {
                    e.as_str()
                }
            })
            .unwrap_or("Unknown translation API error");
        return Err(format!("Translation HTTP Error {}: {}", status.as_u16(), err_msg));
    }

    let mut translated = String::new();
    if let Some(choices) = resp_val.get("choices").and_then(|c| c.as_array()) {
        if let Some(first) = choices.first() {
            if let Some(content) = first
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                translated = content.trim().to_string();
            }
        }
    }

    // Strip leading and trailing quotes if wrapped
    if (translated.starts_with('"') && translated.ends_with('"'))
        || (translated.starts_with('\'') && translated.ends_with('\''))
    {
        if translated.len() >= 2 {
            translated = translated[1..translated.len() - 1].trim().to_string();
        }
    }

    if translated.is_empty() {
        Err("Translation provider returned an empty response".to_string())
    } else {
        Ok(translated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_lang() {
        assert_eq!(normalize_lang("No Translation"), "none");
        assert_eq!(normalize_lang("none"), "none");
        assert_eq!(normalize_lang("no_translation"), "none");
        assert_eq!(normalize_lang("no-translation"), "none");
        assert_eq!(normalize_lang("off"), "none");
        assert_eq!(normalize_lang("disabled"), "none");
        assert_eq!(normalize_lang("English"), "english");
        assert_eq!(normalize_lang("UK"), "ukrainian");
    }

    #[test]
    fn test_should_skip() {
        // When target is No Translation, should always skip
        assert!(should_skip("Привіт світ", "uk", "Auto", "No Translation"));
        assert!(should_skip("Hello world", "en", "Auto", "none"));

        // When detected equals target, should skip
        assert!(should_skip("Hello world", "en", "Auto", "English"));
        assert!(should_skip("Hello world", "en", "Ukrainian", "English"));

        // User case: Spoken is configured as Ukrainian, Target is English:
        // 1. Spoken matches configured source (Ukrainian) -> Do NOT skip (translate to English!)
        assert!(!should_skip("Привіт світ", "uk", "Ukrainian", "English"));
        assert!(!should_skip("Привіт світ", "auto", "Ukrainian", "English"));

        // 2. User speaks Italian, but configured spoken is Ukrainian -> SKIP!
        assert!(should_skip("Ciao mondo come stai", "it", "Ukrainian", "English"));

        // 3. User speaks English, but configured spoken is Ukrainian -> SKIP!
        assert!(should_skip("Hello world how are you", "en", "Ukrainian", "English"));

        // 4. User speaks German, but configured spoken is Ukrainian -> SKIP!
        assert!(should_skip("Guten Tag wie geht es dir", "de", "Ukrainian", "English"));

        // Auto mode:
        // When source is Auto and detected != target -> Translate!
        assert!(!should_skip("Привіт світ", "uk", "Auto", "English"));
        assert!(!should_skip("Ciao mondo", "it", "Auto", "English"));
        // When source is Auto and detected == target -> Skip!
        assert!(should_skip("Hello world", "en", "Auto", "English"));
    }

    #[test]
    fn test_detect_text_language() {
        assert_eq!(detect_text_language("цікаво що ти знаєш про систему тривоги в Україні"), "ukrainian");
        assert_eq!(detect_text_language("Привіт! Як твої справи?"), "ukrainian");
        assert_eq!(detect_text_language("Hello world, this is an English sentence."), "english");
        assert_eq!(detect_text_language("Привет, это русский текст."), "russian");
    }

    #[test]
    fn test_is_language_excluded() {
        // Excluded by detected_lang code or name
        assert!(is_language_excluded("ru", "нейтральний текст", "Russian"));
        assert!(is_language_excluded("russian", "нейтральний текст", "ru, pl"));

        // Excluded by Russian-specific characters
        assert!(is_language_excluded("auto", "Привет, это было очень интересно", "Russian"));
        assert!(is_language_excluded("unknown", "слово с буквой ы", "Russian"));

        // NOT excluded when speech is Ukrainian
        assert!(!is_language_excluded("uk", "Привіт як справи", "Russian"));
        assert!(!is_language_excluded("ukrainian", "Цікаво що ти знаєш про систему", "Russian"));
        assert!(!is_language_excluded("en", "Hello world", "Russian"));

        // Empty exclusions list
        assert!(!is_language_excluded("ru", "Привет", ""));
    }

    #[test]
    fn test_get_fallback_language_for_excluded() {
        // Configured source language should be preserved if not excluded
        assert_eq!(
            get_fallback_language_for_excluded("Ukrainian", "English", "текст", "Russian"),
            "ukrainian"
        );

        // Auto source: Cyrillic text with Russian excluded resolves to Ukrainian
        assert_eq!(
            get_fallback_language_for_excluded("Auto", "English", "какой-то текст", "Russian"),
            "ukrainian"
        );

        // Auto source: Latin text resolves to English or target
        assert_eq!(
            get_fallback_language_for_excluded("Auto", "Spanish", "hello world", "Russian"),
            "spanish"
        );
    }

    #[test]
    fn test_render_prompt_template() {
        // Custom template with placeholders
        let custom = "Translate {text} from {source_lang} to {target_lang} politely.";
        let res = render_prompt_template(custom, "Hello", "English", "Spanish");
        assert_eq!(res, "Translate Hello from English to Spanish politely.");

        // Default template when empty
        let empty_res = render_prompt_template("", "Test speech", "Auto", "German");
        assert!(empty_res.contains("Translate the following text from the detected spoken language to German"));
        assert!(empty_res.contains("Test speech"));
    }
}
