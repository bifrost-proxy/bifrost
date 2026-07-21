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

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set_data_dir(path: &Path) -> Self {
            Self {
                _guard: crate::test_env::BifrostDataDirGuard::set(path),
            }
        }
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }

    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.previous.as_ref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn test_directory_task(id: &str, audio_dir: PathBuf) -> AsrDirectoryTask {
        AsrDirectoryTask {
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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
        assert_eq!(task.transcription_mode, AsrTranscriptionMode::Standard);
        assert!(task.transcription_prompt.is_empty());
    }

    #[test]
    fn transcription_prompt_normalization_preserves_lines_and_rejects_unsafe_input() {
        assert_eq!(
            normalize_transcription_prompt("  Bifrost\r\nNextOnCall  ".to_string()).unwrap(),
            "Bifrost\nNextOnCall"
        );
        assert!(normalize_transcription_prompt("bad\0prompt".to_string()).is_err());
        assert!(normalize_transcription_prompt("x".repeat(4_001)).is_err());
    }

    #[test]
    fn moss_json_parser_preserves_native_speakers_and_timestamps() {
        let result = parse_moss_json(
            r#"[
              {"id":"0","start":0.48,"end":3.12,"speaker":"S01","text":"你好"},
              {"id":"1","start":3.5,"end":5.0,"speaker":"S02","text":"开始开会"}
            ]"#
                .as_bytes(),
            6_000,
        )
        .unwrap();
        assert_eq!(result.text, "你好\n开始开会");
        assert_eq!(result.structured.segments.len(), 2);
        assert_eq!(result.structured.segments[0].start_ms, 480);
        assert_eq!(result.structured.segments[0].end_ms, 3_120);
        assert_eq!(result.structured.segments[0].speaker.as_deref(), Some("S01"));
        assert_eq!(
            result.structured.finish_reason,
            bifrost_asr::transcription::TranscriptionFinishReason::Completed
        );

        let unsupported_finish_reason = parse_moss_json(
            br#"{"segments":[],"finish_reason":"stopped"}"#,
            6_000,
        )
        .unwrap_err();
        assert!(unsupported_finish_reason.contains("unsupported finish reason stopped"));
    }

    #[cfg(unix)]
    #[test]
    fn moss_site_packages_hash_skips_cache_artifacts_and_non_files() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let paths = moss_runtime_paths(temp.path());
        let package = paths.site_packages.join("fixture");
        std::fs::create_dir_all(package.join("__pycache__")).unwrap();
        std::fs::write(package.join("module.py"), b"print('fixture')\n").unwrap();
        std::fs::write(package.join("module.pyc"), b"cached").unwrap();
        std::fs::write(package.join(".DS_Store"), b"finder metadata").unwrap();
        std::fs::write(package.join("__pycache__/module.pyc"), b"cached").unwrap();
        symlink("module.py", package.join("module-link.py")).unwrap();

        let checksums = moss_site_packages_sha256(&paths).unwrap();
        assert_eq!(checksums.len(), 1);
        assert!(checksums.contains_key("fixture/module.py"));

        let invalid_paths = moss_runtime_paths(&temp.path().join("invalid"));
        std::fs::create_dir_all(invalid_paths.site_packages.parent().unwrap()).unwrap();
        std::fs::write(&invalid_paths.site_packages, b"not a directory").unwrap();
        assert!(moss_site_packages_sha256(&invalid_paths)
            .unwrap_err()
            .contains("read MOSS site-packages directory"));
    }

    #[test]
    fn moss_token_budget_scales_with_duration_and_rejects_oversized_whole_files() {
        assert_eq!(moss_output_token_budget(30_000).unwrap(), 5_120);
        assert_eq!(moss_output_token_budget(600_000).unwrap(), 12_000);
        let error =
            moss_output_token_budget((MOSS_MAX_WHOLE_FILE_SECONDS + 1) * 1_000).unwrap_err();
        assert!(error.starts_with(&format!(
            "moss_non_retryable_v{}: moss_audio_too_long:",
            env!("CARGO_PKG_VERSION")
        )));
    }

    #[test]
    fn moss_runtime_smoke_accepts_usage_from_either_output_stream() {
        assert!(moss_runtime_help_is_valid(
            b"moss-mlx-runtime ok",
            b""
        ));
        assert!(moss_runtime_help_is_valid(
            b"",
            b"error\nmoss-mlx-runtime ok"
        ));
        assert!(!moss_runtime_help_is_valid(b"unexpected", b"failure"));
    }

    #[test]
    fn moss_runtime_checksum_manifest_selects_exact_asset_and_rejects_bad_hashes() {
        let asset = "moss-joint-runtime-v0.0.156-aarch64-apple-darwin.zip";
        let expected = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let manifest = format!(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  other.zip\n{expected}  dist/{asset}\n"
        );
        assert_eq!(
            parse_runtime_checksum_manifest(&manifest, asset).unwrap(),
            expected
        );
        assert!(parse_runtime_checksum_manifest(&manifest, "missing.zip").is_err());
        assert!(normalize_sha256("not-a-checksum", asset).is_err());
    }

    fn moss_test_model_spec(path: &Path, contents: &[u8]) -> MossModelSpec {
        std::fs::write(path, contents).unwrap();
        MossModelSpec {
            url: format!("file://{}", path.display()),
            bytes: contents.len() as u64,
            sha256: sha256_file(path).unwrap(),
        }
    }

    fn write_moss_runtime_zip(path: &Path, script: &str) {
        use std::io::Write;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let executable = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        let regular = zip::write::SimpleFileOptions::default().unix_permissions(0o644);
        zip.add_directory("moss-joint-runtime/", regular).unwrap();
        zip.add_directory("moss-joint-runtime/runtime/empty/", regular)
            .unwrap();
        zip.start_file("moss-joint-runtime/LICENSE", regular)
            .unwrap();
        zip.write_all(b"archive metadata outside installed roots")
            .unwrap();
        zip.start_file(
            "moss-joint-runtime/runtime/python/bin/python3.12-real",
            executable,
        )
        .unwrap();
        zip.write_all(script.as_bytes()).unwrap();
        zip.add_symlink(
            "moss-joint-runtime/runtime/python/bin/python3.12",
            "python3.12-real",
            executable,
        )
        .unwrap();
        zip.start_file("moss-joint-runtime/runtime/moss_mlx_runner.py", regular)
            .unwrap();
        zip.write_all(b"# fixture runner\n").unwrap();
        zip.start_file("moss-joint-runtime/runtime/site-packages/.keep", regular)
            .unwrap();
        zip.write_all(b"fixture").unwrap();
        zip.start_file(
            "moss-joint-runtime/runtime/site-packages/._invalid.py",
            regular,
        )
        .unwrap();
        zip.write_all(b"\x00\x05\x16\x07appledouble").unwrap();
        zip.start_file("moss-joint-runtime/model/._config.json", regular)
            .unwrap();
        zip.write_all(b"\x00\x05\x16\x07appledouble").unwrap();
        zip.start_file("moss-joint-runtime/model/.DS_Store", regular)
            .unwrap();
        zip.write_all(b"metadata").unwrap();
        for required in MOSS_MODEL_REQUIRED_FILES {
            zip.start_file(format!("moss-joint-runtime/model/{required}"), regular)
                .unwrap();
            zip.write_all(b"{}").unwrap();
        }
        zip.finish().unwrap();
    }

    fn prepare_moss_model_snapshot(paths: &MossRuntimePaths) {
        std::fs::create_dir_all(&paths.model_dir).unwrap();
        for required in MOSS_MODEL_REQUIRED_FILES {
            std::fs::write(paths.model_dir.join(required), b"{}").unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn moss_model_management_status_reports_missing_ready_and_unsupported_assets() {
        let temp = TempDir::new().unwrap();
        let asr_home = temp.path().join("asr-home");
        let paths = moss_runtime_paths(&asr_home);
        let model_source = temp.path().join("model-source.safetensors");
        let model_spec = moss_test_model_spec(&model_source, b"verified model fixture");

        let missing = moss_management_status_with_spec(
            &asr_home,
            "macos",
            "aarch64",
            &model_spec,
        )
        .await;
        assert_eq!(missing.status, "missing");
        assert!(!missing.ready);
        assert!(!missing.runtime_ready);
        assert!(!missing.model_ready);
        assert_eq!(missing.expected_model_bytes, 22);
        assert!(!missing.initializing);
        assert!(missing.initialization.is_none());

        let initialization_guard = begin_moss_initialization(&asr_home, "");
        publish_moss_initialization_progress(
            &asr_home,
            crate::resource_download::DownloadProgress {
                label: "MOSS runtime".to_string(),
                url: "https://example.invalid/runtime.zip".to_string(),
                dest: paths
                    .python_home
                    .parent()
                    .unwrap()
                    .join("runtime.zip")
                    .display()
                    .to_string(),
                downloaded_bytes: 68,
                total_bytes: Some(100),
                percent: Some(68),
                bytes_per_second: Some(10),
                eta_seconds: Some(4),
                elapsed_ms: 6_800,
                resumed: true,
                complete: false,
            },
        );
        let initializing = moss_management_status_with_spec(
            &asr_home,
            "macos",
            "aarch64",
            &model_spec,
        )
        .await;
        assert!(initializing.initializing);
        let serialized = serde_json::to_value(&initializing).unwrap();
        assert!(serialized["initialization"].get("url").is_none());
        assert!(serialized["initialization"].get("dest").is_none());
        let initialization = initializing.initialization.unwrap();
        assert_eq!(initialization.downloaded_bytes, 68);
        assert_eq!(initialization.percent, Some(68));
        assert!(initialization.resumed);
        drop(initialization_guard);
        let completed =
            moss_management_status_with_spec(&asr_home, "macos", "aarch64", &model_spec).await;
        assert!(!completed.initializing);
        assert!(completed.initialization.is_none());

        let runtime_framework = paths.python_home.join("lib/framework.fixture");
        write_executable(
            &paths.python,
            "#!/bin/sh\nframework=\"$(dirname \"$0\")/../lib/framework.fixture\"\nif [ \"$(cat \"$framework\" 2>/dev/null)\" = 'verified framework' ]; then echo 'moss-mlx-runtime ok'; else echo 'framework missing or corrupt' >&2; exit 1; fi\n",
        );
        std::fs::create_dir_all(runtime_framework.parent().unwrap()).unwrap();
        std::fs::write(&runtime_framework, b"verified framework\n").unwrap();
        std::fs::write(&paths.runner, b"fixture runner").unwrap();
        std::fs::create_dir_all(&paths.site_packages).unwrap();
        std::fs::write(paths.site_packages.join("mlx.fixture"), b"verified package").unwrap();
        prepare_moss_model_snapshot(&paths);
        std::fs::copy(&model_source, &paths.model).unwrap();
        write_moss_verification_marker(&asr_home, &paths, &model_spec).unwrap();

        let ready = moss_management_status_with_spec(
            &asr_home,
            "macos",
            "aarch64",
            &model_spec,
        )
        .await;
        assert_eq!(ready.status, "ready");
        assert!(ready.ready && ready.installed);
        assert!(ready.runtime_ready && ready.model_ready);
        assert_eq!(ready.installed_model_bytes, model_spec.bytes);
        assert_eq!(ready.model, MOSS_MODEL_ID);

        let verification_path = moss_verification_path(&asr_home);
        let mut marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&verification_path).unwrap()).unwrap();
        assert_eq!(marker["runtime_version"], moss_runtime_version());
        marker["runtime_version"] = serde_json::Value::String("0.9.0".to_string());
        std::fs::write(
            &verification_path,
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();
        let stale_runtime = moss_management_status_with_spec(
            &asr_home,
            "macos",
            "aarch64",
            &model_spec,
        )
        .await;
        assert!(!stale_runtime.runtime_ready);
        assert!(!stale_runtime.model_ready);
        write_moss_verification_marker(&asr_home, &paths, &model_spec).unwrap();

        std::fs::remove_file(&runtime_framework).unwrap();
        let missing_runtime_framework = moss_management_status_with_spec(
            &asr_home,
            "macos",
            "aarch64",
            &model_spec,
        )
        .await;
        assert_eq!(missing_runtime_framework.status, "partial");
        assert!(!missing_runtime_framework.runtime_ready);
        assert!(missing_runtime_framework.model_ready);

        std::fs::write(&runtime_framework, b"corrupt framework\n").unwrap();
        let corrupt_runtime_framework = moss_management_status_with_spec(
            &asr_home,
            "macos",
            "aarch64",
            &model_spec,
        )
        .await;
        assert!(!corrupt_runtime_framework.runtime_ready);
        assert!(corrupt_runtime_framework.model_ready);
        std::fs::write(&runtime_framework, b"verified framework\n").unwrap();

        std::fs::remove_file(paths.site_packages.join("mlx.fixture")).unwrap();
        let missing_runtime_dependency = moss_management_status_with_spec(
            &asr_home,
            "macos",
            "aarch64",
            &model_spec,
        )
        .await;
        assert!(!missing_runtime_dependency.runtime_ready);
        assert!(missing_runtime_dependency.model_ready);
        std::fs::write(paths.site_packages.join("mlx.fixture"), b"verified package").unwrap();

        std::fs::write(paths.site_packages.join("mlx.fixture"), b"corrupt package").unwrap();
        let corrupt_runtime = moss_management_status_with_spec(
            &asr_home,
            "macos",
            "aarch64",
            &model_spec,
        )
        .await;
        assert_eq!(corrupt_runtime.status, "partial");
        assert!(!corrupt_runtime.runtime_ready);
        assert!(corrupt_runtime.model_ready);
        std::fs::write(paths.site_packages.join("mlx.fixture"), b"verified package").unwrap();

        std::fs::write(paths.site_packages.join("unexpected.fixture"), b"unexpected package")
            .unwrap();
        let unexpected_runtime_dependency = moss_management_status_with_spec(
            &asr_home,
            "macos",
            "aarch64",
            &model_spec,
        )
        .await;
        assert!(!unexpected_runtime_dependency.runtime_ready);
        assert!(unexpected_runtime_dependency.model_ready);
        std::fs::remove_file(paths.site_packages.join("unexpected.fixture")).unwrap();

        std::thread::sleep(Duration::from_millis(5));
        std::fs::write(&paths.model, b"tampered model fixture").unwrap();
        let tampered = moss_management_status_with_spec(
            &asr_home,
            "macos",
            "aarch64",
            &model_spec,
        )
        .await;
        assert_eq!(tampered.status, "partial");
        assert!(tampered.runtime_ready);
        assert!(!tampered.model_ready);

        std::fs::copy(&model_source, &paths.model).unwrap();
        write_moss_verification_marker(&asr_home, &paths, &model_spec).unwrap();
        std::fs::remove_file(&paths.python).unwrap();
        let model_only = moss_management_status_with_spec(
            &asr_home,
            "macos",
            "aarch64",
            &model_spec,
        )
        .await;
        assert_eq!(model_only.status, "partial");
        assert!(!model_only.runtime_ready);
        assert!(model_only.model_ready);
        assert!(model_only.message.contains("runtime is missing"));

        let unsupported = moss_management_status_with_spec(
            &asr_home,
            "linux",
            "x86_64",
            &model_spec,
        )
        .await;
        assert_eq!(unsupported.status, "unsupported");
        assert!(!unsupported.platform_supported);
        assert!(unsupported.unsupported_reason.is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn moss_management_handlers_stream_success_and_failures() {
        let _lock = test_data_dir_lock();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path().join("data").as_path());

        let status_response = handle_moss_model_status().await;
        assert_eq!(status_response.status(), hyper::StatusCode::OK);
        let status_body = http_body_util::BodyExt::collect(status_response.into_body())
            .await
            .unwrap()
            .to_bytes();
        let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
        assert!(status_json.get("status").is_some());

        let archive = temp.path().join("runtime-source.zip");
        write_moss_runtime_zip(&archive, "#!/bin/sh\necho 'moss-mlx-runtime ok'\n");
        let model_source = temp.path().join("model-source.safetensors");
        let model_spec = moss_test_model_spec(&model_source, b"verified model fixture");
        let runtime_source = MossRuntimeSource {
            asset: "runtime.zip".to_string(),
            url: format!("file://{}", archive.display()),
            sha256: sha256_file(&archive).unwrap(),
        };

        let success_home = temp.path().join("success-home");
        let (success_tx, mut success_rx) = tokio::sync::mpsc::channel(32);
        stream_moss_model_initialization_with_spec(
            success_tx,
            success_home.clone(),
            Ok(runtime_source.asset.clone()),
            model_spec.clone(),
            Some(runtime_source.clone()),
        )
        .await;
        let mut success_events = String::new();
        while let Some(frame) = success_rx.recv().await {
            success_events.push_str(&String::from_utf8_lossy(&frame));
        }
        assert!(success_events.contains("event: progress"));
        assert!(success_events.contains("Checking MOSS runtime"));
        assert!(success_events.contains("Downloading verified MOSS assets"));
        assert!(success_events.contains("MOSS runtime and verified 8-bit model are ready"));
        assert!(success_events.contains("event: done"));
        assert!(moss_verification_path(&success_home).is_file());

        let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel(8);
        stream_moss_model_initialization_with_spec(
            ready_tx,
            success_home,
            Ok(runtime_source.asset.clone()),
            model_spec.clone(),
            Some(runtime_source.clone()),
        )
        .await;
        let mut ready_events = String::new();
        while let Some(frame) = ready_rx.recv().await {
            ready_events.push_str(&String::from_utf8_lossy(&frame));
        }
        assert!(!ready_events.contains("Downloading verified MOSS assets"));
        assert!(ready_events.contains("MOSS runtime and verified 8-bit model are ready"));
        assert!(ready_events.contains("event: done"));

        let (unsupported_tx, mut unsupported_rx) = tokio::sync::mpsc::channel(4);
        stream_moss_model_initialization_with_spec(
            unsupported_tx,
            temp.path().join("unsupported-home"),
            Err("unsupported fixture".to_string()),
            model_spec.clone(),
            None,
        )
        .await;
        let unsupported_event = unsupported_rx.recv().await.unwrap();
        let unsupported_event = String::from_utf8_lossy(&unsupported_event);
        assert!(unsupported_event.contains("event: error"));
        assert!(unsupported_event.contains("not supported"));

        let (failure_tx, mut failure_rx) = tokio::sync::mpsc::channel(16);
        let invalid_runtime = MossRuntimeSource {
            sha256: "0".repeat(64),
            ..runtime_source
        };
        stream_moss_model_initialization_with_spec(
            failure_tx,
            temp.path().join("failure-home"),
            Ok(invalid_runtime.asset.clone()),
            model_spec,
            Some(invalid_runtime),
        )
        .await;
        let mut failure_events = String::new();
        while let Some(frame) = failure_rx.recv().await {
            failure_events.push_str(&String::from_utf8_lossy(&frame));
        }
        assert!(failure_events.contains("event: error"));
        assert!(failure_events.contains("MOSS initialization failed"));
        assert!(failure_events.contains("checksum mismatch"));
    }

    fn moss_process_test_paths(root: &Path, python: PathBuf) -> MossRuntimePaths {
        let python_home = root.join("python-home");
        let site_packages = root.join("site-packages");
        let runner = root.join("moss_mlx_runner.py");
        let model_dir = root.join("model");
        std::fs::create_dir_all(&python_home).unwrap();
        std::fs::create_dir_all(&site_packages).unwrap();
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(&runner, b"fixture runner").unwrap();
        let model = model_dir.join(MOSS_MODEL_FILE);
        std::fs::write(&model, b"model").unwrap();
        MossRuntimePaths {
            python_home,
            python,
            site_packages,
            runner,
            model_dir,
            model,
        }
    }

    #[cfg(unix)]
    fn write_executable(path: &Path, script: &str) {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, script).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    fn spawn_moss_http_server(body: Vec<u8>) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::{Duration, Instant};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0_u8; 4096];
                        let read = stream.read(&mut request).unwrap_or(0);
                        let is_head = request[..read].starts_with(b"HEAD ");
                        let header = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        stream.write_all(header.as_bytes()).unwrap();
                        if !is_head {
                            stream.write_all(&body).unwrap();
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        format!("http://{address}/resource")
    }

    fn spawn_flaky_resumable_moss_http_server(
        body: Vec<u8>,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpListener};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = requests.clone();
        std::thread::spawn(move || {
            let split = body.len() / 2;
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let read = stream.read(&mut request).unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]).into_owned();
                recorded.lock().unwrap().push(request);
                if attempt == 0 {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(header.as_bytes()).unwrap();
                    stream.write_all(&body[..split]).unwrap();
                    let _ = stream.shutdown(Shutdown::Both);
                } else {
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                        body.len() - split,
                        split,
                        body.len() - 1,
                        body.len()
                    );
                    stream.write_all(header.as_bytes()).unwrap();
                    stream.write_all(&body[split..]).unwrap();
                }
            }
        });
        (format!("http://{address}/resource"), requests)
    }

    fn spawn_failing_moss_http_server(
        attempts: usize,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(AtomicUsize::new(0));
        let recorded = requests.clone();
        std::thread::spawn(move || {
            for _ in 0..attempts {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request);
                recorded.fetch_add(1, Ordering::SeqCst);
                stream
                    .write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
            }
        });
        (format!("http://{address}/resource"), requests)
    }

    #[tokio::test]
    async fn moss_resource_download_retries_and_resumes_transient_body_failure() {
        let temp = TempDir::new().unwrap();
        let body = vec![0x5a; 1024 * 1024];
        let split = body.len() / 2;
        let (url, requests) = spawn_flaky_resumable_moss_http_server(body.clone());
        let destination = temp.path().join("model.safetensors");

        download_moss_resource(temp.path(), url, destination.clone(), "MOSS test model", None)
            .await
            .unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), body);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(!requests[0].to_ascii_lowercase().contains("range:"));
        assert!(requests[1]
            .to_ascii_lowercase()
            .contains(&format!("range: bytes={split}-")));
    }

    #[tokio::test]
    async fn moss_resource_download_stops_after_bounded_failures() {
        use std::sync::atomic::Ordering;

        let temp = TempDir::new().unwrap();
        let (url, requests) =
            spawn_failing_moss_http_server(MOSS_RESOURCE_DOWNLOAD_MAX_ATTEMPTS);
        let error = download_moss_resource(
            temp.path(),
            url,
            temp.path().join("unavailable.bin"),
            "unavailable MOSS test resource",
            None,
        )
        .await
        .unwrap_err();

        assert!(error.contains("failed after 3 attempts"));
        assert_eq!(
            requests.load(Ordering::SeqCst),
            MOSS_RESOURCE_DOWNLOAD_MAX_ATTEMPTS
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn moss_runtime_configuration_and_checksum_sources_cover_supported_paths() {
        let _lock = test_data_dir_lock();
        let temp = TempDir::new().unwrap();
        let paths = moss_runtime_paths(temp.path());
        assert_eq!(
            moss_runtime_dir(temp.path()),
            temp.path().join("moss_joint_mlx")
        );
        assert_eq!(
            paths.python,
            temp.path()
                .join("moss_joint_mlx/runtime/python/bin/python3.12")
        );
        assert_eq!(
            paths.model,
            temp.path().join("moss_joint_mlx/model").join(MOSS_MODEL_FILE)
        );
        assert!(moss_runtime_asset_name_for("linux", "x86_64").is_err());
        let asset = moss_runtime_asset_name_for("macos", "aarch64").unwrap();
        assert_eq!(moss_runtime_version(), "1.0.0");
        assert_eq!(moss_runtime_release_tag(), "moss-runtime-v1.0.0");
        assert_eq!(
            asset,
            "moss-joint-runtime-v1.0.0-aarch64-apple-darwin.zip"
        );
        assert_eq!(
            moss_runtime_asset_name().is_ok(),
            cfg!(all(target_os = "macos", target_arch = "aarch64"))
        );
        assert_eq!(AsrTranscriptionMode::Standard.as_str(), "standard");
        assert_eq!(AsrTranscriptionMode::MossJoint.as_str(), "moss_joint");
        assert!(validate_moss_transcription_mode(AsrTranscriptionMode::Standard).is_ok());
        assert!(!AsrTranscriptionMode::Standard.uses_native_speakers());
        assert!(AsrTranscriptionMode::MossJoint.uses_native_speakers());
        assert!(moss_runtime_checksum_url(&asset).ends_with(&format!("{asset}.sha256")));

        let custom_url = std::ffi::OsStr::new("file:///tmp/moss-runtime.zip");
        let _url_guard = EnvVarGuard::set("BIFROST_MOSS_RUNTIME_URL", custom_url);
        let _sha_guard = EnvVarGuard::set(
            "BIFROST_MOSS_RUNTIME_SHA256",
            std::ffi::OsStr::new(
                "ABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCD",
            ),
        );
        assert_eq!(moss_runtime_url(&asset), custom_url.to_string_lossy());
        assert_eq!(expected_moss_runtime_checksum(&asset).await.unwrap().len(), 64);
        let source = moss_runtime_source_for_asset(asset.clone()).await.unwrap();
        assert_eq!(source.asset, asset);
        assert_eq!(source.url, custom_url.to_string_lossy());
        assert_eq!(source.sha256.len(), 64);

        drop(_sha_guard);
        assert!(expected_moss_runtime_checksum(&asset)
            .await
            .unwrap_err()
            .contains("required"));
        drop(_url_guard);
        let default_url = moss_runtime_url(&asset);
        assert!(default_url.contains(
            "github.com/bifrost-proxy/bifrost/releases/download/moss-runtime-v1.0.0"
        ));
        let checksum = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let manifest_url = spawn_moss_http_server(format!("{checksum}  {asset}\n").into_bytes());
        let _checksum_url_guard = EnvVarGuard::set(
            "BIFROST_MOSS_RUNTIME_CHECKSUM_URL",
            std::ffi::OsStr::new(&manifest_url),
        );
        assert_eq!(
            expected_moss_runtime_checksum(&asset).await.unwrap(),
            checksum
        );
        drop(_checksum_url_guard);

        let custom_model = std::ffi::OsStr::new("file:///tmp/model.safetensors");
        let _model_guard = EnvVarGuard::set("BIFROST_MOSS_MODEL_URL", custom_model);
        let spec = moss_model_spec();
        assert_eq!(spec.url, custom_model.to_string_lossy());
        assert_eq!(spec.bytes, MOSS_MODEL_BYTES);
        assert_eq!(spec.sha256, MOSS_MODEL_SHA256);
        drop(_model_guard);
        assert_eq!(moss_model_url(), MOSS_MODEL_URL);
    }

    #[tokio::test]
    async fn moss_checksum_and_resource_download_support_http_and_file_urls() {
        let temp = TempDir::new().unwrap();
        let asset = "moss-runtime.zip";
        let checksum = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let manifest = format!("{checksum}  dist/{asset}\n").into_bytes();
        let manifest_url = spawn_moss_http_server(manifest);
        assert_eq!(
            download_runtime_checksum(manifest_url, asset).await.unwrap(),
            checksum
        );

        let source = temp.path().join("source.bin");
        let file_dest = temp.path().join("file-dest.bin");
        std::fs::write(&source, b"file-resource").unwrap();
        download_moss_resource(
            temp.path(),
            format!("file://{}", source.display()),
            file_dest.clone(),
            "test file",
            None,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(file_dest).unwrap(), b"file-resource");

        let http_dest = temp.path().join("http-dest.bin");
        let resource_url = spawn_moss_http_server(b"http-resource".to_vec());
        download_moss_resource(temp.path(), resource_url, http_dest.clone(), "test HTTP", None)
            .await
            .unwrap();
        assert_eq!(std::fs::read(http_dest).unwrap(), b"http-resource");
        assert!(download_moss_resource(
            temp.path(),
            "file:///missing/moss-resource".to_string(),
            temp.path().join("missing.bin"),
            "missing",
            None,
        )
        .await
        .is_err());
    }

    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    #[tokio::test]
    async fn moss_initializer_reports_unsupported_platform_before_download() {
        let temp = TempDir::new().unwrap();
        let paths = moss_runtime_paths(temp.path());
        let status = moss_runtime_status(temp.path(), &paths, &moss_model_spec()).await;
        assert!(!status.runtime_valid);
        assert!(!status.model_valid());
        assert!(ensure_moss_joint_runtime(temp.path(), "unsupported-platform")
            .await
            .unwrap_err()
            .contains("only on Apple Silicon macOS"));
    }

    #[test]
    fn moss_hash_model_and_archive_validation_cover_success_and_failures() {
        let temp = TempDir::new().unwrap();
        let model = temp.path().join("model.safetensors");
        let spec = moss_test_model_spec(&model, b"tiny verified model");
        assert!(verify_moss_model(&model, &spec).is_ok());

        let mut wrong_size = spec.clone();
        wrong_size.bytes += 1;
        assert!(verify_moss_model(&model, &wrong_size)
            .unwrap_err()
            .contains("size mismatch"));
        let mut wrong_hash = spec.clone();
        wrong_hash.sha256 = "0".repeat(64);
        assert!(verify_moss_model(&model, &wrong_hash)
            .unwrap_err()
            .contains("checksum mismatch"));
        assert!(sha256_file(&temp.path().join("missing")).is_err());

        let snapshot_paths = moss_runtime_paths(&temp.path().join("snapshot"));
        std::fs::create_dir_all(&snapshot_paths.model_dir).unwrap();
        let snapshot_spec = moss_test_model_spec(&snapshot_paths.model, b"snapshot model");
        for required in &MOSS_MODEL_REQUIRED_FILES[..MOSS_MODEL_REQUIRED_FILES.len() - 1] {
            std::fs::write(snapshot_paths.model_dir.join(required), b"{}").unwrap();
        }
        assert!(verify_moss_model_snapshot(&snapshot_paths, &snapshot_spec)
            .unwrap_err()
            .contains("snapshot is missing"));

        let archive = temp.path().join("runtime.zip");
        write_moss_runtime_zip(&archive, "#!/bin/sh\necho 'moss-mlx-runtime ok'\n");
        let destination = temp.path().join("installed");
        #[cfg(unix)]
        {
            install_moss_runtime_archive(&archive, &destination).unwrap();
            install_moss_runtime_archive(&archive, &destination).unwrap();
            assert!(destination
                .join("runtime/python/bin/python3.12")
                .is_file());
            assert!(std::fs::symlink_metadata(
                destination.join("runtime/python/bin/python3.12")
            )
            .unwrap()
            .file_type()
            .is_symlink());
            assert_eq!(
                std::fs::read_link(destination.join("runtime/python/bin/python3.12")).unwrap(),
                PathBuf::from("python3.12-real")
            );
            assert!(destination.join("runtime/moss_mlx_runner.py").is_file());
            assert!(destination.join("model/config.json").is_file());
            assert!(destination.join("runtime/empty").is_dir());
            assert!(!destination.join("LICENSE").exists());
            assert!(!destination
                .join("runtime/site-packages/._invalid.py")
                .exists());
            assert!(!destination.join("model/._config.json").exists());
            assert!(!destination.join("model/.DS_Store").exists());
        }
        #[cfg(not(unix))]
        {
            assert!(install_moss_runtime_archive(&archive, &destination)
                .unwrap_err()
                .contains("unsupported on this platform"));
            std::fs::create_dir_all(&destination).unwrap();
        }

        let blocked_destination = temp.path().join("blocked-install-destination");
        std::fs::write(&blocked_destination, b"not a directory").unwrap();
        assert!(install_moss_runtime_archive(&archive, &blocked_destination)
            .unwrap_err()
            .contains("create MOSS runtime directory"));
        let parent_error_archive = temp.path().join("parent-error-runtime.zip");
        {
            use std::io::Write;
            let file = std::fs::File::create(&parent_error_archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file(
                "moss-joint-runtime/runtime/python/bin/python3.12",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"runtime").unwrap();
            zip.finish().unwrap();
        }
        assert!(install_moss_runtime_archive(&parent_error_archive, &blocked_destination)
            .unwrap_err()
            .contains("create MOSS runtime directory"));

        let missing_archive = temp.path().join("missing-runtime.zip");
        {
            use std::io::Write;
            let file = std::fs::File::create(&missing_archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("README", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"no binary").unwrap();
            zip.finish().unwrap();
        }
        assert!(install_moss_runtime_archive(&missing_archive, &destination)
            .unwrap_err()
            .contains("does not contain"));
        let unsafe_archive = temp.path().join("unsafe-runtime.zip");
        {
            use std::io::Write;
            let file = std::fs::File::create(&unsafe_archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file(
                "moss-joint-runtime/runtime/../../../escape",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"escape").unwrap();
            zip.finish().unwrap();
        }
        assert!(install_moss_runtime_archive(&unsafe_archive, &destination)
            .unwrap_err()
            .contains("unsafe MOSS runtime archive entry"));
        let unsafe_symlink_archive = temp.path().join("unsafe-symlink-runtime.zip");
        {
            let file = std::fs::File::create(&unsafe_symlink_archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.add_symlink(
                "moss-joint-runtime/runtime/python/escape",
                "../../../escape",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.finish().unwrap();
        }
        assert!(install_moss_runtime_archive(&unsafe_symlink_archive, &destination)
            .unwrap_err()
            .contains("unsafe escaping MOSS runtime symlink"));
        assert!(validate_moss_runtime_symlink(
            Path::new("runtime/python/bin/python3.12"),
            Path::new("./python3.12-real")
        )
        .is_ok());
        let absolute_symlink_target = if cfg!(windows) {
            Path::new(r"C:\tmp\python3.12")
        } else {
            Path::new("/tmp/python3.12")
        };
        assert!(validate_moss_runtime_symlink(
            Path::new("runtime/python/bin/python3.12"),
            absolute_symlink_target
        )
        .unwrap_err()
        .contains("unsafe absolute"));
        assert!(validate_moss_runtime_symlink(
            Path::new("runtime/escape"),
            Path::new("../../escape")
        )
        .unwrap_err()
        .contains("unsafe escaping"));
        assert!(validate_moss_runtime_symlink(
            Path::new("runtime/escape"),
            Path::new("../escape")
        )
        .unwrap_err()
        .contains("unsafe escaping"));

        let directory_conflict = temp.path().join("symlink-directory-conflict");
        std::fs::create_dir_all(
            directory_conflict.join("runtime/python/bin/python3.12"),
        )
        .unwrap();
        let directory_conflict_error =
            install_moss_runtime_archive(&archive, &directory_conflict).unwrap_err();
        #[cfg(unix)]
        assert!(directory_conflict_error.contains("refuse to replace MOSS runtime directory"));
        #[cfg(not(unix))]
        assert!(directory_conflict_error.contains("unsupported on this platform"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let write_symlink_archive = |path: &Path, entry: String| {
                let file = std::fs::File::create(path).unwrap();
                let mut zip = zip::ZipWriter::new(file);
                zip.add_symlink(
                    entry,
                    "target",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                zip.finish().unwrap();
            };

            let parent_error_archive = temp.path().join("symlink-parent-error.zip");
            write_symlink_archive(
                &parent_error_archive,
                "moss-joint-runtime/runtime/blocked/link".to_string(),
            );
            let parent_error_destination = temp.path().join("symlink-parent-error");
            std::fs::create_dir_all(parent_error_destination.join("runtime")).unwrap();
            std::fs::write(parent_error_destination.join("runtime/blocked"), b"file").unwrap();
            assert!(install_moss_runtime_archive(
                &parent_error_archive,
                &parent_error_destination,
            )
            .unwrap_err()
            .contains("create MOSS runtime directory"));

            let metadata_error_archive = temp.path().join("symlink-metadata-error.zip");
            write_symlink_archive(
                &metadata_error_archive,
                format!("moss-joint-runtime/runtime/{}", "x".repeat(300)),
            );
            let metadata_error_destination = temp.path().join("symlink-metadata-error");
            std::fs::create_dir_all(metadata_error_destination.join("runtime")).unwrap();
            assert!(install_moss_runtime_archive(
                &metadata_error_archive,
                &metadata_error_destination,
            )
            .unwrap_err()
            .contains("inspect MOSS runtime symlink target"));

            let invalid_utf8_archive = temp.path().join("symlink-invalid-utf8.zip");
            write_symlink_archive(
                &invalid_utf8_archive,
                "moss-joint-runtime/runtime/invalid-utf8".to_string(),
            );
            let mut archive_bytes = std::fs::read(&invalid_utf8_archive).unwrap();
            let target_offset = archive_bytes
                .windows(b"target".len())
                .position(|window| window == b"target")
                .expect("symlink payload");
            archive_bytes[target_offset] = 0xff;
            std::fs::write(&invalid_utf8_archive, archive_bytes).unwrap();
            assert!(install_moss_runtime_archive(
                &invalid_utf8_archive,
                &temp.path().join("symlink-invalid-utf8"),
            )
            .unwrap_err()
            .contains("read MOSS runtime symlink"));

            for (case, existing, expected) in [
                ("remove", true, "replace MOSS runtime symlink"),
                ("create", false, "create MOSS runtime symlink"),
            ] {
                let archive = temp.path().join(format!("symlink-{case}-error.zip"));
                write_symlink_archive(
                    &archive,
                    format!("moss-joint-runtime/runtime/readonly/{case}"),
                );
                let destination = temp.path().join(format!("symlink-{case}-error"));
                let readonly = destination.join("runtime/readonly");
                std::fs::create_dir_all(&readonly).unwrap();
                if existing {
                    std::fs::write(readonly.join(case), b"existing").unwrap();
                }
                std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o555))
                    .unwrap();
                let error = install_moss_runtime_archive(&archive, &destination).unwrap_err();
                std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
                assert!(error.contains(expected), "unexpected error: {error}");
            }
        }
        let invalid_archive = temp.path().join("invalid.zip");
        std::fs::write(&invalid_archive, b"not a zip").unwrap();
        assert!(install_moss_runtime_archive(&invalid_archive, &destination).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn moss_quarantine_cleanup_removes_only_managed_timestamped_entries() {
        use std::os::unix::fs::symlink;
        use std::os::unix::fs::PermissionsExt;

        assert!(!moss_quarantine_timestamp_is_valid(""));
        assert!(!moss_root_quarantine_name_is_managed(
            "moss-joint-runtime-v1.0.0+local-aarch64-apple-darwin.zip.invalid-1"
        ));

        let temp = TempDir::new().unwrap();
        let asr_home = temp.path().join("asr-home");
        let root = moss_runtime_dir(&asr_home);
        let model_dir = root.join("model");
        std::fs::create_dir_all(&model_dir).unwrap();

        let obsolete_runtime = root.join("runtime.invalid-100");
        std::fs::create_dir_all(&obsolete_runtime).unwrap();
        std::fs::write(obsolete_runtime.join("old-runtime"), b"old").unwrap();
        let obsolete_archive =
            root.join("moss-joint-runtime-v0.9.0-aarch64-apple-darwin.zip.invalid-101");
        std::fs::write(&obsolete_archive, b"bad archive").unwrap();
        let obsolete_model = model_dir.join("model.safetensors.invalid-102");
        std::fs::write(&obsolete_model, b"old model").unwrap();
        let external = temp.path().join("outside-quarantine");
        std::fs::create_dir_all(&external).unwrap();
        std::fs::write(external.join("keep"), b"keep").unwrap();
        let quarantined_symlink = root.join("runtime.invalid-103");
        symlink(&external, &quarantined_symlink).unwrap();

        let unmanaged = [
            root.join("runtime.invalid-latest"),
            root.join("moss-joint-runtime-custom.zip.invalid-104"),
            root.join("unrelated.invalid-105"),
            model_dir.join("model.safetensors.invalid-latest"),
        ];
        for path in &unmanaged {
            std::fs::write(path, b"keep").unwrap();
        }
        cleanup_moss_quarantined_resources(&asr_home).await;

        assert!(!obsolete_runtime.exists());
        assert!(!obsolete_archive.exists());
        assert!(!obsolete_model.exists());
        assert!(!quarantined_symlink.exists());
        assert!(external.join("keep").is_file());
        assert!(unmanaged.iter().all(|path| path.is_file()));

        let linked_home = temp.path().join("linked-home");
        let linked_root = moss_runtime_dir(&linked_home);
        std::fs::create_dir_all(&linked_root).unwrap();
        let external_model = temp.path().join("external-model");
        std::fs::create_dir_all(&external_model).unwrap();
        let external_quarantine = external_model.join("model.safetensors.invalid-200");
        std::fs::write(&external_quarantine, b"outside").unwrap();
        symlink(&external_model, linked_root.join("model")).unwrap();
        let linked_cleanup = cleanup_moss_quarantined_resources_sync(&linked_home);
        assert_eq!(linked_cleanup.removed, 0);
        assert_eq!(linked_cleanup.errors.len(), 1);
        assert!(linked_cleanup.errors[0].contains("refuse to scan non-directory"));
        assert!(external_quarantine.is_file());
        cleanup_moss_quarantined_resources(&linked_home).await;
        assert!(external_quarantine.is_file());

        let readonly_home = temp.path().join("readonly-home");
        let readonly_root = moss_runtime_dir(&readonly_home);
        std::fs::create_dir_all(&readonly_root).unwrap();
        let retry_quarantine = readonly_root.join("runtime.invalid-300");
        std::fs::write(&retry_quarantine, b"retry").unwrap();
        std::fs::set_permissions(&readonly_root, std::fs::Permissions::from_mode(0o555)).unwrap();
        let failed_cleanup = cleanup_moss_quarantined_resources_sync(&readonly_home);
        std::fs::set_permissions(&readonly_root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(failed_cleanup.removed, 0);
        assert_eq!(failed_cleanup.errors.len(), 1);
        assert!(failed_cleanup.errors[0].contains("remove obsolete MOSS quarantine"));
        assert!(retry_quarantine.is_file());
        let retried_cleanup = cleanup_moss_quarantined_resources_sync(&readonly_home);
        assert_eq!(retried_cleanup.removed, 1);
        assert!(retried_cleanup.errors.is_empty());
        assert!(!retry_quarantine.exists());

        let unreadable_home = temp.path().join("unreadable-home");
        let unreadable_root = moss_runtime_dir(&unreadable_home);
        std::fs::create_dir_all(&unreadable_root).unwrap();
        std::fs::set_permissions(&unreadable_root, std::fs::Permissions::from_mode(0o000)).unwrap();
        let unreadable_cleanup = cleanup_moss_quarantined_resources_sync(&unreadable_home);
        std::fs::set_permissions(&unreadable_root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(unreadable_cleanup.removed, 0);
        assert!(unreadable_cleanup
            .errors
            .iter()
            .any(|error| error.contains("read MOSS quarantine directory")));

        let missing_cleanup =
            cleanup_moss_quarantined_resources_sync(&temp.path().join("missing-home"));
        assert_eq!(missing_cleanup.removed, 0);
        assert!(missing_cleanup.errors.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn moss_initializer_installs_quarantines_and_reuses_verified_resources() {
        let _lock = test_data_dir_lock();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path().join("data").as_path());
        let asr_home = temp.path().join("asr-home");
        let paths = moss_runtime_paths(&asr_home);
        std::fs::create_dir_all(moss_runtime_dir(&asr_home)).unwrap();
        write_executable(&paths.python, "#!/bin/sh\necho 'invalid runtime' >&2\nexit 1\n");
        std::fs::write(&paths.runner, b"fixture runner").unwrap();
        std::fs::create_dir_all(&paths.site_packages).unwrap();
        prepare_moss_model_snapshot(&paths);
        std::fs::write(&paths.model, b"invalid model").unwrap();

        let archive = temp.path().join("runtime-source.zip");
        write_moss_runtime_zip(
            &archive,
            "#!/bin/sh\necho 'moss-mlx-runtime ok'\n",
        );
        let model_source = temp.path().join("model-source.safetensors");
        let model_spec = moss_test_model_spec(&model_source, b"verified model fixture");
        let runtime_source = MossRuntimeSource {
            asset: "runtime.zip".to_string(),
            url: format!("file://{}", archive.display()),
            sha256: sha256_file(&archive).unwrap(),
        };

        let installed_paths = ensure_moss_joint_runtime_with_spec(
            &asr_home,
            "moss-init-test",
            model_spec.clone(),
            Some(runtime_source.clone()),
        )
        .await
        .unwrap();
        assert_eq!(installed_paths.python, paths.python);
        let status = moss_runtime_status(&asr_home, &paths, &model_spec).await;
        assert!(status.all_valid());
        assert!(!std::fs::read_dir(moss_runtime_dir(&asr_home))
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".invalid-")));
        assert!(!std::fs::read_dir(&paths.model_dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".invalid-")));

        ensure_moss_joint_runtime_with_spec(
            &asr_home,
            "moss-reuse-test",
            model_spec.clone(),
            None,
        )
        .await
        .unwrap();
        assert!(verify_moss_runtime_binary(&paths).await.is_ok());

        std::fs::write(paths.model_dir.join("config.json"), b"corrupted metadata").unwrap();
        let corrupted = moss_management_status_with_spec(
            &asr_home,
            "macos",
            "aarch64",
            &model_spec,
        )
        .await;
        assert!(!corrupted.model_ready);
        let corrupted_status = moss_runtime_status(&asr_home, &paths, &model_spec).await;
        assert!(corrupted_status.model_weight_valid);
        assert!(!corrupted_status.model_metadata_valid);
        std::fs::remove_file(&model_source).unwrap();
        ensure_moss_joint_runtime_with_spec(
            &asr_home,
            "moss-repair-corrupt-metadata",
            model_spec.clone(),
            Some(runtime_source.clone()),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(paths.model_dir.join("config.json")).unwrap(),
            b"{}"
        );

        std::fs::remove_file(paths.model_dir.join("tokenizer.json")).unwrap();
        ensure_moss_joint_runtime_with_spec(
            &asr_home,
            "moss-repair-missing-metadata",
            model_spec.clone(),
            Some(runtime_source.clone()),
        )
        .await
        .unwrap();
        assert!(paths.model_dir.join("tokenizer.json").is_file());
        let invalid_runtime = temp.path().join("invalid-runtime");
        write_executable(&invalid_runtime, "#!/bin/sh\necho 'not moss usage' >&2\n");
        let mut invalid_paths = paths.clone();
        invalid_paths.python = invalid_runtime;
        assert!(verify_moss_runtime_binary(&invalid_paths).await.is_err());
        invalid_paths.python = temp.path().join("missing");
        assert!(verify_moss_runtime_binary(&invalid_paths).await.is_err());
        let hanging_runtime = temp.path().join("hanging-runtime");
        write_executable(&hanging_runtime, "#!/bin/sh\nsleep 2\n");
        invalid_paths.python = hanging_runtime;
        assert!(
            verify_moss_runtime_binary_with_timeout(&invalid_paths, Duration::from_millis(100))
                .await
                .unwrap_err()
                .contains("smoke check timed out")
        );

        let bad_home = temp.path().join("bad-home");
        let bad_source = MossRuntimeSource {
            sha256: "0".repeat(64),
            ..runtime_source.clone()
        };
        let error = ensure_moss_joint_runtime_with_spec(
            &bad_home,
            "moss-bad-checksum-test",
            model_spec.clone(),
            Some(bad_source),
        )
        .await
        .unwrap_err();
        assert!(error.contains("checksum mismatch"));
        assert!(std::fs::read_dir(moss_runtime_dir(&bad_home))
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".invalid-")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn moss_initializer_reports_runtime_archive_and_model_quarantine_failures() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let asr_home = temp.path().join("asr-home");
        let root = moss_runtime_dir(&asr_home);
        let paths = moss_runtime_paths(&asr_home);
        std::fs::create_dir_all(&root).unwrap();
        write_executable(&paths.python, "#!/bin/sh\necho 'invalid runtime' >&2\nexit 1\n");
        std::fs::write(&paths.runner, b"fixture runner").unwrap();
        std::fs::create_dir_all(&paths.site_packages).unwrap();
        prepare_moss_model_snapshot(&paths);
        std::fs::write(&paths.model, b"invalid model").unwrap();
        let model_source = temp.path().join("model-source.safetensors");
        let model_spec = moss_test_model_spec(&model_source, b"verified model fixture");
        let runtime_source = MossRuntimeSource {
            asset: "runtime.zip".to_string(),
            url: "file:///unused/runtime.zip".to_string(),
            sha256: "0".repeat(64),
        };

        write_executable(
            &paths.python,
            "#!/bin/sh\necho 'moss-mlx-runtime ok'\n",
        );
        std::fs::copy(&model_source, &paths.model).unwrap();
        initialize_moss_joint_runtime(
            &asr_home,
            "moss-valid-resources",
            &paths,
            MossComponentStatus {
                runtime_valid: true,
                model_weight_valid: true,
                model_metadata_valid: true,
            },
            &runtime_source,
            &model_spec,
            None,
        )
        .await
        .unwrap();
        std::fs::write(&paths.python, b"invalid runtime").unwrap();
        std::fs::write(&paths.model, b"invalid model").unwrap();

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();
        let runtime_error = initialize_moss_joint_runtime(
            &asr_home,
            "moss-runtime-quarantine-error",
            &paths,
            MossComponentStatus {
                runtime_valid: false,
                model_weight_valid: true,
                model_metadata_valid: true,
            },
            &runtime_source,
            &model_spec,
            None,
        )
        .await
        .unwrap_err();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(runtime_error.contains("quarantine invalid MOSS runtime"));

        write_executable(
            &paths.python,
            "#!/bin/sh\necho 'moss-mlx-runtime ok'\n",
        );
        std::fs::set_permissions(&paths.model_dir, std::fs::Permissions::from_mode(0o555))
            .unwrap();
        let model_error = initialize_moss_joint_runtime(
            &asr_home,
            "moss-model-quarantine-error",
            &paths,
            MossComponentStatus {
                runtime_valid: true,
                model_weight_valid: false,
                model_metadata_valid: true,
            },
            &runtime_source,
            &model_spec,
            None,
        )
        .await
        .unwrap_err();
        std::fs::set_permissions(&paths.model_dir, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(model_error.contains("quarantine invalid MOSS model"));

        let blocked_model_home = temp.path().join("blocked-model-home");
        let blocked_paths = moss_runtime_paths(&blocked_model_home);
        write_executable(
            &blocked_paths.python,
            "#!/bin/sh\necho 'moss-mlx-runtime ok'\n",
        );
        std::fs::write(&blocked_paths.runner, b"fixture runner").unwrap();
        std::fs::create_dir_all(&blocked_paths.site_packages).unwrap();
        std::fs::write(&blocked_paths.model_dir, b"not a directory").unwrap();
        let blocked_model_error = initialize_moss_joint_runtime(
            &blocked_model_home,
            "moss-model-dir-error",
            &blocked_paths,
            MossComponentStatus {
                runtime_valid: true,
                model_weight_valid: false,
                model_metadata_valid: true,
            },
            &runtime_source,
            &model_spec,
            None,
        )
        .await
        .unwrap_err();
        assert!(blocked_model_error.contains("create MOSS model directory"));

        let archive_source = temp.path().join("large-bad-runtime.zip");
        std::fs::File::create(&archive_source)
            .unwrap()
            .set_len(64 * 1024 * 1024)
            .unwrap();
        let archive_runtime_source = MossRuntimeSource {
            asset: "runtime.zip".to_string(),
            url: format!("file://{}", archive_source.display()),
            sha256: "f".repeat(64),
        };
        let archive_path = root.join(&archive_runtime_source.asset);
        let permissions_root = root.clone();
        let permission_thread = std::thread::spawn(move || {
            while !archive_path.exists() {
                std::thread::yield_now();
            }
            std::fs::set_permissions(
                permissions_root,
                std::fs::Permissions::from_mode(0o555),
            )
            .unwrap();
        });
        let archive_error = initialize_moss_joint_runtime(
            &asr_home,
            "moss-archive-quarantine-error",
            &paths,
            MossComponentStatus {
                runtime_valid: false,
                model_weight_valid: true,
                model_metadata_valid: true,
            },
            &archive_runtime_source,
            &model_spec,
            None,
        )
        .await
        .unwrap_err();
        permission_thread.join().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(archive_error.contains("quarantine invalid MOSS runtime archive"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn moss_runtime_process_covers_prompt_success_failure_and_pause() {
        let temp = TempDir::new().unwrap();
        let binary = temp.path().join("moss-transcribe");
        let wav = temp.path().join("audio.wav");
        // Leave enough watchdog headroom for process startup while the full workspace test suite
        // is running thousands of tests in parallel. Dedicated watchdog coverage uses a slow
        // fixture and a deliberately short duration below.
        let process_fixture_audio_ms = 10_000;
        std::fs::write(&wav, b"wav").unwrap();
        let runtime = moss_process_test_paths(temp.path(), binary.clone());

        write_executable(
            &binary,
            "#!/bin/sh\nprompt=''\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = '--prompt-file' ]; then shift; prompt=$(cat \"$1\"); fi\n  shift\ndone\n[ \"$prompt\" = 'Bifrost prompt' ] || { echo 'prompt mismatch' >&2; exit 9; }\nprintf '[{\"start\":0.1,\"end\":1.2,\"speaker\":\"S01\",\"text\":\" hello \"}]'\n",
        );
        let result = run_moss_joint_transcription(
            &runtime,
            &wav,
            process_fixture_audio_ms,
            "Bifrost prompt",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.text, "hello");

        write_executable(&binary, "#!/bin/sh\necho 'runtime failed' >&2\nexit 7\n");
        assert!(
            run_moss_joint_transcription(&runtime, &wav, process_fixture_audio_ms, "", None, None)
                .await
                .unwrap_err()
                .contains("runtime failed")
        );

        write_executable(
            &binary,
            "#!/bin/sh\necho 'MOSS output has no complete speaker-aware segment before 256 generated tokens' >&2\nexit 7\n",
        );
        let deterministic_process_error = run_moss_joint_transcription(
            &runtime,
            &wav,
            process_fixture_audio_ms,
            "",
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(deterministic_process_error.starts_with(&format!(
            "moss_non_retryable_v{}:",
            env!("CARGO_PKG_VERSION")
        )));

        write_executable(
            &binary,
            "#!/bin/sh\necho 'MOSS MLX returned no valid speaker-aware segments' >&2\nexit 7\n",
        );
        let no_valid_segments_error = run_moss_joint_transcription(
            &runtime,
            &wav,
            process_fixture_audio_ms,
            "",
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(no_valid_segments_error.starts_with(&format!(
            "moss_non_retryable_v{}:",
            env!("CARGO_PKG_VERSION")
        )));

        write_executable(&binary, "#!/bin/sh\nprintf '[]'\n");
        let deterministic_parse_error = run_moss_joint_transcription(
            &runtime,
            &wav,
            process_fixture_audio_ms,
            "",
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(deterministic_parse_error.starts_with(&format!(
            "moss_non_retryable_v{}:",
            env!("CARGO_PKG_VERSION")
        )));

        write_executable(&binary, "#!/bin/sh\nprintf '{'\n");
        let malformed_json_error = run_moss_joint_transcription(
            &runtime,
            &wav,
            process_fixture_audio_ms,
            "",
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(malformed_json_error.starts_with("parse MOSS runtime JSON:"));
        assert!(!malformed_json_error.starts_with("moss_non_retryable_"));

        write_executable(&binary, "#!/bin/sh\nsleep 5\n");
        let pause = || true;
        assert_eq!(
            run_moss_joint_transcription(&runtime, &wav, 1_000, "", Some(&pause), None)
                .await
                .unwrap_err(),
            ASR_TASK_PAUSED_MESSAGE
        );
        write_executable(
            &binary,
            "#!/bin/sh\nsleep 1\nprintf '[{\"start\":0.0,\"end\":0.5,\"speaker\":\"S01\",\"text\":\"resumed\"}]'\n",
        );
        let keep_running = || false;
        assert_eq!(
            run_moss_joint_transcription(
                &runtime,
                &wav,
                3_000,
                "",
                Some(&keep_running),
                None,
            )
            .await
            .unwrap()
            .text,
            "resumed"
        );
        write_executable(&binary, "#!/bin/sh\nsleep 2\n");
        let watchdog_started = std::time::Instant::now();
        assert!(run_moss_joint_transcription(&runtime, &wav, 1_200, "", None, None)
            .await
            .unwrap_err()
            .contains("moss_rtf_exceeded"));
        let watchdog_elapsed = watchdog_started.elapsed();
        assert!(watchdog_elapsed >= std::time::Duration::from_millis(500));
        assert!(watchdog_elapsed < std::time::Duration::from_millis(900));
        assert!(run_moss_joint_transcription(
            &runtime,
            &wav,
            1_000,
            &"x".repeat(MOSS_MAX_PROMPT_CHARS + 1),
            None,
            None,
        )
        .await
        .unwrap_err()
        .contains("prompt exceeds"));

        let mut missing_runtime = runtime.clone();
        missing_runtime.python = temp.path().join("missing-runtime");
        assert!(run_moss_joint_transcription(&missing_runtime, &wav, 1_000, "", None, None)
            .await
            .unwrap_err()
            .contains("start MOSS"));

        let marker = temp.path().join("unexpected-spawn");
        write_executable(
            &binary,
            &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        );
        let exhausted_started_at = now_ms().saturating_sub(600);
        assert!(run_moss_joint_transcription(
            &runtime,
            &wav,
            1_000,
            "",
            None,
            Some(exhausted_started_at),
        )
        .await
        .unwrap_err()
        .contains("before inference"));
        assert!(!marker.exists(), "exhausted budget must not spawn MOSS");
    }

    #[test]
    fn moss_input_guard_rejects_unknown_short_silent_and_invalid_audio() {
        let temp = TempDir::new().unwrap();
        let speech = temp.path().join("speech.wav");
        let silence = temp.path().join("silence.wav");
        let invalid = temp.path().join("invalid.wav");
        std::fs::write(&speech, make_wav(&[500i16; 100])).unwrap();
        std::fs::write(&silence, make_wav(&[0i16; 100])).unwrap();
        std::fs::write(&invalid, b"not wav").unwrap();

        assert!(validate_moss_audio_input(&speech, 0)
            .unwrap_err()
            .contains("moss_duration_unavailable"));
        assert!(validate_moss_audio_input(&speech, MOSS_MIN_AUDIO_DURATION_MS - 1)
            .unwrap_err()
            .contains("moss_audio_too_short"));
        assert!(validate_moss_audio_input(&silence, MOSS_MIN_AUDIO_DURATION_MS)
            .unwrap_err()
            .contains("moss_audio_silent"));
        assert!(validate_moss_audio_input(&invalid, MOSS_MIN_AUDIO_DURATION_MS)
            .unwrap_err()
            .contains("moss_audio_invalid"));
        validate_moss_audio_input(&speech, MOSS_MIN_AUDIO_DURATION_MS).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn moss_task_transcription_writes_native_speaker_timeline_and_safe_metadata() {
        let _lock = test_data_dir_lock();
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let source = audio_dir.join("meeting.wav");
        let source_wav = make_wav(&[500i16; 100]);
        std::fs::write(&source, &source_wav).unwrap();

        let binary = temp.path().join("moss-transcribe");
        write_executable(
            &binary,
            "#!/bin/sh\nprintf '[{\"start\":0.1,\"end\":1.2,\"speaker\":\" S01 \",\"text\":\"hello\"},{\"start\":1.2,\"end\":65.0,\"speaker\":\"S02\",\"text\":\"abcdefghijklmnopqrstuvwxyz\"},{\"start\":2.5,\"end\":2.0,\"speaker\":\"S03\",\"text\":\"invalid range\"}]'\n",
        );
        let runtime = moss_process_test_paths(temp.path(), binary);
        let mut task = test_directory_task("moss-artifacts", audio_dir.clone());
        task.transcription_mode = AsrTranscriptionMode::MossJoint;
        task.transcription_prompt = "private prompt".to_string();
        task.diarization.enabled = true;
        let source_info = SourceAudioInfo {
            source_size: Some(source_wav.len() as u64),
            source_modified_ms: Some(9_000),
            source_created_at_ms: Some(10_000),
            source_created_at_source: Some("fixture".to_string()),
            media_duration_ms: Some(65_000),
        };
        let observed_metrics = StdMutex::new(Vec::new());
        let metric_callback = |metric| observed_metrics.lock().unwrap().push(metric);
        let hooks = TaskTranscribeHooks {
            on_chunk_progress: None,
            on_chunk_metric: Some(&metric_callback),
            pause_check: None,
            force_pause_task_id: None,
            memory_limit_hints: &[],
            server_url: None,
            startup_fallback_reason: None,
            server_state: None,
            managed_server_restart: None,
            partial_artifacts: None,
            moss_runtime: Some(&runtime),
        };

        let output = transcribe_file_for_task_with_wav(
            &task,
            Path::new("/unused/asr"),
            Path::new("/unused/model"),
            &source,
            &source,
            &source_info,
            hooks,
        )
        .await
        .unwrap();
        assert_eq!(output.text, "hello\nabcdefghijklmnopqrstuvwxyz");
        assert_eq!(output.timeline.model, "MOSS-Transcribe-Diarize-MLX-8bit");
        assert_eq!(
            output.timeline.diarization_profile.as_deref(),
            Some("moss_joint_native")
        );
        assert_eq!(output.timeline.segments.len(), 4);
        assert!(output.timeline.segments.iter().all(|segment| {
            segment.audio_end_ms.saturating_sub(segment.audio_start_ms)
                <= ASR_TASK_SEGMENT_MAX_MS
        }));
        assert_eq!(output.timeline.segments[1].speaker.as_deref(), Some("S02"));
        assert_eq!(output.timeline.segments[2].speaker.as_deref(), Some("S02"));
        assert_eq!(output.timeline.segments[3].speaker.as_deref(), Some("S02"));
        assert_eq!(output.chunk_metrics.len(), 1);
        assert_eq!(output.chunk_metrics[0].runner, "moss_joint");
        assert_eq!(observed_metrics.lock().unwrap().len(), 1);
        assert_eq!(output.chunk_metrics[0].status, "ok");
        assert_eq!(output.chunk_metrics[0].duration_secs, 65);
        assert!(output.chunk_metrics[0].elapsed_ms >= 1);
        assert_eq!(output.chunk_metrics[0].text_chars, 32);
        assert_eq!(
            output.chunk_metrics[0].text_sha1,
            sha1_hex(b"hello\nabcdefghijklmnopqrstuvwxyz")
        );
        assert_eq!(output.timeline.segments[0].absolute_start_ms, Some(10_100));
        assert_eq!(output.timeline.segments[3].absolute_end_ms, Some(75_000));
        assert_eq!(
            output
                .timeline
                .segments
                .iter()
                .skip(1)
                .map(|segment| segment.text.as_str())
                .collect::<String>(),
            "abcdefghijklmnopqrstuvwxyz"
        );
        assert_eq!(
            output
                .timeline
                .speakers
                .iter()
                .map(|speaker| speaker.id.as_str())
                .collect::<Vec<_>>(),
            vec!["S01", "S02"]
        );
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&output.metadata_path).unwrap()).unwrap();
        assert_eq!(metadata["model"], "MOSS-Transcribe-Diarize-MLX-8bit");
        assert_eq!(metadata["transcription_mode"], "moss_joint");
        assert_eq!(metadata["transcription_prompt_configured"], true);
        assert_eq!(metadata["chunk_metrics"][0]["runner"], "moss_joint");
        assert_eq!(metadata["chunk_metrics"][0]["status"], "ok");
        assert!(metadata["chunk_metrics"][0]["elapsed_ms"]
            .as_u64()
            .is_some_and(|elapsed_ms| elapsed_ms >= 1));
        assert!(!std::fs::read_to_string(&output.metadata_path)
            .unwrap()
            .contains("private prompt"));

        let default_runtime_result = transcribe_file_for_task_with_wav(
            &task,
            Path::new("/unused/asr"),
            Path::new("/unused/model"),
            &source,
            &source,
            &source_info,
            TaskTranscribeHooks {
                on_chunk_progress: None,
                on_chunk_metric: None,
                pause_check: None,
                force_pause_task_id: None,
                memory_limit_hints: &[],
                server_url: None,
                startup_fallback_reason: None,
                server_state: None,
                managed_server_restart: None,
                partial_artifacts: None,
                moss_runtime: None,
            },
        )
        .await;
        match default_runtime_result {
            Ok(output) => assert_eq!(output.chunk_metrics[0].runner, "moss_joint"),
            Err(error) => {
                assert!(error.contains("start MOSS MLX joint transcription runtime"))
            }
        }

        let pause = || true;
        let paused = match transcribe_file_for_task_with_wav(
            &task,
            Path::new("/unused/asr"),
            Path::new("/unused/model"),
            &source,
            &source,
            &source_info,
            TaskTranscribeHooks {
                on_chunk_progress: None,
                on_chunk_metric: None,
                pause_check: Some(&pause),
                force_pause_task_id: None,
                memory_limit_hints: &[],
                server_url: None,
                startup_fallback_reason: None,
                server_state: None,
                managed_server_restart: None,
                partial_artifacts: None,
                moss_runtime: Some(&runtime),
            },
        )
        .await
        {
            Ok(_) => panic!("paused MOSS task unexpectedly completed"),
            Err(error) => error,
        };
        assert_eq!(paused, ASR_TASK_PAUSED_MESSAGE);
    }

    #[test]
    fn moss_json_parser_filters_empty_segments_and_reports_invalid_json() {
        let error = parse_moss_json(
            br#"[
              {"start":-1.0,"end":-2.0,"speaker":"","text":" kept "},
              {"start":-2.0,"end":-1.0,"speaker":"S01","text":" clamps to zero "},
              {"start":1.0,"end":2.0,"speaker":"S02","text":"   "}
            ]"#,
            2_000,
        )
        .unwrap_err();
        assert!(error.contains("no positive-duration speaker-aware segments"));
        assert!(parse_moss_json(b"not-json", 2_000).is_err());
    }

    #[test]
    fn moss_json_parser_rejects_length_stop_and_bounds_segments_to_audio() {
        let length_error = parse_moss_json(
            br#"{"segments":[{"start":0.0,"end":1.0,"speaker":"S01","text":"partial"}],"finish_reason":"length"}"#,
            2_000,
        )
        .unwrap_err();
        assert!(length_error.contains("max-new token limit"));

        let result = parse_moss_json(
            br#"{"segments":[
              {"start":0.5,"end":3.0,"speaker":"S01","text":"clamped"},
              {"start":2.5,"end":3.0,"speaker":"S02","text":"outside"},
              {"start":1.5,"end":1.0,"speaker":"S03","text":"inverted"}
            ],"finish_reason":"completed"}"#,
            2_000,
        )
        .unwrap();
        assert_eq!(result.text, "clamped");
        assert_eq!(result.structured.segments.len(), 1);
        assert_eq!(result.structured.segments[0].end_ms, 2_000);
    }

    #[test]
    fn moss_platform_validation_rejects_unsupported_hosts_only_for_moss() {
        assert!(validate_moss_transcription_mode_for_platform(
            AsrTranscriptionMode::Standard,
            "linux",
            "x86_64"
        )
        .is_ok());
        assert!(validate_moss_transcription_mode_for_platform(
            AsrTranscriptionMode::MossJoint,
            "macos",
            "aarch64"
        )
        .is_ok());
        assert!(validate_moss_transcription_mode_for_platform(
            AsrTranscriptionMode::MossJoint,
            "linux",
            "x86_64"
        )
        .unwrap_err()
        .contains("only on Apple Silicon macOS"));
    }

    #[test]
    fn transcription_mode_helpers_select_runtime_diarization_progress_and_model() {
        let temp = TempDir::new().unwrap();
        let mut task = test_directory_task("mode-helpers", temp.path().to_path_buf());
        task.runtime_strategy = AsrRuntimeStrategy::ForkPerChunk;
        assert!(task_uses_standard_runtime(&task));
        assert!(!task_uses_external_diarization(&task));
        assert!(!task_uses_task_lifetime_server(&task));
        assert!(!task_uses_file_lifetime_server(&task));
        assert_eq!(task_initial_processing_stage(&task), ("asr", None));
        assert_eq!(task_external_diarization_profile(&task), None);
        assert_eq!(task_asr_stage_message(&task), "transcribing audio");
        assert_eq!(effective_task_model(&task), task.model);

        task.diarization.enabled = true;
        assert!(task_uses_external_diarization(&task));
        assert_eq!(
            task_initial_processing_stage(&task),
            (
                "normalize",
                Some(format!(
                    "speaker diarization profile: {}",
                    task.diarization.profile
                ))
            )
        );
        assert_eq!(
            task_external_diarization_profile(&task),
            Some(task.diarization.profile.clone())
        );
        assert_eq!(
            task_asr_stage_message(&task),
            "transcribing diarized audio segments"
        );

        task.runtime_strategy = AsrRuntimeStrategy::ReusePerFile;
        assert!(task_uses_file_lifetime_server(&task));
        task.runtime_strategy = AsrRuntimeStrategy::ReuseServer;
        assert!(task_uses_task_lifetime_server(&task));

        task.transcription_mode = AsrTranscriptionMode::MossJoint;
        assert!(!task_uses_standard_runtime(&task));
        assert!(!task_uses_external_diarization(&task));
        assert!(!task_uses_task_lifetime_server(&task));
        assert!(!task_uses_file_lifetime_server(&task));
        assert_eq!(task_initial_processing_stage(&task), ("asr", None));
        assert_eq!(task_external_diarization_profile(&task), None);
        assert_eq!(
            task_asr_stage_message(&task),
            "jointly transcribing audio with native speaker labels"
        );
        assert_eq!(
            effective_task_model(&task),
            "MOSS-Transcribe-Diarize-MLX-8bit"
        );
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

        task.transcription_mode = AsrTranscriptionMode::MossJoint;
        assert_eq!(effective_max_concurrent_files(&task), 1);
    }

    #[test]
    fn moss_summary_reports_native_diarization_ready_without_external_assets() {
        let temp = tempfile::tempdir().unwrap();
        let mut task = test_directory_task("moss-diarization-summary", temp.path().join("audio"));
        task.diarization.enabled = true;
        task.diarization.profile = "missing-external-profile".to_string();
        let files = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };

        task.transcription_mode = AsrTranscriptionMode::Standard;
        assert!(!summarize_diarization(&task, &files).0);

        task.transcription_mode = AsrTranscriptionMode::MossJoint;
        assert!(summarize_diarization(&task, &files).0);
    }

    #[test]
    fn running_task_allows_concurrency_update_but_rejects_runtime_risk() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let source = audio_dir.join("completed.wav");
        std::fs::write(&source, make_wav(&[500i16; 100])).unwrap();
        let task = test_directory_task("running-concurrency-task", audio_dir);
        add_task(task.clone()).unwrap();
        let mut completed = pending_record(&task.id, &source);
        completed.status = FileStatus::Success;
        completed.output_text_path = Some(temp.path().join("old.txt"));
        completed.output_metadata_path = Some(temp.path().join("old.json"));
        completed.output_timeline_path = Some(temp.path().join("old.timeline.json"));
        completed.text_chars = 8;
        completed.chunk_metrics.push(AsrChunkMetric {
            chunk_index: 0,
            offset_secs: 0,
            duration_secs: 1,
            runner: "fork_per_chunk".to_string(),
            status: "ok".to_string(),
            elapsed_ms: 1,
            rtf: 0.001,
            text_chars: 8,
            text_sha1: sha1_hex(b"old text"),
            server_url: None,
            fallback_reason: None,
            error: None,
            recorded_at_ms: now_ms(),
        });
        let completed_key = "success".to_string();
        let files = [
            (completed_key.clone(), FileStatus::Success),
            ("partial".to_string(), FileStatus::PartialSuccess),
            ("failed".to_string(), FileStatus::Failed),
            ("processing".to_string(), FileStatus::Processing),
            ("pending".to_string(), FileStatus::Pending),
        ]
        .into_iter()
        .map(|(key, status)| {
            let mut record = completed.clone();
            record.status = status;
            (key, record)
        })
        .collect();
        save_file_store(
            &task.id,
            &FileStore {
                version: TASK_STORE_VERSION,
                files,
            },
        )
        .unwrap();
        RUNNING_TASKS.lock().unwrap().insert(task.id.clone());

        let update = UpdateTaskRequest {
            transcription_mode: None,
            transcription_prompt: None,
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
            transcription_mode: None,
            transcription_prompt: None,
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

        let prompt_update = UpdateTaskRequest {
            transcription_mode: Some(AsrTranscriptionMode::MossJoint),
            transcription_prompt: Some("  Bifrost 专有词  ".to_string()),
            name: None,
            audio_dir: None,
            recursive: None,
            enabled: None,
            paused: None,
            schedule: None,
            language: None,
            model: None,
            runtime_strategy: None,
            max_concurrent_files: None,
            diarization: None,
            daily_agent: None,
            external_devices: None,
            import_policy: None,
        };
        let updated = update_task_config(&task.id, prompt_update).unwrap();
        assert_eq!(updated.transcription_mode, AsrTranscriptionMode::MossJoint);
        assert_eq!(updated.transcription_prompt, "Bifrost 专有词");
        let reloaded = find_task(&task.id).unwrap();
        assert_eq!(reloaded.transcription_prompt, "Bifrost 专有词");
        let requeued = load_file_store(&task.id);
        assert_eq!(requeued.files.len(), 5);
        for (key, record) in &requeued.files {
            assert_eq!(record.status, FileStatus::Pending);
            if key == "pending" {
                assert!(record.output_text_path.is_some());
                assert_eq!(record.text_chars, 8);
                continue;
            }
            assert!(record.output_text_path.is_none());
            assert!(record.output_metadata_path.is_none());
            assert!(record.output_timeline_path.is_none());
            assert!(record.chunk_metrics.is_empty());
            assert_eq!(record.text_chars, 0);
        }

        let mut completed_again = requeued.files.get(&completed_key).unwrap().clone();
        completed_again.status = FileStatus::Success;
        save_file_store(
            &task.id,
            &FileStore {
                version: TASK_STORE_VERSION,
                files: BTreeMap::from([(completed_key.clone(), completed_again)]),
            },
        )
        .unwrap();
        let idempotent_update = UpdateTaskRequest {
            transcription_mode: Some(AsrTranscriptionMode::MossJoint),
            transcription_prompt: Some("Bifrost 专有词".to_string()),
            name: None,
            audio_dir: None,
            recursive: None,
            enabled: None,
            paused: None,
            schedule: None,
            language: None,
            model: None,
            runtime_strategy: None,
            max_concurrent_files: None,
            diarization: None,
            daily_agent: None,
            external_devices: None,
            import_policy: None,
        };
        update_task_config(&task.id, idempotent_update).unwrap();
        assert_eq!(
            load_file_store(&task.id)
                .files
                .get(&completed_key)
                .unwrap()
                .status,
            FileStatus::Success
        );

        let prompt_only_update = UpdateTaskRequest {
            transcription_mode: None,
            transcription_prompt: Some("Bifrost 新专有词".to_string()),
            name: None,
            audio_dir: None,
            recursive: None,
            enabled: None,
            paused: None,
            schedule: None,
            language: None,
            model: None,
            runtime_strategy: None,
            max_concurrent_files: None,
            diarization: None,
            daily_agent: None,
            external_devices: None,
            import_policy: None,
        };
        update_task_config(&task.id, prompt_only_update).unwrap();
        assert_eq!(
            find_task(&task.id).unwrap().transcription_prompt,
            "Bifrost 新专有词"
        );
        assert_eq!(
            load_file_store(&task.id)
                .files
                .get(&completed_key)
                .unwrap()
                .status,
            FileStatus::Pending
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn create_task_http_endpoint_normalizes_prompt_and_persists_defaults() {
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper_util::rt::TokioIo;
        use tokio::net::TcpListener;

        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let service = service_fn(|req: Request<Incoming>| async move {
                Ok::<_, hyper::Error>(create_task_response(req).await)
            });
            http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
        });

        // This request targets the listener owned by this test. Ignore process-wide
        // proxy variables so parallel proxy-environment tests cannot redirect it.
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("http://{address}/api/asr/tasks"))
            .header(reqwest::header::CONNECTION, "close")
            .json(&serde_json::json!({
                "name": "HTTP coverage task",
                "audio_dir": audio_dir,
                "transcription_mode": "standard",
                "transcription_prompt": "  Bifrost\r\nAPI  "
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["name"], "HTTP coverage task");
        assert_eq!(body["transcription_mode"], "standard");
        assert_eq!(body["transcription_prompt"], "Bifrost\nAPI");
        assert_eq!(body["recursive"], true);
        assert_eq!(body["enabled"], true);
        assert!(audio_dir.is_dir());

        let task_id = body["id"].as_str().unwrap();
        let persisted = find_task(task_id).unwrap();
        assert_eq!(persisted.audio_dir, audio_dir);
        assert_eq!(persisted.transcription_prompt, "Bifrost\nAPI");
        server.abort();
        if let Err(error) = server.await {
            assert!(error.is_cancelled());
        }
    }

    #[test]
    fn diarization_cluster_count_is_fixed_only_when_known() {
        let mut config = AsrDiarizationConfig::default();
        assert_eq!(resolved_diarization_cluster_count(&config), -1);

        config.max_speakers = Some(3);
        assert_eq!(resolved_diarization_cluster_count(&config), -1);

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
            schema_version: 1,
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
            templates: Vec::new(),
            prototypes: Vec::new(),
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
            schema_version: 1,
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
            templates: Vec::new(),
            prototypes: Vec::new(),
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
            schema_version: 1,
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
            templates: Vec::new(),
            prototypes: Vec::new(),
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
            schema_version: 1,
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
            templates: Vec::new(),
            prototypes: Vec::new(),
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
            schema_version: 1,
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
            templates: Vec::new(),
            prototypes: Vec::new(),
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
            schema_version: 1,
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
            templates: Vec::new(),
            prototypes: Vec::new(),
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
            schema_version: 1,
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
            templates: Vec::new(),
            prototypes: Vec::new(),
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
    fn moss_pending_batch_skips_deterministic_failure_until_source_changes() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let source = audio_dir.join("sparse.wav");
        std::fs::write(&source, b"audio").unwrap();

        let mut task = test_directory_task("moss-terminal", audio_dir);
        task.transcription_mode = AsrTranscriptionMode::MossJoint;
        let attempted = HashSet::new();
        let mut files = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let initial = discover_and_prepare_pending_batch(&task, &mut files, &attempted).unwrap();
        assert_eq!(initial.pending, vec![source.clone()]);

        let key = source_key(&source);
        let record = files.files.get_mut(&key).unwrap();
        assert!(!moss_failure_is_non_retryable_for_unchanged_source(
            &task, &source, record,
        ));
        record.status = FileStatus::Failed;
        assert!(!moss_failure_is_non_retryable_for_unchanged_source(
            &task, &source, record,
        ));
        record.error = Some(format!(
            "moss_non_retryable_v{}: MOSS output has no complete speaker-aware segment before 256 generated tokens",
            env!("CARGO_PKG_VERSION")
        ));
        let unchanged = discover_and_prepare_pending_batch(&task, &mut files, &attempted).unwrap();
        assert!(unchanged.pending.is_empty());

        std::fs::write(&source, b"audio changed").unwrap();
        let changed = discover_and_prepare_pending_batch(&task, &mut files, &attempted).unwrap();
        assert_eq!(changed.pending, vec![source.clone()]);

        task.transcription_mode = AsrTranscriptionMode::Standard;
        let standard = discover_and_prepare_pending_batch(&task, &mut files, &attempted).unwrap();
        assert_eq!(standard.pending, vec![source]);
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
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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

    #[cfg(unix)]
    #[tokio::test]
    async fn test_server_chunk_and_bisect_empty_results_initialize_structured_view() {
        use std::os::unix::fs::PermissionsExt;

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

        let fake_bin = temp.path().join("fake-bin");
        std::fs::create_dir_all(&fake_bin).unwrap();
        let fake_ffmpeg = fake_bin.join("ffmpeg");
        std::fs::write(
            &fake_ffmpeg,
            "#!/bin/sh\ninput=''\noutput=''\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = '-i' ]; then\n    shift\n    input=\"$1\"\n  fi\n  output=\"$1\"\n  shift\ndone\ncp \"$input\" \"$output\"\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_ffmpeg).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_ffmpeg, permissions).unwrap();
        let mut path_entries = vec![fake_bin];
        if let Some(path) = std::env::var_os("PATH") {
            path_entries.extend(std::env::split_paths(&path));
        }
        let fake_path = std::env::join_paths(path_entries).unwrap();
        let _path_guard = EnvVarGuard::set("PATH", &fake_path);

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

        let speech_wav = temp.path().join("speech-four-seconds.wav");
        std::fs::write(&speech_wav, make_wav(&vec![500i16; 4 * 16_000])).unwrap();
        let fake_asr = temp.path().join("fake-asr");
        write_executable(
            &fake_asr,
            "#!/bin/sh\necho 'asr cli exceeded memory footprint limit' >&2\nexit 1\n",
        );
        let both_halves_failed = transcribe_single_chunk_with_bisect(
            &fake_asr,
            Path::new("/nonexistent/model"),
            "chinese",
            &speech_wav,
            0,
            4,
            3,
            temp.path(),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(both_halves_failed.contains("both halves produced no output"));

        write_executable(
            &fake_asr,
            "#!/bin/sh\ncase \"$2\" in *-d0-a.wav) printf '<asr_text>left half</asr_text>' ;; *) echo 'asr cli exceeded memory footprint limit' >&2; exit 1 ;; esac\n",
        );
        let one_half_recovered = transcribe_single_chunk_with_bisect(
            &fake_asr,
            Path::new("/nonexistent/model"),
            "chinese",
            &speech_wav,
            0,
            4,
            4,
            temp.path(),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(one_half_recovered.text, "left half");
        assert!(one_half_recovered.structured.segments.is_empty());
    }

    #[test]
    fn force_pause_requires_persisted_pause_state() {
        let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let audio_dir = temp.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let task = AsrDirectoryTask {
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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
    fn rms_energy_streams_across_internal_buffer_boundaries() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("long.wav");
        std::fs::write(&path, make_wav(&[750i16; 40_000])).unwrap();
        let rms = compute_wav_rms_energy(&path).unwrap();
        assert!((rms - 750.0).abs() < 0.01, "expected 750, got {rms}");
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
    fn rms_energy_rejects_truncated_data_chunk() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("truncated-data.wav");
        let mut wav = make_wav(&[100, -100, 200, -200]);
        wav.truncate(wav.len() - 2);
        std::fs::write(&path, wav).unwrap();
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
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
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

    fn assisted_test_timeline(segments: Vec<TimelineSegment>) -> TranscriptTimeline {
        TranscriptTimeline {
            task_id: "task-assisted".to_string(),
            task_name: "Assisted".to_string(),
            source_path: PathBuf::from("meeting.wav"),
            source_size: None,
            source_modified_ms: None,
            source_created_at_ms: None,
            source_created_at_source: None,
            media_duration_ms: Some(60_000),
            model: "test".to_string(),
            language: "chinese".to_string(),
            diarization_profile: Some(DEFAULT_DIARIZATION_PROFILE.to_string()),
            speakers: Vec::new(),
            processed_at_ms: 1,
            segments,
        }
    }

    fn assisted_test_segment(
        index: usize,
        speaker: Option<&str>,
        start_ms: u64,
        end_ms: u64,
        overlap: bool,
    ) -> TimelineSegment {
        TimelineSegment {
            index,
            audio_start_ms: start_ms,
            audio_end_ms: end_ms,
            absolute_start_ms: None,
            absolute_end_ms: None,
            speaker: speaker.map(str::to_string),
            speaker_display_name: None,
            overlap,
            text: format!("segment {index}"),
        }
    }

    fn assisted_test_candidate(index: usize) -> AssistedVoiceprintCandidate {
        AssistedVoiceprintCandidate {
            id: format!("candidate-{index}"),
            speaker: "speaker_00".to_string(),
            start_ms: index as u64 * 4_000,
            end_ms: index as u64 * 4_000 + 4_000,
            duration_ms: 4_000,
            text: format!("candidate {index}"),
            quality: 1.0,
            overlap: false,
            label: AssistedCandidateLabel::Mine,
        }
    }

    fn assisted_test_pcm() -> Vec<u8> {
        (0..VOICEPRINT_SAMPLE_RATE * 4)
            .flat_map(|index| {
                let sample = if index % 2 == 0 { 8_000i16 } else { -8_000i16 };
                sample.to_le_bytes()
            })
            .collect()
    }

    fn assisted_test_session(id: &str) -> AssistedVoiceprintSession {
        AssistedVoiceprintSession {
            id: id.to_string(),
            state: AssistedVoiceprintSessionState::Open,
            speaker_name: "Eden".to_string(),
            profile_id: None,
            task_id: "task-assisted".to_string(),
            file_key: "file-a".to_string(),
            source_path: PathBuf::from("meeting.wav"),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            candidates: (0..3).map(assisted_test_candidate).collect(),
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn assisted_test_template(id: &str, embedding: Vec<f32>) -> SpeakerVoiceprintTemplate {
        SpeakerVoiceprintTemplate {
            id: id.to_string(),
            source_kind: "test".to_string(),
            prompt_id: None,
            task_id: None,
            file_key: None,
            speaker: None,
            start_ms: None,
            end_ms: None,
            duration_ms: 4_000,
            quality: 1.0,
            overlap: false,
            embedding,
            created_at_ms: 1,
        }
    }

    #[test]
    fn assisted_candidates_exclude_overlap_short_and_anonymous_segments() {
        let timeline = assisted_test_timeline(vec![
            assisted_test_segment(0, Some("speaker_00"), 0, 2_999, false),
            assisted_test_segment(1, Some("speaker_00"), 3_000, 9_000, true),
            assisted_test_segment(2, None, 9_000, 15_000, false),
            assisted_test_segment(3, Some("speaker_00"), 15_000, 30_000, false),
        ]);

        let candidates = assisted_voiceprint_candidates(&timeline);

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].start_ms, 15_000);
        assert_eq!(candidates[0].end_ms, 27_000);
        assert_eq!(candidates[1].start_ms, 27_000);
        assert_eq!(candidates[1].end_ms, 30_000);
        assert!(candidates.iter().all(|candidate| !candidate.overlap));
    }

    #[test]
    fn assisted_candidates_cap_each_speaker_at_eight() {
        let segments = (0..10)
            .map(|index| {
                assisted_test_segment(
                    index,
                    Some("speaker_00"),
                    index as u64 * 4_000,
                    index as u64 * 4_000 + 4_000,
                    false,
                )
            })
            .collect();

        let candidates = assisted_voiceprint_candidates(&assisted_test_timeline(segments));

        assert_eq!(candidates.len(), ASSISTED_CANDIDATES_PER_SPEAKER);
    }

    #[test]
    fn assisted_candidates_drop_a_trailing_chunk_below_minimum_duration() {
        let timeline = assisted_test_timeline(vec![assisted_test_segment(
            0,
            Some("speaker_00"),
            0,
            ASSISTED_CANDIDATE_MAX_MS + ASSISTED_CANDIDATE_MIN_MS - 1,
            false,
        )]);

        let candidates = assisted_voiceprint_candidates(&timeline);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].duration_ms, ASSISTED_CANDIDATE_MAX_MS);
    }

    #[test]
    fn assisted_session_legacy_json_defaults_to_open() {
        let mut value = serde_json::to_value(assisted_test_session("assisted-legacy")).unwrap();
        value.as_object_mut().unwrap().remove("state");

        let session: AssistedVoiceprintSession = serde_json::from_value(value).unwrap();

        assert_eq!(session.state, AssistedVoiceprintSessionState::Open);
    }

    #[test]
    fn legacy_voiceprint_profile_reads_without_v2_fields() {
        let raw = r#"{
            "id":"spk-legacy","display_name":"Legacy","source":"live_enrollment",
            "diarization_profile":"sherpa-onnx-balanced","embedding_model":"test",
            "embedding_dim":2,"embedding":[1.0,0.0],"sample_rate":16000,
            "total_duration_ms":3000,"samples":[],"created_at_ms":1,"updated_at_ms":1
        }"#;

        let profile: SpeakerVoiceprintProfile = serde_json::from_str(raw).unwrap();

        assert_eq!(profile.schema_version, 1);
        assert!(profile.templates.is_empty());
        assert!(profile.prototypes.is_empty());
        assert_eq!(profile.embedding, vec![1.0, 0.0]);
    }

    #[test]
    fn voiceprint_prototypes_preserve_distinct_acoustic_domains() {
        let templates = vec![
            SpeakerVoiceprintTemplate {
                id: "near-1".to_string(),
                source_kind: "test".to_string(),
                prompt_id: None,
                task_id: None,
                file_key: None,
                speaker: None,
                start_ms: None,
                end_ms: None,
                duration_ms: 4_000,
                quality: 1.0,
                overlap: false,
                embedding: vec![1.0, 0.0],
                created_at_ms: 1,
            },
            SpeakerVoiceprintTemplate {
                id: "near-2".to_string(),
                source_kind: "test".to_string(),
                prompt_id: None,
                task_id: None,
                file_key: None,
                speaker: None,
                start_ms: None,
                end_ms: None,
                duration_ms: 4_000,
                quality: 1.0,
                overlap: false,
                embedding: vec![0.99, 0.01],
                created_at_ms: 1,
            },
            SpeakerVoiceprintTemplate {
                id: "far".to_string(),
                source_kind: "test".to_string(),
                prompt_id: None,
                task_id: None,
                file_key: None,
                speaker: None,
                start_ms: None,
                end_ms: None,
                duration_ms: 4_000,
                quality: 1.0,
                overlap: false,
                embedding: vec![0.0, 1.0],
                created_at_ms: 1,
            },
        ];

        let prototypes = build_voiceprint_prototypes(&templates).unwrap();

        assert_eq!(prototypes.len(), 2);
        assert_eq!(prototypes[0].template_ids, vec!["near-1", "near-2"]);
        assert_eq!(prototypes[1].template_ids, vec!["far"]);
    }

    #[test]
    fn assisted_finish_appends_templates_and_sample_delete_rebuilds_profile() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let pcm = assisted_test_pcm();
        let first_session = AssistedVoiceprintSession {
            id: "assisted-first".to_string(),
            state: AssistedVoiceprintSessionState::Open,
            speaker_name: "Eden".to_string(),
            profile_id: None,
            task_id: "task-assisted".to_string(),
            file_key: "file-a".to_string(),
            source_path: temp.path().join("meeting.wav"),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            candidates: (0..3).map(assisted_test_candidate).collect(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        std::fs::create_dir_all(assisted_voiceprint_session_dir(&first_session.id)).unwrap();
        for candidate in &first_session.candidates {
            std::fs::write(
                assisted_voiceprint_candidate_audio_path(&first_session.id, &candidate.id),
                &pcm,
            )
            .unwrap();
        }

        let first = finish_assisted_voiceprint_enrollment_in_process(&first_session).unwrap();

        assert_eq!(first.profile.schema_version, VOICEPRINT_PROFILE_SCHEMA_VERSION);
        assert_eq!(first.profile.templates.len(), 3);
        assert!(!first.profile.prototypes.is_empty());
        assert!(!assisted_voiceprint_session_dir(&first_session.id).exists());

        let second_session = AssistedVoiceprintSession {
            id: "assisted-second".to_string(),
            profile_id: Some(first.profile.id.clone()),
            file_key: "file-b".to_string(),
            ..first_session
        };
        std::fs::create_dir_all(assisted_voiceprint_session_dir(&second_session.id)).unwrap();
        for candidate in &second_session.candidates {
            std::fs::write(
                assisted_voiceprint_candidate_audio_path(&second_session.id, &candidate.id),
                &pcm,
            )
            .unwrap();
        }

        let second = finish_assisted_voiceprint_enrollment_in_process(&second_session).unwrap();
        assert_eq!(second.profile.templates.len(), 6);
        let deleted_id = second.profile.templates[0].id.clone();
        let response = delete_speaker_profile_sample_response(&second.profile.id, &deleted_id);
        assert_eq!(response.status(), StatusCode::OK);
        let rebuilt = read_speaker_voiceprint_profile(&second.profile.id).unwrap();
        assert_eq!(rebuilt.templates.len(), 5);
        assert_eq!(rebuilt.total_duration_ms, 20_000);
    }

    #[test]
    fn deleting_live_template_removes_its_legacy_prompt_metadata() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        std::fs::create_dir_all(voiceprint_dir()).unwrap();
        let template = |id: &str, prompt_id: &str, embedding: Vec<f32>| {
            SpeakerVoiceprintTemplate {
                id: id.to_string(),
                source_kind: "live_prompt".to_string(),
                prompt_id: Some(prompt_id.to_string()),
                task_id: None,
                file_key: None,
                speaker: None,
                start_ms: None,
                end_ms: None,
                duration_ms: 4_000,
                quality: 1.0,
                overlap: false,
                embedding,
                created_at_ms: 1,
            }
        };
        let mut profile = SpeakerVoiceprintProfile {
            schema_version: VOICEPRINT_PROFILE_SCHEMA_VERSION,
            id: "spk-live-delete".to_string(),
            display_name: "Eden".to_string(),
            source: "live_enrollment".to_string(),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            embedding_model: "test".to_string(),
            embedding_dim: 2,
            embedding: vec![1.0, 0.0],
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            total_duration_ms: 8_000,
            samples: vec![
                SpeakerVoiceprintSample {
                    prompt_id: "prompt-1".to_string(),
                    text: "one".to_string(),
                    duration_ms: 4_000,
                    rms: 0.5,
                    clipped_ratio: 0.0,
                },
                SpeakerVoiceprintSample {
                    prompt_id: "prompt-2".to_string(),
                    text: "two".to_string(),
                    duration_ms: 4_000,
                    rms: 0.5,
                    clipped_ratio: 0.0,
                },
            ],
            templates: vec![
                template("sample-1", "prompt-1", vec![1.0, 0.0]),
                template("sample-2", "prompt-2", vec![0.9, 0.1]),
            ],
            prototypes: Vec::new(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        rebuild_voiceprint_profile(&mut profile).unwrap();
        atomic_json_write(&speaker_profile_path(&profile.id), &profile).unwrap();

        let response = delete_speaker_profile_sample_response(&profile.id, "sample-1");

        assert_eq!(response.status(), StatusCode::OK);
        let rebuilt = read_speaker_voiceprint_profile(&profile.id).unwrap();
        assert_eq!(rebuilt.templates.len(), 1);
        assert_eq!(rebuilt.samples.len(), 1);
        assert_eq!(rebuilt.samples[0].prompt_id, "prompt-2");
        assert_eq!(rebuilt.total_duration_ms, 4_000);
    }

    #[test]
    fn legacy_profile_append_migrates_centroid_and_validates_compatibility() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        std::fs::create_dir_all(voiceprint_dir()).unwrap();
        let legacy = SpeakerVoiceprintProfile {
            schema_version: 1,
            id: "spk-legacy-append".to_string(),
            display_name: "Eden".to_string(),
            source: "live_enrollment".to_string(),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            embedding_model: "test".to_string(),
            embedding_dim: 2,
            embedding: vec![1.0, 0.0],
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            total_duration_ms: 4_000,
            samples: Vec::new(),
            templates: Vec::new(),
            prototypes: Vec::new(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        atomic_json_write(&speaker_profile_path(&legacy.id), &legacy).unwrap();
        let session = AssistedVoiceprintSession {
            profile_id: Some(legacy.id.clone()),
            ..assisted_test_session("assisted-legacy-append")
        };

        let response = persist_assisted_voiceprint_templates(
            &session,
            vec![assisted_test_template("new", vec![0.9, 0.1])],
        )
        .unwrap();

        assert_eq!(response.profile.schema_version, VOICEPRINT_PROFILE_SCHEMA_VERSION);
        assert_eq!(response.profile.templates.len(), 2);
        assert_eq!(response.profile.templates[0].id, "legacy-spk-legacy-append");

        for (field, altered_session, template) in [
            (
                "speaker name",
                AssistedVoiceprintSession {
                    speaker_name: "Other".to_string(),
                    ..session.clone()
                },
                assisted_test_template("name", vec![1.0, 0.0]),
            ),
            (
                "diarization profile",
                AssistedVoiceprintSession {
                    diarization_profile: "other-profile".to_string(),
                    ..session.clone()
                },
                assisted_test_template("profile", vec![1.0, 0.0]),
            ),
            (
                "sample rate",
                AssistedVoiceprintSession {
                    sample_rate: 8_000,
                    ..session.clone()
                },
                assisted_test_template("rate", vec![1.0, 0.0]),
            ),
            (
                "embedding dimension",
                session.clone(),
                assisted_test_template("dimension", vec![1.0, 0.0, 0.0]),
            ),
        ] {
            let error = persist_assisted_voiceprint_templates(&altered_session, vec![template])
                .unwrap_err();
            assert!(error.contains(field), "unexpected error: {error}");
        }
    }

    #[test]
    fn assisted_template_quality_failures_are_rejected_before_embedding() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let mut session = assisted_test_session("assisted-quality");
        std::fs::create_dir_all(assisted_voiceprint_session_dir(&session.id)).unwrap();

        session.candidates[0].overlap = true;
        assert!(compute_assisted_voiceprint_templates(&session)
            .unwrap_err()
            .contains("quality gate"));
        session.candidates[0].overlap = false;

        for candidate in &session.candidates {
            std::fs::write(
                assisted_voiceprint_candidate_audio_path(&session.id, &candidate.id),
                assisted_test_pcm(),
            )
            .unwrap();
        }
        std::fs::write(
            assisted_voiceprint_candidate_audio_path(&session.id, &session.candidates[0].id),
            vec![0_u8; VOICEPRINT_SAMPLE_RATE as usize * 2],
        )
        .unwrap();
        assert!(compute_assisted_voiceprint_templates(&session)
            .unwrap_err()
            .contains("too short"));

        std::fs::write(
            assisted_voiceprint_candidate_audio_path(&session.id, &session.candidates[0].id),
            vec![0_u8; VOICEPRINT_SAMPLE_RATE as usize * 2 * 4],
        )
        .unwrap();
        assert!(compute_assisted_voiceprint_templates(&session)
            .unwrap_err()
            .contains("insufficient speech energy"));
    }

    #[test]
    fn voiceprint_rebuild_rejects_empty_templates_and_embeddings() {
        let mut profile = SpeakerVoiceprintProfile {
            schema_version: 1,
            id: "spk-invalid".to_string(),
            display_name: "Invalid".to_string(),
            source: "test".to_string(),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            embedding_model: "test".to_string(),
            embedding_dim: 0,
            embedding: Vec::new(),
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            total_duration_ms: 0,
            samples: Vec::new(),
            templates: Vec::new(),
            prototypes: Vec::new(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        assert!(rebuild_voiceprint_profile(&mut profile)
            .unwrap_err()
            .contains("at least one template"));
        assert!(build_voiceprint_prototypes(&[assisted_test_template("empty", Vec::new())])
            .unwrap_err()
            .contains("empty embedding"));
    }

    #[test]
    fn deleting_the_last_voiceprint_template_is_rejected() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        std::fs::create_dir_all(voiceprint_dir()).unwrap();
        let profile = SpeakerVoiceprintProfile {
            schema_version: VOICEPRINT_PROFILE_SCHEMA_VERSION,
            id: "spk-last-template".to_string(),
            display_name: "Eden".to_string(),
            source: "assisted_recording".to_string(),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            embedding_model: "test".to_string(),
            embedding_dim: 2,
            embedding: vec![1.0, 0.0],
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            total_duration_ms: 4_000,
            samples: Vec::new(),
            templates: vec![assisted_test_template("only", vec![1.0, 0.0])],
            prototypes: Vec::new(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        atomic_json_write(&speaker_profile_path(&profile.id), &profile).unwrap();

        let response = delete_speaker_profile_sample_response(&profile.id, "only");

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn assisted_finish_rejects_selection_below_gate() {
        let session = AssistedVoiceprintSession {
            id: "assisted-short".to_string(),
            state: AssistedVoiceprintSessionState::Open,
            speaker_name: "Eden".to_string(),
            profile_id: None,
            task_id: "task-assisted".to_string(),
            file_key: "file-a".to_string(),
            source_path: PathBuf::from("meeting.wav"),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            candidates: vec![assisted_test_candidate(0), assisted_test_candidate(1)],
            created_at_ms: 1,
            updated_at_ms: 1,
        };

        let error = finish_assisted_voiceprint_enrollment_in_process(&session).unwrap_err();

        assert!(error.contains("select at least 3 clips"));
    }

    #[test]
    fn assisted_session_finish_state_blocks_duplicate_finish_and_delete() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        let session = AssistedVoiceprintSession {
            id: "assisted-state".to_string(),
            state: AssistedVoiceprintSessionState::Open,
            speaker_name: "Eden".to_string(),
            profile_id: None,
            task_id: "task-assisted".to_string(),
            file_key: "file-a".to_string(),
            source_path: PathBuf::from("meeting.wav"),
            diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
            sample_rate: VOICEPRINT_SAMPLE_RATE,
            candidates: (0..3).map(assisted_test_candidate).collect(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        std::fs::create_dir_all(assisted_voiceprint_session_dir(&session.id)).unwrap();
        atomic_json_write(&assisted_voiceprint_session_path(&session.id), &session).unwrap();
        let candidate_audio = assisted_voiceprint_candidate_audio_path(
            &session.id,
            &session.candidates[0].id,
        );
        std::fs::write(&candidate_audio, assisted_test_pcm()).unwrap();

        let finishing = begin_assisted_voiceprint_finish(&session.id).unwrap();

        assert_eq!(finishing.state, AssistedVoiceprintSessionState::Finishing);
        let duplicate = begin_assisted_voiceprint_finish(&session.id).unwrap_err();
        assert_eq!(duplicate.0, StatusCode::CONFLICT);
        assert_eq!(
            delete_assisted_voiceprint_session_response(&session.id).status(),
            StatusCode::CONFLICT
        );

        restore_assisted_voiceprint_session(&finishing);
        assert_eq!(
            read_assisted_voiceprint_session(&session.id).unwrap().state,
            AssistedVoiceprintSessionState::Open
        );
        assert!(!candidate_audio.exists());
    }

    #[test]
    fn assisted_session_cleanup_removes_expired_sessions_and_keeps_fresh_ones() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        for (id, state, updated_at_ms) in [
            ("expired-open", AssistedVoiceprintSessionState::Open, 1),
            ("expired-finishing", AssistedVoiceprintSessionState::Finishing, 1),
            ("fresh-open", AssistedVoiceprintSessionState::Open, now_ms()),
        ] {
            let session = AssistedVoiceprintSession {
                id: id.to_string(),
                state,
                speaker_name: "Eden".to_string(),
                profile_id: None,
                task_id: "task-assisted".to_string(),
                file_key: "file-a".to_string(),
                source_path: PathBuf::from("meeting.wav"),
                diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
                sample_rate: VOICEPRINT_SAMPLE_RATE,
                candidates: Vec::new(),
                created_at_ms: 1,
                updated_at_ms,
            };
            std::fs::create_dir_all(assisted_voiceprint_session_dir(id)).unwrap();
            atomic_json_write(&assisted_voiceprint_session_path(id), &session).unwrap();
        }

        cleanup_expired_assisted_voiceprint_sessions();

        assert!(!assisted_voiceprint_session_dir("expired-open").exists());
        assert!(!assisted_voiceprint_session_dir("expired-finishing").exists());
        assert!(assisted_voiceprint_session_dir("fresh-open").exists());
    }

    #[test]
    fn close_voiceprint_profiles_remain_ambiguous_instead_of_auto_matching() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        std::fs::create_dir_all(voiceprint_dir()).unwrap();
        for (id, name, embedding) in [
            ("spk-a", "Alice", vec![1.0, 0.0]),
            ("spk-b", "Bob", vec![0.999, 0.001]),
        ] {
            let profile = SpeakerVoiceprintProfile {
                schema_version: 1,
                id: id.to_string(),
                display_name: name.to_string(),
                source: "live_enrollment".to_string(),
                diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
                embedding_model: "test".to_string(),
                embedding_dim: 2,
                embedding,
                sample_rate: VOICEPRINT_SAMPLE_RATE,
                total_duration_ms: 3_000,
                samples: Vec::new(),
                templates: Vec::new(),
                prototypes: Vec::new(),
                created_at_ms: 1,
                updated_at_ms: 1,
            };
            atomic_json_write(&speaker_profile_path(id), &profile).unwrap();
        }

        let candidate = best_registered_voiceprint_match(&[1.0, 0.0]).unwrap();

        assert_eq!(candidate.profile_id, "spk-a");
        assert!(!candidate.unambiguous);
    }

    #[test]
    fn diarization_mapping_marks_close_multi_profile_candidates_as_conflicted() {
        let _lock = test_data_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set_data_dir(temp.path());
        std::fs::create_dir_all(voiceprint_dir()).unwrap();
        for (id, name, embedding) in [
            ("spk-a", "Alice", vec![1.0, 0.0]),
            ("spk-b", "Bob", vec![0.999, 0.001]),
        ] {
            let profile = SpeakerVoiceprintProfile {
                schema_version: 1,
                id: id.to_string(),
                display_name: name.to_string(),
                source: "test".to_string(),
                diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
                embedding_model: "test".to_string(),
                embedding_dim: 2,
                embedding,
                sample_rate: VOICEPRINT_SAMPLE_RATE,
                total_duration_ms: 4_000,
                samples: Vec::new(),
                templates: Vec::new(),
                prototypes: Vec::new(),
                created_at_ms: 1,
                updated_at_ms: 1,
            };
            atomic_json_write(&speaker_profile_path(id), &profile).unwrap();
        }
        let mut segments = vec![DiarizationSegment {
            speaker: "speaker_00".to_string(),
            display_name: "User A".to_string(),
            mapped_profile_id: None,
            confidence: None,
            candidate_profile_id: None,
            candidate_display_name: None,
            candidate_confidence: None,
            start_ms: 0,
            end_ms: 6_000,
            overlap: false,
        }];
        let embeddings = BTreeMap::from([("speaker_00".to_string(), vec![1.0, 0.0])]);

        map_speakers_with_registered_voiceprints(&mut segments, &embeddings);

        assert_eq!(segments[0].display_name, "User A");
        assert_eq!(segments[0].mapped_profile_id, None);
        assert_eq!(segments[0].candidate_profile_id.as_deref(), Some("spk-a"));
    }

    #[tokio::test]
    async fn voiceprint_ffmpeg_cut_rejects_empty_duration_and_invalid_source() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output.pcm16le");

        assert!(ffmpeg_cut_pcm16le_ms(Path::new("missing.wav"), &output, 1_000, 1_000)
            .await
            .unwrap_err()
            .contains("empty duration"));
        assert!(ffmpeg_cut_pcm16le_ms(Path::new("missing.wav"), &output, 0, 1_000)
            .await
            .unwrap_err()
            .contains("ffmpeg voiceprint"));
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn diarization_embedding_waveforms_exclude_overlap_segments() {
        let waveform = vec![1.0; 16_000];
        let segments = vec![
            DiarizationSegment {
                speaker: "speaker_00".to_string(),
                display_name: "User A".to_string(),
                mapped_profile_id: None,
                confidence: None,
                candidate_profile_id: None,
                candidate_display_name: None,
                candidate_confidence: None,
                start_ms: 0,
                end_ms: 500,
                overlap: false,
            },
            DiarizationSegment {
                speaker: "speaker_00".to_string(),
                display_name: "User A".to_string(),
                mapped_profile_id: None,
                confidence: None,
                candidate_profile_id: None,
                candidate_display_name: None,
                candidate_confidence: None,
                start_ms: 500,
                end_ms: 1_000,
                overlap: true,
            },
        ];

        let by_speaker = collect_diarization_speaker_waveforms(&waveform, 16_000, &segments);

        assert_eq!(by_speaker["speaker_00"].len(), 8_000);
    }
}
