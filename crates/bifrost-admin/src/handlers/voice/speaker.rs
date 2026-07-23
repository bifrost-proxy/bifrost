use bifrost_asr::planner::speaker_display_name;
use bifrost_asr::speaker::cosine_similarity;

use crate::handlers::asr_jobs::{
    best_registered_voiceprint_match, compute_voiceprint_embedding_from_pcm16le,
    voiceprint_match_threshold, voiceprint_registered_profile_count,
    voiceprint_self_priority_threshold,
};

const REALTIME_CLUSTER_MATCH_THRESHOLD: f32 = 0.66;
const REALTIME_MIN_SELF_PRIORITY_SPEECH_MS: u64 = 1_200;
const REALTIME_MAX_SESSION_SPEAKERS: usize = 4;

#[derive(Debug, Clone, Default)]
pub(super) struct RealtimeSpeakerAssignment {
    pub(super) speaker: String,
    pub(super) display_name: String,
    pub(super) mapped_profile_id: Option<String>,
    pub(super) confidence: Option<f32>,
    pub(super) candidate_profile_id: Option<String>,
    pub(super) candidate_display_name: Option<String>,
    pub(super) candidate_confidence: Option<f32>,
}

#[derive(Debug, Clone)]
struct RealtimeSpeakerCluster {
    speaker: String,
    display_name: String,
    embedding: Vec<f32>,
    samples: usize,
    mapped_profile_id: Option<String>,
    confidence: Option<f32>,
    candidate_profile_id: Option<String>,
    candidate_display_name: Option<String>,
    candidate_confidence: Option<f32>,
}

#[derive(Debug, Default, Clone)]
pub(super) struct RealtimeVoiceSpeakerTracker {
    clusters: Vec<RealtimeSpeakerCluster>,
    warned_unavailable: bool,
}

impl RealtimeVoiceSpeakerTracker {
    pub(super) fn reset(&mut self) {
        self.clusters.clear();
        self.warned_unavailable = false;
    }

    pub(super) fn classify_utterance(
        &mut self,
        pcm16le: &[u8],
        sample_rate: u32,
        session_id: &str,
    ) -> Option<RealtimeSpeakerAssignment> {
        let embedding = match compute_voiceprint_embedding_from_pcm16le(pcm16le, sample_rate) {
            Ok(Some(embedding)) => embedding,
            Ok(None) => return None,
            Err(error) => {
                if !self.warned_unavailable {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %error,
                        "realtime speaker tracking is unavailable"
                    );
                    self.warned_unavailable = true;
                }
                return None;
            }
        };

        let registered = best_registered_voiceprint_match(&embedding.embedding);
        let registered_count = voiceprint_registered_profile_count();
        let matched_registered = registered.as_ref().filter(|candidate| {
            candidate.unambiguous
                && (candidate.confidence >= voiceprint_match_threshold()
                    || (registered_count == 1
                        && embedding.speech_duration_ms >= REALTIME_MIN_SELF_PRIORITY_SPEECH_MS
                        && candidate.confidence >= voiceprint_self_priority_threshold()))
        });

        let cluster_index = if let Some(candidate) = matched_registered {
            self.find_cluster_for_profile(&candidate.profile_id)
                .or_else(|| self.find_best_cluster(&embedding.embedding))
        } else {
            self.find_best_cluster(&embedding.embedding)
        };

        let assignment = if let Some(index) = cluster_index {
            self.update_cluster(
                index,
                &embedding.embedding,
                matched_registered,
                registered.as_ref(),
            )
        } else if self.clusters.len() < REALTIME_MAX_SESSION_SPEAKERS {
            self.create_cluster(
                &embedding.embedding,
                matched_registered,
                registered.as_ref(),
            )
        } else {
            let index = self
                .find_nearest_cluster(&embedding.embedding)
                .unwrap_or_else(|| self.clusters.len().saturating_sub(1));
            self.update_cluster(
                index,
                &embedding.embedding,
                matched_registered,
                registered.as_ref(),
            )
        };

        tracing::info!(
            target: "bifrost_admin::voice",
            session_id = %session_id,
            speaker = %assignment.speaker,
            display_name = %assignment.display_name,
            profile_id = ?assignment.mapped_profile_id,
            confidence = ?assignment.confidence,
            candidate_profile_id = ?assignment.candidate_profile_id,
            candidate_confidence = ?assignment.candidate_confidence,
            audio_duration_ms = embedding.audio_duration_ms,
            speech_duration_ms = embedding.speech_duration_ms,
            "classified realtime voice utterance speaker"
        );
        Some(assignment)
    }

    fn find_cluster_for_profile(&self, profile_id: &str) -> Option<usize> {
        self.clusters.iter().position(|cluster| {
            cluster
                .mapped_profile_id
                .as_deref()
                .is_some_and(|mapped| mapped == profile_id)
        })
    }

    fn find_best_cluster(&self, embedding: &[f32]) -> Option<usize> {
        let (index, score) = self.best_cluster_score(embedding)?;
        (score >= REALTIME_CLUSTER_MATCH_THRESHOLD).then_some(index)
    }

    fn find_nearest_cluster(&self, embedding: &[f32]) -> Option<usize> {
        self.best_cluster_score(embedding).map(|(index, _)| index)
    }

    fn best_cluster_score(&self, embedding: &[f32]) -> Option<(usize, f32)> {
        self.clusters
            .iter()
            .enumerate()
            .filter_map(|(index, cluster)| {
                cosine_similarity(embedding, &cluster.embedding).map(|score| (index, score))
            })
            .max_by(|(_, left), (_, right)| {
                left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    fn create_cluster(
        &mut self,
        embedding: &[f32],
        matched_registered: Option<&crate::handlers::asr_jobs::VoiceprintMatchResult>,
        registered: Option<&crate::handlers::asr_jobs::VoiceprintMatchResult>,
    ) -> RealtimeSpeakerAssignment {
        let index = self.clusters.len();
        let speaker = format!("speaker_{index:02}");
        let fallback_name = speaker_display_name(index);
        let display_name = matched_registered
            .map(|candidate| candidate.display_name.clone())
            .unwrap_or_else(|| fallback_name.clone());
        let cluster = RealtimeSpeakerCluster {
            speaker,
            display_name,
            embedding: normalized(embedding),
            samples: 1,
            mapped_profile_id: matched_registered.map(|candidate| candidate.profile_id.clone()),
            confidence: matched_registered.map(|candidate| candidate.confidence),
            candidate_profile_id: registered.map(|candidate| candidate.profile_id.clone()),
            candidate_display_name: registered.map(|candidate| candidate.display_name.clone()),
            candidate_confidence: registered.map(|candidate| candidate.confidence),
        };
        let assignment = cluster.assignment();
        self.clusters.push(cluster);
        assignment
    }

    fn update_cluster(
        &mut self,
        index: usize,
        embedding: &[f32],
        matched_registered: Option<&crate::handlers::asr_jobs::VoiceprintMatchResult>,
        registered: Option<&crate::handlers::asr_jobs::VoiceprintMatchResult>,
    ) -> RealtimeSpeakerAssignment {
        let Some(cluster) = self.clusters.get_mut(index) else {
            return RealtimeSpeakerAssignment::default();
        };
        let next_weight = 1.0 / (cluster.samples.saturating_add(1) as f32);
        let current_weight = 1.0 - next_weight;
        for (current, next) in cluster.embedding.iter_mut().zip(embedding.iter()) {
            *current = (*current * current_weight) + (*next * next_weight);
        }
        normalize_embedding(&mut cluster.embedding);
        cluster.samples = cluster.samples.saturating_add(1);
        if let Some(candidate) = matched_registered {
            cluster.display_name = candidate.display_name.clone();
            cluster.mapped_profile_id = Some(candidate.profile_id.clone());
            cluster.confidence = Some(candidate.confidence);
        }
        if let Some(candidate) = registered {
            cluster.candidate_profile_id = Some(candidate.profile_id.clone());
            cluster.candidate_display_name = Some(candidate.display_name.clone());
            cluster.candidate_confidence = Some(candidate.confidence);
        }
        cluster.assignment()
    }
}

impl RealtimeSpeakerCluster {
    fn assignment(&self) -> RealtimeSpeakerAssignment {
        RealtimeSpeakerAssignment {
            speaker: self.speaker.clone(),
            display_name: self.display_name.clone(),
            mapped_profile_id: self.mapped_profile_id.clone(),
            confidence: self.confidence,
            candidate_profile_id: self.candidate_profile_id.clone(),
            candidate_display_name: self.candidate_display_name.clone(),
            candidate_confidence: self.candidate_confidence,
        }
    }
}

fn normalized(embedding: &[f32]) -> Vec<f32> {
    let mut normalized = embedding.to_vec();
    normalize_embedding(&mut normalized);
    normalized
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::BifrostDataDirGuard;

    #[test]
    fn best_cluster_requires_similarity_threshold() {
        let mut tracker = RealtimeVoiceSpeakerTracker::default();
        tracker.clusters.push(RealtimeSpeakerCluster {
            speaker: "speaker_00".to_string(),
            display_name: "用户A".to_string(),
            embedding: vec![1.0, 0.0],
            samples: 1,
            mapped_profile_id: None,
            confidence: None,
            candidate_profile_id: None,
            candidate_display_name: None,
            candidate_confidence: None,
        });
        assert_eq!(tracker.find_best_cluster(&[0.98, 0.1]), Some(0));
        assert_eq!(tracker.find_best_cluster(&[0.0, 1.0]), None);
    }

    #[test]
    fn registered_profile_self_priority_is_considered_for_realtime_assignment() {
        let temp = tempfile::tempdir().unwrap();
        let _guard = BifrostDataDirGuard::set(temp.path());
        let pcm16le = (0..16_000 * 2)
            .flat_map(|index| {
                let sample = if index % 2 == 0 { 8_000i16 } else { -8_000i16 };
                sample.to_le_bytes()
            })
            .collect::<Vec<_>>();
        let utterance_embedding = compute_voiceprint_embedding_from_pcm16le(&pcm16le, 16_000)
            .unwrap()
            .unwrap()
            .embedding;
        let mut orthogonal = vec![0.0; utterance_embedding.len()];
        orthogonal[0] = -utterance_embedding[1];
        orthogonal[1] = utterance_embedding[0];
        normalize_embedding(&mut orthogonal);
        let confidence = 0.55_f32;
        let orthogonal_weight = (1.0 - confidence * confidence).sqrt();
        let embedding = utterance_embedding
            .iter()
            .zip(&orthogonal)
            .map(|(voice, other)| confidence * voice + orthogonal_weight * other)
            .collect::<Vec<_>>();
        let profile_dir = temp
            .path()
            .join("asr")
            .join("diarization")
            .join("speaker-profiles");
        std::fs::create_dir_all(&profile_dir).unwrap();
        std::fs::write(
            profile_dir.join("spk-realtime.json"),
            serde_json::to_vec(&serde_json::json!({
                "id": "spk-realtime",
                "display_name": "Eden",
                "source": "test",
                "diarization_profile": "sherpa-onnx-balanced",
                "embedding_model": "test",
                "embedding_dim": embedding.len(),
                "embedding": embedding,
                "sample_rate": 16000,
                "total_duration_ms": 2000,
                "samples": [],
                "created_at_ms": 1,
                "updated_at_ms": 1
            }))
            .unwrap(),
        )
        .unwrap();
        let mut tracker = RealtimeVoiceSpeakerTracker::default();

        let assignment = tracker
            .classify_utterance(&pcm16le, 16_000, "voice-test")
            .unwrap();

        assert_eq!(assignment.display_name, "Eden");
        assert_eq!(
            assignment.mapped_profile_id.as_deref(),
            Some("spk-realtime")
        );
    }
}
