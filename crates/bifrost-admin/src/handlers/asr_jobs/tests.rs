#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use tempfile::TempDir;

    static TEST_DATA_DIR_LOCK: StdMutex<()> = StdMutex::new(());

    struct EnvGuard {
        previous: PathBuf,
    }

    impl EnvGuard {
        fn set_data_dir(path: &Path) -> Self {
            let previous = bifrost_storage::data_dir();
            bifrost_storage::set_data_dir(path.to_path_buf());
            Self { previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            bifrost_storage::set_data_dir(self.previous.clone());
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
    }

    #[test]
    fn chunk_metric_records_runner_rtf_hash_and_error() {
        let ok = Ok(WholeFileTranscription {
            text: "hello".to_string(),
            segments: Vec::new(),
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
    fn server_failure_fallback_reason_classifies_transport_errors() {
        let connect_error = "status: 502 Bad Gateway; cause: tcp connect error";
        assert!(is_server_transport_failure(connect_error));
        let reason =
            server_failure_fallback_reason(AsrRuntimeStrategy::ReusePerFile, connect_error);
        assert!(reason.contains("reuse_per_file strategy transport failure"));
        assert!(reason.contains("fork_per_chunk"));

        let http_error = "status: 500 Internal Server Error; model panic";
        assert!(!is_server_transport_failure(http_error));
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
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: Some(1),
            last_error: Some("old error".to_string()),
            daily_agent: AsrDailyAgentConfig::default(),
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
    fn query_flag_enabled_accepts_truthy_force_values() {
        assert!(query_flag_enabled("force=true", "force"));
        assert!(query_flag_enabled("force=1&other=false", "force"));
        assert!(query_flag_enabled("force", "force"));
        assert!(!query_flag_enabled("force=false", "force"));
        assert!(!query_flag_enabled("other=true", "force"));
    }

    #[tokio::test]
    async fn abortable_command_stops_on_pause_request() {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg("sleep 10");
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
    fn force_pause_requires_persisted_pause_state() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: Some(1),
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
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
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
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
    fn summary_reports_cleanable_source_audio_usage() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
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
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
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
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
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
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
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
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
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
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
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
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
            created_at_ms: start,
            updated_at_ms: start,
            last_run_at_ms: Some(start),
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
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
            processed_at_ms: start + 2_000,
            segments: vec![TimelineSegment {
                index: 0,
                audio_start_ms: 0,
                audio_end_ms: 2_000,
                absolute_start_ms: Some(start),
                absolute_end_ms: Some(start + 2_000),
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
            .join("asr/data/text/task-daily-refresh/daily/2026-05-14.md");
        let daily = std::fs::read_to_string(daily_path).unwrap();
        assert!(daily.contains("# Daily Refresh"));
        assert!(daily.contains("完整按天整理内容"));
    }

    #[test]
    fn interrupted_processing_records_reset_to_pending_before_next_run() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
            processed_at_ms: 1,
            segments: vec![TimelineSegment {
                index: 0,
                audio_start_ms: 0,
                audio_end_ms: 65_000,
                absolute_start_ms: Some(1_000_000),
                absolute_end_ms: Some(1_065_000),
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
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
    fn task_run_lock_recovers_dead_owner_lock() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap();
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
}
