fn default_mono_channels() -> u8 {
    1
}

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
    let profiles = load_registered_speaker_profiles()
        .into_iter()
        .map(|profile| {
            serde_json::json!({
                "id": profile.id,
                "display_name": profile.display_name,
                "embedding_dim": profile.embedding.len()
            })
        })
        .collect::<Vec<_>>();
    json_response(&serde_json::json!({ "profiles": profiles }))
}

fn get_speaker_profile_response(profile_id: &str) -> Response<BoxBody> {
    if validate_profile_id(profile_id).is_err() {
        return error_response(StatusCode::BAD_REQUEST, "invalid speaker profile id");
    }
    let path = speaker_profile_path(profile_id);
    match std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<SpeakerVoiceprintProfile>(&raw).ok())
    {
        Some(profile) => json_response(&profile),
        None => error_response(StatusCode::NOT_FOUND, "speaker profile not found"),
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
    let preparation = match prepare_voiceprint_identify_audio(&bytes, request.sample_rate) {
        Ok(preparation) => preparation,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    let prepared = match preparation.ready {
        Some(prepared) => prepared,
        None => {
            return json_response(&insufficient_speaker_identify_response(
                preparation.audio_duration_ms,
                preparation.speech_duration_ms,
            ))
        }
    };
    match identify_speaker_voice(
        &prepared.waveform,
        prepared.audio_duration_ms,
        prepared.speech_duration_ms,
    ) {
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
    let mut samples = Vec::new();
    let mut embeddings = Vec::<Vec<f32>>::new();
    for prompt in &session.prompts {
        let audio_path = speaker_audio_path(&session.id, &prompt.id);
        let bytes = std::fs::read(&audio_path).unwrap_or_default();
        if bytes.is_empty() {
            continue;
        }
        let prompt_waveform = pcm16le_to_f32(&bytes)?;
        let stats = voiceprint_sample_stats(&prompt_waveform, session.sample_rate);
        embeddings.push(compute_speaker_embedding(
            &session.diarization_profile,
            &prompt_waveform,
        )?);
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
    let embedding = average_speaker_embeddings(&embeddings)
        .ok_or_else(|| "voiceprint enrollment has no speaker embedding".to_string())?;
    let profile_id = format!("spk-{}", uuid::Uuid::new_v4());
    let now = now_ms();
    let embedding_model = sherpa_model_pack_paths(&session.diarization_profile)
        .embedding_model
        .to_string_lossy()
        .into_owned();
    let profile = SpeakerVoiceprintProfile {
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

fn load_registered_speaker_profiles() -> Vec<RegisteredSpeakerProfile> {
    std::fs::read_dir(voiceprint_dir())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
                .filter_map(|raw| serde_json::from_str::<SpeakerVoiceprintProfile>(&raw).ok())
                .filter(|profile| !profile.embedding.is_empty())
                .map(|profile| RegisteredSpeakerProfile {
                    id: profile.id,
                    display_name: profile.display_name,
                    embedding: profile.embedding,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn registered_speaker_profile_exists(profile_id: &str) -> bool {
    load_registered_speaker_profiles()
        .iter()
        .any(|profile| profile.id == profile_id)
}

const VOICEPRINT_SELF_PRIORITY_THRESHOLD: f32 = 0.52;
const VOICEPRINT_SELF_PRIORITY_MIN_DURATION_MS: u64 = 5_000;

#[derive(Debug, Clone)]
struct DiarizationVoiceprintCandidate {
    profile_id: String,
    display_name: String,
    score: f32,
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
        let Some((profile, score)) = profiles
            .iter()
            .filter_map(|profile| {
                cosine_similarity(embedding, &profile.embedding).map(|score| (profile, score))
            })
            .max_by(|(_, left), (_, right)| {
                left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
            })
        else {
            continue;
        };
        candidates.insert(
            speaker.clone(),
            DiarizationVoiceprintCandidate {
                profile_id: profile.id.clone(),
                display_name: profile.display_name.clone(),
                score,
            },
        );
    }
    let self_priority_speaker = if profiles.len() == 1 {
        let durations = diarization_speaker_durations(diarization_segments);
        candidates
            .iter()
            .filter(|(speaker, candidate)| {
                candidate.score >= VOICEPRINT_SELF_PRIORITY_THRESHOLD
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
            matched = candidate.score >= VOICEPRINT_SPEAKER_MATCH_THRESHOLD || self_priority_matched,
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
        if candidate.score >= VOICEPRINT_SPEAKER_MATCH_THRESHOLD || self_priority_matched {
            segment.display_name = candidate.display_name.clone();
            segment.mapped_profile_id = Some(candidate.profile_id.clone());
            segment.confidence = Some(candidate.score);
        }
    }
}

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
    let Some((profile, confidence)) = best_registered_speaker_match(&embedding) else {
        return Ok(unknown_speaker_identify_response(
            0.0,
            audio_duration_ms,
            speech_duration_ms,
        ));
    };
    Ok(SpeakerVoiceIdentifyResponse {
        matched: confidence >= VOICEPRINT_SPEAKER_MATCH_THRESHOLD,
        profile_id: Some(profile.id),
        display_name: profile.display_name,
        speaker: "speaker_00".to_string(),
        confidence,
        status: if confidence >= VOICEPRINT_SPEAKER_MATCH_THRESHOLD {
            "matched".to_string()
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
        identify_speaker_voice_from_waveform(wave.samples(), sample_rate)
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

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn identify_speaker_voice_from_waveform(
    waveform: &[f32],
    sample_rate: u32,
) -> Result<SpeakerVoiceIdentifyResponse, String> {
    if sample_rate != VOICEPRINT_SAMPLE_RATE {
        return Err(format!(
            "voice wake speaker verification expects {}Hz audio, got {}Hz",
            VOICEPRINT_SAMPLE_RATE, sample_rate
        ));
    }
    let audio_duration_ms =
        (waveform.len() as u64).saturating_mul(1_000) / u64::from(sample_rate.max(1));
    let Some((start, end)) =
        active_speech_bounds(waveform, sample_rate, VOICEPRINT_IDENTIFY_SPEECH_RMS)
    else {
        return Ok(insufficient_speaker_identify_response(audio_duration_ms, 0));
    };
    let speech_duration_ms = ((end.saturating_sub(start)) as u64)
        .saturating_mul(1_000)
        / u64::from(sample_rate.max(1));
    if speech_duration_ms < VOICEPRINT_MIN_IDENTIFY_SPEECH_MS {
        return Ok(insufficient_speaker_identify_response(
            audio_duration_ms,
            speech_duration_ms,
        ));
    }
    identify_speaker_voice(&waveform[start..end], audio_duration_ms, speech_duration_ms)
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
    best_registered_speaker_match(embedding).map(|(profile, confidence)| VoiceprintMatchResult {
        profile_id: profile.id,
        display_name: profile.display_name,
        confidence,
    })
}

fn best_registered_speaker_match(embedding: &[f32]) -> Option<(RegisteredSpeakerProfile, f32)> {
    load_registered_speaker_profiles()
        .into_iter()
        .filter_map(|profile| {
            cosine_similarity(embedding, &profile.embedding).map(|score| (profile, score))
        })
        .max_by(|(_, left), (_, right)| {
            left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
        })
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
