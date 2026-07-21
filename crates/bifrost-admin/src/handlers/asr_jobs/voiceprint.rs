fn default_mono_channels() -> u8 {
    1
}

fn default_voiceprint_profile_schema_version() -> u32 {
    1
}

static SPEAKER_PROFILE_MUTATION_LOCK: Lazy<StdMutex<()>> = Lazy::new(|| StdMutex::new(()));
static ASSISTED_SESSION_MUTATION_LOCK: Lazy<StdMutex<()>> = Lazy::new(|| StdMutex::new(()));

async fn read_json_body<T: serde::de::DeserializeOwned>(
    req: Request<Incoming>,
) -> Result<T, Response<BoxBody>> {
    let body = req
        .into_body()
        .collect()
        .await
        .map_err(|error| {
            error_response(
                StatusCode::BAD_REQUEST,
                &format!("failed to read request body: {error}"),
            )
        })?
        .to_bytes();
    serde_json::from_slice::<T>(&body)
        .map_err(|error| error_response(StatusCode::BAD_REQUEST, &format!("invalid JSON: {error}")))
}

fn speaker_enrollment_sessions_dir() -> PathBuf {
    voiceprint_dir().join("sessions")
}

fn speaker_enrollment_session_dir(session_id: &str) -> PathBuf {
    speaker_enrollment_sessions_dir().join(session_id)
}

fn assisted_voiceprint_sessions_dir() -> PathBuf {
    voiceprint_dir().join("assisted-sessions")
}

fn assisted_voiceprint_session_dir(session_id: &str) -> PathBuf {
    assisted_voiceprint_sessions_dir().join(session_id)
}

fn assisted_voiceprint_session_path(session_id: &str) -> PathBuf {
    assisted_voiceprint_session_dir(session_id).join("session.json")
}

fn assisted_voiceprint_candidate_audio_path(session_id: &str, candidate_id: &str) -> PathBuf {
    assisted_voiceprint_session_dir(session_id).join(format!("{candidate_id}.pcm16le"))
}

fn speaker_profile_path(profile_id: &str) -> PathBuf {
    voiceprint_dir().join(format!("{profile_id}.json"))
}

fn speaker_audio_path(session_id: &str, prompt_id: &str) -> PathBuf {
    speaker_enrollment_session_dir(session_id).join(format!("{prompt_id}.pcm16le"))
}

fn voiceprint_prompts() -> Vec<SpeakerEnrollmentPrompt> {
    VOICEPRINT_PROMPTS
        .iter()
        .enumerate()
        .map(|(index, text)| SpeakerEnrollmentPrompt {
            id: format!("prompt_{}", index + 1),
            text: (*text).to_string(),
        })
        .collect()
}

fn assisted_voiceprint_candidates(timeline: &TranscriptTimeline) -> Vec<AssistedVoiceprintCandidate> {
    let mut per_speaker = BTreeMap::<String, usize>::new();
    let mut candidates = Vec::new();
    for (segment_position, segment) in timeline.segments.iter().enumerate() {
        let Some(speaker) = segment.speaker.as_deref() else {
            continue;
        };
        if segment.overlap {
            continue;
        }
        let duration_ms = segment.audio_end_ms.saturating_sub(segment.audio_start_ms);
        if duration_ms < ASSISTED_CANDIDATE_MIN_MS {
            continue;
        }
        let mut start_ms = segment.audio_start_ms;
        let mut chunk_index = 0usize;
        while start_ms < segment.audio_end_ms {
            let end_ms = (start_ms + ASSISTED_CANDIDATE_MAX_MS).min(segment.audio_end_ms);
            let chunk_duration_ms = end_ms.saturating_sub(start_ms);
            if chunk_duration_ms < ASSISTED_CANDIDATE_MIN_MS {
                break;
            }
            let count = per_speaker.entry(speaker.to_string()).or_default();
            if *count >= ASSISTED_CANDIDATES_PER_SPEAKER {
                break;
            }
            let quality = (chunk_duration_ms as f32 / ASSISTED_CANDIDATE_MAX_MS as f32)
                .clamp(0.0, 1.0);
            candidates.push(AssistedVoiceprintCandidate {
                id: format!("candidate-{segment_position}-{chunk_index}"),
                speaker: speaker.to_string(),
                start_ms,
                end_ms,
                duration_ms: chunk_duration_ms,
                text: segment.text.clone(),
                quality,
                overlap: false,
                label: AssistedCandidateLabel::Unsure,
            });
            *count += 1;
            chunk_index += 1;
            start_ms = end_ms;
        }
    }
    candidates
}

fn read_assisted_voiceprint_session(session_id: &str) -> Result<AssistedVoiceprintSession, String> {
    validate_profile_id(session_id).map_err(|_| "invalid assisted session id".to_string())?;
    let raw = std::fs::read_to_string(assisted_voiceprint_session_path(session_id))
        .map_err(|error| format!("assisted voiceprint session not found: {error}"))?;
    serde_json::from_str(&raw)
        .map_err(|error| format!("read assisted voiceprint session: {error}"))
}

fn assisted_voiceprint_selection(session: &AssistedVoiceprintSession) -> (usize, u64) {
    session
        .candidates
        .iter()
        .filter(|candidate| candidate.label == AssistedCandidateLabel::Mine)
        .fold((0usize, 0u64), |(count, duration), candidate| {
            (count + 1, duration.saturating_add(candidate.duration_ms))
        })
}

fn assisted_voiceprint_session_payload(session: &AssistedVoiceprintSession) -> serde_json::Value {
    let (selected_count, selected_duration_ms) = assisted_voiceprint_selection(session);
    serde_json::json!({
        "session": session,
        "selected_count": selected_count,
        "selected_duration_ms": selected_duration_ms,
        "minimum_clips": ASSISTED_ENROLLMENT_MIN_CLIPS,
        "minimum_duration_ms": ASSISTED_ENROLLMENT_MIN_TOTAL_MS,
        "ready_to_finish": selected_count >= ASSISTED_ENROLLMENT_MIN_CLIPS
            && selected_duration_ms >= ASSISTED_ENROLLMENT_MIN_TOTAL_MS,
    })
}

async fn post_assisted_voiceprint_session_response(req: Request<Incoming>) -> Response<BoxBody> {
    cleanup_expired_assisted_voiceprint_sessions();
    let request = match read_json_body::<AssistedVoiceprintSessionCreateRequest>(req).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let requested_name = request.name.trim();
    if requested_name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "speaker name is required");
    }
    let speaker_name = if let Some(profile_id) = request.profile_id.as_deref() {
        if let Err(error) = validate_profile_id(profile_id) {
            return error_response(StatusCode::BAD_REQUEST, &error);
        }
        let profile = match read_speaker_voiceprint_profile(profile_id) {
            Ok(profile) => profile,
            Err(error) => return error_response(StatusCode::NOT_FOUND, &error),
        };
        if profile.display_name != requested_name {
            return error_response(
                StatusCode::CONFLICT,
                "speaker name must match the existing profile display name",
            );
        }
        profile.display_name
    } else {
        requested_name.to_string()
    };
    if find_task(&request.task_id).is_none() {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    }
    let files = load_file_store(&request.task_id);
    let Some(record) = files.files.get(&request.file_key) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task file not found");
    };
    if !matches!(record.status, FileStatus::Success | FileStatus::PartialSuccess) {
        return error_response(StatusCode::CONFLICT, "ASR task file is not completed");
    }
    if !record.source_path.is_file() {
        return error_response(StatusCode::CONFLICT, "ASR task source audio is no longer available");
    }
    let Some(timeline_path) = record.output_timeline_path.as_ref() else {
        return error_response(StatusCode::CONFLICT, "ASR task file has no transcript timeline");
    };
    let timeline = match std::fs::read_to_string(timeline_path)
        .map_err(|error| format!("read transcript timeline: {error}"))
        .and_then(|raw| {
            serde_json::from_str::<TranscriptTimeline>(&raw)
                .map_err(|error| format!("parse transcript timeline: {error}"))
        }) {
        Ok(timeline) => timeline,
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    if timeline.diarization_profile.is_none() {
        return error_response(StatusCode::CONFLICT, "ASR task file has no speaker-aware timeline");
    }
    let deterministic_test_embedding =
        std::env::var("BIFROST_ASR_VOICEPRINT_TEST_EMBEDDING").as_deref() == Ok("1");
    if !deterministic_test_embedding && !diarization_profile_ready(DEFAULT_DIARIZATION_PROFILE) {
        return error_response(
            StatusCode::CONFLICT,
            "speaker embedding profile is not initialized",
        );
    }
    let candidates = assisted_voiceprint_candidates(&timeline);
    if candidates.is_empty() {
        return error_response(
            StatusCode::CONFLICT,
            "no non-overlapping speaker candidate is at least 3 seconds long",
        );
    }
    let session_id = format!("assisted-{}", uuid::Uuid::new_v4());
    let now = now_ms();
    let session = AssistedVoiceprintSession {
        id: session_id.clone(),
        state: AssistedVoiceprintSessionState::Open,
        speaker_name,
        profile_id: request.profile_id,
        task_id: request.task_id,
        file_key: request.file_key,
        source_path: record.source_path.clone(),
        diarization_profile: DEFAULT_DIARIZATION_PROFILE.to_string(),
        sample_rate: VOICEPRINT_SAMPLE_RATE,
        candidates,
        created_at_ms: now,
        updated_at_ms: now,
    };
    if let Err(error) = std::fs::create_dir_all(assisted_voiceprint_session_dir(&session_id)) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("create assisted session: {error}"));
    }
    if let Err(error) = atomic_json_write(&assisted_voiceprint_session_path(&session_id), &session) {
        let _ = std::fs::remove_dir_all(assisted_voiceprint_session_dir(&session_id));
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    json_response_with_status(StatusCode::CREATED, &assisted_voiceprint_session_payload(&session))
}

fn get_assisted_voiceprint_session_response(session_id: &str) -> Response<BoxBody> {
    match read_assisted_voiceprint_session(session_id) {
        Ok(session) => json_response(&assisted_voiceprint_session_payload(&session)),
        Err(error) if error.starts_with("invalid") => error_response(StatusCode::BAD_REQUEST, &error),
        Err(error) => error_response(StatusCode::NOT_FOUND, &error),
    }
}

async fn post_assisted_voiceprint_labels_response(
    session_id: &str,
    req: Request<Incoming>,
) -> Response<BoxBody> {
    let request = match read_json_body::<AssistedVoiceprintLabelsRequest>(req).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let _guard = match ASSISTED_SESSION_MUTATION_LOCK.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "assisted session mutation lock is poisoned",
            )
        }
    };
    let mut session = match read_assisted_voiceprint_session(session_id) {
        Ok(session) => session,
        Err(error) => return error_response(StatusCode::NOT_FOUND, &error),
    };
    if session.state != AssistedVoiceprintSessionState::Open {
        return error_response(StatusCode::CONFLICT, "assisted voiceprint session is finishing");
    }
    let candidate_ids = session
        .candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect::<HashSet<_>>();
    if request
        .labels
        .iter()
        .any(|update| !candidate_ids.contains(update.candidate_id.as_str()))
    {
        return error_response(StatusCode::BAD_REQUEST, "unknown assisted candidate_id");
    }
    for update in request.labels {
        if let Some(candidate) = session
            .candidates
            .iter_mut()
            .find(|candidate| candidate.id == update.candidate_id)
        {
            candidate.label = update.label;
        }
    }
    session.updated_at_ms = now_ms();
    if let Err(error) = atomic_json_write(&assisted_voiceprint_session_path(session_id), &session) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    json_response(&assisted_voiceprint_session_payload(&session))
}

fn delete_assisted_voiceprint_session_response(session_id: &str) -> Response<BoxBody> {
    if validate_profile_id(session_id).is_err() {
        return error_response(StatusCode::BAD_REQUEST, "invalid assisted session id");
    }
    let _guard = match ASSISTED_SESSION_MUTATION_LOCK.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "assisted session mutation lock is poisoned",
            )
        }
    };
    let path = assisted_voiceprint_session_dir(session_id);
    if !path.exists() {
        return error_response(StatusCode::NOT_FOUND, "assisted voiceprint session not found");
    }
    if read_assisted_voiceprint_session(session_id)
        .is_ok_and(|session| session.state == AssistedVoiceprintSessionState::Finishing)
    {
        return error_response(StatusCode::CONFLICT, "assisted voiceprint session is finishing");
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => json_response(&serde_json::json!({ "deleted": true, "session_id": session_id })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("delete assisted session: {error}")),
    }
}

async fn post_assisted_voiceprint_finish_response(session_id: &str) -> Response<BoxBody> {
    let session = match begin_assisted_voiceprint_finish(session_id) {
        Ok(session) => session,
        Err((status, error)) => return error_response(status, &error),
    };
    if !session.source_path.is_file() {
        restore_assisted_voiceprint_session(&session);
        return error_response(StatusCode::CONFLICT, "ASR task source audio is no longer available");
    }
    for candidate in session
        .candidates
        .iter()
        .filter(|candidate| candidate.label == AssistedCandidateLabel::Mine)
    {
        let audio_path = assisted_voiceprint_candidate_audio_path(session_id, &candidate.id);
        if let Err(error) = ffmpeg_cut_pcm16le_ms(
            &session.source_path,
            &audio_path,
            candidate.start_ms,
            candidate.end_ms,
        )
        .await
        {
            restore_assisted_voiceprint_session(&session);
            return error_response(StatusCode::CONFLICT, &error);
        }
    }
    match finish_assisted_voiceprint_enrollment(&session) {
        Ok(response) => json_response(&response),
        Err(error) => {
            restore_assisted_voiceprint_session(&session);
            error_response(StatusCode::BAD_REQUEST, &error)
        }
    }
}

fn begin_assisted_voiceprint_finish(
    session_id: &str,
) -> Result<AssistedVoiceprintSession, (StatusCode, String)> {
    let _guard = ASSISTED_SESSION_MUTATION_LOCK.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "assisted session mutation lock is poisoned".to_string(),
        )
    })?;
    let mut session = read_assisted_voiceprint_session(session_id)
        .map_err(|error| (StatusCode::NOT_FOUND, error))?;
    if session.state != AssistedVoiceprintSessionState::Open {
        return Err((
            StatusCode::CONFLICT,
            "assisted voiceprint session is already finishing".to_string(),
        ));
    }
    let (selected_count, selected_duration_ms) = assisted_voiceprint_selection(&session);
    if selected_count < ASSISTED_ENROLLMENT_MIN_CLIPS
        || selected_duration_ms < ASSISTED_ENROLLMENT_MIN_TOTAL_MS
    {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "select at least {ASSISTED_ENROLLMENT_MIN_CLIPS} clips and {ASSISTED_ENROLLMENT_MIN_TOTAL_MS}ms of speech"
            ),
        ));
    }
    session.state = AssistedVoiceprintSessionState::Finishing;
    session.updated_at_ms = now_ms();
    atomic_json_write(&assisted_voiceprint_session_path(session_id), &session)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    Ok(session)
}

fn restore_assisted_voiceprint_session(session: &AssistedVoiceprintSession) {
    let Ok(_guard) = ASSISTED_SESSION_MUTATION_LOCK.lock() else {
        return;
    };
    let Ok(mut current) = read_assisted_voiceprint_session(&session.id) else {
        return;
    };
    current.state = AssistedVoiceprintSessionState::Open;
    current.updated_at_ms = now_ms();
    if let Err(error) = atomic_json_write(&assisted_voiceprint_session_path(&session.id), &current) {
        tracing::warn!(
            session_id = %session.id,
            error = %error,
            "failed to restore assisted voiceprint session after finish error"
        );
    }
    for candidate in current.candidates {
        let _ = std::fs::remove_file(assisted_voiceprint_candidate_audio_path(
            &session.id,
            &candidate.id,
        ));
    }
}

async fn post_speaker_enrollment_session_response(req: Request<Incoming>) -> Response<BoxBody> {
    let request = match read_json_body::<SpeakerEnrollmentCreateRequest>(req).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let name = request.name.trim();
    if name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "speaker name is required");
    }
    if let Err(error) = validate_profile_id(&request.diarization_profile) {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }
    if let Err(error) = ensure_diarization_profile_ready_for_voiceprint(&request.diarization_profile)
    {
        return error_response(StatusCode::CONFLICT, &error);
    }
    let session_id = format!("enroll-{}", uuid::Uuid::new_v4());
    let now = now_ms();
    let session = SpeakerEnrollmentSession {
        id: session_id.clone(),
        speaker_name: name.to_string(),
        diarization_profile: request.diarization_profile,
        sample_rate: VOICEPRINT_SAMPLE_RATE,
        audio_format: "pcm_s16le_mono".to_string(),
        prompts: voiceprint_prompts(),
        created_at_ms: now,
        updated_at_ms: now,
    };
    let session_dir = speaker_enrollment_session_dir(&session_id);
    if let Err(error) = std::fs::create_dir_all(&session_dir) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("create enrollment session dir: {error}"),
        );
    }
    if let Err(error) = atomic_json_write(&session_dir.join("session.json"), &session) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    json_response_with_status(
        StatusCode::CREATED,
        &SpeakerEnrollmentSessionResponse { session },
    )
}

async fn post_speaker_enrollment_audio_response(
    session_id: &str,
    req: Request<Incoming>,
) -> Response<BoxBody> {
    let session = match read_speaker_enrollment_session(session_id) {
        Ok(session) => session,
        Err(error) => return error_response(StatusCode::NOT_FOUND, &error),
    };
    let request = match read_json_body::<SpeakerEnrollmentAudioRequest>(req).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.sample_rate != session.sample_rate {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!(
                "sample_rate must be {}, got {}",
                session.sample_rate, request.sample_rate
            ),
        );
    }
    if request.channels != 1 {
        return error_response(StatusCode::BAD_REQUEST, "only mono pcm16le audio is supported");
    }
    if !session.prompts.iter().any(|prompt| prompt.id == request.prompt_id) {
        return error_response(StatusCode::BAD_REQUEST, "unknown enrollment prompt_id");
    }
    use base64::Engine as _;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(&request.pcm16le_base64) {
        Ok(bytes) => bytes,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid pcm16le_base64 audio: {error}"),
            )
        }
    };
    if !bytes.len().is_multiple_of(2) {
        return error_response(StatusCode::BAD_REQUEST, "pcm16le audio byte length must be even");
    }
    let audio_path = speaker_audio_path(session_id, &request.prompt_id);
    if let Some(parent) = audio_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("create enrollment audio dir: {error}"),
            );
        }
    }
    use std::io::Write as _;
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&audio_path)
        .and_then(|mut file| file.write_all(&bytes))
    {
        Ok(()) => {
            let prompt_bytes = std::fs::metadata(&audio_path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            json_response(&SpeakerEnrollmentAudioResponse {
                session_id: session.id,
                prompt_id: request.prompt_id,
                received_bytes: bytes.len() as u64,
                duration_ms: pcm16_duration_ms(prompt_bytes, session.sample_rate),
                total_duration_ms: speaker_enrollment_total_duration_ms(
                    session_id,
                    session.sample_rate,
                ),
                final_chunk: request.final_chunk,
            })
        }
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("append enrollment audio: {error}"),
        ),
    }
}

async fn post_speaker_enrollment_verify_response(
    session_id: &str,
    req: Request<Incoming>,
) -> Response<BoxBody> {
    let session = match read_speaker_enrollment_session(session_id) {
        Ok(session) => session,
        Err(error) => return error_response(StatusCode::NOT_FOUND, &error),
    };
    let request = match read_json_body::<SpeakerEnrollmentVerifyRequest>(req).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.sample_rate != session.sample_rate {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!(
                "sample_rate must be {}, got {}",
                session.sample_rate, request.sample_rate
            ),
        );
    }
    if request.channels != 1 {
        return error_response(StatusCode::BAD_REQUEST, "only mono pcm16le audio is supported");
    }
    let Some(prompt) = session
        .prompts
        .iter()
        .find(|prompt| prompt.id == request.prompt_id)
    else {
        return error_response(StatusCode::BAD_REQUEST, "unknown enrollment prompt_id");
    };
    use base64::Engine as _;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(&request.pcm16le_base64) {
        Ok(bytes) => bytes,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid pcm16le_base64 audio: {error}"),
            )
        }
    };
    if !bytes.len().is_multiple_of(2) {
        return error_response(StatusCode::BAD_REQUEST, "pcm16le audio byte length must be even");
    }
    let ready_for_verify = match voiceprint_prompt_audio_ready(&bytes, request.sample_rate) {
        Ok(ready) => ready,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    if !ready_for_verify {
        return json_response(&SpeakerEnrollmentVerifyResponse {
            prompt_id: request.prompt_id,
            transcript: String::new(),
            match_score: 0.0,
            matched: false,
        });
    }
    let transcript = match transcribe_voiceprint_prompt_pcm16(&bytes, request.sample_rate).await {
        Ok(transcript) => transcript,
        Err(error) => return error_response(StatusCode::CONFLICT, &error),
    };
    let match_score = voiceprint_prompt_match_score(&prompt.text, &transcript);
    json_response(&SpeakerEnrollmentVerifyResponse {
        prompt_id: request.prompt_id,
        transcript,
        match_score,
        matched: match_score >= VOICEPRINT_PROMPT_MATCH_THRESHOLD,
    })
}

async fn post_speaker_enrollment_finish_response(session_id: &str) -> Response<BoxBody> {
    let session = match read_speaker_enrollment_session(session_id) {
        Ok(session) => session,
        Err(error) => return error_response(StatusCode::NOT_FOUND, &error),
    };
    match finish_speaker_enrollment(&session) {
        Ok(response) => json_response(&response),
        Err(error) => error_response(StatusCode::BAD_REQUEST, &error),
    }
}

fn list_speaker_profiles_response() -> Response<BoxBody> {
    cleanup_expired_assisted_voiceprint_sessions();
    let profiles = load_speaker_voiceprint_profiles()
        .into_iter()
        .map(|profile| {
            serde_json::json!({
                "id": profile.id,
                "display_name": profile.display_name,
                "embedding_dim": profile.embedding_dim,
                "template_count": profile.templates.len().max((!profile.embedding.is_empty()) as usize),
                "prototype_count": profile.prototypes.len().max((!profile.embedding.is_empty()) as usize),
                "total_duration_ms": profile.total_duration_ms,
                "source": profile.source,
            })
        })
        .collect::<Vec<_>>();
    json_response(&serde_json::json!({ "profiles": profiles }))
}

fn cleanup_expired_assisted_voiceprint_sessions() {
    let Ok(_guard) = ASSISTED_SESSION_MUTATION_LOCK.lock() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(assisted_voiceprint_sessions_dir()) else {
        return;
    };
    let now = now_ms();
    for entry in entries.filter_map(Result::ok).filter(|entry| entry.path().is_dir()) {
        let session_path = entry.path().join("session.json");
        let Some(session) = std::fs::read_to_string(&session_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<AssistedVoiceprintSession>(&raw).ok())
        else {
            continue;
        };
        if now.saturating_sub(session.updated_at_ms) >= ASSISTED_SESSION_TTL_MS {
            if let Err(error) = std::fs::remove_dir_all(entry.path()) {
                tracing::warn!(
                    session_id = %session.id,
                    error = %error,
                    "failed to clean expired assisted voiceprint session"
                );
            }
        }
    }
}

fn read_speaker_voiceprint_profile(profile_id: &str) -> Result<SpeakerVoiceprintProfile, String> {
    validate_profile_id(profile_id)?;
    let path = speaker_profile_path(profile_id);
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| format!("speaker profile not found: {error}"))?;
    serde_json::from_str(&raw).map_err(|error| format!("read speaker profile: {error}"))
}

fn get_speaker_profile_response(profile_id: &str) -> Response<BoxBody> {
    if validate_profile_id(profile_id).is_err() {
        return error_response(StatusCode::BAD_REQUEST, "invalid speaker profile id");
    }
    match read_speaker_voiceprint_profile(profile_id) {
        Ok(profile) => json_response(&profile),
        Err(error) => error_response(StatusCode::NOT_FOUND, &error),
    }
}

fn delete_speaker_profile_sample_response(
    profile_id: &str,
    sample_id: &str,
) -> Response<BoxBody> {
    if validate_profile_id(profile_id).is_err() || validate_profile_id(sample_id).is_err() {
        return error_response(StatusCode::BAD_REQUEST, "invalid speaker profile or sample id");
    }
    let _guard = match SPEAKER_PROFILE_MUTATION_LOCK.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "speaker profile mutation lock is poisoned",
            )
        }
    };
    let mut profile = match read_speaker_voiceprint_profile(profile_id) {
        Ok(profile) => profile,
        Err(error) => return error_response(StatusCode::NOT_FOUND, &error),
    };
    migrate_legacy_profile_templates(&mut profile);
    let Some(index) = profile
        .templates
        .iter()
        .position(|template| template.id == sample_id)
    else {
        return error_response(StatusCode::NOT_FOUND, "speaker profile sample not found");
    };
    if profile.templates.len() == 1 {
        return error_response(
            StatusCode::CONFLICT,
            "cannot delete the last speaker profile sample",
        );
    }
    let removed = profile.templates.remove(index);
    if let Some(prompt_id) = removed.prompt_id.as_deref() {
        profile.samples.retain(|sample| sample.prompt_id != prompt_id);
    }
    if let Err(error) = rebuild_voiceprint_profile(&mut profile) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    match atomic_json_write(&speaker_profile_path(profile_id), &profile) {
        Ok(()) => json_response(&serde_json::json!({
            "deleted": true,
            "sample_id": sample_id,
            "profile": profile,
        })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn delete_speaker_profile_response(profile_id: &str) -> Response<BoxBody> {
    if validate_profile_id(profile_id).is_err() {
        return error_response(StatusCode::BAD_REQUEST, "invalid speaker profile id");
    }
    let path = speaker_profile_path(profile_id);
    if !path.exists() {
        return error_response(StatusCode::NOT_FOUND, "speaker profile not found");
    }
    let _guard = match SPEAKER_PROFILE_MUTATION_LOCK.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "speaker profile mutation lock is poisoned",
            )
        }
    };
    match std::fs::remove_file(&path) {
        Ok(()) => json_response(&serde_json::json!({
            "deleted": true,
            "profile_id": profile_id,
        })),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("delete speaker profile: {error}"),
        ),
    }
}

async fn post_speaker_voice_identify_response(req: Request<Incoming>) -> Response<BoxBody> {
    let request = match read_json_body::<SpeakerVoiceIdentifyRequest>(req).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.sample_rate != VOICEPRINT_SAMPLE_RATE {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!(
                "sample_rate must be {}, got {}",
                VOICEPRINT_SAMPLE_RATE, request.sample_rate
            ),
        );
    }
    if request.channels != 1 {
        return error_response(StatusCode::BAD_REQUEST, "only mono pcm16le audio is supported");
    }
    use base64::Engine as _;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(&request.pcm16le_base64) {
        Ok(bytes) => bytes,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid pcm16le_base64 audio: {error}"),
            )
        }
    };
    if !bytes.len().is_multiple_of(2) {
        return error_response(StatusCode::BAD_REQUEST, "pcm16le audio byte length must be even");
    }
    match identify_speaker_voice_pcm16(&bytes, request.sample_rate) {
        Ok(response) => json_response(&response),
        Err(error) => error_response(StatusCode::CONFLICT, &error),
    }
}

#[derive(Debug)]
struct PreparedVoiceIdentifyAudio {
    waveform: Vec<f32>,
    audio_duration_ms: u64,
    speech_duration_ms: u64,
}

#[derive(Debug)]
struct VoiceIdentifyAudioPreparation {
    ready: Option<PreparedVoiceIdentifyAudio>,
    audio_duration_ms: u64,
    speech_duration_ms: u64,
}

fn prepare_voiceprint_identify_audio(
    pcm16le: &[u8],
    sample_rate: u32,
) -> Result<VoiceIdentifyAudioPreparation, String> {
    let audio_duration_ms = pcm16_duration_ms(pcm16le.len() as u64, sample_rate);
    let waveform = pcm16le_to_f32(pcm16le)?;
    let Some((start, end)) = active_speech_bounds(
        &waveform,
        sample_rate,
        VOICEPRINT_IDENTIFY_SPEECH_RMS,
    ) else {
        return Ok(VoiceIdentifyAudioPreparation {
            ready: None,
            audio_duration_ms,
            speech_duration_ms: 0,
        });
    };
    let speech_duration_ms = ((end.saturating_sub(start)) as u64)
        .saturating_mul(1_000)
        / u64::from(sample_rate.max(1));
    if speech_duration_ms < VOICEPRINT_MIN_IDENTIFY_SPEECH_MS {
        return Ok(VoiceIdentifyAudioPreparation {
            ready: None,
            audio_duration_ms,
            speech_duration_ms,
        });
    }
    Ok(VoiceIdentifyAudioPreparation {
        ready: Some(PreparedVoiceIdentifyAudio {
            waveform: waveform[start..end].to_vec(),
            audio_duration_ms,
            speech_duration_ms,
        }),
        audio_duration_ms,
        speech_duration_ms,
    })
}

fn active_speech_bounds(
    waveform: &[f32],
    sample_rate: u32,
    rms_threshold: f32,
) -> Option<(usize, usize)> {
    if waveform.is_empty() || sample_rate == 0 {
        return None;
    }
    let frame_size = ((u64::from(sample_rate) * 30) / 1_000).max(1) as usize;
    let padding = ((u64::from(sample_rate) * 120) / 1_000).max(1) as usize;
    let mut first = None;
    let mut last = None;
    for start in (0..waveform.len()).step_by(frame_size) {
        let end = (start + frame_size).min(waveform.len());
        let frame = &waveform[start..end];
        let rms = (frame.iter().map(|sample| sample * sample).sum::<f32>()
            / frame.len() as f32)
            .sqrt();
        if rms >= rms_threshold {
            first.get_or_insert(start);
            last = Some(end);
        }
    }
    let (first, last) = (first?, last?);
    Some((
        first.saturating_sub(padding),
        (last + padding).min(waveform.len()),
    ))
}

async fn transcribe_voiceprint_prompt_pcm16(
    pcm16le: &[u8],
    sample_rate: u32,
) -> Result<String, String> {
    let requested_target =
        target_from_query(Some("model=Qwen3-ASR-0.6B&owner_module=speech_workbench"))?;
    let target = match start_managed_service(requested_target.clone()).await {
        Ok(_) => resolve_managed_target(requested_target).await,
        Err(response) => {
            return Err(format!(
                "{}{}",
                response.message,
                response
                    .detail
                    .as_deref()
                    .map(|detail| format!(" {detail}"))
                    .unwrap_or_default()
            ))
        }
    };
    let Some(server_url) = target.server_url() else {
        return Err("Qwen3-ASR-0.6B service did not expose a local port".to_string());
    };
    let tmp_dir =
        tempfile::tempdir().map_err(|error| format!("create voiceprint ASR temp dir: {error}"))?;
    let wav_path = tmp_dir.path().join("voiceprint_prompt.wav");
    let wav_bytes = pcm16le_wav_bytes(pcm16le, sample_rate, 1)?;
    std::fs::write(&wav_path, wav_bytes)
        .map_err(|error| format!("write voiceprint prompt WAV: {error}"))?;
    call_asr_text_endpoint(&server_url, &target.language, &wav_path)
        .await
        .map(|text| clean_voiceprint_asr_text(&text))
}

fn voiceprint_prompt_audio_ready(pcm16le: &[u8], sample_rate: u32) -> Result<bool, String> {
    let duration_ms = pcm16_duration_ms(pcm16le.len() as u64, sample_rate);
    if duration_ms < VOICEPRINT_MIN_PROMPT_VERIFY_MS {
        return Ok(false);
    }
    let waveform = pcm16le_to_f32(pcm16le)?;
    let (rms, _) = voiceprint_sample_stats(&waveform, sample_rate);
    Ok(rms >= VOICEPRINT_MIN_PROMPT_RMS)
}

fn pcm16le_wav_bytes(pcm16le: &[u8], sample_rate: u32, channels: u16) -> Result<Vec<u8>, String> {
    let data_len = u32::try_from(pcm16le.len())
        .map_err(|_| "voiceprint prompt audio is too large".to_string())?;
    let byte_rate = sample_rate
        .checked_mul(u32::from(channels))
        .and_then(|value| value.checked_mul(2))
        .ok_or_else(|| "invalid voiceprint prompt WAV rate".to_string())?;
    let block_align = channels
        .checked_mul(2)
        .ok_or_else(|| "invalid voiceprint prompt WAV block align".to_string())?;
    let riff_len = 36u32
        .checked_add(data_len)
        .ok_or_else(|| "voiceprint prompt WAV is too large".to_string())?;
    let mut wav = Vec::with_capacity(44 + pcm16le.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_len.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm16le);
    Ok(wav)
}

fn read_speaker_enrollment_session(session_id: &str) -> Result<SpeakerEnrollmentSession, String> {
    if validate_profile_id(session_id).is_err() {
        return Err("invalid enrollment session id".to_string());
    }
    let path = speaker_enrollment_session_dir(session_id).join("session.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| format!("speaker enrollment session not found: {error}"))?;
    serde_json::from_str::<SpeakerEnrollmentSession>(&raw)
        .map_err(|error| format!("read speaker enrollment session: {error}"))
}

fn finish_speaker_enrollment(
    session: &SpeakerEnrollmentSession,
) -> Result<SpeakerEnrollmentFinishResponse, String> {
    #[cfg(test)]
    {
        finish_speaker_enrollment_in_process(session)
    }
    #[cfg(not(test))]
    {
        run_asr_diarization_worker_request(AsrDiarizationWorkerRequest::FinishEnrollment {
            session_id: session.id.clone(),
        })
        .and_then(|response| match response {
            AsrDiarizationWorkerResponse::FinishEnrollment { response } => Ok(response),
            other => Err(format!("unexpected ASR diarization worker response: {other:?}")),
        })
    }
}

fn finish_speaker_enrollment_in_process(
    session: &SpeakerEnrollmentSession,
) -> Result<SpeakerEnrollmentFinishResponse, String> {
    let mut samples = Vec::new();
    let mut templates = Vec::new();
    for prompt in &session.prompts {
        let audio_path = speaker_audio_path(&session.id, &prompt.id);
        let bytes = std::fs::read(&audio_path).unwrap_or_default();
        if bytes.is_empty() {
            continue;
        }
        let prompt_waveform = pcm16le_to_f32(&bytes)?;
        let stats = voiceprint_sample_stats(&prompt_waveform, session.sample_rate);
        let embedding = compute_speaker_embedding(
            &session.diarization_profile,
            &prompt_waveform,
        )?;
        let now = now_ms();
        templates.push(SpeakerVoiceprintTemplate {
            id: format!("sample-{}", uuid::Uuid::new_v4()),
            source_kind: "live_prompt".to_string(),
            prompt_id: Some(prompt.id.clone()),
            task_id: None,
            file_key: None,
            speaker: None,
            start_ms: None,
            end_ms: None,
            duration_ms: pcm16_duration_ms(bytes.len() as u64, session.sample_rate),
            quality: (1.0 - stats.1).clamp(0.0, 1.0),
            overlap: false,
            embedding,
            created_at_ms: now,
        });
        samples.push(SpeakerVoiceprintSample {
            prompt_id: prompt.id.clone(),
            text: prompt.text.clone(),
            duration_ms: pcm16_duration_ms(bytes.len() as u64, session.sample_rate),
            rms: stats.0,
            clipped_ratio: stats.1,
        });
    }
    let total_duration_ms = samples.iter().map(|sample| sample.duration_ms).sum::<u64>();
    if total_duration_ms < VOICEPRINT_MIN_TOTAL_MS {
        return Err(format!(
            "voiceprint audio is too short: {total_duration_ms}ms, expected at least {VOICEPRINT_MIN_TOTAL_MS}ms"
        ));
    }
    let embedding = average_speaker_embeddings(
        &templates
            .iter()
            .map(|template| template.embedding.clone())
            .collect::<Vec<_>>(),
    )
        .ok_or_else(|| "voiceprint enrollment has no speaker embedding".to_string())?;
    let profile_id = format!("spk-{}", uuid::Uuid::new_v4());
    let now = now_ms();
    let embedding_model = sherpa_model_pack_paths(&session.diarization_profile)
        .embedding_model
        .to_string_lossy()
        .into_owned();
    let profile = SpeakerVoiceprintProfile {
        schema_version: VOICEPRINT_PROFILE_SCHEMA_VERSION,
        id: profile_id.clone(),
        display_name: session.speaker_name.clone(),
        source: "live_enrollment".to_string(),
        diarization_profile: session.diarization_profile.clone(),
        embedding_model,
        embedding_dim: embedding.len(),
        embedding,
        sample_rate: session.sample_rate,
        total_duration_ms,
        samples,
        prototypes: build_voiceprint_prototypes(&templates)?,
        templates,
        created_at_ms: now,
        updated_at_ms: now,
    };
    std::fs::create_dir_all(voiceprint_dir())
        .map_err(|error| format!("create speaker profile dir: {error}"))?;
    let profile_path = speaker_profile_path(&profile_id);
    atomic_json_write(&profile_path, &profile)?;
    let _ = std::fs::remove_dir_all(speaker_enrollment_session_dir(&session.id));
    Ok(SpeakerEnrollmentFinishResponse {
        profile,
        profile_path,
    })
}

fn finish_assisted_voiceprint_enrollment(
    session: &AssistedVoiceprintSession,
) -> Result<SpeakerEnrollmentFinishResponse, String> {
    #[cfg(test)]
    {
        let templates = compute_assisted_voiceprint_templates(session)?;
        persist_assisted_voiceprint_templates(session, templates)
    }
    #[cfg(not(test))]
    {
        let templates = run_asr_diarization_worker_request(
            AsrDiarizationWorkerRequest::FinishAssistedEnrollment {
                session_id: session.id.clone(),
            },
        )
        .and_then(|response| match response {
            AsrDiarizationWorkerResponse::AssistedEnrollmentTemplates { templates } => {
                Ok(templates)
            }
            other => Err(format!("unexpected ASR diarization worker response: {other:?}")),
        })?;
        persist_assisted_voiceprint_templates(session, templates)
    }
}

#[cfg(test)]
fn finish_assisted_voiceprint_enrollment_in_process(
    session: &AssistedVoiceprintSession,
) -> Result<SpeakerEnrollmentFinishResponse, String> {
    let templates = compute_assisted_voiceprint_templates(session)?;
    persist_assisted_voiceprint_templates(session, templates)
}

fn compute_assisted_voiceprint_templates(
    session: &AssistedVoiceprintSession,
) -> Result<Vec<SpeakerVoiceprintTemplate>, String> {
    let selected = session
        .candidates
        .iter()
        .filter(|candidate| candidate.label == AssistedCandidateLabel::Mine)
        .collect::<Vec<_>>();
    let selected_duration_ms = selected
        .iter()
        .map(|candidate| candidate.duration_ms)
        .sum::<u64>();
    if selected.len() < ASSISTED_ENROLLMENT_MIN_CLIPS
        || selected_duration_ms < ASSISTED_ENROLLMENT_MIN_TOTAL_MS
    {
        return Err(format!(
            "select at least {ASSISTED_ENROLLMENT_MIN_CLIPS} clips and {ASSISTED_ENROLLMENT_MIN_TOTAL_MS}ms of speech"
        ));
    }
    let now = now_ms();
    let mut templates = Vec::with_capacity(selected.len());
    for candidate in selected {
        if candidate.overlap || candidate.duration_ms < ASSISTED_CANDIDATE_MIN_MS {
            return Err(format!("candidate {} does not pass the quality gate", candidate.id));
        }
        let audio_path = assisted_voiceprint_candidate_audio_path(&session.id, &candidate.id);
        let bytes = std::fs::read(&audio_path)
            .map_err(|error| format!("read assisted candidate {}: {error}", candidate.id))?;
        let waveform = pcm16le_to_f32(&bytes)?;
        let actual_duration_ms = pcm16_duration_ms(bytes.len() as u64, session.sample_rate);
        if actual_duration_ms < ASSISTED_CANDIDATE_MIN_MS {
            return Err(format!("candidate {} audio is too short", candidate.id));
        }
        let (rms, clipped_ratio) = voiceprint_sample_stats(&waveform, session.sample_rate);
        if rms < VOICEPRINT_MIN_PROMPT_RMS {
            return Err(format!("candidate {} has insufficient speech energy", candidate.id));
        }
        let embedding = compute_speaker_embedding(&session.diarization_profile, &waveform)?;
        templates.push(SpeakerVoiceprintTemplate {
            id: format!("sample-{}", uuid::Uuid::new_v4()),
            source_kind: "task_segment".to_string(),
            prompt_id: None,
            task_id: Some(session.task_id.clone()),
            file_key: Some(session.file_key.clone()),
            speaker: Some(candidate.speaker.clone()),
            start_ms: Some(candidate.start_ms),
            end_ms: Some(candidate.end_ms),
            duration_ms: actual_duration_ms,
            quality: (candidate.quality * (1.0 - clipped_ratio)).clamp(0.0, 1.0),
            overlap: false,
            embedding,
            created_at_ms: now,
        });
    }
    Ok(templates)
}

fn persist_assisted_voiceprint_templates(
    session: &AssistedVoiceprintSession,
    templates: Vec<SpeakerVoiceprintTemplate>,
) -> Result<SpeakerEnrollmentFinishResponse, String> {
    let _guard = SPEAKER_PROFILE_MUTATION_LOCK
        .lock()
        .map_err(|_| "speaker profile mutation lock is poisoned".to_string())?;
    let now = now_ms();
    let mut profile = if let Some(profile_id) = session.profile_id.as_deref() {
        let mut profile = read_speaker_voiceprint_profile(profile_id)?;
        if profile.display_name != session.speaker_name {
            return Err("speaker name does not match existing profile".to_string());
        }
        if profile.diarization_profile != session.diarization_profile {
            return Err("diarization profile does not match existing speaker profile".to_string());
        }
        if profile.sample_rate != session.sample_rate {
            return Err("sample rate does not match existing speaker profile".to_string());
        }
        if profile.embedding_dim != 0
            && templates
                .iter()
                .any(|template| template.embedding.len() != profile.embedding_dim)
        {
            return Err("embedding dimension does not match existing speaker profile".to_string());
        }
        migrate_legacy_profile_templates(&mut profile);
        profile.templates.extend(templates);
        profile
    } else {
        let profile_id = format!("spk-{}", uuid::Uuid::new_v4());
        SpeakerVoiceprintProfile {
            schema_version: VOICEPRINT_PROFILE_SCHEMA_VERSION,
            id: profile_id,
            display_name: session.speaker_name.clone(),
            source: "assisted_recording".to_string(),
            diarization_profile: session.diarization_profile.clone(),
            embedding_model: sherpa_model_pack_paths(&session.diarization_profile)
                .embedding_model
                .to_string_lossy()
                .into_owned(),
            embedding_dim: 0,
            embedding: Vec::new(),
            sample_rate: session.sample_rate,
            total_duration_ms: 0,
            samples: Vec::new(),
            templates,
            prototypes: Vec::new(),
            created_at_ms: now,
            updated_at_ms: now,
        }
    };
    rebuild_voiceprint_profile(&mut profile)?;
    let profile_path = speaker_profile_path(&profile.id);
    std::fs::create_dir_all(voiceprint_dir())
        .map_err(|error| format!("create speaker profile dir: {error}"))?;
    atomic_json_write(&profile_path, &profile)?;
    drop(_guard);
    if let Err(error) = std::fs::remove_dir_all(assisted_voiceprint_session_dir(&session.id)) {
        tracing::warn!(
            session_id = %session.id,
            error = %error,
            "speaker profile saved but assisted session cleanup failed"
        );
    }
    Ok(SpeakerEnrollmentFinishResponse { profile, profile_path })
}

fn migrate_legacy_profile_templates(profile: &mut SpeakerVoiceprintProfile) {
    if profile.templates.is_empty() && !profile.embedding.is_empty() {
        profile.templates.push(SpeakerVoiceprintTemplate {
            id: format!("legacy-{}", profile.id),
            source_kind: "legacy_centroid".to_string(),
            prompt_id: None,
            task_id: None,
            file_key: None,
            speaker: None,
            start_ms: None,
            end_ms: None,
            duration_ms: profile.total_duration_ms,
            quality: 1.0,
            overlap: false,
            embedding: profile.embedding.clone(),
            created_at_ms: profile.created_at_ms,
        });
    }
}

fn rebuild_voiceprint_profile(profile: &mut SpeakerVoiceprintProfile) -> Result<(), String> {
    if profile.templates.is_empty() {
        return Err("speaker profile must retain at least one template".to_string());
    }
    let embeddings = profile
        .templates
        .iter()
        .map(|template| template.embedding.clone())
        .collect::<Vec<_>>();
    let embedding = average_speaker_embeddings(&embeddings)
        .ok_or_else(|| "speaker profile templates have incompatible embeddings".to_string())?;
    profile.schema_version = VOICEPRINT_PROFILE_SCHEMA_VERSION;
    profile.embedding_dim = embedding.len();
    profile.embedding = embedding;
    profile.total_duration_ms = profile
        .templates
        .iter()
        .map(|template| template.duration_ms)
        .sum();
    profile.prototypes = build_voiceprint_prototypes(&profile.templates)?;
    profile.updated_at_ms = now_ms();
    Ok(())
}

fn build_voiceprint_prototypes(
    templates: &[SpeakerVoiceprintTemplate],
) -> Result<Vec<SpeakerVoiceprintPrototype>, String> {
    let mut clusters = Vec::<(Vec<String>, Vec<Vec<f32>>, Vec<f32>)>::new();
    for template in templates {
        if template.embedding.is_empty() {
            return Err(format!("template {} has an empty embedding", template.id));
        }
        let best = clusters
            .iter()
            .enumerate()
            .filter_map(|(index, (_, _, centroid))| {
                cosine_similarity(&template.embedding, centroid).map(|score| (index, score))
            })
            .max_by(|(_, left), (_, right)| {
                left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some((index, _score)) = best.filter(|(_, score)| {
            *score >= VOICEPRINT_PROTOTYPE_CLUSTER_THRESHOLD
        }) {
            let cluster = &mut clusters[index];
            cluster.0.push(template.id.clone());
            cluster.1.push(template.embedding.clone());
            cluster.2 = average_speaker_embeddings(&cluster.1)
                .ok_or_else(|| "prototype embeddings have incompatible dimensions".to_string())?;
        } else {
            let mut normalized = template.embedding.clone();
            normalize_embedding(&mut normalized);
            clusters.push((
                vec![template.id.clone()],
                vec![template.embedding.clone()],
                normalized,
            ));
        }
    }
    Ok(clusters
        .into_iter()
        .enumerate()
        .map(|(index, (template_ids, _, embedding))| SpeakerVoiceprintPrototype {
            id: format!("prototype-{}", index + 1),
            template_ids,
            embedding,
        })
        .collect())
}

fn identify_speaker_voice_pcm16(
    pcm16le: &[u8],
    sample_rate: u32,
) -> Result<SpeakerVoiceIdentifyResponse, String> {
    #[cfg(test)]
    {
        identify_speaker_voice_pcm16_in_process(pcm16le, sample_rate)
    }
    #[cfg(not(test))]
    {
        let request_dir = asr_diarization_worker_request_dir()?;
        let audio_path = request_dir.join(format!("voiceprint-{}.pcm16le", uuid::Uuid::new_v4()));
        std::fs::write(&audio_path, pcm16le)
            .map_err(|error| format!("write voiceprint identify audio: {error}"))?;
        let result = run_asr_diarization_worker_request(
            AsrDiarizationWorkerRequest::IdentifyPcm16 {
                pcm16le_path: audio_path.clone(),
                sample_rate,
            },
        )
        .and_then(|response| match response {
            AsrDiarizationWorkerResponse::Identify { response } => Ok(response),
            other => Err(format!("unexpected ASR diarization worker response: {other:?}")),
        });
        let _ = std::fs::remove_file(audio_path);
        result
    }
}

fn identify_speaker_voice_pcm16_in_process(
    pcm16le: &[u8],
    sample_rate: u32,
) -> Result<SpeakerVoiceIdentifyResponse, String> {
    let preparation = prepare_voiceprint_identify_audio(pcm16le, sample_rate)?;
    let Some(prepared) = preparation.ready else {
        return Ok(insufficient_speaker_identify_response(
            preparation.audio_duration_ms,
            preparation.speech_duration_ms,
        ));
    };
    identify_speaker_voice(
        &prepared.waveform,
        prepared.audio_duration_ms,
        prepared.speech_duration_ms,
    )
}

fn average_speaker_embeddings(embeddings: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = embeddings.first()?;
    let dim = first.len();
    if dim == 0 || embeddings.iter().any(|embedding| embedding.len() != dim) {
        return None;
    }
    let mut averaged = vec![0.0; dim];
    for embedding in embeddings {
        for (index, value) in embedding.iter().enumerate() {
            averaged[index] += value;
        }
    }
    for value in &mut averaged {
        *value /= embeddings.len() as f32;
    }
    normalize_embedding(&mut averaged);
    Some(averaged)
}

fn normalize_embedding(embedding: &mut [f32]) {
    let norm = embedding
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    if norm > 0.0 {
        for value in embedding {
            *value /= norm;
        }
    }
}

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
fn compute_speaker_embedding(profile: &str, waveform: &[f32]) -> Result<Vec<f32>, String> {
    #[cfg(test)]
    {
        let _ = profile;
        Ok(test_embedding_from_waveform(waveform))
    }
    #[cfg(not(test))]
    {
        if std::env::var("BIFROST_ASR_VOICEPRINT_TEST_EMBEDDING").as_deref() == Ok("1") {
            return Ok(deterministic_embedding_from_waveform(waveform));
        }
        let embedding_model = sherpa_model_pack_paths(profile).embedding_model;
        if !model_file_has_min_size(&embedding_model, SHERPA_EMBEDDING_MIN_BYTES) {
            return Err("speaker embedding model is not initialized".to_string());
        }
        let model = embedding_model
            .to_str()
            .ok_or_else(|| "speaker embedding model path contains non-utf8 characters".to_string())?
            .to_string();
        let extractor = SpeakerEmbeddingExtractor::create(&SpeakerEmbeddingExtractorConfig {
            model: Some(model),
            num_threads: 2,
            debug: false,
            provider: Some("cpu".to_string()),
        })
        .ok_or_else(|| "create sherpa-onnx speaker embedding extractor failed".to_string())?;
        let stream = extractor
            .create_stream()
            .ok_or_else(|| "create speaker embedding stream failed".to_string())?;
        stream.accept_waveform(VOICEPRINT_SAMPLE_RATE as i32, waveform);
        stream.input_finished();
        if !extractor.is_ready(&stream) {
            return Err("speaker enrollment audio is not sufficient for embedding".to_string());
        }
        extractor
            .compute(&stream)
            .ok_or_else(|| "compute speaker embedding failed".to_string())
    }
}

#[cfg(all(not(test), not(all(target_os = "macos", target_arch = "aarch64"))))]
fn compute_speaker_embedding(_profile: &str, waveform: &[f32]) -> Result<Vec<f32>, String> {
    if std::env::var("BIFROST_ASR_VOICEPRINT_TEST_EMBEDDING").as_deref() == Ok("1") {
        return Ok(deterministic_embedding_from_waveform(waveform));
    }
    Err(format!(
        "speaker_embedding_unsupported_platform: sherpa-onnx is not available for {}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn compute_diarization_speaker_embeddings(
    embedding_model: &Path,
    waveform: &[f32],
    sample_rate: i32,
    segments: &[DiarizationSegment],
) -> Result<BTreeMap<String, Vec<f32>>, String> {
    let speaker_waveforms = collect_diarization_speaker_waveforms(waveform, sample_rate, segments);
    #[cfg(test)]
    {
        let _ = embedding_model;
        Ok(speaker_waveforms
            .into_iter()
            .map(|(speaker, samples)| (speaker, test_embedding_from_waveform(&samples)))
            .collect())
    }
    #[cfg(not(test))]
    {
        if std::env::var("BIFROST_ASR_VOICEPRINT_TEST_EMBEDDING").as_deref() == Ok("1") {
            return Ok(speaker_waveforms
                .into_iter()
                .map(|(speaker, samples)| {
                    (speaker, deterministic_embedding_from_waveform(&samples))
                })
                .collect());
        }
        if !model_file_has_min_size(embedding_model, SHERPA_EMBEDDING_MIN_BYTES) {
            return Err("speaker embedding model is not initialized".to_string());
        }
        let model = embedding_model
            .to_str()
            .ok_or_else(|| "speaker embedding model path contains non-utf8 characters".to_string())?
            .to_string();
        let extractor = SpeakerEmbeddingExtractor::create(&SpeakerEmbeddingExtractorConfig {
            model: Some(model),
            num_threads: 2,
            debug: false,
            provider: Some("cpu".to_string()),
        })
        .ok_or_else(|| "create sherpa-onnx speaker embedding extractor failed".to_string())?;
        let mut embeddings = BTreeMap::new();
        for (speaker, samples) in speaker_waveforms {
            let stream = extractor
                .create_stream()
                .ok_or_else(|| "create speaker embedding stream failed".to_string())?;
            stream.accept_waveform(sample_rate, &samples);
            stream.input_finished();
            if extractor.is_ready(&stream) {
                if let Some(embedding) = extractor.compute(&stream) {
                    embeddings.insert(speaker, embedding);
                }
            }
        }
        Ok(embeddings)
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn collect_diarization_speaker_waveforms(
    waveform: &[f32],
    sample_rate: i32,
    segments: &[DiarizationSegment],
) -> BTreeMap<String, Vec<f32>> {
    let mut by_speaker = BTreeMap::<String, Vec<f32>>::new();
    let sample_rate = sample_rate.max(1) as u64;
    for segment in segments {
        if segment.overlap {
            continue;
        }
        let start = (segment.start_ms.saturating_mul(sample_rate) / 1_000) as usize;
        let end = (segment.end_ms.saturating_mul(sample_rate) / 1_000) as usize;
        if start >= waveform.len() || end <= start {
            continue;
        }
        let end = end.min(waveform.len());
        by_speaker
            .entry(segment.speaker.clone())
            .or_default()
            .extend_from_slice(&waveform[start..end]);
    }
    by_speaker
}

#[cfg(test)]
fn test_embedding_from_waveform(waveform: &[f32]) -> Vec<f32> {
    deterministic_embedding_from_waveform(waveform)
}

fn deterministic_embedding_from_waveform(waveform: &[f32]) -> Vec<f32> {
    let mut buckets = vec![0.0; 16];
    for (index, sample) in waveform.iter().enumerate() {
        buckets[index % 16] += sample.abs();
    }
    let norm = buckets.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut buckets {
            *value /= norm;
        }
    }
    buckets
}

fn voiceprint_prompt_match_score(prompt: &str, transcript: &str) -> f32 {
    let expected = normalize_voiceprint_text(prompt);
    let actual = normalize_voiceprint_text(&clean_voiceprint_asr_text(transcript));
    if expected.is_empty() || actual.is_empty() {
        return 0.0;
    }
    if actual.contains(&expected) {
        return 1.0;
    }
    longest_common_subsequence_len(&expected, &actual) as f32 / expected.chars().count() as f32
}

fn clean_voiceprint_asr_text(value: &str) -> String {
    let mut cleaned = value.trim().to_string();
    for tag in [
        "<asr_text>",
        "</asr_text>",
        "<|startoftranscript|>",
        "<|endoftext|>",
        "<|zh|>",
        "<|transcribe|>",
        "<|timestamps|>",
        "<|notimestamps|>",
    ] {
        cleaned = cleaned.replace(tag, "");
    }
    cleaned
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn normalize_voiceprint_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|ch| ch.is_alphanumeric())
        .collect()
}

fn longest_common_subsequence_len(left: &str, right: &str) -> usize {
    let left_chars = left.chars().collect::<Vec<_>>();
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = vec![0usize; right_chars.len() + 1];
    let mut current = vec![0usize; right_chars.len() + 1];
    for left_char in &left_chars {
        for (right_index, right_char) in right_chars.iter().enumerate() {
            current[right_index + 1] = if left_char == right_char {
                previous[right_index] + 1
            } else {
                previous[right_index + 1].max(current[right_index])
            };
        }
        std::mem::swap(&mut previous, &mut current);
        current.fill(0);
    }
    previous[right_chars.len()]
}

fn pcm16le_to_f32(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("pcm16le audio byte length must be even".to_string());
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            sample as f32 / i16::MAX as f32
        })
        .collect())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn f32_waveform_to_pcm16le(waveform: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(waveform.len().saturating_mul(2));
    for sample in waveform {
        let normalized = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let pcm = (normalized * i16::MAX as f32).round() as i16;
        bytes.extend_from_slice(&pcm.to_le_bytes());
    }
    bytes
}

fn voiceprint_sample_stats(waveform: &[f32], _sample_rate: u32) -> (f32, f32) {
    if waveform.is_empty() {
        return (0.0, 0.0);
    }
    let sum_sq = waveform.iter().map(|sample| sample * sample).sum::<f32>();
    let clipped = waveform
        .iter()
        .filter(|sample| sample.abs() >= 0.98)
        .count();
    (
        (sum_sq / waveform.len() as f32).sqrt(),
        clipped as f32 / waveform.len() as f32,
    )
}

fn pcm16_duration_ms(bytes: u64, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    bytes / 2 * 1_000 / u64::from(sample_rate)
}

fn speaker_enrollment_total_duration_ms(session_id: &str, sample_rate: u32) -> u64 {
    let Ok(session) = read_speaker_enrollment_session(session_id) else {
        return 0;
    };
    session
        .prompts
        .iter()
        .map(|prompt| {
            std::fs::metadata(speaker_audio_path(session_id, &prompt.id))
                .map(|metadata| pcm16_duration_ms(metadata.len(), sample_rate))
                .unwrap_or(0)
        })
        .sum()
}

fn load_speaker_voiceprint_profiles() -> Vec<SpeakerVoiceprintProfile> {
    std::fs::read_dir(voiceprint_dir())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
                .filter_map(|raw| serde_json::from_str::<SpeakerVoiceprintProfile>(&raw).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn load_registered_speaker_profiles() -> Vec<RegisteredSpeakerProfile> {
    load_speaker_voiceprint_profiles()
        .into_iter()
        .filter_map(|profile| {
            let mut embeddings = profile
                .prototypes
                .into_iter()
                .filter_map(|prototype| (!prototype.embedding.is_empty()).then_some(prototype.embedding))
                .collect::<Vec<_>>();
            if !profile.embedding.is_empty() {
                embeddings.push(profile.embedding);
            }
            (!embeddings.is_empty()).then_some(RegisteredSpeakerProfile {
                id: profile.id,
                display_name: profile.display_name,
                embeddings,
            })
        })
        .collect()
}

fn registered_speaker_profile_score(
    embedding: &[f32],
    profile: &RegisteredSpeakerProfile,
) -> Option<f32> {
    profile
        .embeddings
        .iter()
        .filter_map(|prototype| cosine_similarity(embedding, prototype))
        .max_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal))
}

pub(crate) fn registered_speaker_profile_exists(profile_id: &str) -> bool {
    load_registered_speaker_profiles()
        .iter()
        .any(|profile| profile.id == profile_id)
}

const VOICEPRINT_SELF_PRIORITY_THRESHOLD: f32 = 0.52;
#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
const VOICEPRINT_SELF_PRIORITY_MIN_DURATION_MS: u64 = 5_000;

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
#[derive(Debug, Clone)]
struct DiarizationVoiceprintCandidate {
    profile_id: String,
    display_name: String,
    score: f32,
    conflicted: bool,
}

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
fn map_speakers_with_registered_voiceprints(
    diarization_segments: &mut [DiarizationSegment],
    speaker_embeddings: &BTreeMap<String, Vec<f32>>,
) {
    let profiles = load_registered_speaker_profiles();
    if profiles.is_empty() {
        return;
    }
    let mut candidates = BTreeMap::<String, DiarizationVoiceprintCandidate>::new();
    for (speaker, embedding) in speaker_embeddings {
        let mut ranked = profiles
            .iter()
            .filter_map(|profile| {
                registered_speaker_profile_score(embedding, profile).map(|score| (profile, score))
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|(left_profile, left), (right_profile, right)| {
            right
                .partial_cmp(left)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left_profile.id.cmp(&right_profile.id))
        });
        let Some((profile, score)) = ranked.first().copied() else {
            continue;
        };
        let conflicted = ranked.get(1).is_some_and(|(_, runner_up)| {
            score - *runner_up < VOICEPRINT_PROFILE_CONFLICT_MARGIN
        });
        candidates.insert(
            speaker.clone(),
            DiarizationVoiceprintCandidate {
                profile_id: profile.id.clone(),
                display_name: profile.display_name.clone(),
                score,
                conflicted,
            },
        );
    }
    let self_priority_speaker = if profiles.len() == 1 {
        let durations = diarization_speaker_durations(diarization_segments);
        candidates
            .iter()
            .filter(|(speaker, candidate)| {
                candidate.score >= VOICEPRINT_SELF_PRIORITY_THRESHOLD
                    && !candidate.conflicted
                    && durations.get(*speaker).is_some_and(|duration| {
                        *duration >= VOICEPRINT_SELF_PRIORITY_MIN_DURATION_MS
                    })
            })
            .max_by(|(_, left), (_, right)| {
                left.score
                    .partial_cmp(&right.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(speaker, _)| speaker.clone())
    } else {
        None
    };
    for (speaker, candidate) in &candidates {
        let self_priority_matched = self_priority_speaker
            .as_ref()
            .is_some_and(|matched_speaker| matched_speaker == speaker);
        tracing::info!(
            target: "bifrost_admin::asr_jobs",
            speaker = %speaker,
            profile_id = %candidate.profile_id,
            profile_name = %candidate.display_name,
            confidence = candidate.score,
            matched = !candidate.conflicted && (candidate.score >= VOICEPRINT_SPEAKER_MATCH_THRESHOLD || self_priority_matched),
            conflicted = candidate.conflicted,
            self_priority_matched = self_priority_matched,
            threshold = VOICEPRINT_SPEAKER_MATCH_THRESHOLD,
            "evaluated diarization speaker voiceprint candidate"
        );
    }
    for segment in diarization_segments {
        let Some(candidate) = candidates.get(&segment.speaker) else {
            continue;
        };
        segment.candidate_profile_id = Some(candidate.profile_id.clone());
        segment.candidate_display_name = Some(candidate.display_name.clone());
        segment.candidate_confidence = Some(candidate.score);
        let self_priority_matched = self_priority_speaker
            .as_ref()
            .is_some_and(|speaker| speaker == &segment.speaker);
        if !candidate.conflicted
            && (candidate.score >= VOICEPRINT_SPEAKER_MATCH_THRESHOLD || self_priority_matched)
        {
            segment.display_name = candidate.display_name.clone();
            segment.mapped_profile_id = Some(candidate.profile_id.clone());
            segment.confidence = Some(candidate.score);
        }
    }
}

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
fn diarization_speaker_durations(segments: &[DiarizationSegment]) -> BTreeMap<String, u64> {
    let mut durations = BTreeMap::new();
    for segment in segments {
        *durations.entry(segment.speaker.clone()).or_default() +=
            segment.end_ms.saturating_sub(segment.start_ms);
    }
    durations
}

fn identify_speaker_voice(
    waveform: &[f32],
    audio_duration_ms: u64,
    speech_duration_ms: u64,
) -> Result<SpeakerVoiceIdentifyResponse, String> {
    ensure_diarization_profile_ready_for_voiceprint(DEFAULT_DIARIZATION_PROFILE)?;
    let embedding = compute_speaker_embedding(DEFAULT_DIARIZATION_PROFILE, waveform)?;
    let Some((profile, confidence, conflicted)) = best_registered_speaker_match(&embedding) else {
        return Ok(unknown_speaker_identify_response(
            0.0,
            audio_duration_ms,
            speech_duration_ms,
        ));
    };
    Ok(SpeakerVoiceIdentifyResponse {
        matched: confidence >= VOICEPRINT_SPEAKER_MATCH_THRESHOLD && !conflicted,
        profile_id: Some(profile.id),
        display_name: profile.display_name,
        speaker: "speaker_00".to_string(),
        confidence,
        status: if confidence >= VOICEPRINT_SPEAKER_MATCH_THRESHOLD && !conflicted {
            "matched".to_string()
        } else if conflicted {
            "ambiguous".to_string()
        } else {
            "unmatched".to_string()
        },
        reason: None,
        audio_duration_ms,
        speech_duration_ms,
    })
}

fn ensure_diarization_profile_ready_for_voiceprint(profile: &str) -> Result<(), String> {
    validate_profile_id(profile)?;
    if diarization_profile_ready(profile) {
        return Ok(());
    }
    if profile != DEFAULT_DIARIZATION_PROFILE {
        return Err(format!(
            "diarization profile '{profile}' is not installable by the built-in initializer"
        ));
    }

    #[cfg(test)]
    {
        std::fs::create_dir_all(diarization_profile_dir(profile))
            .map_err(|error| format!("create diarization profile: {error}"))?;
        std::fs::create_dir_all(voiceprint_dir())
            .map_err(|error| format!("create speaker profile dir: {error}"))?;
        Ok(())
    }

    #[cfg(not(test))]
    {
        prepare_diarization_profile(profile)
    }
}

pub(crate) fn identify_speaker_voice_from_wav_file(
    wav_path: &Path,
) -> Result<SpeakerVoiceIdentifyResponse, String> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let path = wav_path
            .to_str()
            .ok_or_else(|| "voice wake chunk path contains non-utf8 characters".to_string())?;
        let wave = Wave::read(path).ok_or_else(|| {
            format!(
                "read voice wake chunk for speaker identification failed: {}",
                wav_path.display()
            )
        })?;
        let sample_rate = u32::try_from(wave.sample_rate())
            .map_err(|_| format!("invalid voice wake sample rate {}", wave.sample_rate()))?;
        if sample_rate != VOICEPRINT_SAMPLE_RATE {
            return Err(format!(
                "voice wake speaker verification expects {}Hz audio, got {}Hz",
                VOICEPRINT_SAMPLE_RATE, sample_rate
            ));
        }
        identify_speaker_voice_pcm16(&f32_waveform_to_pcm16le(wave.samples()), sample_rate)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = wav_path;
        Err(format!(
            "speaker_embedding_unsupported_platform: sherpa-onnx is not available for {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    }
}

fn insufficient_speaker_identify_response(
    audio_duration_ms: u64,
    speech_duration_ms: u64,
) -> SpeakerVoiceIdentifyResponse {
    SpeakerVoiceIdentifyResponse {
        matched: false,
        profile_id: None,
        display_name: speaker_display_name(0),
        speaker: "speaker_00".to_string(),
        confidence: 0.0,
        status: "insufficient_audio".to_string(),
        reason: Some("need_more_speech".to_string()),
        audio_duration_ms,
        speech_duration_ms,
    }
}

fn unknown_speaker_identify_response(
    confidence: f32,
    audio_duration_ms: u64,
    speech_duration_ms: u64,
) -> SpeakerVoiceIdentifyResponse {
    SpeakerVoiceIdentifyResponse {
        matched: false,
        profile_id: None,
        display_name: speaker_display_name(0),
        speaker: "speaker_00".to_string(),
        confidence,
        status: "unmatched".to_string(),
        reason: None,
        audio_duration_ms,
        speech_duration_ms,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct VoiceprintEmbeddingResult {
    pub(crate) embedding: Vec<f32>,
    pub(crate) audio_duration_ms: u64,
    pub(crate) speech_duration_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct VoiceprintMatchResult {
    pub(crate) profile_id: String,
    pub(crate) display_name: String,
    pub(crate) confidence: f32,
    pub(crate) unambiguous: bool,
}

pub(crate) fn voiceprint_registered_profile_count() -> usize {
    load_registered_speaker_profiles().len()
}

pub(crate) fn voiceprint_match_threshold() -> f32 {
    VOICEPRINT_SPEAKER_MATCH_THRESHOLD
}

pub(crate) fn voiceprint_self_priority_threshold() -> f32 {
    VOICEPRINT_SELF_PRIORITY_THRESHOLD
}

pub(crate) fn compute_voiceprint_embedding_from_pcm16le(
    pcm16le: &[u8],
    sample_rate: u32,
) -> Result<Option<VoiceprintEmbeddingResult>, String> {
    if sample_rate != VOICEPRINT_SAMPLE_RATE {
        return Err(format!(
            "realtime speaker tracking expects {}Hz audio, got {}Hz",
            VOICEPRINT_SAMPLE_RATE, sample_rate
        ));
    }
    #[cfg(not(test))]
    let test_embedding =
        std::env::var("BIFROST_ASR_VOICEPRINT_TEST_EMBEDDING").as_deref() == Ok("1");
    #[cfg(not(test))]
    if !test_embedding && !diarization_profile_ready(DEFAULT_DIARIZATION_PROFILE) {
        return Err(format!(
            "realtime_speaker_tracking_unavailable: diarization profile '{}' is not initialized",
            DEFAULT_DIARIZATION_PROFILE
        ));
    }
    #[cfg(test)]
    ensure_diarization_profile_ready_for_voiceprint(DEFAULT_DIARIZATION_PROFILE)?;

    let prepared = prepare_voiceprint_identify_audio(pcm16le, sample_rate)?;
    let Some(ready) = prepared.ready else {
        return Ok(None);
    };
    let embedding = compute_speaker_embedding(DEFAULT_DIARIZATION_PROFILE, &ready.waveform)?;
    Ok(Some(VoiceprintEmbeddingResult {
        embedding,
        audio_duration_ms: ready.audio_duration_ms,
        speech_duration_ms: ready.speech_duration_ms,
    }))
}

pub(crate) fn best_registered_voiceprint_match(
    embedding: &[f32],
) -> Option<VoiceprintMatchResult> {
    best_registered_speaker_match(embedding).map(|(profile, confidence, conflicted)| VoiceprintMatchResult {
        profile_id: profile.id,
        display_name: profile.display_name,
        confidence,
        unambiguous: !conflicted,
    })
}

fn best_registered_speaker_match(
    embedding: &[f32],
) -> Option<(RegisteredSpeakerProfile, f32, bool)> {
    let mut ranked = load_registered_speaker_profiles()
        .into_iter()
        .filter_map(|profile| {
            registered_speaker_profile_score(embedding, &profile).map(|score| (profile, score))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_profile, left), (right_profile, right)| {
        right
            .partial_cmp(left)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left_profile.id.cmp(&right_profile.id))
    });
    let runner_up = ranked.get(1).map(|(_, score)| *score);
    let (profile, score) = ranked.into_iter().next()?;
    let conflicted = runner_up
        .is_some_and(|runner_up| score - runner_up < VOICEPRINT_PROFILE_CONFLICT_MARGIN);
    Some((profile, score, conflicted))
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;
    for (left_value, right_value) in left.iter().zip(right.iter()) {
        dot += left_value * right_value;
        left_norm += left_value * left_value;
        right_norm += right_value * right_value;
    }
    let denom = left_norm.sqrt() * right_norm.sqrt();
    (denom > 0.0).then(|| dot / denom)
}
