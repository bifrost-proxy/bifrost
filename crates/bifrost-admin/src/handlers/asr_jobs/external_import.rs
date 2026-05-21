fn external_import_store_path(task_id: &str) -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr/tasks")
        .join(task_id)
        .join("external_imports.json")
}

fn content_hash_index_path(task_id: &str) -> PathBuf {
    bifrost_storage::data_dir()
        .join("asr/tasks")
        .join(task_id)
        .join("content_hash_index.json")
}

fn load_external_import_store(task_id: &str) -> AsrExternalImportStore {
    let path = external_import_store_path(task_id);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<AsrExternalImportStore>(&content).ok())
        .filter(|store| store.version == TASK_STORE_VERSION)
        .unwrap_or(AsrExternalImportStore {
            version: TASK_STORE_VERSION,
            devices: BTreeMap::new(),
            runs: Vec::new(),
        })
}

fn save_external_import_store(task_id: &str, store: &AsrExternalImportStore) -> Result<(), String> {
    atomic_json_write(&external_import_store_path(task_id), store)
}

fn load_content_hash_index(task_id: &str) -> AsrContentHashIndex {
    let path = content_hash_index_path(task_id);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<AsrContentHashIndex>(&content).ok())
        .filter(|index| index.version == TASK_STORE_VERSION)
        .unwrap_or(AsrContentHashIndex {
            version: TASK_STORE_VERSION,
            hashes: BTreeMap::new(),
        })
}

fn save_content_hash_index(task_id: &str, index: &AsrContentHashIndex) -> Result<(), String> {
    atomic_json_write(&content_hash_index_path(task_id), index)
}

fn list_external_volumes() -> Vec<ExternalVolumeInfo> {
    let mut volumes = Vec::new();
    let Ok(entries) = std::fs::read_dir("/Volumes") else {
        return volumes;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if matches!(name, "Macintosh HD" | "Data" | "Preboot" | "Recovery" | "VM") {
            continue;
        }
        let info = diskutil_info(&path);
        let kind = if info.external {
            "external".to_string()
        } else {
            "mounted".to_string()
        };
        volumes.push(ExternalVolumeInfo {
            name: name.to_string(),
            mount_path: path.clone(),
            volume_uuid: info.volume_uuid,
            device_identifier: info.device_identifier,
            kind,
            read_only: info.read_only,
            available_bytes: path_available_bytes(&path),
        });
    }
    volumes.sort_by(|left, right| left.name.cmp(&right.name));
    volumes
}

#[derive(Default)]
struct DiskutilInfo {
    external: bool,
    read_only: bool,
    volume_uuid: Option<String>,
    device_identifier: Option<String>,
}

fn diskutil_info(path: &Path) -> DiskutilInfo {
    let output = std::process::Command::new("diskutil")
        .arg("info")
        .arg(path)
        .output();
    let Ok(output) = output else {
        return DiskutilInfo::default();
    };
    if !output.status.success() {
        return DiskutilInfo::default();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut info = DiskutilInfo::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("Volume UUID:") {
            let value = value.trim();
            if !value.is_empty() && value != "Not applicable" {
                info.volume_uuid = Some(value.to_string());
            }
        } else if let Some(value) = trimmed.strip_prefix("Device Identifier:") {
            let value = value.trim();
            if !value.is_empty() {
                info.device_identifier = Some(value.to_string());
            }
        } else if let Some(value) = trimmed.strip_prefix("Device Location:") {
            info.external = value.trim().eq_ignore_ascii_case("external");
        } else if let Some(value) = trimmed.strip_prefix("Read-Only Media:") {
            info.read_only |= value.trim().eq_ignore_ascii_case("yes");
        } else if let Some(value) = trimmed.strip_prefix("Read-Only Volume:") {
            info.read_only |= value.trim().eq_ignore_ascii_case("yes");
        }
    }
    info
}

#[cfg(unix)]
fn path_available_bytes(path: &Path) -> Option<u64> {
    let output = std::process::Command::new("df")
        .arg("-k")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().nth(1)?;
    let available_kib = line.split_whitespace().nth(3)?.parse::<u64>().ok()?;
    Some(available_kib * 1024)
}

#[cfg(not(unix))]
fn path_available_bytes(_path: &Path) -> Option<u64> {
    None
}

fn sanitize_device_root(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('.')
        .trim_matches('_')
        .to_string();
    if sanitized.is_empty() {
        "external_device".to_string()
    } else {
        sanitized
    }
}

fn normalize_bindings(
    bindings: Vec<AsrExternalDeviceBinding>,
) -> Result<Vec<AsrExternalDeviceBinding>, String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for mut binding in bindings {
        binding.name = binding.name.trim().to_string();
        if binding.name.is_empty() {
            return Err("external device name cannot be empty".to_string());
        }
        if !seen.insert(binding.name.clone()) {
            return Err(format!("duplicate external device name '{}'", binding.name));
        }
        out.push(binding);
    }
    Ok(out)
}

fn validate_import_policy(policy: &AsrExternalImportPolicy) -> Result<(), String> {
    if policy.file_stable_secs > 24 * 60 * 60 {
        return Err("external import file_stable_secs must be at most 86400".to_string());
    }
    if policy.max_file_bytes == 0 {
        return Err("external import max_file_bytes must be greater than 0".to_string());
    }
    if policy.content_hash_algorithm != "sha256" {
        return Err("external import content_hash_algorithm currently only supports sha256".to_string());
    }
    Ok(())
}

async fn sync_external_devices_for_task(task: &AsrDirectoryTask) -> Result<usize, String> {
    if !task.import_policy.enabled || task.external_devices.is_empty() {
        return Ok(0);
    }
    let _guard = EXTERNAL_IMPORT_LOCK.lock().await;
    let volumes = list_external_volumes();
    let mut store = load_external_import_store(&task.id);
    let run_started = now_ms();
    let mut imported_total = 0usize;
    let mut skipped_total = 0usize;
    let mut failed_total = 0usize;

    for binding in task.external_devices.iter().filter(|binding| binding.enabled) {
        let matches = volumes
            .iter()
            .filter(|volume| external_volume_matches(binding, volume))
            .collect::<Vec<_>>();
        let state = store
            .devices
            .entry(binding.name.clone())
            .or_insert_with(|| AsrExternalDeviceState {
                binding_name: binding.name.clone(),
                ..Default::default()
            });
        state.last_scan_at_ms = Some(now_ms());
        if matches.is_empty() {
            state.last_status = Some("not_connected".to_string());
            continue;
        }
        if matches.len() > 1 {
            state.last_status = Some("ambiguous".to_string());
            state.last_error = Some("multiple mounted volumes match this device name".to_string());
            failed_total += 1;
            continue;
        }
        let volume = matches[0];
        state.last_seen_mount_path = Some(volume.mount_path.clone());
        if let Some(available) = path_available_bytes(&task.audio_dir) {
            if available < task.import_policy.min_free_bytes {
                state.last_status = Some("insufficient_space".to_string());
                state.last_error = Some(format!(
                    "target free space below threshold: {available} < {}",
                    task.import_policy.min_free_bytes
                ));
                failed_total += 1;
                continue;
            }
        }
        match import_volume_files(task, binding, volume, state) {
            Ok(summary) => {
                imported_total += summary.0;
                skipped_total += summary.1;
                failed_total += summary.2;
                state.last_status = Some(if summary.2 > 0 {
                    "completed_with_errors".to_string()
                } else {
                    "completed".to_string()
                });
                state.last_error = None;
                if summary.0 > 0 {
                    state.last_import_at_ms = Some(now_ms());
                }
            }
            Err(error) => {
                state.last_status = Some("failed".to_string());
                state.last_error = Some(error);
                failed_total += 1;
            }
        }
    }

    store.runs.push(AsrExternalImportRunSummary {
        run_id: uuid::Uuid::new_v4().as_simple().to_string(),
        device_name: None,
        started_at_ms: run_started,
        finished_at_ms: now_ms(),
        imported: imported_total,
        skipped: skipped_total,
        failed: failed_total,
        status: if failed_total > 0 {
            "completed_with_errors".to_string()
        } else {
            "completed".to_string()
        },
    });
    if store.runs.len() > 50 {
        let drain = store.runs.len() - 50;
        store.runs.drain(0..drain);
    }
    save_external_import_store(&task.id, &store)?;
    Ok(imported_total)
}

fn external_volume_matches(binding: &AsrExternalDeviceBinding, volume: &ExternalVolumeInfo) -> bool {
    if volume.name != binding.name {
        return false;
    }
    if let Some(uuid) = binding.volume_uuid.as_deref() {
        if volume.volume_uuid.as_deref() != Some(uuid) {
            return false;
        }
    }
    if let Some(device_identifier) = binding.device_identifier.as_deref() {
        if volume.device_identifier.as_deref() != Some(device_identifier) {
            return false;
        }
    }
    true
}

fn import_volume_files(
    task: &AsrDirectoryTask,
    binding: &AsrExternalDeviceBinding,
    volume: &ExternalVolumeInfo,
    state: &mut AsrExternalDeviceState,
) -> Result<(usize, usize, usize), String> {
    let target_root = task.audio_dir.join(sanitize_device_root(&binding.name));
    std::fs::create_dir_all(&target_root)
        .map_err(|error| format!("create import target {}: {error}", target_root.display()))?;
    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut stack = vec![volume.mount_path.clone()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(dir = %dir.display(), %error, "skipping unreadable external directory");
                failed += 1;
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if matches!(
                file_name.as_ref(),
                ".Spotlight-V100"
                    | ".Trashes"
                    | ".fseventsd"
                    | ".TemporaryItems"
                    | ".DS_Store"
                    | "System Volume Information"
                    | "$RECYCLE.BIN"
            ) {
                continue;
            }
            if is_macos_metadata_file(&file_name) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() || !is_audio_file(&path) {
                continue;
            }
            match import_one_file(task, volume, &target_root, &path, state) {
                Ok("imported") => imported += 1,
                Ok("skipped") => skipped += 1,
                Ok(_) => skipped += 1,
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "external import file failed");
                    failed += 1;
                }
            }
        }
    }
    Ok((imported, skipped, failed))
}

fn is_macos_metadata_file(file_name: &str) -> bool {
    file_name.starts_with("._")
}

fn import_one_file(
    task: &AsrDirectoryTask,
    volume: &ExternalVolumeInfo,
    target_root: &Path,
    source: &Path,
    state: &mut AsrExternalDeviceState,
) -> Result<&'static str, String> {
    let metadata = source
        .metadata()
        .map_err(|error| format!("read source metadata {}: {error}", source.display()))?;
    if metadata.len() == 0 || metadata.len() > task.import_policy.max_file_bytes {
        return Ok("skipped");
    }
    let relative = source
        .strip_prefix(&volume.mount_path)
        .map_err(|_| format!("source escaped mount path: {}", source.display()))?;
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("unsafe relative path: {}", relative.display()));
    }
    let target = target_root.join(relative);
    let relative_key = relative.to_string_lossy().to_string();
    let source_modified = source_modified_ms(source);
    if let Some(record) = state.files.get(&relative_key) {
        if record.status == "imported"
            && record.source_size == metadata.len()
            && record.source_modified_ms == source_modified
            && target.is_file()
        {
            return Ok("skipped");
        }
    }
    if should_defer_unstable_source(
        state.files.get(&relative_key),
        metadata.len(),
        source_modified,
        task.import_policy.file_stable_secs,
    ) {
        let now = now_ms();
        let first_seen_at_ms = state
            .files
            .get(&relative_key)
            .filter(|record| {
                record.source_size == metadata.len() && record.source_modified_ms == source_modified
            })
            .and_then(|record| record.first_seen_at_ms)
            .unwrap_or(now);
        state.files.insert(
            relative_key,
            AsrImportedFileRecord {
                relative_path: relative.to_path_buf(),
                source_size: metadata.len(),
                source_modified_ms: source_modified,
                source_sha256: None,
                target_path: target,
                target_size: 0,
                first_seen_at_ms: Some(first_seen_at_ms),
                imported_at_ms: now,
                status: "deferred".to_string(),
                error: Some(format!(
                    "waiting for source file to stay unchanged for {} seconds",
                    task.import_policy.file_stable_secs
                )),
            },
        );
        return Ok("skipped");
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create target dir {}: {error}", parent.display()))?;
    }
    let tmp = target.with_extension(format!(
        "bifrost-import-{}.part",
        uuid::Uuid::new_v4().as_simple()
    ));
    let hash = copy_with_sha256(source, &tmp)?;
    let copied_size = tmp
        .metadata()
        .map_err(|error| format!("read temp metadata {}: {error}", tmp.display()))?
        .len();
    if copied_size != metadata.len() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "copy size mismatch for {}: {} != {}",
            source.display(),
            copied_size,
            metadata.len()
        ));
    }
    std::fs::rename(&tmp, &target)
        .map_err(|error| format!("rename {} -> {}: {error}", tmp.display(), target.display()))?;
    state.files.insert(
        relative_key,
        AsrImportedFileRecord {
            relative_path: relative.to_path_buf(),
            source_size: metadata.len(),
            source_modified_ms: source_modified,
            source_sha256: Some(hash),
            target_path: target,
            target_size: copied_size,
            first_seen_at_ms: None,
            imported_at_ms: now_ms(),
            status: "imported".to_string(),
            error: None,
        },
    );
    Ok("imported")
}

fn should_defer_unstable_source(
    record: Option<&AsrImportedFileRecord>,
    source_size: u64,
    source_modified_ms: Option<u64>,
    file_stable_secs: u64,
) -> bool {
    if file_stable_secs == 0 {
        return false;
    }
    let stable_ms = file_stable_secs.saturating_mul(1000);
    let now = now_ms();
    if let Some(modified_ms) = source_modified_ms {
        return now.saturating_sub(modified_ms) < stable_ms;
    }
    let Some(record) = record else {
        return true;
    };
    if record.source_size != source_size || record.source_modified_ms != source_modified_ms {
        return true;
    }
    let first_seen_at_ms = record.first_seen_at_ms.unwrap_or(record.imported_at_ms);
    now.saturating_sub(first_seen_at_ms) < stable_ms
}

fn copy_with_sha256(source: &Path, target: &Path) -> Result<String, String> {
    let mut input = std::fs::File::open(source)
        .map_err(|error| format!("open source {}: {error}", source.display()))?;
    let mut output = std::fs::File::create(target)
        .map_err(|error| format!("create target {}: {error}", target.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let read = std::io::Read::read(&mut input, &mut buf)
            .map_err(|error| format!("read source {}: {error}", source.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        std::io::Write::write_all(&mut output, &buf[..read])
            .map_err(|error| format!("write target {}: {error}", target.display()))?;
    }
    std::io::Write::flush(&mut output)
        .map_err(|error| format!("flush target {}: {error}", target.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut input =
        std::fs::File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let read = std::io::Read::read(&mut input, &mut buf)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn apply_content_hash_dedupe(
    task: &AsrDirectoryTask,
    discovered: &[PathBuf],
    files: &mut FileStore,
) -> Result<(), String> {
    if !task.import_policy.content_hash_dedupe_enabled {
        return Ok(());
    }
    let mut index = load_content_hash_index(&task.id);
    let mut changed = false;
    for path in discovered {
        let key = source_key(path);
        let Some(record) = files.files.get_mut(&key) else {
            continue;
        };
        if record.content_hash.is_none() {
            let before = path.metadata().ok();
            let hash = match sha256_file(path) {
                Ok(hash) => hash,
                Err(error) => {
                    record.error = Some(format!("hash_unavailable: {error}"));
                    continue;
                }
            };
            let after = path.metadata().ok();
            if before.as_ref().map(|m| (m.len(), source_modified_ms(path)))
                != after.as_ref().map(|m| (m.len(), source_modified_ms(path)))
            {
                record.error = Some("hash_changed_during_read".to_string());
                continue;
            }
            record.content_hash = Some(hash);
            record.content_hash_algorithm = Some("sha256".to_string());
            changed = true;
        }
        let Some(hash) = record.content_hash.clone() else {
            continue;
        };
        let hash_key = format!("sha256:{hash}");
        if matches!(record.status, FileStatus::Success | FileStatus::PartialSuccess)
            && record.output_text_path.as_ref().is_some_and(|p| p.is_file())
            && record.output_metadata_path.as_ref().is_some_and(|p| p.is_file())
        {
            index.hashes.entry(hash_key.clone()).or_insert_with(|| AsrContentHashRecord {
                algorithm: "sha256".to_string(),
                hash: hash.clone(),
                size: record.source_size.unwrap_or_default(),
                canonical_source_key: key.clone(),
                canonical_source_path: record.source_path.clone(),
                transcript_artifacts: AsrTranscriptArtifacts {
                    text_path: record.output_text_path.clone().unwrap(),
                    metadata_path: record.output_metadata_path.clone().unwrap(),
                    timeline_path: record.output_timeline_path.clone(),
                },
                model: task.model.clone(),
                language: task.language.clone(),
                runtime_strategy: task.runtime_strategy,
                completed_at_ms: record.finished_at_ms.unwrap_or_else(now_ms),
                duplicate_count: 0,
            });
            changed = true;
            continue;
        }
        if !matches!(record.status, FileStatus::Pending | FileStatus::Failed | FileStatus::Processing) {
            continue;
        }
        let Some(existing) = index.hashes.get_mut(&hash_key) else {
            continue;
        };
        if existing.canonical_source_key == key
            || existing.language != task.language
            || existing.model != task.model
            || existing.runtime_strategy != task.runtime_strategy
            || !existing.transcript_artifacts.text_path.is_file()
            || !existing.transcript_artifacts.metadata_path.is_file()
        {
            continue;
        }
        record.status = FileStatus::Success;
        record.output_text_path = Some(existing.transcript_artifacts.text_path.clone());
        record.output_metadata_path = Some(existing.transcript_artifacts.metadata_path.clone());
        record.output_timeline_path = existing.transcript_artifacts.timeline_path.clone();
        record.text_chars = std::fs::read_to_string(&existing.transcript_artifacts.text_path)
            .map(|text| text.chars().count())
            .unwrap_or(0);
        record.error = None;
        record.duplicate_of_source_key = Some(existing.canonical_source_key.clone());
        record.transcript_alias = Some(existing.canonical_source_path.display().to_string());
        record.finished_at_ms = Some(now_ms());
        existing.duplicate_count += 1;
        changed = true;
    }
    if changed {
        save_content_hash_index(&task.id, &index)?;
    }
    Ok(())
}

fn index_completed_file_hash(task: &AsrDirectoryTask, key: &str, record: &FileRecord) {
    if !task.import_policy.content_hash_dedupe_enabled {
        return;
    }
    let Some(hash) = record.content_hash.clone() else {
        return;
    };
    let (Some(text_path), Some(metadata_path)) = (
        record.output_text_path.clone(),
        record.output_metadata_path.clone(),
    ) else {
        return;
    };
    let mut index = load_content_hash_index(&task.id);
    index.hashes.entry(format!("sha256:{hash}")).or_insert_with(|| AsrContentHashRecord {
        algorithm: "sha256".to_string(),
        hash,
        size: record.source_size.unwrap_or_default(),
        canonical_source_key: key.to_string(),
        canonical_source_path: record.source_path.clone(),
        transcript_artifacts: AsrTranscriptArtifacts {
            text_path,
            metadata_path,
            timeline_path: record.output_timeline_path.clone(),
        },
        model: task.model.clone(),
        language: task.language.clone(),
        runtime_strategy: task.runtime_strategy,
        completed_at_ms: record.finished_at_ms.unwrap_or_else(now_ms),
        duplicate_count: 0,
    });
    let _ = save_content_hash_index(&task.id, &index);
}
