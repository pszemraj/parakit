//! Source-language hints and generation limits for Qwen3-ASR.

/// Inference controls applied only to the detected `qwen3` backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QwenOptions {
    /// Supported language code or English name. `None` or `auto` detects the language.
    pub language: Option<String>,
    /// Maximum generated tokens per native audio chunk, from 1 through 4096.
    pub max_new_tokens: usize,
}

impl Default for QwenOptions {
    fn default() -> Self {
        Self {
            language: None,
            max_new_tokens: 512,
        }
    }
}

impl QwenOptions {
    /// Validate the language hint and generated-token budget.
    ///
    /// # Returns
    ///
    /// Success when both controls are supported.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsupported language or a budget outside 1–4096.
    pub fn validate(&self) -> Result<(), String> {
        self.normalized_language()?;
        if !(1..=4096).contains(&self.max_new_tokens) {
            return Err("Qwen max_new_tokens must be between 1 and 4096".to_string());
        }
        Ok(())
    }

    /// Resolve the hint to the English language name expected by Qwen's prompt.
    ///
    /// # Returns
    ///
    /// The canonical language name, or `None` for automatic detection.
    ///
    /// # Errors
    ///
    /// Returns an error when the hint is not a supported code or language name.
    pub fn normalized_language(&self) -> Result<Option<String>, String> {
        self.language
            .as_deref()
            .map_or(Ok(None), normalize_qwen_language)
    }
}

/// Normalize one of Qwen3-ASR's 30 language codes or English names.
///
/// # Arguments
///
/// * `language` - Case-insensitive supported code, English name, or `auto`.
///
/// # Returns
///
/// The canonical English language name, or `None` for `auto`.
///
/// # Errors
///
/// Returns an error for an empty or unsupported hint.
pub fn normalize_qwen_language(language: &str) -> Result<Option<String>, String> {
    let language = language.trim();
    if language.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    SUPPORTED_LANGUAGES
        .iter()
        .find(|(code, name)| {
            language.eq_ignore_ascii_case(code) || language.eq_ignore_ascii_case(name)
        })
        .map(|(_, name)| Some((*name).to_string()))
        .ok_or_else(|| format!("unsupported Qwen language {language:?}: use auto or a supported language code/name"))
}

// Official Qwen3-ASR language set. Canonical names bypass the native generic
// code map, which does not cover every Qwen language (notably yue and fil).
const SUPPORTED_LANGUAGES: [(&str, &str); 30] = [
    ("zh", "Chinese"),
    ("en", "English"),
    ("yue", "Cantonese"),
    ("ar", "Arabic"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("pt", "Portuguese"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ko", "Korean"),
    ("ru", "Russian"),
    ("th", "Thai"),
    ("vi", "Vietnamese"),
    ("ja", "Japanese"),
    ("tr", "Turkish"),
    ("hi", "Hindi"),
    ("ms", "Malay"),
    ("nl", "Dutch"),
    ("sv", "Swedish"),
    ("da", "Danish"),
    ("fi", "Finnish"),
    ("pl", "Polish"),
    ("cs", "Czech"),
    ("fil", "Filipino"),
    ("fa", "Persian"),
    ("el", "Greek"),
    ("hu", "Hungarian"),
    ("mk", "Macedonian"),
    ("ro", "Romanian"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_use_automatic_language_and_native_token_budget() {
        let options = QwenOptions::default();
        assert_eq!(options.normalized_language(), Ok(None));
        assert_eq!(options.max_new_tokens, 512);
        assert_eq!(options.validate(), Ok(()));
    }

    #[test]
    fn all_supported_codes_and_names_normalize_without_losing_languages() {
        for (code, name) in SUPPORTED_LANGUAGES {
            assert_eq!(normalize_qwen_language(code), Ok(Some(name.to_string())));
            assert_eq!(
                normalize_qwen_language(&name.to_ascii_uppercase()),
                Ok(Some(name.to_string()))
            );
        }
        assert_eq!(
            normalize_qwen_language(" yue "),
            Ok(Some("Cantonese".to_string()))
        );
        assert_eq!(
            normalize_qwen_language("FIL"),
            Ok(Some("Filipino".to_string()))
        );
        assert_eq!(normalize_qwen_language("ms"), Ok(Some("Malay".to_string())));
        assert_eq!(
            normalize_qwen_language("mk"),
            Ok(Some("Macedonian".to_string()))
        );
        assert_eq!(normalize_qwen_language(" AUTO "), Ok(None));
    }

    #[test]
    fn unsupported_and_empty_hints_fail_instead_of_becoming_auto() {
        for language in ["", " ", "xx", "bn", "en-US", "English\0", "中文"] {
            let options = QwenOptions {
                language: Some(language.to_string()),
                ..QwenOptions::default()
            };
            assert!(options.validate().is_err(), "{language:?}");
        }
    }

    #[test]
    fn token_budget_accepts_boundaries_and_rejects_out_of_range_values() {
        for max_new_tokens in [1, 512, 4096] {
            assert!(QwenOptions {
                max_new_tokens,
                ..QwenOptions::default()
            }
            .validate()
            .is_ok());
        }
        for max_new_tokens in [0, 4097, usize::MAX] {
            assert!(QwenOptions {
                max_new_tokens,
                ..QwenOptions::default()
            }
            .validate()
            .is_err());
        }
    }
}
