use std::collections::{BTreeMap, BTreeSet};

use crate::planner::{speaker_display_name, DiarizationSegment};

pub const DEFAULT_SPEAKER_MERGE_SIMILARITY_THRESHOLD: f32 = 0.78;
pub const DEFAULT_SHORT_SPEAKER_MERGE_SIMILARITY_THRESHOLD: f32 = 0.66;
pub const DEFAULT_SHORT_SPEAKER_MAX_DURATION_MS: u64 = 10_000;
pub const DEFAULT_SHORT_SPEAKER_MAX_SEGMENTS: usize = 4;
pub const DEFAULT_FRAGMENT_NEIGHBOR_MAX_GAP_MS: u64 = 5_000;

#[derive(Debug, Clone)]
pub struct SpeakerStabilizationConfig {
    pub merge_similarity_threshold: f32,
    pub short_speaker_merge_similarity_threshold: f32,
    pub short_speaker_max_duration_ms: u64,
    pub short_speaker_max_segments: usize,
    pub fragment_neighbor_max_gap_ms: u64,
}

impl Default for SpeakerStabilizationConfig {
    fn default() -> Self {
        Self {
            merge_similarity_threshold: DEFAULT_SPEAKER_MERGE_SIMILARITY_THRESHOLD,
            short_speaker_merge_similarity_threshold:
                DEFAULT_SHORT_SPEAKER_MERGE_SIMILARITY_THRESHOLD,
            short_speaker_max_duration_ms: DEFAULT_SHORT_SPEAKER_MAX_DURATION_MS,
            short_speaker_max_segments: DEFAULT_SHORT_SPEAKER_MAX_SEGMENTS,
            fragment_neighbor_max_gap_ms: DEFAULT_FRAGMENT_NEIGHBOR_MAX_GAP_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerMergeDecision {
    pub from_speaker: String,
    pub to_speaker: String,
    pub similarity: f32,
    pub reason: String,
}

pub fn stabilize_diarization_speakers(
    segments: &mut [DiarizationSegment],
    speaker_embeddings: &BTreeMap<String, Vec<f32>>,
    config: &SpeakerStabilizationConfig,
) -> Vec<SpeakerMergeDecision> {
    if segments.is_empty() {
        return Vec::new();
    }
    let stats = speaker_stats(segments);
    let speakers = stats.keys().cloned().collect::<Vec<_>>();
    let mut parent = speakers
        .iter()
        .map(|speaker| (speaker.clone(), speaker.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut decisions = Vec::new();

    if speaker_embeddings.len() >= 2 {
        for speaker in speakers {
            let Some(current_stats) = stats.get(&speaker) else {
                continue;
            };
            let Some(current_embedding) = speaker_embeddings.get(&speaker) else {
                continue;
            };
            let is_short = current_stats.duration_ms <= config.short_speaker_max_duration_ms
                || current_stats.segment_count <= config.short_speaker_max_segments;
            let threshold = if is_short {
                config.short_speaker_merge_similarity_threshold
            } else {
                config.merge_similarity_threshold
            };
            let Some((target, similarity)) = stats
                .iter()
                .filter(|(candidate, candidate_stats)| {
                    *candidate != &speaker
                        && candidate_stats.duration_ms >= current_stats.duration_ms
                        && speaker_embeddings.contains_key(*candidate)
                })
                .filter_map(|(candidate, _)| {
                    let candidate_embedding = speaker_embeddings.get(candidate)?;
                    cosine_similarity(current_embedding, candidate_embedding)
                        .map(|similarity| (candidate.clone(), similarity))
                })
                .max_by(|(_, left), (_, right)| {
                    left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
                })
            else {
                continue;
            };
            if similarity >= threshold {
                let target_root = find_root(&mut parent, &target);
                parent.insert(speaker.clone(), target_root.clone());
                decisions.push(SpeakerMergeDecision {
                    from_speaker: speaker,
                    to_speaker: target_root,
                    similarity,
                    reason: if is_short {
                        "short_speaker_embedding_match".to_string()
                    } else {
                        "speaker_embedding_match".to_string()
                    },
                });
            }
        }
    }

    let mut changed = !decisions.is_empty();
    if changed {
        apply_parent_mapping(segments, &mut parent);
    }
    let fragment_decisions = absorb_fragmentary_speakers(segments, config);
    changed |= !fragment_decisions.is_empty();
    decisions.extend(fragment_decisions);
    if !changed {
        return decisions;
    }
    clear_speaker_identity_metadata(segments);
    densify_speaker_ids(segments);
    decisions
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let dot = left
        .iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum::<f32>();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        return None;
    }
    Some((dot / (left_norm * right_norm)).clamp(-1.0, 1.0))
}

fn densify_speaker_ids(segments: &mut [DiarizationSegment]) {
    let mut ordered = segments
        .iter()
        .map(|segment| (segment.start_ms, segment.speaker.clone()))
        .collect::<Vec<_>>();
    ordered.sort();
    let mut seen = BTreeSet::new();
    let mut mapping = BTreeMap::new();
    for (_, speaker) in ordered {
        if seen.insert(speaker.clone()) {
            let index = mapping.len();
            mapping.insert(
                speaker,
                (format!("speaker_{index:02}"), speaker_display_name(index)),
            );
        }
    }
    for segment in segments {
        if let Some((speaker, display_name)) = mapping.get(&segment.speaker) {
            segment.speaker.clone_from(speaker);
            segment.display_name.clone_from(display_name);
        }
    }
}

fn apply_parent_mapping(
    segments: &mut [DiarizationSegment],
    parent: &mut BTreeMap<String, String>,
) {
    for segment in segments {
        segment.speaker = find_root(parent, &segment.speaker);
    }
}

fn clear_speaker_identity_metadata(segments: &mut [DiarizationSegment]) {
    for segment in segments {
        segment.display_name.clear();
        segment.mapped_profile_id = None;
        segment.confidence = None;
        segment.candidate_profile_id = None;
        segment.candidate_display_name = None;
        segment.candidate_confidence = None;
    }
}

fn absorb_fragmentary_speakers(
    segments: &mut [DiarizationSegment],
    config: &SpeakerStabilizationConfig,
) -> Vec<SpeakerMergeDecision> {
    let stats = speaker_stats(segments);
    let mut decisions = Vec::new();
    let mut mapping = BTreeMap::<String, String>::new();
    for (speaker, current_stats) in &stats {
        let is_fragment = current_stats.duration_ms <= config.short_speaker_max_duration_ms
            && current_stats.segment_count <= config.short_speaker_max_segments;
        if !is_fragment {
            continue;
        }
        let Some((target, gap_ms)) = nearest_stable_neighbor(segments, speaker, &stats) else {
            continue;
        };
        if gap_ms > config.fragment_neighbor_max_gap_ms {
            continue;
        }
        mapping.insert(speaker.clone(), target.clone());
        decisions.push(SpeakerMergeDecision {
            from_speaker: speaker.clone(),
            to_speaker: target,
            similarity: 0.0,
            reason: "fragmentary_temporal_neighbor".to_string(),
        });
    }
    if mapping.is_empty() {
        return decisions;
    }
    for segment in segments {
        if let Some(target) = mapping.get(&segment.speaker) {
            segment.speaker.clone_from(target);
        }
    }
    decisions
}

fn nearest_stable_neighbor(
    segments: &[DiarizationSegment],
    speaker: &str,
    stats: &BTreeMap<String, SpeakerStats>,
) -> Option<(String, u64)> {
    let current = stats.get(speaker)?;
    segments
        .iter()
        .filter(|segment| segment.speaker == speaker)
        .flat_map(|fragment| {
            segments
                .iter()
                .filter(move |candidate| candidate.speaker != speaker)
                .filter(move |candidate| {
                    stats
                        .get(&candidate.speaker)
                        .is_some_and(|candidate_stats| {
                            candidate_stats.duration_ms > current.duration_ms
                                || candidate_stats.segment_count > current.segment_count
                        })
                })
                .map(move |candidate| {
                    (
                        candidate.speaker.clone(),
                        interval_gap_ms(
                            fragment.start_ms,
                            fragment.end_ms,
                            candidate.start_ms,
                            candidate.end_ms,
                        ),
                    )
                })
        })
        .min_by_key(|(_, gap_ms)| *gap_ms)
}

fn interval_gap_ms(
    left_start_ms: u64,
    left_end_ms: u64,
    right_start_ms: u64,
    right_end_ms: u64,
) -> u64 {
    if left_end_ms < right_start_ms {
        right_start_ms - left_end_ms
    } else {
        left_start_ms.saturating_sub(right_end_ms)
    }
}

fn find_root(parent: &mut BTreeMap<String, String>, speaker: &str) -> String {
    let Some(next) = parent.get(speaker).cloned() else {
        return speaker.to_string();
    };
    if next == speaker {
        return next;
    }
    let root = find_root(parent, &next);
    parent.insert(speaker.to_string(), root.clone());
    root
}

fn speaker_stats(segments: &[DiarizationSegment]) -> BTreeMap<String, SpeakerStats> {
    let mut stats = BTreeMap::<String, SpeakerStats>::new();
    for segment in segments {
        let entry = stats.entry(segment.speaker.clone()).or_default();
        entry.duration_ms = entry
            .duration_ms
            .saturating_add(segment.end_ms.saturating_sub(segment.start_ms));
        entry.segment_count += 1;
    }
    stats
}

#[derive(Debug, Clone, Default)]
struct SpeakerStats {
    duration_ms: u64,
    segment_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(speaker: &str, start_ms: u64, end_ms: u64) -> DiarizationSegment {
        DiarizationSegment {
            speaker: speaker.to_string(),
            display_name: speaker.to_string(),
            mapped_profile_id: None,
            confidence: None,
            candidate_profile_id: None,
            candidate_display_name: None,
            candidate_confidence: None,
            start_ms,
            end_ms,
            overlap: false,
        }
    }

    #[test]
    fn stabilizer_merges_short_similar_speaker_into_dominant_cluster() {
        let mut segments = vec![
            segment("speaker_00", 0, 20_000),
            segment("speaker_01", 21_000, 23_000),
            segment("speaker_02", 24_000, 35_000),
        ];
        let embeddings = BTreeMap::from([
            ("speaker_00".to_string(), vec![1.0, 0.0]),
            ("speaker_01".to_string(), vec![0.90, 0.435_889_9]),
            ("speaker_02".to_string(), vec![0.0, 1.0]),
        ]);

        let decisions = stabilize_diarization_speakers(
            &mut segments,
            &embeddings,
            &SpeakerStabilizationConfig::default(),
        );

        assert_eq!(decisions.len(), 1);
        assert_eq!(segments[0].speaker, "speaker_00");
        assert_eq!(segments[1].speaker, "speaker_00");
        assert_eq!(segments[2].speaker, "speaker_01");
        assert_eq!(segments[0].display_name, "用户A");
        assert_eq!(segments[2].display_name, "用户B");
    }

    #[test]
    fn stabilizer_absorbs_fragmentary_temporal_neighbor() {
        let mut segments = vec![
            segment("speaker_00", 0, 20_000),
            segment("speaker_01", 22_000, 23_000),
            segment("speaker_00", 25_000, 40_000),
        ];

        let decisions = stabilize_diarization_speakers(
            &mut segments,
            &BTreeMap::new(),
            &SpeakerStabilizationConfig::default(),
        );

        assert_eq!(decisions.len(), 1);
        assert_eq!(segments[0].speaker, "speaker_00");
        assert_eq!(segments[1].speaker, "speaker_00");
        assert_eq!(segments[2].speaker, "speaker_00");
    }
}
