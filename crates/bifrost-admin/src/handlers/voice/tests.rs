use super::*;

#[test]
fn voice_sources_include_local_statuses() {
    let sources = discover_voice_sources();
    assert!(sources.iter().any(|source| source.id == "web_mic"));
    assert!(sources.iter().any(|source| source.id == "file:realtime"));
    assert!(sources.iter().any(|source| source.kind == "mic"));
    assert!(sources.iter().any(|source| source.kind == "system"));
    assert!(sources.iter().any(|source| source.kind == "app"));
}

#[test]
fn vocabulary_rewrites_aliases_without_touching_raw_input() {
    let vocabulary = VoiceVocabulary {
        version: 1,
        terms: vec![VoiceVocabularyTerm {
            canonical: "Bifrost".to_string(),
            aliases: vec!["宽增".to_string(), "白 Frost".to_string()],
            category: Some("project".to_string()),
        }],
    };
    let raw = "请打开宽增并搜索白 Frost 的日志";
    assert_eq!(
        apply_voice_vocabulary(raw, &vocabulary),
        "请打开Bifrost并搜索Bifrost 的日志"
    );
    assert_eq!(raw, "请打开宽增并搜索白 Frost 的日志");
}

#[test]
fn invalid_base64_audio_is_rejected() {
    let err = decode_voice_audio_payload("***").unwrap_err();
    assert!(err.contains("invalid base64 voice audio payload"));
}

#[test]
fn voice_provider_selection_defaults_and_validates() {
    assert_eq!(
        VoiceAsrProvider::from_query("source=file").unwrap(),
        VoiceAsrProvider::Qwen3Stateful
    );
    assert_eq!(
        VoiceAsrProvider::from_query("provider=qwen3_stateful_streaming").unwrap(),
        VoiceAsrProvider::Qwen3Stateful
    );
    assert!(VoiceAsrProvider::from_query("provider=qwen3_rs_http_chunked").is_err());
    assert!(VoiceAsrProvider::from_query("provider=remote_cloud").is_err());
}

#[test]
fn stateful_large_model_can_be_enabled_from_query() {
    assert!(!stateful_17b_enabled("provider=qwen3_stateful_streaming"));
    assert!(stateful_17b_enabled(
        "provider=qwen3_stateful_streaming&allow_stateful_17b=1"
    ));
}

#[test]
fn voice_target_defaults_to_realtime_06b() {
    let target = voice_target_from_query("source=web_mic&language=english").unwrap();
    assert_eq!(target.model, DEFAULT_VOICE_MODEL);
    assert_eq!(target.language, "english");
}

#[test]
fn voice_start_rejects_non_16k_pcm() {
    let error = VoiceAudioConfig::from_start(Some(48_000), Some(1), Some("pcm_s16le")).unwrap_err();
    assert!(error.contains("16000Hz PCM"));
    let error = VoiceAudioConfig::from_start(Some(16_000), Some(2), Some("pcm_s16le")).unwrap_err();
    assert!(error.contains("mono PCM"));
    let error = VoiceAudioConfig::from_start(Some(16_000), Some(1), Some("pcm_f32")).unwrap_err();
    assert!(error.contains("pcm_s16le"));
}

#[test]
fn partial_commit_keeps_stable_text_monotonic() {
    let mut state = VoiceTranscriptState::new(VoiceRuntimeTuning::from_query(""));
    state.partial = "hello bifrost".to_string();
    assert_eq!(state.committed, "");

    state.partial = "hello".to_string();
    assert_eq!(state.committed, "");

    state.partial = "hello bifrost voice".to_string();
    let (delta, partial) = state.commit_partial();
    assert_eq!(partial, "hello bifrost voice");
    assert_eq!(delta, "hello bifrost voice");
    assert_eq!(state.committed, "hello bifrost voice");

    state.partial = "hello bifrost voice input".to_string();
    let (delta, _) = state.commit_partial();
    assert_eq!(delta, " input");
    assert_eq!(state.committed, "hello bifrost voice input");
}

#[test]
fn voice_vad_detects_silence_and_speech_boundaries() {
    let silence = vec![0u8; 16_000 * 2];
    assert!(!is_voice_speech_chunk(&silence).unwrap());
    assert_eq!(
        voice_audio_chunk_duration_ms(&silence, VoiceAudioConfig::default()),
        1000
    );

    let mut speech = Vec::new();
    for _ in 0..16_000 {
        speech.extend_from_slice(&10_000i16.to_le_bytes());
    }
    assert!(is_voice_speech_chunk(&speech).unwrap());
}

#[test]
fn transcript_boundaries_use_testable_runtime_tuning() {
    let mut state = VoiceTranscriptState::new(VoiceRuntimeTuning::from_query(
        "silence_commit_ms=25&worker_idle_unload_ms=50&max_utterance_ms=75",
    ));
    state.partial = "hello bifrost".to_string();
    state.mark_audio_activity(10, 10, true);
    assert!(!state.should_commit_for_max_duration(74));
    assert!(state.should_commit_for_max_duration(75));

    state.mark_audio_activity(90, 25, false);
    assert!(state.should_commit_for_silence());
    let (delta, _) = state.commit_partial();
    assert_eq!(delta, "hello bifrost");

    state.mark_audio_activity(120, 50, false);
    assert!(state.should_unload_idle_worker());
}
