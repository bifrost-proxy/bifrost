#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex as StdMutex, MutexGuard};
    use tempfile::TempDir;

    static TEST_DATA_DIR_LOCK: StdMutex<()> = StdMutex::new(());

    fn test_data_dir_lock() -> MutexGuard<'static, ()> {
        TEST_DATA_DIR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct EnvGuard {
        _guard: crate::test_env::BifrostDataDirGuard,
    }

    impl EnvGuard {
        fn set_data_dir(path: &Path) -> Self {
            Self {
                _guard: crate::test_env::BifrostDataDirGuard::set(path),
            }
        }
    }

    fn test_directory_task(id: &str, audio_dir: PathBuf) -> AsrDirectoryTask {
        AsrDirectoryTask {
            id: id.to_string(),
            name: id.to_string(),
            audio_dir,
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: AsrTaskSchedule::Hourly { minute: 0 },
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: Some(1),
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        }
    }

    #[test]
    fn runtime_strategy_defaults_to_reuse_per_file_for_old_task_json() {
        let json = r#"{
            "id":"legacy",
            "name":"Legacy",
            "audio_dir":"/tmp",
            "recursive":true,
            "enabled":true,
            "schedule":{"kind":"daily","hour":2,"minute":0},
            "language":"chinese",
            "model":"Qwen3-ASR-1.7B",
            "created_at_ms":1,
            "updated_at_ms":1,
            "last_run_at_ms":null,
            "next_run_at_ms":null,
            "last_error":null
        }"#;
        let task: AsrDirectoryTask = serde_json::from_str(json).unwrap();
        assert_eq!(task.runtime_strategy, AsrRuntimeStrategy::ReusePerFile);
        assert_eq!(task.max_concurrent_files, 1);
    }

    #[test]
    fn load_tasks_normalizes_legacy_home_and_relative_audio_dirs() {
        let _lock = test_data_dir_lock();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let home = dirs::home_dir().expect("home directory should be available in tests");
        let home_task = test_directory_task("legacy-home-task", PathBuf::from("~/audio"));
        let relative_task =
            test_directory_task("legacy-relative-task", PathBuf::from("recordings/audio"));

        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![home_task, relative_task],
        })
        .unwrap();

        let loaded = load_tasks();
        let loaded_home = loaded
            .tasks
            .iter()
            .find(|task| task.id == "legacy-home-task")
            .unwrap();
        let loaded_relative = loaded
            .tasks
            .iter()
            .find(|task| task.id == "legacy-relative-task")
            .unwrap();
        assert_eq!(loaded_home.audio_dir, home.join("audio"));
        assert_eq!(loaded_relative.audio_dir, home.join("recordings/audio"));
        assert!(!loaded_home.audio_dir.starts_with(temp.path()));
        assert!(!loaded_relative.audio_dir.starts_with(temp.path()));
    }

    #[test]
    fn max_concurrent_files_is_clamped_and_effective_for_fork_per_chunk() {
        let temp = tempfile::tempdir().unwrap();
        let mut task = test_directory_task("concurrency-task", temp.path().join("audio"));
        task.max_concurrent_files = 99;
        task.runtime_strategy = AsrRuntimeStrategy::ForkPerChunk;
        assert_eq!(normalize_max_concurrent_files(task.max_concurrent_files), 16);
        assert_eq!(effective_max_concurrent_files(&task), 16);

        task.runtime_strategy = AsrRuntimeStrategy::ReusePerFile;
        assert_eq!(effective_max_concurrent_files(&task), 1);

        task.runtime_strategy = AsrRuntimeStrategy::ForkPerChunk;
        task.diarization.enabled = true;
        assert_eq!(effective_max_concurrent_files(&task), 16);
    }

    #[test]
    fn running_task_allows_concurrency_update_but_rejects_runtime_risk() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let task = test_directory_task("running-concurrency-task", audio_dir);
        add_task(task.clone()).unwrap();
        RUNNING_TASKS.lock().unwrap().insert(task.id.clone());

        let update = UpdateTaskRequest {
            name: None,
            audio_dir: None,
            recursive: None,
            enabled: None,
            paused: None,
            schedule: None,
            language: None,
            model: None,
            runtime_strategy: None,
            max_concurrent_files: Some(4),
            diarization: None,
            daily_agent: None,
            external_devices: None,
            import_policy: None,
        };
        let updated = update_task_config(&task.id, update).unwrap();
        assert_eq!(updated.max_concurrent_files, 4);

        let risky_update = UpdateTaskRequest {
            name: None,
            audio_dir: None,
            recursive: None,
            enabled: None,
            paused: None,
            schedule: None,
            language: None,
            model: Some("Qwen3-ASR-0.6B".to_string()),
            runtime_strategy: None,
            max_concurrent_files: None,
            diarization: None,
            daily_agent: None,
            external_devices: None,
            import_policy: None,
        };
        let error = update_task_config(&task.id, risky_update).unwrap_err();
        assert_eq!(error.0, StatusCode::CONFLICT);

        RUNNING_TASKS.lock().unwrap().remove(&task.id);
    }

    #[test]
    fn diarization_cluster_count_prefers_known_then_max_then_default_cap() {
        let mut config = AsrDiarizationConfig::default();
        assert_eq!(
            resolved_diarization_cluster_count(&config),
            i32::from(DEFAULT_AUTO_MAX_SPEAKERS)
        );

        config.max_speakers = Some(3);
        assert_eq!(resolved_diarization_cluster_count(&config), 3);

        config.known_speaker_count = Some(2);
        assert_eq!(resolved_diarization_cluster_count(&config), 2);
    }

    #[test]
    fn diarization_profile_ready_requires_real_model_files() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let profile_dir = bifrost_storage::data_dir()
            .join("asr")
            .join("diarization")
            .join("profiles")
            .join(DEFAULT_DIARIZATION_PROFILE);
        std::fs::create_dir_all(&profile_dir).unwrap();
        std::fs::write(profile_dir.join("profile.json"), "{}").unwrap();
        std::fs::write(profile_dir.join("segmentation.ready"), "old marker").unwrap();
        std::fs::write(profile_dir.join("embedding.ready"), "old marker").unwrap();
        assert!(!diarization_profile_ready(DEFAULT_DIARIZATION_PROFILE));
    }

    #[test]
    fn voiceprint_enrollment_auto_prepares_default_diarization_profile() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());

        assert!(!voiceprint_dir().exists());
        ensure_diarization_profile_ready_for_voiceprint(DEFAULT_DIARIZATION_PROFILE).unwrap();
        assert!(voiceprint_dir().is_dir());
        assert!(diarization_profile_dir(DEFAULT_DIARIZATION_PROFILE).is_dir());
    }

    #[test]
    fn diarization_overlap_mapping_uses_model_segments() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let source_path = audio_dir.join("meeting.wav");
        std::fs::write(&source_path, b"audio").unwrap();

        let mut task = test_directory_task("diarization-task", audio_dir.clone());
        task.diarization.enabled = true;
        task.diarization.known_speaker_count = Some(2);
        let mut timeline = TranscriptTimeline {
            task_id: task.id.clone(),
            task_name: task.name.clone(),
            source_path: source_path.clone(),
            source_size: Some(5),
            source_modified_ms: None,
            source_created_at_ms: Some(1_000),
            source_created_at_source: Some("test".to_string()),
            media_duration_ms: Some(2_000),
            model: task.model.clone(),
            language: task.language.clone(),
            diarization_profile: None,
            speakers: Vec::new(),
            processed_at_ms: 2_000,
            segments: vec![
                TimelineSegment {
                    index: 0,
                    audio_start_ms: 0,
                    audio_end_ms: 1_000,
                    absolute_start_ms: Some(1_000),
                    absolute_end_ms: Some(2_000),
                    speaker: None,
                    speaker_display_name: None,
                    overlap: false,
                    text: "hello".to_string(),
                },
                TimelineSegment {
                    index: 1,
                    audio_start_ms: 1_000,
                    audio_end_ms: 2_000,
                    absolute_start_ms: Some(2_000),
                    absolute_end_ms: Some(3_000),
                    speaker: None,
                    speaker_display_name: None,
                    overlap: false,
                    text: "world".to_string(),
                },
            ],
        };

        let diarization_segments = vec![
            DiarizationSegment {
                speaker: "speaker_03".to_string(),
                display_name: "用户D".to_string(),
                mapped_profile_id: None,
                confidence: None,
                candidate_profile_id: None,
                candidate_display_name: None,
                candidate_confidence: None,
                start_ms: 0,
                end_ms: 1_100,
                overlap: false,
            },
            DiarizationSegment {
                speaker: "speaker_01".to_string(),
                display_name: "用户B".to_string(),
                mapped_profile_id: None,
                confidence: None,
                candidate_profile_id: None,
                candidate_display_name: None,
                candidate_confidence: None,
                start_ms: 1_100,
                end_ms: 2_000,
                overlap: false,
            },
        ];
        apply_speaker_segments_to_asr_timeline(&mut timeline, &diarization_segments).unwrap();
        timeline.diarization_profile = Some(task.diarization.profile.clone());
        timeline.speakers = speakers_from_diarization_segments(&diarization_segments);
        write_diarization_manifest(
            &task,
            &timeline,
            timeline.speakers.clone(),
            &diarization_segments,
        )
        .unwrap();

        assert_eq!(
            timeline.diarization_profile.as_deref(),
            Some(DEFAULT_DIARIZATION_PROFILE)
        );
        assert_eq!(timeline.speakers.len(), 2);
        assert_eq!(timeline.segments[0].speaker.as_deref(), Some("speaker_03"));
        assert_eq!(timeline.segments[1].speaker.as_deref(), Some("speaker_01"));
        assert!(diarization_manifest_path(&task.id, &source_path, &audio_dir).is_file());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn live_voiceprint_enrollment_writes_named_profile() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let session = SpeakerEnrollmentSession {
            id: "enroll-test".to_string(),
            speaker_name: "Eden".to_string(),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            audio_format: "pcm_s16le_mono".to_string(),
            prompts: voiceprint_prompts(),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        };
        let session_dir = speaker_enrollment_session_dir(&session.id);
        std::fs::create_dir_all(&session_dir).unwrap();
        atomic_json_write(&session_dir.join("session.json"), &session).unwrap();
        let one_second_pcm = (0..VOICEPRINT_SAMPLE_RATE)
            .flat_map(|index| {
                let sample = if index % 2 == 0 { 8_000i16 } else { -8_000i16 };
                sample.to_le_bytes()
            })
            .collect::<Vec<_>>();
        for prompt in &session.prompts {
            std::fs::write(speaker_audio_path(&session.id, &prompt.id), &one_second_pcm).unwrap();
        }

        let result = finish_speaker_enrollment(&session).unwrap();

        assert_eq!(result.profile.display_name, "Eden");
        assert_eq!(result.profile.source, "live_enrollment");
        assert_eq!(result.profile.sample_rate, VOICEPRINT_SAMPLE_RATE);
        assert!(result.profile.total_duration_ms >= VOICEPRINT_MIN_TOTAL_MS);
        assert!(result.profile_path.is_file());
        assert_eq!(load_registered_speaker_profiles().len(), 1);
    }

    #[test]
    fn voiceprint_mapping_replaces_generated_display_name() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        std::fs::create_dir_all(voiceprint_dir()).unwrap();
        let profile = SpeakerVoiceprintProfile {
            id: "spk-eden".to_string(),
            display_name: "Eden".to_string(),
            source: "live_enrollment".to_string(),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            embedding_model: "test".to_string(),
            embedding_dim: 2,
            embedding: vec![1.0, 0.0],
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            total_duration_ms: 3_000,
            samples: Vec::new(),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        };
        atomic_json_write(&speaker_profile_path(&profile.id), &profile).unwrap();
        let mut segments = vec![DiarizationSegment {
            speaker: "speaker_00".to_string(),
            display_name: "用户A".to_string(),
            mapped_profile_id: None,
            confidence: None,
            candidate_profile_id: None,
            candidate_display_name: None,
            candidate_confidence: None,
            start_ms: 0,
            end_ms: 1_000,
            overlap: false,
        }];
        let embeddings = BTreeMap::from([(
            "speaker_00".to_string(),
            vec![0.70, (1.0_f32 - 0.70_f32 * 0.70_f32).sqrt()],
        )]);

        map_speakers_with_registered_voiceprints(&mut segments, &embeddings);
        let speakers = speakers_from_diarization_segments(&segments);

        assert_eq!(segments[0].display_name, "Eden");
        assert_eq!(segments[0].mapped_profile_id.as_deref(), Some("spk-eden"));
        assert!((segments[0].confidence.unwrap() - 0.70).abs() < 0.001);
        assert_eq!(segments[0].candidate_profile_id.as_deref(), Some("spk-eden"));
        assert_eq!(segments[0].candidate_display_name.as_deref(), Some("Eden"));
        assert!((segments[0].candidate_confidence.unwrap() - 0.70).abs() < 0.001);
        assert_eq!(speaker_transcript_label(&segments[0]), "Eden (70% match)");
        assert_eq!(speakers[0].display_name, "Eden");
        assert_eq!(speakers[0].mapped_profile_id.as_deref(), Some("spk-eden"));
        assert!((speakers[0].confidence.unwrap() - 0.70).abs() < 0.001);
    }

    #[test]
    fn voiceprint_mapping_records_below_threshold_candidate() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        std::fs::create_dir_all(voiceprint_dir()).unwrap();
        let profile = SpeakerVoiceprintProfile {
            id: "spk-eden".to_string(),
            display_name: "Eden".to_string(),
            source: "live_enrollment".to_string(),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            embedding_model: "test".to_string(),
            embedding_dim: 2,
            embedding: vec![1.0, 0.0],
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            total_duration_ms: 3_000,
            samples: Vec::new(),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        };
        atomic_json_write(&speaker_profile_path(&profile.id), &profile).unwrap();
        let mut segments = vec![DiarizationSegment {
            speaker: "speaker_00".to_string(),
            display_name: "用户A".to_string(),
            mapped_profile_id: None,
            confidence: None,
            candidate_profile_id: None,
            candidate_display_name: None,
            candidate_confidence: None,
            start_ms: 0,
            end_ms: 1_000,
            overlap: false,
        }];
        let embeddings = BTreeMap::from([(
            "speaker_00".to_string(),
            vec![0.50, (1.0_f32 - 0.50_f32 * 0.50_f32).sqrt()],
        )]);

        map_speakers_with_registered_voiceprints(&mut segments, &embeddings);
        let speakers = speakers_from_diarization_segments(&segments);

        assert_eq!(segments[0].display_name, "用户A");
        assert_eq!(segments[0].mapped_profile_id, None);
        assert_eq!(segments[0].confidence, None);
        assert_eq!(segments[0].candidate_profile_id.as_deref(), Some("spk-eden"));
        assert_eq!(segments[0].candidate_display_name.as_deref(), Some("Eden"));
        assert!((segments[0].candidate_confidence.unwrap() - 0.50).abs() < 0.001);
        assert_eq!(speakers[0].mapped_profile_id, None);
        assert_eq!(speakers[0].candidate_display_name.as_deref(), Some("Eden"));
    }

    #[test]
    fn voiceprint_mapping_uses_single_registered_self_priority() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        std::fs::create_dir_all(voiceprint_dir()).unwrap();
        let profile = SpeakerVoiceprintProfile {
            id: "spk-eden".to_string(),
            display_name: "Eden".to_string(),
            source: "live_enrollment".to_string(),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            embedding_model: "test".to_string(),
            embedding_dim: 2,
            embedding: vec![1.0, 0.0],
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            total_duration_ms: 3_000,
            samples: Vec::new(),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        };
        atomic_json_write(&speaker_profile_path(&profile.id), &profile).unwrap();
        let mut segments = vec![
            DiarizationSegment {
                speaker: "speaker_00".to_string(),
                display_name: "用户A".to_string(),
                mapped_profile_id: None,
                confidence: None,
                candidate_profile_id: None,
                candidate_display_name: None,
                candidate_confidence: None,
                start_ms: 0,
                end_ms: 6_000,
                overlap: false,
            },
            DiarizationSegment {
                speaker: "speaker_01".to_string(),
                display_name: "用户B".to_string(),
                mapped_profile_id: None,
                confidence: None,
                candidate_profile_id: None,
                candidate_display_name: None,
                candidate_confidence: None,
                start_ms: 7_000,
                end_ms: 15_000,
                overlap: false,
            },
        ];
        let embeddings = BTreeMap::from([
            (
                "speaker_00".to_string(),
                vec![0.53, (1.0_f32 - 0.53_f32 * 0.53_f32).sqrt()],
            ),
            (
                "speaker_01".to_string(),
                vec![0.49, (1.0_f32 - 0.49_f32 * 0.49_f32).sqrt()],
            ),
        ]);

        map_speakers_with_registered_voiceprints(&mut segments, &embeddings);

        assert_eq!(segments[0].display_name, "Eden");
        assert_eq!(segments[0].mapped_profile_id.as_deref(), Some("spk-eden"));
        assert!((segments[0].confidence.unwrap() - 0.53).abs() < 0.001);
        assert_eq!(segments[1].display_name, "用户B");
        assert_eq!(segments[1].mapped_profile_id, None);
        assert_eq!(segments[1].candidate_display_name.as_deref(), Some("Eden"));
    }

    #[test]
    fn voiceprint_prompt_match_requires_substantial_reading() {
        let prompt = "今天我会用 Bifrost 录入自己的声纹，用于本地离线音频处理。";

        assert!(
            voiceprint_prompt_match_score(prompt, "今天我会用Bifrost录入自己的声纹用于本地离线音频处理")
                >= VOICEPRINT_PROMPT_MATCH_THRESHOLD
        );
        assert!(
            voiceprint_prompt_match_score(prompt, "今天我会用 Bifrost")
                < VOICEPRINT_PROMPT_MATCH_THRESHOLD
        );
    }

    #[test]
    fn voiceprint_prompt_match_strips_asr_tags() {
        let prompt = "今天我会用 Bifrost 录入自己的声纹，用于本地离线音频处理。";
        let transcript = "<asr_text>今天我会用 Bifrost 录入自己的声纹，用于本地离线音频处理。</asr_text>";

        assert_eq!(
            clean_voiceprint_asr_text(transcript),
            "今天我会用 Bifrost 录入自己的声纹，用于本地离线音频处理。"
        );
        assert!(voiceprint_prompt_match_score(prompt, transcript) >= 0.72);
    }

    #[test]
    fn voiceprint_prompt_verify_rejects_silence_before_asr() {
        let silence = vec![0u8; VOICEPRINT_SAMPLE_RATE as usize * 2 * 2];
        let speech = (0..VOICEPRINT_SAMPLE_RATE * 2)
            .flat_map(|index| {
                let sample = if index % 2 == 0 { 8_000i16 } else { -8_000i16 };
                sample.to_le_bytes()
            })
            .collect::<Vec<_>>();

        assert!(!voiceprint_prompt_audio_ready(&silence, VOICEPRINT_SAMPLE_RATE).unwrap());
        assert!(voiceprint_prompt_audio_ready(&speech, VOICEPRINT_SAMPLE_RATE).unwrap());
    }

    #[test]
    fn voiceprint_embedding_average_normalizes_multiple_prompt_embeddings() {
        let embeddings = vec![vec![1.0, 0.0], vec![0.0, 1.0]];

        let averaged = average_speaker_embeddings(&embeddings).unwrap();
        let norm = averaged
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();

        assert!((norm - 1.0).abs() < 0.0001);
        assert!((averaged[0] - averaged[1]).abs() < 0.0001);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn voiceprint_identity_matches_and_delete_removes_profile() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        std::fs::create_dir_all(voiceprint_dir()).unwrap();
        let audio = (0..VOICEPRINT_SAMPLE_RATE * 2)
            .flat_map(|index| {
                let sample = if index % 2 == 0 { 8_000i16 } else { -8_000i16 };
                sample.to_le_bytes()
            })
            .collect::<Vec<_>>();
        let waveform = pcm16le_to_f32(&audio).unwrap();
        let embedding = compute_speaker_embedding(DEFAULT_DIARIZATION_PROFILE, &waveform).unwrap();
        let profile = SpeakerVoiceprintProfile {
            id: "spk-eden".to_string(),
            display_name: "Eden".to_string(),
            source: "live_enrollment".to_string(),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            embedding_model: "test".to_string(),
            embedding_dim: embedding.len(),
            embedding,
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            total_duration_ms: 2_000,
            samples: Vec::new(),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        };
        atomic_json_write(&speaker_profile_path(&profile.id), &profile).unwrap();

        let prepared = prepare_voiceprint_identify_audio(&audio, VOICEPRINT_SAMPLE_RATE)
            .unwrap()
            .ready
            .unwrap();
        let identified = identify_speaker_voice(
            &prepared.waveform,
            prepared.audio_duration_ms,
            prepared.speech_duration_ms,
        )
        .unwrap();
        assert!(identified.matched);
        assert_eq!(identified.profile_id.as_deref(), Some("spk-eden"));
        assert_eq!(identified.display_name, "Eden");
        assert!(identified.confidence >= VOICEPRINT_SPEAKER_MATCH_THRESHOLD);
        assert_eq!(identified.status, "matched");
        assert!(identified.speech_duration_ms >= VOICEPRINT_MIN_IDENTIFY_SPEECH_MS);

        let response = delete_speaker_profile_response("spk-eden");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!speaker_profile_path("spk-eden").exists());
    }

    #[test]
    fn voiceprint_identity_short_audio_reports_insufficient_speech() {
        let short_speech = (0..VOICEPRINT_SAMPLE_RATE / 4)
            .flat_map(|index| {
                let sample = if index % 2 == 0 { 8_000i16 } else { -8_000i16 };
                sample.to_le_bytes()
            })
            .collect::<Vec<_>>();

        let prepared =
            prepare_voiceprint_identify_audio(&short_speech, VOICEPRINT_SAMPLE_RATE).unwrap();
        assert!(prepared.ready.is_none());
        assert!(prepared.speech_duration_ms > 0);

        let response = insufficient_speaker_identify_response(
            pcm16_duration_ms(short_speech.len() as u64, VOICEPRINT_SAMPLE_RATE),
            prepared.speech_duration_ms,
        );
        assert!(!response.matched);
        assert_eq!(response.status, "insufficient_audio");
        assert_eq!(response.reason.as_deref(), Some("need_more_speech"));
        assert_eq!(response.confidence, 0.0);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn voiceprint_identity_trims_edge_silence_before_matching() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        std::fs::create_dir_all(voiceprint_dir()).unwrap();
        let speech = (0..VOICEPRINT_SAMPLE_RATE * 2)
            .flat_map(|index| {
                let sample = if index % 2 == 0 { 8_000i16 } else { -8_000i16 };
                sample.to_le_bytes()
            })
            .collect::<Vec<_>>();
        let waveform = pcm16le_to_f32(&speech).unwrap();
        let embedding = compute_speaker_embedding(DEFAULT_DIARIZATION_PROFILE, &waveform).unwrap();
        let profile = SpeakerVoiceprintProfile {
            id: "spk-eden".to_string(),
            display_name: "Eden".to_string(),
            source: "live_enrollment".to_string(),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            embedding_model: "test".to_string(),
            embedding_dim: embedding.len(),
            embedding,
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            total_duration_ms: 2_000,
            samples: Vec::new(),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        };
        atomic_json_write(&speaker_profile_path(&profile.id), &profile).unwrap();

        let silence = vec![0u8; VOICEPRINT_SAMPLE_RATE as usize * 2];
        let mut audio = Vec::new();
        audio.extend_from_slice(&silence);
        audio.extend_from_slice(&speech);
        audio.extend_from_slice(&silence);

        let prepared = prepare_voiceprint_identify_audio(&audio, VOICEPRINT_SAMPLE_RATE)
            .unwrap()
            .ready
            .unwrap();
        assert!(prepared.audio_duration_ms >= 4_000);
        assert!(prepared.speech_duration_ms < prepared.audio_duration_ms);
        let identified = identify_speaker_voice(
            &prepared.waveform,
            prepared.audio_duration_ms,
            prepared.speech_duration_ms,
        )
        .unwrap();

        assert!(identified.matched);
        assert_eq!(identified.profile_id.as_deref(), Some("spk-eden"));
        assert_eq!(identified.display_name, "Eden");
    }

    #[test]
    fn voiceprint_identity_uses_sixty_percent_threshold_and_keeps_candidate_name() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        std::fs::create_dir_all(voiceprint_dir()).unwrap();
        let profile = SpeakerVoiceprintProfile {
            id: "spk-eden".to_string(),
            display_name: "Eden".to_string(),
            source: "live_enrollment".to_string(),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            embedding_model: "test".to_string(),
            embedding_dim: 16,
            embedding: {
                let mut embedding = vec![0.0; 16];
                embedding[0] = 0.70;
                embedding[1] = (1.0_f32 - 0.70_f32 * 0.70_f32).sqrt();
                embedding
            },
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            total_duration_ms: 2_000,
            samples: Vec::new(),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        };
        atomic_json_write(&speaker_profile_path(&profile.id), &profile).unwrap();

        let identified = identify_speaker_voice(&[1.0, 0.0], 1_000, 1_000).unwrap();

        assert!(identified.matched);
        assert_eq!(identified.profile_id.as_deref(), Some("spk-eden"));
        assert_eq!(identified.display_name, "Eden");
        assert!(identified.confidence >= VOICEPRINT_SPEAKER_MATCH_THRESHOLD);
    }

    #[test]
    fn voiceprint_identity_keeps_candidate_name_even_below_match_threshold() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        std::fs::create_dir_all(voiceprint_dir()).unwrap();
        let profile = SpeakerVoiceprintProfile {
            id: "spk-eden".to_string(),
            display_name: "Eden".to_string(),
            source: "live_enrollment".to_string(),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            embedding_model: "test".to_string(),
            embedding_dim: 16,
            embedding: {
                let mut embedding = vec![0.0; 16];
                embedding[0] = 0.55;
                embedding[1] = (1.0_f32 - 0.55_f32 * 0.55_f32).sqrt();
                embedding
            },
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            total_duration_ms: 2_000,
            samples: Vec::new(),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        };
        atomic_json_write(&speaker_profile_path(&profile.id), &profile).unwrap();

        let identified = identify_speaker_voice(&[1.0, 0.0], 1_000, 1_000).unwrap();

        assert!(!identified.matched);
        assert_eq!(identified.status, "unmatched");
        assert_eq!(identified.profile_id.as_deref(), Some("spk-eden"));
        assert_eq!(identified.display_name, "Eden");
        assert!(identified.confidence < VOICEPRINT_SPEAKER_MATCH_THRESHOLD);
    }

    #[test]
    fn uploaded_speaker_asr_chunks_keep_each_voiceprint_segment_bounded() {
        assert_eq!(plan_uploaded_speaker_asr_chunks(30_000), vec![(0, 30_000)]);
        assert_eq!(
            plan_uploaded_speaker_asr_chunks(30_001),
            vec![(0, 30_000), (28_000, 3_000)]
        );
        let chunks = plan_uploaded_speaker_asr_chunks(180_015);
        assert_eq!(chunks.first(), Some(&(0, 30_000)));
        assert!(chunks.iter().all(|(_, duration)| *duration <= 30_000));
        assert_eq!(chunks.last(), Some(&(168_000, 13_000)));
    }

    #[test]
    fn paused_task_still_allows_external_device_event_import() {
        let mut task = test_directory_task("paused-import", PathBuf::from("/tmp/asr"));
        task.paused = true;
        task.import_policy.enabled = true;
        task.external_devices = vec![AsrExternalDeviceBinding {
            name: "RIGHT".to_string(),
            enabled: true,
            ..AsrExternalDeviceBinding::default()
        }];

        assert!(task_allows_external_device_event_import(&task));
    }

    #[test]
    fn chunk_metric_records_runner_rtf_hash_and_error() {
        let ok = Ok(WholeFileTranscription {
            text: "hello".to_string(),
            segments: Vec::new(),
            structured: Default::default(),
        });
        let metric = chunk_metric(
            2,
            28,
            30,
            "reuse_server",
            &ok,
            1500,
            Some("http://127.0.0.1:12345".to_string()),
            Some("compare_shadow".to_string()),
        );
        assert_eq!(metric.runner, "reuse_server");
        assert_eq!(metric.status, "ok");
        assert_eq!(metric.rtf, 0.05);
        assert_eq!(metric.text_chars, 5);
        assert_eq!(metric.text_sha1, sha1_hex(b"hello"));
        assert_eq!(metric.server_url.as_deref(), Some("http://127.0.0.1:12345"));
        assert_eq!(metric.fallback_reason.as_deref(), Some("compare_shadow"));

        let err = Err::<WholeFileTranscription, _>("server crashed".to_string());
        let metric = chunk_metric(0, 0, 30, "reuse_server", &err, 3000, None, None);
        assert_eq!(metric.status, "error");
        assert_eq!(metric.text_chars, 0);
        assert_eq!(metric.error.as_deref(), Some("server crashed"));
    }

    #[test]
    fn task_watch_snapshot_prefers_run_progress_for_current_work() {
        let _guard = test_data_dir_lock();
        let temp = TempDir::new().unwrap();
        let _env = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let current_path = audio_dir.join("meeting.m4a");
        std::fs::write(&current_path, b"audio").unwrap();
        let task = test_directory_task("watch-progress", audio_dir);
        let key = source_key(&current_path);
        let store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::from([(
                key,
                FileRecord {
                    task_id: task.id.clone(),
                    source_path: current_path.clone(),
                    source_size: Some(100),
                    source_modified_ms: Some(1),
                    source_created_at_ms: None,
                    source_created_at_source: None,
                    content_hash: None,
                    content_hash_algorithm: None,
                    duplicate_of_source_key: None,
                    transcript_alias: None,
                    media_duration_ms: Some(10_000),
                    status: FileStatus::Processing,
                    output_text_path: None,
                    output_metadata_path: None,
                    output_timeline_path: None,
                    text_chars: 0,
                    error: None,
                    runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
                    chunk_metrics: vec![AsrChunkMetric {
                        chunk_index: 0,
                        offset_secs: 0,
                        duration_secs: 5,
                        runner: "fork_per_chunk".to_string(),
                        status: "ok".to_string(),
                        elapsed_ms: 2_000,
                        rtf: 0.4,
                        text_chars: 3,
                        text_sha1: "abc".to_string(),
                        server_url: None,
                        fallback_reason: None,
                        error: None,
                        recorded_at_ms: 2,
                    }],
                    fallback_reason: None,
                    started_at_ms: Some(1),
                    finished_at_ms: None,
                    progress_current: Some(1),
                    progress_total: Some(2),
                    failed_chunks: Vec::new(),
                    memory_limit_hints: Vec::new(),
                },
            )]),
        };
        save_run_progress(
            &task.id,
            &AsrRunProgress {
                run_id: "run".to_string(),
                trigger: "test".to_string(),
                status: "running".to_string(),
                started_at_ms: 1,
                updated_at_ms: 2,
                finished_at_ms: None,
                current_source_path: Some(current_path.clone()),
                current_file_index: 3,
                current_file_total: 8,
                current_chunk_done: 4,
                current_chunk_total: 9,
                processed_now: 2,
                failed_now: 0,
                max_concurrent_files: default_max_concurrent_files(),
                effective_max_concurrent_files: default_max_concurrent_files(),
                active_file_count: 0,
                stage: "asr".to_string(),
                stage_message: Some("processing chunks".to_string()),
                message: Some("processing".to_string()),
            },
        )
        .unwrap();

        let snapshot = task_watch_snapshot_from_store(task, &store, true);

        assert_eq!(snapshot.progress.current_file_index, 3);
        assert_eq!(snapshot.progress.current_file_total, 8);
        assert_eq!(snapshot.progress.current_chunk_done, 4);
        assert_eq!(snapshot.progress.current_chunk_total, 9);
        assert_eq!(snapshot.snapshot_source, "stale_recovered");
        assert_eq!(snapshot.consumption.inference_elapsed_ms, 2_000);
        assert_eq!(snapshot.recent_files.len(), 1);
    }

    #[test]
    fn atomic_json_write_uses_unique_temp_files_under_concurrency() {
        let _guard = test_data_dir_lock();
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("run_progress.json");
        let mut handles = Vec::new();
        for index in 0..16usize {
            let path = path.clone();
            handles.push(std::thread::spawn(move || {
                atomic_json_write(&path, &serde_json::json!({ "index": index })).unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let persisted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(persisted.get("index").is_some(), "{persisted}");
        let leftovers = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".tmp"))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn completed_processing_record_recovers_from_partial_artifacts() {
        let _guard = test_data_dir_lock();
        let temp = TempDir::new().unwrap();
        let _env = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("meeting.wav");
        std::fs::write(&audio, b"audio").unwrap();
        let text_path = temp.path().join("meeting.txt");
        let timeline_path = temp.path().join("meeting.timeline.json");
        std::fs::write(&text_path, "hello world").unwrap();
        std::fs::write(&timeline_path, "{}").unwrap();

        let mut store = FileStore::default();
        let key = source_key(&audio);
        let mut record = pending_record("recover-complete", &audio);
        record.status = FileStatus::Processing;
        record.started_at_ms = Some(10);
        record.progress_current = Some(1);
        record.progress_total = Some(2);
        record.output_text_path = Some(text_path);
        record.output_timeline_path = Some(timeline_path);
        record.chunk_metrics = vec![
            AsrChunkMetric {
                chunk_index: 0,
                offset_secs: 0,
                duration_secs: 3,
                runner: "fork_per_chunk".to_string(),
                status: "ok".to_string(),
                elapsed_ms: 100,
                rtf: 0.1,
                text_chars: 5,
                text_sha1: "a".to_string(),
                server_url: None,
                fallback_reason: None,
                error: None,
                recorded_at_ms: 20,
            },
            AsrChunkMetric {
                chunk_index: 1,
                offset_secs: 3,
                duration_secs: 3,
                runner: "fork_per_chunk".to_string(),
                status: "ok".to_string(),
                elapsed_ms: 100,
                rtf: 0.1,
                text_chars: 6,
                text_sha1: "b".to_string(),
                server_url: None,
                fallback_reason: None,
                error: None,
                recorded_at_ms: 30,
            },
        ];
        store.files.insert(key.clone(), record);

        assert_eq!(
            normalize_completed_processing_records("recover-complete", &mut store),
            1
        );
        let recovered = store.files.get(&key).unwrap();
        assert_eq!(recovered.status, FileStatus::Success);
        assert_eq!(recovered.progress_current, Some(2));
        assert_eq!(recovered.progress_total, Some(2));
        assert_eq!(recovered.finished_at_ms, Some(30));
        assert_eq!(recovered.text_chars, 11);
    }

    #[test]
    fn task_detail_collapses_superseded_same_source_records() {
        let _guard = test_data_dir_lock();
        let temp = TempDir::new().unwrap();
        let _env = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("meeting.wav");
        std::fs::write(&audio, b"audio").unwrap();
        let task = test_directory_task("collapse-same-source", audio_dir.clone());
        add_task(task.clone()).unwrap();

        let mut store = FileStore::default();
        let mut current = pending_record(&task.id, &audio);
        current.status = FileStatus::Success;
        current.finished_at_ms = Some(200);
        current.text_chars = 42;
        store.files.insert(source_key(&audio), current);

        let mut stale_pending = pending_record(&task.id, &audio);
        stale_pending.status = FileStatus::Pending;
        store.files.insert("old-pending-key".to_string(), stale_pending);

        let mut stale_processing = pending_record(&task.id, &audio);
        stale_processing.status = FileStatus::Processing;
        stale_processing.progress_current = Some(9);
        stale_processing.progress_total = Some(9);
        store
            .files
            .insert("old-processing-key".to_string(), stale_processing);
        save_file_store(&task.id, &store).unwrap();

        let detail = task_detail(task);
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].record.status, FileStatus::Success);
        assert_eq!(detail.summary.processed, 1);
        assert_eq!(detail.summary.pending, 0);
    }

    #[test]
    fn task_watch_snapshot_marks_eta_confidence_without_duration() {
        let _guard = test_data_dir_lock();
        let temp = TempDir::new().unwrap();
        let _env = EnvGuard::set_data_dir(temp.path());
        let task = test_directory_task("watch-empty", temp.path().to_path_buf());
        let store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };

        let snapshot = task_watch_snapshot_from_store(task, &store, false);

        assert_eq!(snapshot.progress.eta_confidence, "none");
        assert!(snapshot.progress.eta_ms.is_none());
        assert!(snapshot.consumption.average_rtf.is_none());
    }

    #[test]
    fn server_failure_fallback_reason_classifies_transport_errors() {
        let connect_error = "status: 502 Bad Gateway; cause: tcp connect error";
        assert!(is_server_transport_failure(connect_error));
        let reason =
            server_failure_fallback_reason(AsrRuntimeStrategy::ReusePerFile, connect_error);
        assert!(reason.contains("reuse_per_file strategy transport failure"));
        assert!(reason.contains("fork_per_chunk"));
        assert!(reason.contains("scheduling managed ASR server restart for later chunks"));
        assert!(is_server_restart_retriable(connect_error));

        let mlx_error =
            "status: 500 Internal Server Error; MLX error: [reshape] Cannot reshape array of size 0 into shape (1,1,2048)";
        assert!(is_server_restart_retriable(mlx_error));
        let reason = server_failure_fallback_reason(AsrRuntimeStrategy::ReuseServer, mlx_error);
        assert!(reason.contains("reuse_server strategy mlx_empty_tensor failure"));

        let http_error = "status: 500 Internal Server Error; model panic";
        assert!(!is_server_transport_failure(http_error));
        assert!(!is_server_restart_retriable(http_error));
        let reason = server_failure_fallback_reason(AsrRuntimeStrategy::ReuseServer, http_error);
        assert!(reason.contains("reuse_server strategy server failure"));
    }

    #[test]
    fn discovers_audio_files_recursively_and_filters_extensions() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("nested")).unwrap();
        std::fs::write(temp.path().join("a.wav"), b"not real audio").unwrap();
        std::fs::write(temp.path().join("nested/b.m4a"), b"not real audio").unwrap();
        std::fs::write(temp.path().join("note.txt"), b"ignore").unwrap();

        let flat = discover_audio_files(temp.path(), false).unwrap();
        assert_eq!(flat.len(), 1);
        assert!(flat[0].ends_with("a.wav"));

        let recursive = discover_audio_files(temp.path(), true).unwrap();
        assert_eq!(recursive.len(), 2);
    }

    #[test]
    fn pending_batch_rescan_picks_up_appended_files_without_retrying_same_run_failures() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let first = audio_dir.join("TX02_MIC001_20260525_090000_orig.wav");
        let appended = audio_dir.join("TX02_MIC002_20260525_100000_orig.wav");
        std::fs::write(&first, b"audio").unwrap();

        let task = test_directory_task("rescan-appended", audio_dir.clone());
        let mut files = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let mut attempted = HashSet::new();

        let initial =
            discover_and_prepare_pending_batch(&task, &mut files, &attempted).unwrap();
        assert_eq!(initial.pending, vec![first.clone()]);

        let first_key = source_key(&first);
        attempted.insert(first_key.clone());
        let mut failed_record = files.files.remove(&first_key).unwrap();
        failed_record.status = FileStatus::Failed;
        files.files.insert(first_key, failed_record);
        std::fs::write(&appended, b"audio").unwrap();

        let after_append =
            discover_and_prepare_pending_batch(&task, &mut files, &attempted).unwrap();
        assert_eq!(after_append.pending, vec![appended.clone()]);

        attempted.insert(source_key(&appended));
        let final_scan =
            discover_and_prepare_pending_batch(&task, &mut files, &attempted).unwrap();
        assert!(final_scan.pending.is_empty());
    }

    #[test]
    fn pending_batch_sorts_older_source_time_first() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let later = audio_dir.join("TX02_MIC002_20260525_100000_orig.wav");
        let earlier = audio_dir.join("TX02_MIC001_20260525_090000_orig.wav");
        std::fs::write(&later, b"audio").unwrap();
        std::fs::write(&earlier, b"audio").unwrap();

        let task = test_directory_task("sort-pending", audio_dir);
        let mut files = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let attempted = HashSet::new();

        let scan = discover_and_prepare_pending_batch(&task, &mut files, &attempted).unwrap();
        assert_eq!(scan.pending, vec![earlier, later]);
    }

    #[test]
    fn daily_schedule_can_start_on_current_minute_then_advances_to_next_day() {
        let now = Local
            .with_ymd_and_hms(2026, 5, 14, 10, 30, 20)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;
        let schedule = AsrTaskSchedule::Daily {
            hour: 10,
            minute: 30,
        };

        assert_eq!(schedule.initial_next_run_at_ms(now), Some(now));

        let next = schedule.next_run_at_ms(now.saturating_add(60_000), false);
        let next_dt = Local
            .timestamp_millis_opt(next.unwrap() as i64)
            .earliest()
            .unwrap();
        assert_eq!(next_dt.day(), 15);
        assert_eq!(next_dt.hour(), 10);
        assert_eq!(next_dt.minute(), 30);
    }

    #[test]
    fn weekly_schedule_uses_iso_weekday_and_wall_clock_time() {
        let now = Local
            .with_ymd_and_hms(2026, 5, 14, 10, 30, 0)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;
        let schedule = AsrTaskSchedule::Weekly {
            weekday: 5,
            hour: 9,
            minute: 15,
        };

        let next = schedule.next_run_at_ms(now, false).unwrap();
        let next_dt = Local.timestamp_millis_opt(next as i64).earliest().unwrap();
        assert_eq!(next_dt.weekday().number_from_monday(), 5);
        assert_eq!(next_dt.hour(), 9);
        assert_eq!(next_dt.minute(), 15);
    }

    #[test]
    fn monthly_schedule_clamps_oversized_day_to_month_end() {
        let now = Local
            .with_ymd_and_hms(2026, 4, 1, 9, 0, 0)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;
        let schedule = AsrTaskSchedule::Monthly {
            day: 31,
            hour: 10,
            minute: 5,
        };

        let next = schedule.next_run_at_ms(now, false).unwrap();
        let next_dt = Local.timestamp_millis_opt(next as i64).earliest().unwrap();
        assert_eq!(next_dt.month(), 4);
        assert_eq!(next_dt.day(), 30);
        assert_eq!(next_dt.hour(), 10);
        assert_eq!(next_dt.minute(), 5);
    }

    #[test]
    fn schedule_validation_rejects_out_of_range_values() {
        assert!(AsrTaskSchedule::Hourly { minute: 60 }.validate().is_err());
        assert!(AsrTaskSchedule::Weekly {
            weekday: 0,
            hour: 9,
            minute: 0
        }
        .validate()
        .is_err());
        assert!(AsrTaskSchedule::Monthly {
            day: 32,
            hour: 9,
            minute: 0
        }
        .validate()
        .is_err());
    }

    #[test]
    fn task_pause_state_defaults_for_existing_json() {
        let task: AsrDirectoryTask = serde_json::from_str(
            r#"{
                "id":"legacy-task",
                "name":"Legacy",
                "audio_dir":"/tmp",
                "recursive":true,
                "enabled":true,
                "schedule":{"kind":"daily","hour":2,"minute":0},
                "language":"chinese",
                "model":"Qwen3-ASR-1.7B",
                "created_at_ms":1,
                "updated_at_ms":1,
                "last_run_at_ms":null,
                "next_run_at_ms":null,
                "last_error":null
            }"#,
        )
        .unwrap();
        assert!(!task.paused);
        assert_eq!(task.paused_at_ms, None);
    }

    #[test]
    fn update_task_paused_toggles_scheduler_state() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let task = AsrDirectoryTask {
            id: "pause-task".to_string(),
            name: "Pause Task".to_string(),
            audio_dir,
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: AsrTaskSchedule::Hourly { minute: 0 },
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: Some(1),
            last_error: Some("old error".to_string()),
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };
        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task],
        })
        .unwrap();

        let paused = update_task_paused("pause-task", true).unwrap();
        assert!(paused.paused);
        assert!(paused.paused_at_ms.is_some());
        assert_eq!(paused.next_run_at_ms, None);
        assert_eq!(paused.last_error, None);
        assert!(task_pause_requested("pause-task"));

        let resumed = update_task_paused("pause-task", false).unwrap();
        assert!(!resumed.paused);
        assert_eq!(resumed.paused_at_ms, None);
        assert!(resumed.next_run_at_ms.is_some());
        assert!(!task_pause_requested("pause-task"));
    }

    #[test]
    fn temporary_pause_keeps_next_schedule_and_auto_resumes_when_due() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let mut task = test_directory_task("temporary-pause-task", audio_dir);
        let future_next_run_at_ms = now_ms().saturating_add(600_000);
        task.next_run_at_ms = Some(future_next_run_at_ms);
        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task],
        })
        .unwrap();

        let paused =
            update_task_paused_with_mode("temporary-pause-task", true, AsrTaskPauseMode::Temporary)
                .unwrap();
        assert!(paused.paused);
        assert_eq!(paused.next_run_at_ms, Some(future_next_run_at_ms));

        let mut store = load_tasks();
        store.tasks[0].next_run_at_ms = Some(1);
        save_tasks(&store).unwrap();

        let resumed = resume_temporary_paused_task_for_schedule("temporary-pause-task", now_ms())
            .unwrap()
            .unwrap();
        assert!(!resumed.paused);
        assert_eq!(resumed.paused_at_ms, None);
        assert_eq!(resumed.last_error, None);
    }

    #[test]
    fn long_term_pause_does_not_auto_resume_for_scheduler() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let task = test_directory_task("long-pause-task", audio_dir);
        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task],
        })
        .unwrap();

        let paused =
            update_task_paused_with_mode("long-pause-task", true, AsrTaskPauseMode::LongTerm)
                .unwrap();
        assert!(paused.paused);
        assert_eq!(paused.next_run_at_ms, None);

        let resumed = resume_temporary_paused_task_for_schedule("long-pause-task", now_ms())
            .unwrap();
        assert!(resumed.is_none());
    }

    #[test]
    fn task_after_run_preserves_temporary_pause_schedule() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let mut task = test_directory_task("paused-after-run-task", audio_dir);
        task.paused = true;
        task.paused_at_ms = Some(10);
        task.next_run_at_ms = Some(123_456);
        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task],
        })
        .unwrap();

        let updated = update_task_after_run("paused-after-run-task", None).unwrap();
        assert!(updated.paused);
        assert_eq!(updated.next_run_at_ms, Some(123_456));
    }

    #[test]
    fn query_flag_enabled_accepts_truthy_force_values() {
        assert!(query_flag_enabled("force=true", "force"));
        assert!(query_flag_enabled("force=1&other=false", "force"));
        assert!(query_flag_enabled("force", "force"));
        assert!(!query_flag_enabled("force=false", "force"));
        assert!(!query_flag_enabled("other=true", "force"));
    }

    #[test]
    fn pause_mode_from_query_accepts_temporary_and_long_term_modes() {
        assert_eq!(
            pause_mode_from_query("mode=temporary").unwrap(),
            AsrTaskPauseMode::Temporary
        );
        assert_eq!(
            pause_mode_from_query("force=true&mode=long_term").unwrap(),
            AsrTaskPauseMode::LongTerm
        );
        assert_eq!(
            pause_mode_from_query("").unwrap(),
            AsrTaskPauseMode::LongTerm
        );
        assert_eq!(
            pause_mode_from_query("mode=unknown").unwrap_err(),
            "invalid pause mode; use temporary or long_term"
        );
    }

    #[tokio::test]
    async fn abortable_command_stops_on_pause_request() {
        let command = if cfg!(windows) {
            let mut command = Command::new("powershell.exe");
            command
                .arg("-NoProfile")
                .arg("-Command")
                .arg("Start-Sleep -Seconds 10");
            command
        } else {
            let mut command = Command::new("/bin/sh");
            command.arg("-c").arg("sleep 10");
            command
        };
        let result = run_abortable_command(
            command,
            "test sleep",
            Some(&|| true),
            Duration::from_secs(30),
        )
        .await;
        assert_eq!(result.unwrap_err(), ASR_TASK_PAUSED_MESSAGE);
    }

    #[test]
    fn ffmpeg_timeouts_are_bounded_by_audio_duration() {
        assert_eq!(
            ffmpeg_normalize_timeout(Some(30_000)),
            Duration::from_secs(FFMPEG_NORMALIZE_MIN_TIMEOUT_SECS)
        );
        assert_eq!(
            ffmpeg_normalize_timeout(Some(2 * 60 * 60 * 1000)),
            Duration::from_secs(FFMPEG_NORMALIZE_MAX_TIMEOUT_SECS)
        );
        assert_eq!(
            ffmpeg_chunk_split_timeout(30),
            Duration::from_secs(FFMPEG_CHUNK_SPLIT_TIMEOUT_SECS)
        );
    }

    #[test]
    fn asr_runtime_timeouts_are_bounded_for_short_chunks() {
        assert_eq!(asr_chunk_timeout(30), Duration::from_secs(90));
        assert_eq!(asr_chunk_timeout(10), Duration::from_secs(45));
        assert_eq!(
            asr_server_request_timeout(Some(30_000)),
            Duration::from_secs(120)
        );
        assert_eq!(
            asr_server_request_timeout(Some(2 * 60 * 1000)),
            Duration::from_secs(180)
        );
        assert_eq!(asr_server_request_timeout(None), Duration::from_secs(600));
        assert_eq!(asr_text_request_timeout(), Duration::from_secs(45));
    }

    #[test]
    fn server_failure_recovery_reason_uses_fork_for_current_chunk() {
        let connect_error = "status: 502 Bad Gateway; cause: tcp connect error";
        let reason =
            server_failure_fallback_reason(AsrRuntimeStrategy::ReusePerFile, connect_error);
        assert!(reason.contains("reuse_per_file strategy transport failure"));
        assert!(reason.contains("retrying current chunk via fork_per_chunk"));
        assert!(reason.contains("scheduling managed ASR server restart for later chunks"));
        assert!(!reason.contains("restarting managed ASR server"));
    }

    #[test]
    fn server_failure_breaker_switches_remaining_chunks_to_fork() {
        let mut state = ServerRunnerState {
            server_url: "test-error:dead-server".to_string(),
            baseline_rtf: None,
            baseline_samples: Vec::new(),
            server_failures: max_server_failures_for_strategy(AsrRuntimeStrategy::ReuseServer),
            force_fork_for_remaining: false,
            restart_required: true,
            current_chunk_failure_reason: None,
            fallback_reason: None,
        };

        apply_server_failure_breaker_if_needed(AsrRuntimeStrategy::ReuseServer, &mut state, 3, 90, 30);

        assert!(state.force_fork_for_remaining);
        assert!(!state.restart_required);
        assert!(state
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("switching remaining chunks to fork_per_chunk isolation")));
    }

    #[tokio::test]
    async fn test_server_chunk_and_bisect_empty_results_initialize_structured_view() {
        let state = ServerRunnerState {
            server_url: "test-empty".to_string(),
            baseline_rtf: None,
            baseline_samples: Vec::new(),
            server_failures: 0,
            force_fork_for_remaining: false,
            restart_required: false,
            current_chunk_failure_reason: None,
            fallback_reason: None,
        };
        let server_result = run_server_chunk_request(
            &state,
            "chinese",
            Path::new("/nonexistent/chunk.wav"),
            1,
        )
        .await
        .unwrap();
        assert!(server_result.structured.segments.is_empty());

        let temp = TempDir::new().unwrap();
        let too_short = transcribe_single_chunk_with_bisect(
            Path::new("/nonexistent/asr"),
            Path::new("/nonexistent/model"),
            "chinese",
            Path::new("/nonexistent/chunk.wav"),
            0,
            0,
            0,
            temp.path(),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(too_short.structured.segments.is_empty());

        let silent = transcribe_single_chunk_with_bisect(
            Path::new("/nonexistent/asr"),
            Path::new("/nonexistent/model"),
            "chinese",
            Path::new("/nonexistent/chunk.wav"),
            0,
            MIN_CHUNK_SECS,
            1,
            temp.path(),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(silent.structured.segments.is_empty());

        let empty_chunks = transcribe_in_chunks(
            Path::new("/nonexistent/asr"),
            Path::new("/nonexistent/model"),
            "chinese",
            Path::new("/nonexistent/input.wav"),
            temp.path(),
            0,
            30,
            5,
            0,
            None,
            None,
            None,
            "Qwen3-ASR-0.6B",
            &[],
            AsrRuntimeStrategy::ForkPerChunk,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(empty_chunks.transcription.structured.segments.is_empty());

        let silent_wav = temp.path().join("silent-four-seconds.wav");
        std::fs::write(&silent_wav, make_wav(&vec![0i16; 4 * 16_000])).unwrap();
        let hinted = transcribe_chunk_with_memory_hint(
            Path::new("/nonexistent/asr"),
            Path::new("/nonexistent/model"),
            "chinese",
            &silent_wav,
            0,
            4,
            2,
            temp.path(),
            2,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(hinted.text.is_empty());
        assert!(hinted.structured.segments.is_empty());
    }

    #[test]
    fn force_pause_requires_persisted_pause_state() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let task = AsrDirectoryTask {
            id: "force-pause-task".to_string(),
            name: "Force Pause Task".to_string(),
            audio_dir,
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: AsrTaskSchedule::Hourly { minute: 0 },
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: Some(1),
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };
        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task],
        })
        .unwrap();

        FORCE_PAUSED_TASKS
            .lock()
            .unwrap()
            .insert("force-pause-task".to_string());
        assert!(!task_force_pause_requested("force-pause-task"));

        update_task_paused("force-pause-task", true).unwrap();
        assert!(task_force_pause_requested("force-pause-task"));

        FORCE_PAUSED_TASKS
            .lock()
            .unwrap()
            .remove("force-pause-task");
    }

    #[test]
    fn diarization_worker_abort_error_is_retryable_on_recovery() {
        assert!(is_retryable_asr_server_acquire_error(
            "ASR diarization worker failed:"
        ));
        assert!(is_retryable_asr_server_acquire_error(
            "managed ASR server start failed: Qwen3-ASR service is busy"
        ));
        assert!(!is_retryable_asr_server_acquire_error(
            "ASR diarization worker failed: missing model assets"
        ));
    }

    #[test]
    fn memory_limit_events_update_root_chunk_hint() {
        let mut hints = vec![AsrChunkMemoryHint {
            model: "Qwen3-ASR-1.7B".to_string(),
            offset_secs: 28,
            duration_secs: 30,
            preferred_chunk_secs: 15,
            trigger_count: 1,
            last_triggered_at_ms: 1,
            last_error: None,
        }];
        merge_memory_limit_events_into_hints(
            &mut hints,
            "Qwen3-ASR-1.7B",
            28,
            30,
            &[
                AsrMemoryLimitEvent {
                    offset_secs: 28,
                    duration_secs: 30,
                    suggested_chunk_secs: 15,
                    error: "30s over limit".to_string(),
                },
                AsrMemoryLimitEvent {
                    offset_secs: 28,
                    duration_secs: 15,
                    suggested_chunk_secs: 7,
                    error: "15s over limit".to_string(),
                },
            ],
        );

        let hint = find_memory_limit_hint(&hints, "Qwen3-ASR-1.7B", 28, 30).unwrap();
        assert_eq!(hint.preferred_chunk_secs, 7);
        assert_eq!(hint.trigger_count, 3);
        assert!(hint
            .last_error
            .as_deref()
            .unwrap()
            .contains("15s over limit"));
        assert!(find_memory_limit_hint(&hints, "Qwen3-ASR-0.6B", 28, 30).is_none());
    }

    #[test]
    fn summary_keeps_processed_files_after_source_deletion() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("done.wav");
        std::fs::write(&audio, b"audio").unwrap();
        let task = AsrDirectoryTask {
            id: "task1".to_string(),
            name: "Task".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Success;
        store.files.insert(source_key(&audio), record);
        save_file_store(&task.id, &store).unwrap();
        std::fs::remove_file(&audio).unwrap();

        let summary = task_with_summary(task).summary;
        assert_eq!(summary.discovered, 0);
        assert_eq!(summary.processed, 1);
        assert_eq!(summary.pending, 0);
        assert_eq!(summary.deleted_after_processing, 1);
    }

    #[test]
    fn control_summary_uses_file_store_without_live_discovery() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let known = audio_dir.join("known.wav");
        let untracked = audio_dir.join("untracked.wav");
        std::fs::write(&known, b"known").unwrap();
        std::fs::write(&untracked, b"untracked").unwrap();
        let task = test_directory_task("control-summary-task", audio_dir);
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        store
            .files
            .insert(source_key(&known), pending_record(&task.id, &known));
        save_file_store(&task.id, &store).unwrap();

        let live_summary = task_with_summary(task.clone()).summary;
        assert_eq!(live_summary.discovered, 2);
        assert_eq!(live_summary.pending, 2);

        let control_summary = task_with_control_summary(task).summary;
        assert_eq!(control_summary.discovered, 1);
        assert_eq!(control_summary.pending, 1);
        assert_eq!(control_summary.audio_source_file_count, 0);
    }

    #[test]
    fn running_task_list_summary_uses_cached_counts() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let known = audio_dir.join("known.wav");
        let untracked = audio_dir.join("untracked.wav");
        std::fs::write(&known, b"known").unwrap();
        std::fs::write(&untracked, b"untracked").unwrap();
        let task = test_directory_task("running-list-summary-task", audio_dir);
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        store
            .files
            .insert(source_key(&known), pending_record(&task.id, &known));
        save_file_store(&task.id, &store).unwrap();

        let _running = RunningTaskGuard::acquire(&task.id).unwrap();
        let summary = task_with_list_summary(task).summary;
        assert_eq!(summary.discovered, 1);
        assert_eq!(summary.pending, 1);
        assert!(summary.running);
    }

    #[test]
    fn summary_reports_cleanable_source_audio_usage() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let done = audio_dir.join("done.wav");
        let pending = audio_dir.join("pending.wav");
        std::fs::write(&done, b"audio").unwrap();
        std::fs::write(&pending, b"pending-audio").unwrap();
        let task = AsrDirectoryTask {
            id: "task-cleanable-summary".to_string(),
            name: "Task".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };
        let (text_path, _metadata_path, timeline_path) =
            output_paths_in(temp.path(), &task.id, &done, &audio_dir);
        std::fs::create_dir_all(text_path.parent().unwrap()).unwrap();
        std::fs::write(&text_path, "done").unwrap();
        std::fs::write(&timeline_path, "{}").unwrap();
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let mut done_record = pending_record(&task.id, &done);
        done_record.status = FileStatus::Success;
        done_record.output_text_path = Some(text_path);
        done_record.output_timeline_path = Some(timeline_path);
        store.files.insert(source_key(&done), done_record);
        store
            .files
            .insert(source_key(&pending), pending_record(&task.id, &pending));
        save_file_store(&task.id, &store).unwrap();

        let summary = task_with_summary(task).summary;
        assert_eq!(summary.audio_source_file_count, 2);
        assert_eq!(summary.audio_source_bytes, 18);
        assert_eq!(summary.cleanable_source_file_count, 1);
        assert_eq!(summary.cleanable_source_bytes, 5);
    }

    #[test]
    fn cleanup_source_audio_deletes_only_successful_records_with_outputs() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        let outside_dir = temp.path().join("outside");
        std::fs::create_dir_all(&audio_dir).unwrap();
        std::fs::create_dir_all(&outside_dir).unwrap();
        let done = audio_dir.join("done.wav");
        let partial = audio_dir.join("partial.wav");
        let outside = outside_dir.join("outside.wav");
        std::fs::write(&done, b"done-audio").unwrap();
        std::fs::write(&partial, b"partial-audio").unwrap();
        std::fs::write(&outside, b"outside-audio").unwrap();
        let task = AsrDirectoryTask {
            id: "task-cleanup".to_string(),
            name: "Task".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        for (path, status) in [
            (&done, FileStatus::Success),
            (&partial, FileStatus::PartialSuccess),
            (&outside, FileStatus::Success),
        ] {
            let (text_path, _metadata_path, timeline_path) =
                output_paths_in(temp.path(), &task.id, path, &audio_dir);
            std::fs::create_dir_all(text_path.parent().unwrap()).unwrap();
            std::fs::write(&text_path, "transcript").unwrap();
            std::fs::write(&timeline_path, "{}").unwrap();
            let mut record = pending_record(&task.id, path);
            record.status = status;
            record.output_text_path = Some(text_path);
            record.output_timeline_path = Some(timeline_path);
            store.files.insert(source_key(path), record);
        }
        save_file_store(&task.id, &store).unwrap();

        let result = cleanup_task_source_audio(&task);

        assert!(result.ok);
        assert_eq!(result.deleted_files, 1);
        assert_eq!(result.deleted_bytes, 10);
        assert!(!done.exists());
        assert!(partial.exists());
        assert!(outside.exists());
        assert_eq!(result.summary.deleted_after_processing, 2);
        assert_eq!(result.summary.cleanable_source_file_count, 0);
    }

    #[test]
    fn summary_counts_failed_files_separately_from_pending() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("bad.wav");
        std::fs::write(&audio, b"bad-audio").unwrap();
        let task = AsrDirectoryTask {
            id: "task1".to_string(),
            name: "Task".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Failed;
        record.error = Some("decode failed".to_string());
        store.files.insert(source_key(&audio), record);
        save_file_store(&task.id, &store).unwrap();

        let summary = task_with_summary(task).summary;
        assert_eq!(summary.discovered, 1);
        assert_eq!(summary.processed, 0);
        assert_eq!(summary.pending, 0);
        assert_eq!(summary.failed, 1);
    }

    #[test]
    fn summary_treats_recreated_same_path_audio_as_new_pending_file() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("same.wav");
        std::fs::write(&audio, b"old-audio").unwrap();
        let task = AsrDirectoryTask {
            id: "task1".to_string(),
            name: "Task".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Success;
        store.files.insert(source_key(&audio), record);
        save_file_store(&task.id, &store).unwrap();

        std::thread::sleep(Duration::from_millis(2));
        std::fs::write(&audio, b"new-audio-with-different-size").unwrap();

        let summary = task_with_summary(task).summary;
        assert_eq!(summary.discovered, 1);
        assert_eq!(summary.processed, 1);
        assert_eq!(summary.pending, 1);
    }

    #[test]
    fn task_detail_includes_file_progress_records() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("done.wav");
        std::fs::write(&audio, b"audio").unwrap();
        let task = AsrDirectoryTask {
            id: "task-detail".to_string(),
            name: "Task detail".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let key = source_key(&audio);
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Success;
        record.text_chars = 12;
        record.output_text_path = Some(temp.path().join("asr/data/text/task-detail/done.txt"));
        record.output_timeline_path = Some(
            temp.path()
                .join("asr/data/text/task-detail/done.timeline.json"),
        );
        store.files.insert(key.clone(), record);
        save_file_store(&task.id, &store).unwrap();

        let detail = task_detail(task);
        assert_eq!(detail.summary.processed, 1);
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].key, key);
        assert_eq!(detail.files[0].record.status, FileStatus::Success);
        assert_eq!(detail.files[0].record.text_chars, 12);
    }

    #[test]
    fn task_detail_sorts_unfinished_files_before_successes() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let task = AsrDirectoryTask {
            id: "task-detail-sort".to_string(),
            name: "Task detail sort".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };

        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        for (name, status) in [
            ("a-success.wav", FileStatus::Success),
            ("b-pending.wav", FileStatus::Pending),
            ("c-processing.wav", FileStatus::Processing),
            ("d-failed.wav", FileStatus::Failed),
            ("e-partial.wav", FileStatus::PartialSuccess),
        ] {
            let audio = audio_dir.join(name);
            std::fs::write(&audio, b"audio").unwrap();
            let mut record = pending_record(&task.id, &audio);
            record.status = status;
            store.files.insert(source_key(&audio), record);
        }
        save_file_store(&task.id, &store).unwrap();

        let statuses = task_detail(task)
            .files
            .into_iter()
            .map(|file| file.record.status)
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            vec![
                FileStatus::Processing,
                FileStatus::Pending,
                FileStatus::Failed,
                FileStatus::PartialSuccess,
                FileStatus::Success,
            ]
        );
    }

    #[test]
    fn bulk_retry_targets_include_only_files_with_failed_chunks_in_path_order() {
        let temp = TempDir::new().unwrap();
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };

        let ok_audio = temp.path().join("b-ok.wav");
        let failed_audio_b = temp.path().join("b-failed.wav");
        let failed_audio_a = temp.path().join("a-failed.wav");
        let mut ok = pending_record("bulk-retry-task", &ok_audio);
        ok.status = FileStatus::Success;
        store.files.insert("ok".to_string(), ok);

        let mut failed_b = pending_record("bulk-retry-task", &failed_audio_b);
        failed_b.status = FileStatus::PartialSuccess;
        failed_b.failed_chunks.push(FailedChunkRecord {
            chunk_index: 2,
            offset_secs: 56,
            duration_secs: 30,
            error: "server disconnected".to_string(),
            attempts: 3,
            energy_rms: None,
            is_silent: false,
        });
        store.files.insert("failed-b".to_string(), failed_b);

        let mut failed_a = pending_record("bulk-retry-task", &failed_audio_a);
        failed_a.status = FileStatus::PartialSuccess;
        failed_a.failed_chunks.push(FailedChunkRecord {
            chunk_index: 1,
            offset_secs: 28,
            duration_secs: 30,
            error: "memory limit".to_string(),
            attempts: 3,
            energy_rms: None,
            is_silent: false,
        });
        failed_a.failed_chunks.push(FailedChunkRecord {
            chunk_index: 3,
            offset_secs: 84,
            duration_secs: 30,
            error: "memory limit".to_string(),
            attempts: 3,
            energy_rms: None,
            is_silent: false,
        });
        store.files.insert("failed-a".to_string(), failed_a);

        let targets = retryable_failed_chunk_files(&store);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].file_key, "failed-a");
        assert_eq!(targets[0].failed_chunks, 2);
        assert_eq!(targets[1].file_key, "failed-b");
        assert_eq!(targets[1].failed_chunks, 1);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn run_without_pending_files_refreshes_daily_summaries() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("TX02_MIC001_20260514_114433_orig.wav");
        std::fs::write(&audio, b"audio").unwrap();
        let start = Local
            .with_ymd_and_hms(2026, 5, 14, 11, 44, 33)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;
        let task = AsrDirectoryTask {
            id: "task-daily-refresh".to_string(),
            name: "Daily Refresh".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: start,
            updated_at_ms: start,
            last_run_at_ms: Some(start),
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };
        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task.clone()],
        })
        .unwrap();
        let (text_path, metadata_path, timeline_path) =
            output_paths_in(temp.path(), &task.id, &audio, &audio_dir);
        std::fs::create_dir_all(text_path.parent().unwrap()).unwrap();
        std::fs::write(&text_path, "完整按天整理内容").unwrap();
        std::fs::write(&metadata_path, "{}").unwrap();
        let timeline = TranscriptTimeline {
            task_id: task.id.clone(),
            task_name: task.name.clone(),
            source_path: audio.clone(),
            source_size: Some(5),
            source_modified_ms: Some(start),
            source_created_at_ms: Some(start),
            source_created_at_source: Some("filename_timestamp".to_string()),
            media_duration_ms: Some(2_000),
            model: task.model.clone(),
            language: task.language.clone(),
            diarization_profile: None,
            speakers: Vec::new(),
            processed_at_ms: start + 2_000,
            segments: vec![TimelineSegment {
                index: 0,
                audio_start_ms: 0,
                audio_end_ms: 2_000,
                absolute_start_ms: Some(start),
                absolute_end_ms: Some(start + 2_000),
                speaker: None,
                speaker_display_name: None,
                overlap: false,
                text: "完整按天整理内容".to_string(),
            }],
        };
        std::fs::write(
            &timeline_path,
            serde_json::to_string_pretty(&timeline).unwrap(),
        )
        .unwrap();
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let key = source_key(&audio);
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Success;
        record.output_text_path = Some(text_path);
        record.output_metadata_path = Some(metadata_path);
        record.output_timeline_path = Some(timeline_path);
        record.text_chars = 8;
        store.files.insert(key, record);
        save_file_store(&task.id, &store).unwrap();

        let (_updated, processed_now, failed_now) = run_directory_task(task).await.unwrap();

        assert_eq!(processed_now, 0);
        assert_eq!(failed_now, 0);
        let daily_path = temp
            .path()
            .join("asr/data/text/task-daily-refresh/.daily/2026-05-14.md");
        let daily = std::fs::read_to_string(daily_path).unwrap();
        assert!(daily.contains("# Daily Refresh"));
        assert!(daily.contains("完整按天整理内容"));
    }

    #[test]
    fn interrupted_processing_records_reset_to_pending_before_next_run() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let processing_audio = audio_dir.join("processing.wav");
        let done_audio = audio_dir.join("done.wav");
        std::fs::write(&processing_audio, b"audio").unwrap();
        std::fs::write(&done_audio, b"audio").unwrap();

        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let processing_key = source_key(&processing_audio);
        let mut processing = pending_record("task-reset", &processing_audio);
        processing.status = FileStatus::Processing;
        processing.started_at_ms = Some(123);
        processing.progress_current = Some(29);
        processing.progress_total = Some(65);
        processing.error = Some("old transient error".to_string());
        store.files.insert(processing_key.clone(), processing);

        let done_key = source_key(&done_audio);
        let mut done = pending_record("task-reset", &done_audio);
        done.status = FileStatus::Success;
        done.started_at_ms = Some(10);
        done.finished_at_ms = Some(20);
        store.files.insert(done_key.clone(), done);

        assert_eq!(
            reset_interrupted_processing_records("task-reset", &mut store),
            1
        );
        let reset = store.files.get(&processing_key).unwrap();
        assert_eq!(reset.status, FileStatus::Pending);
        assert_eq!(reset.started_at_ms, None);
        assert_eq!(reset.finished_at_ms, None);
        assert_eq!(reset.progress_current, None);
        assert_eq!(reset.progress_total, None);
        assert_eq!(reset.error, None);

        let done = store.files.get(&done_key).unwrap();
        assert_eq!(done.status, FileStatus::Success);
        assert_eq!(done.started_at_ms, Some(10));
        assert_eq!(done.finished_at_ms, Some(20));
    }

    #[test]
    fn chunk_boundaries_keep_each_segment_at_or_below_thirty_seconds() {
        let boundaries = plan_asr_chunk_boundaries(231, 30, 2);
        assert_eq!(boundaries.first(), Some(&(0, 30)));
        assert_eq!(boundaries.get(1), Some(&(28, 30)));
        assert_eq!(boundaries.last(), Some(&(224, 7)));
        assert!(boundaries
            .iter()
            .all(|&(offset, duration)| duration <= 30 && offset + duration <= 231));
    }

    #[test]
    fn chunk_plain_text_fallback_creates_one_timeline_segment_per_chunk() {
        let mut segments = Vec::new();
        let mut text = String::new();

        append_chunk_transcription(
            &mut segments,
            &mut text,
            WholeFileTranscription {
                text: "hello world".to_string(),
                segments: Vec::new(),
                structured: Default::default(),
            },
            0,
            30,
            2,
            61_500,
        );
        append_chunk_transcription(
            &mut segments,
            &mut text,
            WholeFileTranscription {
                text: "world again".to_string(),
                segments: Vec::new(),
                structured: Default::default(),
            },
            28,
            30,
            2,
            61_500,
        );
        append_chunk_transcription(
            &mut segments,
            &mut text,
            WholeFileTranscription {
                text: "final words".to_string(),
                segments: Vec::new(),
                structured: Default::default(),
            },
            56,
            6,
            2,
            61_500,
        );

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].0, 0);
        assert_eq!(segments[0].1, 30_000);
        assert_eq!(segments[1].0, 28_000);
        assert_eq!(segments[1].1, 58_000);
        assert_eq!(segments[2].0, 56_000);
        assert_eq!(segments[2].1, 61_500);
        assert!(segments.iter().all(|(start, end, _)| end - start <= 30_000));
        assert!(text.contains("hello world"));
        assert!(text.contains("again"));
        assert!(text.contains("final words"));
    }

    #[test]
    fn timeline_response_normalization_splits_legacy_oversized_segments() {
        let mut timeline = TranscriptTimeline {
            task_id: "task".to_string(),
            task_name: "Task".to_string(),
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_size: None,
            source_modified_ms: None,
            source_created_at_ms: Some(1_000_000),
            source_created_at_source: Some("test".to_string()),
            media_duration_ms: Some(65_000),
            model: "Qwen3-ASR-1.7B".to_string(),
            language: "chinese".to_string(),
            diarization_profile: None,
            speakers: Vec::new(),
            processed_at_ms: 1,
            segments: vec![TimelineSegment {
                index: 0,
                audio_start_ms: 0,
                audio_end_ms: 65_000,
                absolute_start_ms: Some(1_000_000),
                absolute_end_ms: Some(1_065_000),
                speaker: None,
                speaker_display_name: None,
                overlap: false,
                text: "abcdefghijklmnopqrstuvwxyz".to_string(),
            }],
        };

        normalize_timeline_segments(&mut timeline);

        assert_eq!(timeline.segments.len(), 3);
        assert_eq!(timeline.segments[0].audio_start_ms, 0);
        assert_eq!(timeline.segments[0].audio_end_ms, 30_000);
        assert_eq!(timeline.segments[1].audio_start_ms, 30_000);
        assert_eq!(timeline.segments[1].audio_end_ms, 60_000);
        assert_eq!(timeline.segments[2].audio_start_ms, 60_000);
        assert_eq!(timeline.segments[2].audio_end_ms, 65_000);
        assert!(timeline
            .segments
            .iter()
            .all(|segment| segment.audio_end_ms - segment.audio_start_ms <= 30_000));
        assert_eq!(timeline.segments[2].absolute_end_ms, Some(1_065_000));
        assert_eq!(
            timeline
                .segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<String>(),
            "abcdefghijklmnopqrstuvwxyz"
        );
    }

    #[test]
    fn output_paths_are_under_bifrost_asr_text_dir() {
        let temp = TempDir::new().unwrap();
        let (text, metadata, timeline) = output_paths_in(
            temp.path(),
            "task1",
            Path::new("/tmp/audio.wav"),
            Path::new("/tmp"),
        );
        assert!(text.starts_with(temp.path().join("asr/data/text/task1")));
        assert_eq!(text.extension().and_then(|ext| ext.to_str()), Some("txt"));
        assert_eq!(
            metadata.extension().and_then(|ext| ext.to_str()),
            Some("json")
        );
        assert!(timeline
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap()
            .ends_with(".timeline.json"));
        // Output files should use the original source filename stem, not a hash.
        assert_eq!(text.file_name().and_then(|n| n.to_str()), Some("audio.txt"));
        assert_eq!(
            metadata.file_name().and_then(|n| n.to_str()),
            Some("audio.json")
        );
        assert_eq!(
            timeline.file_name().and_then(|n| n.to_str()),
            Some("audio.timeline.json")
        );
    }

    #[test]
    fn output_paths_preserve_subdirectory_structure() {
        let temp = TempDir::new().unwrap();
        let audio_dir = Path::new("/data/recordings");
        let source = Path::new("/data/recordings/meeting1/track_a.wav");
        let (text, metadata, timeline) = output_paths_in(temp.path(), "task2", source, audio_dir);
        let base = temp.path().join("asr/data/text/task2/meeting1");
        assert_eq!(text, base.join("track_a.txt"));
        assert_eq!(metadata, base.join("track_a.json"));
        assert_eq!(timeline, base.join("track_a.timeline.json"));
    }

    #[test]
    fn task_run_lock_rejects_concurrent_runs_and_releases_after_drop() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());

        let first = TaskRunFileLock::acquire("task1").unwrap();
        let second = TaskRunFileLock::acquire("task1");
        assert!(second.is_err());

        drop(first);
        let third = TaskRunFileLock::acquire("task1");
        assert!(third.is_ok());
    }

    #[test]
    fn task_run_lock_recovers_legacy_stale_lock_after_restart() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let lock_path = temp.path().join("asr/tasks/task1/run.lock");
        std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        std::fs::write(&lock_path, b"").unwrap();

        let lock = TaskRunFileLock::acquire("task1").unwrap();
        let content = std::fs::read_to_string(&lock_path).unwrap();
        let parsed = serde_json::from_str::<TaskRunLockFile>(&content).unwrap();
        assert_eq!(parsed.pid, std::process::id());
        drop(lock);
        assert!(!lock_path.exists());
    }

    #[test]
    fn startup_recovery_requeues_enabled_interrupted_task() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        RUNNING_TASKS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("interrupted.wav");
        std::fs::write(&audio, b"audio").unwrap();
        let task = test_directory_task("recover-task", audio_dir);
        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task.clone()],
        })
        .unwrap();

        let lock_path = task_run_lock_path(&task.id);
        std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        std::fs::write(
            &lock_path,
            serde_json::to_string(&TaskRunLockFile {
                pid: u32::MAX,
                process_start_time: 1,
                acquired_at_ms: 1,
            })
            .unwrap(),
        )
        .unwrap();

        let key = source_key(&audio);
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Processing;
        record.started_at_ms = Some(123);
        record.progress_current = Some(3);
        record.progress_total = Some(9);
        record.error = Some("old transient error".to_string());
        save_file_store(
            &task.id,
            &FileStore {
                version: TASK_STORE_VERSION,
                files: BTreeMap::from([(key.clone(), record)]),
            },
        )
        .unwrap();

        let recovery = recover_interrupted_task_runs_on_startup();
        assert_eq!(recovery.len(), 1);
        assert_eq!(recovery[0].id, task.id);
        assert!(!lock_path.exists());

        let store = load_file_store(&task.id);
        let recovered = store.files.get(&key).unwrap();
        assert_eq!(recovered.status, FileStatus::Pending);
        assert_eq!(recovered.started_at_ms, None);
        assert_eq!(recovered.progress_current, None);
        assert_eq!(recovered.progress_total, None);
        assert_eq!(recovered.error, None);
    }

    #[test]
    fn startup_recovery_marks_running_daily_agent_items_interrupted() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let mut task = test_directory_task("recover-daily-agent-items", audio_dir);
        task.daily_agent.last_status = Some("running".to_string());
        task.daily_agent.agents = normalized_daily_agents(&task.daily_agent);
        task.daily_agent.agents[0].last_status = Some("success".to_string());
        task.daily_agent.agents[1].last_status = Some("running".to_string());

        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task],
        })
        .unwrap();

        let recovery = recover_interrupted_task_runs_on_startup();
        assert!(recovery.is_empty());

        let store = load_tasks();
        let recovered = &store.tasks[0];
        assert_eq!(
            recovered.daily_agent.agents[0].last_status.as_deref(),
            Some("success")
        );
        assert_eq!(
            recovered.daily_agent.agents[1].last_status.as_deref(),
            Some("interrupted")
        );
    }

    #[test]
    fn startup_recovery_marks_daily_agent_items_interrupted_before_fresh_task_lock_continue() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let mut task = test_directory_task("recover-daily-agent-fresh-lock", audio_dir);
        task.daily_agent.agents = normalized_daily_agents(&task.daily_agent);
        task.daily_agent.agents[0].last_status = Some("running".to_string());
        task.daily_agent.agents[1].last_status = Some("running".to_string());

        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task.clone()],
        })
        .unwrap();

        let lock_path = task_run_lock_path(&task.id);
        std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        create_task_run_lock(&lock_path).unwrap();
        assert!(!is_task_run_lock_stale(&lock_path));

        let recovery = recover_interrupted_task_runs_on_startup();
        assert!(recovery.is_empty());
        assert!(lock_path.exists());

        let store = load_tasks();
        let recovered = &store.tasks[0];
        assert_eq!(
            recovered.daily_agent.agents[0].last_status.as_deref(),
            Some("interrupted")
        );
        assert_eq!(
            recovered.daily_agent.agents[1].last_status.as_deref(),
            Some("interrupted")
        );
    }

    #[test]
    fn startup_recovery_does_not_requeue_paused_task() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        RUNNING_TASKS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("paused.wav");
        std::fs::write(&audio, b"audio").unwrap();
        let mut task = test_directory_task("paused-recover-task", audio_dir);
        task.paused = true;
        task.paused_at_ms = Some(10);
        task.next_run_at_ms = None;
        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task.clone()],
        })
        .unwrap();

        let lock_path = task_run_lock_path(&task.id);
        std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        std::fs::write(&lock_path, b"{}").unwrap();
        let key = source_key(&audio);
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Processing;
        save_file_store(
            &task.id,
            &FileStore {
                version: TASK_STORE_VERSION,
                files: BTreeMap::from([(key.clone(), record)]),
            },
        )
        .unwrap();

        let recovery = recover_interrupted_task_runs_on_startup();
        assert!(recovery.is_empty());
        assert!(!lock_path.exists());
        assert_eq!(
            load_file_store(&task.id).files.get(&key).unwrap().status,
            FileStatus::Pending
        );
    }

    #[test]
    fn startup_recovery_requeues_retryable_server_start_failures() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        RUNNING_TASKS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("retryable.wav");
        std::fs::write(&audio, b"audio").unwrap();
        let mut task = test_directory_task("retryable-failed-task", audio_dir);
        task.last_error = Some("71 file(s) failed".to_string());
        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task.clone()],
        })
        .unwrap();

        let key = source_key(&audio);
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Failed;
        record.started_at_ms = Some(123);
        record.finished_at_ms = Some(456);
        record.progress_current = Some(1);
        record.progress_total = Some(1);
        record.error = Some(
            "managed ASR server start failed: Qwen3-ASR service is busy.; detail=requested owner=directory_task:retryable-failed-task model=Qwen3-ASR-1.7B; active owner=directory_task:retryable-failed-task model=Qwen3-ASR-1.7B server=http://127.0.0.1:60241"
                .to_string(),
        );
        save_file_store(
            &task.id,
            &FileStore {
                version: TASK_STORE_VERSION,
                files: BTreeMap::from([(key.clone(), record)]),
            },
        )
        .unwrap();

        let recovery = recover_interrupted_task_runs_on_startup();
        assert_eq!(recovery.len(), 1);
        assert_eq!(recovery[0].id, task.id);

        let store = load_file_store(&task.id);
        let recovered = store.files.get(&key).unwrap();
        assert_eq!(recovered.status, FileStatus::Pending);
        assert_eq!(recovered.started_at_ms, None);
        assert_eq!(recovered.finished_at_ms, None);
        assert_eq!(recovered.progress_current, None);
        assert_eq!(recovered.progress_total, None);
        assert_eq!(recovered.error, None);
        assert_eq!(load_tasks().tasks[0].last_error, None);
    }

    #[test]
    fn startup_recovery_does_not_requeue_non_retryable_failed_records() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        RUNNING_TASKS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("bad-audio.wav");
        std::fs::write(&audio, b"audio").unwrap();
        let mut task = test_directory_task("non-retryable-failed-task", audio_dir);
        task.last_error = Some("1 file(s) failed".to_string());
        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task.clone()],
        })
        .unwrap();

        let key = source_key(&audio);
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Failed;
        record.error = Some("ffmpeg normalize failed: invalid data found".to_string());
        save_file_store(
            &task.id,
            &FileStore {
                version: TASK_STORE_VERSION,
                files: BTreeMap::from([(key.clone(), record)]),
            },
        )
        .unwrap();

        let recovery = recover_interrupted_task_runs_on_startup();
        assert!(recovery.is_empty());
        let store = load_file_store(&task.id);
        let unchanged = store.files.get(&key).unwrap();
        assert_eq!(unchanged.status, FileStatus::Failed);
        assert_eq!(
            unchanged.error.as_deref(),
            Some("ffmpeg normalize failed: invalid data found")
        );
        assert_eq!(
            load_tasks().tasks[0].last_error.as_deref(),
            Some("1 file(s) failed")
        );
    }

    #[test]
    fn startup_recovery_preserves_live_owner_lock() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        RUNNING_TASKS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let audio = audio_dir.join("live.wav");
        std::fs::write(&audio, b"audio").unwrap();
        let task = test_directory_task("live-owner-task", audio_dir);
        save_tasks(&TaskStore {
            version: TASK_STORE_VERSION,
            tasks: vec![task.clone()],
        })
        .unwrap();

        let live_lock = TaskRunFileLock::acquire(&task.id).unwrap();
        let key = source_key(&audio);
        let mut record = pending_record(&task.id, &audio);
        record.status = FileStatus::Processing;
        save_file_store(
            &task.id,
            &FileStore {
                version: TASK_STORE_VERSION,
                files: BTreeMap::from([(key.clone(), record)]),
            },
        )
        .unwrap();

        let recovery = recover_interrupted_task_runs_on_startup();
        assert!(recovery.is_empty());
        assert!(task_run_lock_path(&task.id).exists());
        assert_eq!(
            load_file_store(&task.id).files.get(&key).unwrap().status,
            FileStatus::Processing
        );
        drop(live_lock);
    }

    #[test]
    fn running_task_guard_releases_marker_on_drop() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        RUNNING_TASKS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        {
            let _guard = RunningTaskGuard::acquire("guard-task").unwrap();
            assert!(task_is_running("guard-task"));
            assert!(RunningTaskGuard::acquire("guard-task").is_err());
        }
        assert!(!task_is_running("guard-task"));
    }

    // ====================================================================
    //  compute_wav_rms_energy tests
    // ====================================================================

    /// Build a minimal valid WAV (16-bit PCM, 16kHz, mono) from raw i16 samples.
    fn make_wav(samples: &[i16]) -> Vec<u8> {
        let data_size = (samples.len() * 2) as u32;
        let file_size = 36 + data_size; // 4 (WAVE) + 24 (fmt ) + 8 (data header) + data
        let mut buf: Vec<u8> = Vec::with_capacity(file_size as usize + 8);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        // fmt sub-chunk
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes()); // sub-chunk size
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM format
        buf.extend_from_slice(&1u16.to_le_bytes()); // mono
        buf.extend_from_slice(&16000u32.to_le_bytes()); // sample rate
        buf.extend_from_slice(&32000u32.to_le_bytes()); // byte rate
        buf.extend_from_slice(&2u16.to_le_bytes()); // block align
        buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
                                                     // data sub-chunk
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for &s in samples {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        buf
    }

    /// Build a WAV with an extra sub-chunk before "data" (simulating bext/LIST).
    fn make_wav_with_extra_chunk(
        samples: &[i16],
        extra_id: &[u8; 4],
        extra_data: &[u8],
    ) -> Vec<u8> {
        let extra_chunk_size = extra_data.len() as u32;
        let extra_padded = extra_data.len() + (extra_data.len() & 1); // word-align
        let data_size = (samples.len() * 2) as u32;
        let file_size = 4 + 24 + 8 + extra_padded as u32 + 8 + data_size;
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        // fmt
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&16000u32.to_le_bytes());
        buf.extend_from_slice(&32000u32.to_le_bytes());
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        // extra sub-chunk (e.g. bext, LIST)
        buf.extend_from_slice(extra_id);
        buf.extend_from_slice(&extra_chunk_size.to_le_bytes());
        buf.extend_from_slice(extra_data);
        if extra_data.len() & 1 == 1 {
            buf.push(0); // word-align pad byte
        }
        // data
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for &s in samples {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        buf
    }

    #[test]
    fn rms_energy_known_samples() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.wav");
        // samples: [1000, -1000, 2000, -2000]
        // RMS = sqrt((1e6 + 1e6 + 4e6 + 4e6) / 4) = sqrt(2.5e6) ≈ 1581.14
        std::fs::write(&path, make_wav(&[1000, -1000, 2000, -2000])).unwrap();
        let rms = compute_wav_rms_energy(&path).unwrap();
        assert!((rms - 1581.14).abs() < 1.0, "expected ~1581.14, got {rms}");
    }

    #[test]
    fn rms_energy_digital_silence() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("silence.wav");
        std::fs::write(&path, make_wav(&[0i16; 1000])).unwrap();
        let rms = compute_wav_rms_energy(&path).unwrap();
        assert_eq!(rms, 0.0);
    }

    #[test]
    fn rms_energy_single_sample() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("one.wav");
        std::fs::write(&path, make_wav(&[500])).unwrap();
        let rms = compute_wav_rms_energy(&path).unwrap();
        // RMS of single sample = |500| = 500.0
        assert!((rms - 500.0).abs() < 0.01, "expected 500, got {rms}");
    }

    #[test]
    fn rms_energy_empty_data_chunk() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("empty.wav");
        std::fs::write(&path, make_wav(&[])).unwrap();
        let rms = compute_wav_rms_energy(&path).unwrap();
        assert_eq!(rms, 0.0);
    }

    #[test]
    fn rms_energy_with_extra_subchunk_before_data() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("bext.wav");
        let samples = [3000i16, -3000, 4000, -4000];
        let extra = vec![0u8; 602]; // bext chunk with 602 bytes (odd size → needs pad)
        std::fs::write(&path, make_wav_with_extra_chunk(&samples, b"bext", &extra)).unwrap();
        let rms = compute_wav_rms_energy(&path).unwrap();
        // RMS = sqrt((9e6 + 9e6 + 16e6 + 16e6) / 4) = sqrt(12.5e6) ≈ 3535.53
        assert!((rms - 3535.53).abs() < 1.0, "expected ~3535.53, got {rms}");
    }

    #[test]
    fn rms_energy_with_list_info_subchunk() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("list.wav");
        let samples = [100i16; 50];
        // Simulate a LIST chunk with INFO data (even size)
        let list_data = b"INFOsome metadata here!";
        std::fs::write(
            &path,
            make_wav_with_extra_chunk(&samples, b"LIST", list_data),
        )
        .unwrap();
        let rms = compute_wav_rms_energy(&path).unwrap();
        // All 100 → RMS = 100.0
        assert!((rms - 100.0).abs() < 0.01, "expected 100, got {rms}");
    }

    #[test]
    fn rms_energy_non_wav_file() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("not.wav");
        std::fs::write(&path, b"this is not a wav file at all").unwrap();
        assert!(compute_wav_rms_energy(&path).is_none());
    }

    #[test]
    fn normalized_wav_header_detection_accepts_16k_mono_pcm_without_ffprobe() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("normalized.wav");
        std::fs::write(&path, make_wav(&[100, -100, 200, -200])).unwrap();

        assert!(wav_header_is_normalized(&path));
    }

    #[test]
    fn normalized_wav_header_detection_rejects_non_wav() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("not.wav");
        std::fs::write(&path, b"not a wav").unwrap();

        assert!(!wav_header_is_normalized(&path));
    }

    #[test]
    fn rms_energy_truncated_wav_no_data_chunk() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("trunc.wav");
        // Valid RIFF/WAVE header + fmt chunk, but no data chunk
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&28u32.to_le_bytes()); // file size
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&[1, 0, 1, 0]); // PCM, mono
        buf.extend_from_slice(&16000u32.to_le_bytes());
        buf.extend_from_slice(&32000u32.to_le_bytes());
        buf.extend_from_slice(&[2, 0, 16, 0]); // block_align, bits
                                               // No data chunk follows
        std::fs::write(&path, &buf).unwrap();
        assert!(compute_wav_rms_energy(&path).is_none());
    }

    #[test]
    fn rms_energy_nonexistent_file() {
        assert!(compute_wav_rms_energy(Path::new("/tmp/nonexistent_9999.wav")).is_none());
    }

    #[test]
    fn rms_energy_below_silence_threshold_is_silent() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("quiet.wav");
        // 20 RMS → below SILENCE_RMS_THRESHOLD (30)
        std::fs::write(&path, make_wav(&[20i16; 100])).unwrap();
        let rms = compute_wav_rms_energy(&path).unwrap();
        assert!(
            rms < SILENCE_RMS_THRESHOLD,
            "RMS {rms} should be < {SILENCE_RMS_THRESHOLD}"
        );
    }

    #[test]
    fn rms_energy_above_silence_threshold_is_not_silent() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("speech.wav");
        // 500 RMS → well above SILENCE_RMS_THRESHOLD (30)
        std::fs::write(&path, make_wav(&[500i16; 100])).unwrap();
        let rms = compute_wav_rms_energy(&path).unwrap();
        assert!(
            rms >= SILENCE_RMS_THRESHOLD,
            "RMS {rms} should be >= {SILENCE_RMS_THRESHOLD}"
        );
    }

    // ====================================================================
    //  FailedChunkRecord serde compatibility tests
    // ====================================================================

    #[test]
    fn failed_chunk_record_backward_compat_missing_new_fields() {
        // Old data without energy_rms / is_silent should deserialize with defaults.
        let json = r#"{
            "chunk_index": 3,
            "offset_secs": 90,
            "duration_secs": 30,
            "error": "exit 255",
            "attempts": 3
        }"#;
        let record: FailedChunkRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.chunk_index, 3);
        assert_eq!(record.energy_rms, None);
        assert!(!record.is_silent);
    }

    #[test]
    fn failed_chunk_record_round_trip_with_new_fields() {
        let record = FailedChunkRecord {
            chunk_index: 5,
            offset_secs: 150,
            duration_secs: 30,
            error: "reshape error".to_string(),
            attempts: 6,
            energy_rms: Some(18.5),
            is_silent: false,
        };
        let json = serde_json::to_string(&record).unwrap();
        let back: FailedChunkRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.chunk_index, 5);
        assert!((back.energy_rms.unwrap() - 18.5).abs() < 0.001);
        assert!(!back.is_silent);
    }

    #[test]
    fn failed_chunk_record_skip_serializing_none_energy() {
        let record = FailedChunkRecord {
            chunk_index: 0,
            offset_secs: 0,
            duration_secs: 30,
            error: "err".to_string(),
            attempts: 1,
            energy_rms: None,
            is_silent: true,
        };
        let json = serde_json::to_string(&record).unwrap();
        // energy_rms should be absent when None (skip_serializing_if)
        assert!(
            !json.contains("energy_rms"),
            "None energy_rms should be skipped: {json}"
        );
        // is_silent should still be present
        assert!(
            json.contains("is_silent"),
            "is_silent should be present: {json}"
        );
    }

    #[test]
    fn failed_chunk_record_clone_inherits_new_fields() {
        let original = FailedChunkRecord {
            chunk_index: 2,
            offset_secs: 60,
            duration_secs: 30,
            error: "err".to_string(),
            attempts: 3,
            energy_rms: Some(42.0),
            is_silent: false,
        };
        // Simulate how retry handler clones: ..fc.clone() + override attempts/error
        let cloned = FailedChunkRecord {
            attempts: 4,
            error: "new err".to_string(),
            ..original.clone()
        };
        assert_eq!(cloned.energy_rms, Some(42.0));
        assert!(!cloned.is_silent);
        assert_eq!(cloned.attempts, 4);
    }

    #[test]
    fn partial_transcription_artifacts_update_file_store() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let source_path = audio_dir.join("meeting.wav");
        std::fs::write(&source_path, b"fake wav").unwrap();
        let task_id = "partial-stream-task";
        let file_key = source_key(&source_path);
        let source_info = inspect_source_audio(&source_path);
        let (text_path, metadata_path, timeline_path) = bifrost_asr::artifacts::output_paths_in(
            &bifrost_storage::data_dir(),
            task_id,
            &source_path,
            &audio_dir,
        );

        let mut initial_record = file_record_from_info(task_id, &source_path, &source_info);
        initial_record.status = FileStatus::Processing;
        initial_record.started_at_ms = Some(100);
        save_file_store(
            task_id,
            &FileStore {
                version: TASK_STORE_VERSION,
                files: BTreeMap::from([(file_key.clone(), initial_record)]),
            },
        )
        .unwrap();

        persist_partial_transcription_artifacts(
            &PartialArtifactContext {
                task_id: task_id.to_string(),
                file_key: file_key.clone(),
                task_name: "Partial Stream Task".to_string(),
                model: "Qwen3-ASR-0.6B".to_string(),
                language: "chinese".to_string(),
                runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
                source_path: source_path.clone(),
                source_info: source_info.clone(),
                diarization_profile: Some("sherpa-onnx-balanced".to_string()),
                speakers: vec![TimelineSpeaker {
                    id: "speaker_00".to_string(),
                    display_name: "用户A".to_string(),
                    mapped_profile_id: None,
                    confidence: None,
                    candidate_profile_id: None,
                    candidate_display_name: None,
                    candidate_confidence: None,
                }],
                text_path: text_path.clone(),
                metadata_path: metadata_path.clone(),
                timeline_path: timeline_path.clone(),
                started_at_ms: 100,
            },
            DiarizedSegmentProgress {
                text: "用户A: 你好。".to_string(),
                timeline_segments: vec![TimelineSegment {
                    index: 99,
                    audio_start_ms: 0,
                    audio_end_ms: 1200,
                    absolute_start_ms: None,
                    absolute_end_ms: None,
                    speaker: Some("speaker_00".to_string()),
                    speaker_display_name: Some("用户A".to_string()),
                    overlap: false,
                    text: "你好。".to_string(),
                }],
                chunk_metrics: Vec::new(),
                fallback_reason: Some("managed server fallback".to_string()),
            },
        )
        .unwrap();

        let rendered_text = std::fs::read_to_string(&text_path).unwrap();
        assert!(rendered_text.contains("用户A"));
        let timeline =
            serde_json::from_str::<TranscriptTimeline>(&std::fs::read_to_string(&timeline_path).unwrap())
                .unwrap();
        assert_eq!(timeline.segments.len(), 1);
        assert_eq!(timeline.segments[0].index, 0);
        assert_eq!(timeline.speakers[0].display_name, "用户A");
        let metadata =
            serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&metadata_path).unwrap())
                .unwrap();
        assert_eq!(metadata["partial"], true);
        assert_eq!(metadata["partial_segment_count"], 1);

        let stored = load_file_store(task_id);
        let record = stored.files.get(&file_key).unwrap();
        assert_eq!(record.status, FileStatus::Processing);
        assert_eq!(record.output_text_path.as_ref(), Some(&text_path));
        assert_eq!(record.output_metadata_path.as_ref(), Some(&metadata_path));
        assert_eq!(record.output_timeline_path.as_ref(), Some(&timeline_path));
        assert!(record.text_chars > 0);
        assert_eq!(
            record.fallback_reason.as_deref(),
            Some("managed server fallback")
        );
    }

    #[test]
    fn task_run_lock_recovers_dead_owner_lock() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let lock_path = temp.path().join("asr/tasks/task1/run.lock");
        std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        std::fs::write(
            &lock_path,
            serde_json::to_string(&TaskRunLockFile {
                pid: u32::MAX,
                process_start_time: 1,
                acquired_at_ms: 1,
            })
            .unwrap(),
        )
        .unwrap();

        let lock = TaskRunFileLock::acquire("task1").unwrap();
        let content = std::fs::read_to_string(&lock_path).unwrap();
        let parsed = serde_json::from_str::<TaskRunLockFile>(&content).unwrap();
        assert_eq!(parsed.pid, std::process::id());
        drop(lock);
    }

    include!("daily_agent_tests.rs");

    #[test]
    fn external_import_normalizes_duplicate_device_names() {
        let result = normalize_bindings(vec![
            AsrExternalDeviceBinding {
                name: "LEFT".to_string(),
                ..Default::default()
            },
            AsrExternalDeviceBinding {
                name: "LEFT".to_string(),
                ..Default::default()
            },
        ]);
        assert!(result.unwrap_err().contains("duplicate external device name"));
        assert_eq!(sanitize_device_root("LEFT/RIGHT"), "LEFT_RIGHT");
    }

    #[test]
    fn external_import_matches_uuid_without_crossing_device_names() {
        let binding = AsrExternalDeviceBinding {
            name: "RIGHT".to_string(),
            volume_uuid: Some("SHARED-UUID".to_string()),
            ..Default::default()
        };
        let left = ExternalVolumeInfo {
            name: "LEFT".to_string(),
            mount_path: PathBuf::from("/Volumes/LEFT"),
            volume_uuid: Some("SHARED-UUID".to_string()),
            device_identifier: None,
            kind: "external".to_string(),
            read_only: false,
            available_bytes: Some(1024),
        };
        let right = ExternalVolumeInfo {
            name: "RIGHT".to_string(),
            mount_path: PathBuf::from("/Volumes/RIGHT"),
            volume_uuid: Some("SHARED-UUID".to_string()),
            device_identifier: None,
            kind: "external".to_string(),
            read_only: false,
            available_bytes: Some(1024),
        };

        assert!(!external_volume_matches(&binding, &left));
        assert!(external_volume_matches(&binding, &right));
    }

    #[test]
    fn external_import_defers_recently_modified_files() {
        let now = now_ms();
        assert!(should_defer_unstable_source(
            None,
            128,
            Some(now.saturating_sub(500)),
            2
        ));
        assert!(!should_defer_unstable_source(
            None,
            128,
            Some(now.saturating_sub(2_500)),
            2
        ));
        assert!(!should_defer_unstable_source(None, 128, Some(now), 0));
    }

    #[test]
    fn external_import_skips_macos_appledouble_metadata_files() {
        assert!(is_macos_metadata_file("._auto-left.wav"));
        assert!(is_macos_metadata_file("._duplicate.m4a"));
        assert!(!is_macos_metadata_file("auto-left.wav"));
        assert!(!is_macos_metadata_file("nested._audio.wav"));
    }

    #[test]
    fn external_import_detects_completed_processing_record_for_removed_target() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let task = test_directory_task("processed-skip", temp.path().join("audio"));
        let target = task.audio_dir.join("RIGHT").join("done.wav");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"processed-audio").unwrap();
        let mut record = pending_record(&task.id, &target);
        record.status = FileStatus::Success;
        record.source_size = Some(15);
        std::fs::remove_file(&target).unwrap();
        save_file_store(
            &task.id,
            &FileStore {
                version: TASK_STORE_VERSION,
                files: BTreeMap::from([("processed".to_string(), record)]),
            },
        )
        .unwrap();

        assert!(has_completed_processing_record_for_import_target(
            &task.id, &target, 15
        ));
        assert!(!has_completed_processing_record_for_import_target(
            &task.id, &target, 16
        ));
        let mut legacy_record = pending_record(&task.id, &target);
        legacy_record.status = FileStatus::PartialSuccess;
        legacy_record.source_size = None;
        save_file_store(
            &task.id,
            &FileStore {
                version: TASK_STORE_VERSION,
                files: BTreeMap::from([("legacy-processed".to_string(), legacy_record)]),
            },
        )
        .unwrap();
        assert!(has_completed_processing_record_for_import_target(
            &task.id, &target, 16
        ));
    }

    #[test]
    fn external_import_progress_stale_importing_is_marked_failed() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let progress = AsrExternalImportRunProgress {
            run_id: "run1".to_string(),
            trigger: "test".to_string(),
            started_at_ms: 1,
            updated_at_ms: 1,
            finished_at_ms: None,
            imported: 0,
            skipped: 0,
            processed_record_skipped: 0,
            failed: 0,
            status: "importing".to_string(),
            current_device: Some("LEFT".to_string()),
            current_file: None,
            current_file_size: None,
            current_file_copied_bytes: 0,
            total_files_discovered: 0,
            processed_files: 0,
            message: "running".to_string(),
        };
        save_external_import_progress("task1", &progress).unwrap();

        let normalized = normalize_external_import_progress("task1").unwrap();
        assert_eq!(normalized.status, "failed");
        assert!(normalized.finished_at_ms.is_some());
        assert!(normalized.message.contains("interrupted"));
    }

    #[test]
    fn external_import_copy_reports_byte_progress() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source.wav");
        let target = temp.path().join("target.wav");
        std::fs::write(&source, vec![7u8; 3 * 1024 * 1024 + 17]).unwrap();
        let mut progress = Vec::new();

        let hash = copy_with_content_hash_with_progress(&source, &target, "blake3", |copied| {
            progress.push(copied);
        })
        .unwrap();

        assert_eq!(target.metadata().unwrap().len(), source.metadata().unwrap().len());
        assert_eq!(progress.last().copied(), Some(source.metadata().unwrap().len()));
        assert_eq!(hash.hashes.get("blake3"), Some(&blake3_file(&source).unwrap()));
    }

    #[test]
    fn task_audio_dir_creation_allows_missing_nested_directory() {
        let temp = TempDir::new().unwrap();
        let audio_dir = temp.path().join("missing").join("nested").join("audio");

        assert!(!audio_dir.exists());
        ensure_task_audio_dir(&audio_dir).unwrap();
        assert!(audio_dir.is_dir());
    }

    #[test]
    fn task_audio_dir_creation_rejects_existing_file() {
        let temp = TempDir::new().unwrap();
        let audio_dir = temp.path().join("not-a-dir");
        std::fs::write(&audio_dir, b"file").unwrap();

        let error = ensure_task_audio_dir(&audio_dir).unwrap_err();
        assert!(error.contains("must be a directory"));
    }

    #[test]
    fn content_hash_dedupe_reuses_completed_transcript() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let first = audio_dir.join("LEFT").join("a.wav");
        let second = audio_dir.join("RIGHT").join("copy.wav");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::create_dir_all(second.parent().unwrap()).unwrap();
        std::fs::write(&first, b"same-audio").unwrap();
        std::fs::write(&second, b"same-audio").unwrap();

        let task = AsrDirectoryTask {
            id: "hash-task".to_string(),
            name: "Hash Task".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: AsrTaskSchedule::Hourly { minute: 0 },
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: Some(1),
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };

        let text_path = temp.path().join("text.txt");
        let metadata_path = temp.path().join("text.json");
        std::fs::write(&text_path, "hello").unwrap();
        std::fs::write(&metadata_path, "{}").unwrap();
        let first_key = source_key(&first);
        let second_key = source_key(&second);
        let mut files = FileStore::default();
        let mut first_record = pending_record(&task.id, &first);
        first_record.status = FileStatus::Success;
        first_record.output_text_path = Some(text_path.clone());
        first_record.output_metadata_path = Some(metadata_path.clone());
        first_record.content_hash = Some(blake3_file(&first).unwrap());
        first_record.content_hash_algorithm = Some("blake3".to_string());
        first_record.text_chars = 5;
        first_record.finished_at_ms = Some(now_ms());
        files.files.insert(first_key.clone(), first_record.clone());
        let mut second_record = pending_record(&task.id, &second);
        second_record.content_hash = first_record.content_hash.clone();
        second_record.content_hash_algorithm = first_record.content_hash_algorithm.clone();
        files.files.insert(second_key.clone(), second_record);
        index_completed_file_hash(&task, &first_key, &first_record);

        apply_content_hash_dedupe(&task, &[first, second], &mut files).unwrap();
        let duplicate = files.files.get(&second_key).unwrap();
        assert_eq!(duplicate.status, FileStatus::Success);
        assert_eq!(duplicate.duplicate_of_source_key.as_deref(), Some(first_key.as_str()));
        assert_eq!(duplicate.output_text_path.as_ref(), Some(&text_path));
        assert_eq!(duplicate.text_chars, 5);
    }

    #[test]
    fn external_import_blake3_hashes_are_applied_to_asr_records() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        let imported = audio_dir.join("LEFT").join("a.wav");
        std::fs::create_dir_all(imported.parent().unwrap()).unwrap();
        std::fs::write(&imported, b"imported audio").unwrap();

        let task = test_directory_task("import-hash-task", audio_dir);
        let key = source_key(&imported);
        let mut files = FileStore::default();
        files.files.insert(key.clone(), pending_record(&task.id, &imported));
        let mut hashes = BTreeMap::new();
        hashes.insert("blake3".to_string(), blake3_file(&imported).unwrap());
        let store = AsrExternalImportStore {
            version: TASK_STORE_VERSION,
            devices: BTreeMap::from([(
                "LEFT".to_string(),
                AsrExternalDeviceState {
                    binding_name: "LEFT".to_string(),
                    files: BTreeMap::from([(
                        "a.wav".to_string(),
                        AsrImportedFileRecord {
                            relative_path: PathBuf::from("a.wav"),
                            source_size: imported.metadata().unwrap().len(),
                            source_modified_ms: source_modified_ms(&imported),
                            source_hashes: hashes.clone(),
                            sample_fingerprint: None,
                            target_path: imported.clone(),
                            target_size: imported.metadata().unwrap().len(),
                            first_seen_at_ms: None,
                            imported_at_ms: now_ms(),
                            status: "imported".to_string(),
                            error: None,
                        },
                    )]),
                    ..Default::default()
                },
            )]),
            runs: Vec::new(),
        };
        save_external_import_store(&task.id, &store).unwrap();

        assert!(apply_external_import_hashes_to_records(
            &task,
            std::slice::from_ref(&imported),
            &mut files
        ));
        let record = files.files.get(&key).unwrap();
        assert_eq!(record.content_hash_algorithm.as_deref(), Some("blake3"));
        assert_eq!(record.content_hash.as_deref(), hashes.get("blake3").map(String::as_str));
    }

    #[test]
    fn content_hash_dedupe_hashes_manual_copy_when_candidate_exists() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        let first = audio_dir.join("done.wav");
        let second = audio_dir.join("manual-copy.wav");
        std::fs::create_dir_all(&audio_dir).unwrap();
        std::fs::write(&first, b"same manual payload").unwrap();
        std::fs::write(&second, b"same manual payload").unwrap();

        let task = test_directory_task("manual-copy-hash-task", audio_dir);
        let text_path = temp.path().join("done.txt");
        let metadata_path = temp.path().join("done.json");
        std::fs::write(&text_path, "manual transcript").unwrap();
        std::fs::write(&metadata_path, "{}").unwrap();
        let first_key = source_key(&first);
        let second_key = source_key(&second);
        let mut files = FileStore::default();
        let mut first_record = pending_record(&task.id, &first);
        first_record.status = FileStatus::Success;
        first_record.output_text_path = Some(text_path.clone());
        first_record.output_metadata_path = Some(metadata_path);
        first_record.content_hash = Some(blake3_file(&first).unwrap());
        first_record.content_hash_algorithm = Some("blake3".to_string());
        first_record.finished_at_ms = Some(now_ms());
        files.files.insert(first_key.clone(), first_record.clone());
        files.files.insert(second_key.clone(), pending_record(&task.id, &second));

        apply_content_hash_dedupe(&task, &[second, first], &mut files).unwrap();

        let duplicate = files.files.get(&second_key).unwrap();
        assert_eq!(duplicate.status, FileStatus::Success);
        assert_eq!(duplicate.content_hash_algorithm.as_deref(), Some("blake3"));
        assert_eq!(
            duplicate.duplicate_of_source_key.as_deref(),
            Some(first_key.as_str())
        );
        assert_eq!(duplicate.output_text_path.as_ref(), Some(&text_path));
    }

    #[test]
    fn content_hash_dedupe_does_not_hash_large_manual_copy_in_preflight() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        let first = audio_dir.join("done.wav");
        let second = audio_dir.join("large-manual-copy.wav");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let payload = vec![9u8; ASR_PREFLIGHT_HASH_MAX_BYTES as usize + 1];
        std::fs::write(&first, &payload).unwrap();
        std::fs::write(&second, &payload).unwrap();

        let task = test_directory_task("large-manual-copy-hash-task", audio_dir);
        let text_path = temp.path().join("done.txt");
        let metadata_path = temp.path().join("done.json");
        std::fs::write(&text_path, "large transcript").unwrap();
        std::fs::write(&metadata_path, "{}").unwrap();
        let first_key = source_key(&first);
        let second_key = source_key(&second);
        let mut files = FileStore::default();
        let mut first_record = pending_record(&task.id, &first);
        first_record.status = FileStatus::Success;
        first_record.output_text_path = Some(text_path);
        first_record.output_metadata_path = Some(metadata_path);
        first_record.content_hash = Some(blake3_file(&first).unwrap());
        first_record.content_hash_algorithm = Some("blake3".to_string());
        first_record.finished_at_ms = Some(now_ms());
        files.files.insert(first_key.clone(), first_record.clone());
        files.files.insert(second_key.clone(), pending_record(&task.id, &second));
        index_completed_file_hash(&task, &first_key, &first_record);

        apply_content_hash_dedupe(&task, &[first, second], &mut files).unwrap();

        let large_pending = files.files.get(&second_key).unwrap();
        assert_eq!(large_pending.status, FileStatus::Pending);
        assert!(large_pending.content_hash.is_none());
        assert!(large_pending.duplicate_of_source_key.is_none());
    }

    #[test]
    fn content_hash_dedupe_does_not_hash_unknown_records_on_resume() {
        let temp = TempDir::new().unwrap();
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let done = audio_dir.join("done.wav");
        let pending = audio_dir.join("pending.wav");
        std::fs::write(&done, b"already processed").unwrap();
        std::fs::write(&pending, b"new pending").unwrap();

        let task = AsrDirectoryTask {
            id: "hash-skip-task".to_string(),
            name: "Hash Skip Task".to_string(),
            audio_dir: audio_dir.clone(),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: AsrTaskSchedule::Hourly { minute: 0 },
            language: "chinese".to_string(),
            model: "Qwen3-ASR-1.7B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: Some(1),
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };

        let done_key = source_key(&done);
        let pending_key = source_key(&pending);
        let mut files = FileStore::default();
        let mut done_record = pending_record(&task.id, &done);
        done_record.status = FileStatus::Success;
        done_record.output_text_path = Some(temp.path().join("done.txt"));
        done_record.output_metadata_path = Some(temp.path().join("done.json"));
        done_record.finished_at_ms = Some(now_ms());
        files.files.insert(done_key.clone(), done_record);
        files.files.insert(pending_key.clone(), pending_record(&task.id, &pending));

        apply_content_hash_dedupe(&task, &[done, pending], &mut files).unwrap();

        let done_record = files.files.get(&done_key).unwrap();
        assert_eq!(done_record.status, FileStatus::Success);
        assert!(done_record.content_hash.is_none());
        assert!(done_record.content_hash_algorithm.is_none());
        let pending_record = files.files.get(&pending_key).unwrap();
        assert_eq!(pending_record.status, FileStatus::Pending);
        assert!(pending_record.content_hash.is_none());
        assert!(pending_record.content_hash_algorithm.is_none());
    }
}
