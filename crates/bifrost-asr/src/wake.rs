use std::path::{Path, PathBuf};

pub const DEFAULT_WAKE_KWS_PROFILE: &str = "sherpa-onnx-kws-wenetspeech-3.3m";
pub const DEFAULT_WAKE_KWS_ARCHIVE_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/sherpa-onnx-kws-zipformer-wenetspeech-3.3M-2024-01-01.tar.bz2";
pub const DEFAULT_WAKE_KWS_ARCHIVE_NAME: &str =
    "sherpa-onnx-kws-zipformer-wenetspeech-3.3M-2024-01-01.tar.bz2";
pub const DEFAULT_WAKE_KWS_EXTRACTED_DIR: &str =
    "sherpa-onnx-kws-zipformer-wenetspeech-3.3M-2024-01-01";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SherpaKwsModelPack {
    pub profile: String,
    pub root_dir: PathBuf,
    pub model_dir: PathBuf,
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

impl SherpaKwsModelPack {
    pub fn for_root(root_dir: impl AsRef<Path>) -> Self {
        Self::for_profile_root(DEFAULT_WAKE_KWS_PROFILE, root_dir)
    }

    pub fn for_profile_root(profile: impl Into<String>, root_dir: impl AsRef<Path>) -> Self {
        let profile = profile.into();
        let root_dir = root_dir.as_ref().to_path_buf();
        let model_dir = root_dir.join(DEFAULT_WAKE_KWS_EXTRACTED_DIR);
        Self {
            profile,
            root_dir,
            encoder: model_dir.join("encoder-epoch-12-avg-2-chunk-16-left-64.int8.onnx"),
            decoder: model_dir.join("decoder-epoch-12-avg-2-chunk-16-left-64.int8.onnx"),
            joiner: model_dir.join("joiner-epoch-12-avg-2-chunk-16-left-64.int8.onnx"),
            tokens: model_dir.join("tokens.txt"),
            model_dir,
        }
    }

    pub fn archive_path(&self) -> PathBuf {
        self.root_dir.join(DEFAULT_WAKE_KWS_ARCHIVE_NAME)
    }

    pub fn is_ready(&self) -> bool {
        self.required_files()
            .iter()
            .all(|path| path.is_file() && path.metadata().map(|m| m.len() > 0).unwrap_or(false))
    }

    pub fn required_files(&self) -> Vec<PathBuf> {
        vec![
            self.encoder.clone(),
            self.decoder.clone(),
            self.joiner.clone(),
            self.tokens.clone(),
        ]
    }

    pub fn missing_files(&self) -> Vec<PathBuf> {
        self.required_files()
            .into_iter()
            .filter(|path| !path.is_file() || path.metadata().map(|m| m.len() == 0).unwrap_or(true))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WakeKwsDetection {
    pub keyword: String,
    pub start_time: f32,
    pub json: String,
}

pub fn normalize_wake_phrase(phrase: &str) -> String {
    phrase
        .to_lowercase()
        .chars()
        .filter(|ch| !ch.is_whitespace() && !is_wake_phrase_punctuation(*ch))
        .collect()
}

pub fn wake_phrase_matches(normalized_transcript: &str, normalized_binding: &str) -> bool {
    wake_phrase_variants(normalized_transcript)
        .iter()
        .any(|transcript| {
            wake_phrase_variants(normalized_binding)
                .iter()
                .any(|binding| binding.chars().count() >= 2 && transcript.contains(binding))
        })
}

pub fn keywords_buf_from_phrases(phrases: &[String]) -> Result<String, String> {
    let mut lines = Vec::new();
    for phrase in phrases {
        let line = keyword_line_from_phrase(phrase)?;
        if !line.trim().is_empty() && !lines.iter().any(|existing| existing == &line) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return Err("wake_kws_keywords_empty: no valid wake phrases".to_string());
    }
    Ok(format!("{}\n", lines.join("\n")))
}

#[cfg(feature = "wake-sherpa")]
pub fn keyword_line_from_phrase(phrase: &str) -> Result<String, String> {
    use pinyin::ToPinyin;

    let normalized = normalize_wake_phrase(phrase);
    if normalized.chars().count() < 2 {
        return Err("wake phrase must contain at least two normalized characters".to_string());
    }
    let mut tokens = Vec::new();
    for ch in normalized.chars() {
        if let Some(py) = ch.to_pinyin() {
            tokens.extend(split_pinyin_syllable(py.with_tone()));
        } else if ch.is_ascii_alphanumeric() {
            tokens.push(ch.to_string());
        }
    }
    if tokens.is_empty() {
        return Err(format!("wake phrase cannot be tokenized: {phrase}"));
    }
    Ok(format!("{} @{}", tokens.join(" "), normalized))
}

#[cfg(not(feature = "wake-sherpa"))]
pub fn keyword_line_from_phrase(_phrase: &str) -> Result<String, String> {
    Err("wake_sherpa_feature_disabled: lightweight KWS is not compiled".to_string())
}

#[cfg(feature = "wake-sherpa")]
pub fn detect_sherpa_kws_in_wav(
    pack: &SherpaKwsModelPack,
    wav_path: &Path,
    keywords_buf: &str,
    keywords_score: f32,
    keywords_threshold: f32,
) -> Result<Option<WakeKwsDetection>, String> {
    use sherpa_onnx::{KeywordSpotter, KeywordSpotterConfig, Wave};

    if !pack.is_ready() {
        return Err(format!(
            "voice_wake_kws_missing_assets: missing {}",
            pack.missing_files()
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let wav = Wave::read(&wav_path.display().to_string())
        .ok_or_else(|| format!("read wake KWS wav failed: {}", wav_path.display()))?;
    let mut config = KeywordSpotterConfig::default();
    config.model_config.transducer.encoder = Some(pack.encoder.display().to_string());
    config.model_config.transducer.decoder = Some(pack.decoder.display().to_string());
    config.model_config.transducer.joiner = Some(pack.joiner.display().to_string());
    config.model_config.tokens = Some(pack.tokens.display().to_string());
    config.model_config.num_threads = 1;
    config.model_config.provider = Some("cpu".to_string());
    config.keywords_score = keywords_score;
    config.keywords_threshold = keywords_threshold;
    config.keywords_buf = Some(keywords_buf.to_string());
    let kws = KeywordSpotter::create(&config)
        .ok_or_else(|| "create sherpa-onnx keyword spotter failed".to_string())?;
    let stream = kws.create_stream();
    stream.accept_waveform(wav.sample_rate(), wav.samples());
    stream.input_finished();
    while kws.is_ready(&stream) {
        kws.decode(&stream);
    }
    let Some(result) = kws.get_result(&stream) else {
        return Ok(None);
    };
    if result.keyword.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(WakeKwsDetection {
        keyword: normalize_wake_phrase(&result.keyword),
        start_time: result.start_time,
        json: result.json,
    }))
}

#[cfg(not(feature = "wake-sherpa"))]
pub fn detect_sherpa_kws_in_wav(
    _pack: &SherpaKwsModelPack,
    _wav_path: &Path,
    _keywords_buf: &str,
    _keywords_score: f32,
    _keywords_threshold: f32,
) -> Result<Option<WakeKwsDetection>, String> {
    Err("wake_sherpa_feature_disabled: lightweight KWS is not compiled".to_string())
}

fn wake_phrase_variants(normalized: &str) -> Vec<String> {
    let mut variants = vec![normalized.to_string()];
    if let Some(collapsed) = collapse_repeated_wake_phrase(normalized) {
        if collapsed != normalized {
            variants.push(collapsed);
        }
    }
    variants
}

fn collapse_repeated_wake_phrase(normalized: &str) -> Option<String> {
    let chars: Vec<char> = normalized.chars().collect();
    let len = chars.len();
    for unit_len in 2..=len / 2 {
        if !len.is_multiple_of(unit_len) {
            continue;
        }
        let unit = &chars[..unit_len];
        if chars.chunks(unit_len).all(|chunk| chunk == unit) {
            return Some(unit.iter().collect());
        }
    }
    None
}

#[cfg(feature = "wake-sherpa")]
fn split_pinyin_syllable(syllable: &str) -> Vec<String> {
    const INITIALS: &[&str] = &[
        "zh", "ch", "sh", "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x",
        "r", "z", "c", "s", "y", "w",
    ];
    for initial in INITIALS {
        if let Some(final_part) = syllable.strip_prefix(initial) {
            if !final_part.is_empty() {
                return vec![(*initial).to_string(), final_part.to_string()];
            }
        }
    }
    vec![syllable.to_string()]
}

fn is_wake_phrase_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation()
        || matches!(
            ch,
            '。' | '，'
                | '、'
                | '；'
                | '：'
                | '？'
                | '！'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '（'
                | '）'
                | '《'
                | '》'
                | '【'
                | '】'
                | '…'
                | '—'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_phrase_punctuation_and_space() {
        assert_eq!(normalize_wake_phrase(" 哈喽 哈喽。"), "哈喽哈喽");
        assert_eq!(normalize_wake_phrase("Hello, Bifrost!"), "hellobifrost");
    }

    #[test]
    fn phrase_match_collapses_repeated_wake_word() {
        assert!(wake_phrase_matches(
            &normalize_wake_phrase("哈喽"),
            &normalize_wake_phrase("哈喽哈喽。")
        ));
        assert!(!wake_phrase_matches(
            &normalize_wake_phrase("打开"),
            &normalize_wake_phrase("打开录音")
        ));
    }

    #[cfg(feature = "wake-sherpa")]
    #[test]
    fn keyword_line_uses_ppinyin_tokens() {
        let line = keyword_line_from_phrase("哈喽").expect("keyword line");
        assert!(line.ends_with(" @哈喽"));
        assert!(
            line.split('@')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .count()
                >= 2
        );
    }
}
