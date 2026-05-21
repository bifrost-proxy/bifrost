use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct VoiceVocabulary {
    #[serde(default = "default_vocabulary_version")]
    pub version: u32,
    #[serde(default)]
    pub terms: Vec<VoiceVocabularyTerm>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceVocabularyTerm {
    pub canonical: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct VoiceVocabularyImportRequest {
    #[serde(default)]
    pub terms: Vec<VoiceVocabularyTerm>,
}

fn default_vocabulary_version() -> u32 {
    1
}

fn voice_dir() -> PathBuf {
    bifrost_storage::data_dir().join("voice")
}

fn vocabulary_path() -> PathBuf {
    voice_dir().join("vocabulary.json")
}

pub fn load_voice_vocabulary() -> Result<VoiceVocabulary, String> {
    let path = vocabulary_path();
    if !path.exists() {
        return Ok(VoiceVocabulary {
            version: 1,
            terms: Vec::new(),
        });
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("read voice vocabulary {}: {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("parse voice vocabulary {}: {error}", path.display()))
}

pub fn save_voice_vocabulary(vocabulary: &VoiceVocabulary) -> Result<(), String> {
    let dir = voice_dir();
    fs::create_dir_all(&dir)
        .map_err(|error| format!("create voice directory {}: {error}", dir.display()))?;
    let path = vocabulary_path();
    let text = serde_json::to_string_pretty(vocabulary)
        .map_err(|error| format!("serialize voice vocabulary: {error}"))?;
    fs::write(&path, text)
        .map_err(|error| format!("write voice vocabulary {}: {error}", path.display()))
}

pub fn apply_voice_vocabulary(input: &str, vocabulary: &VoiceVocabulary) -> String {
    let mut output = input.to_string();
    for term in &vocabulary.terms {
        if term.canonical.trim().is_empty() {
            continue;
        }
        for alias in &term.aliases {
            let alias = alias.trim();
            if !alias.is_empty() {
                output = output.replace(alias, &term.canonical);
            }
        }
    }
    output
}
