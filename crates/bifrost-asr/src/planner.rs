use serde::{Deserialize, Serialize};

use crate::timeline::{TimelineSpeaker, ASR_TASK_SEGMENT_MAX_MS};

pub const DEFAULT_MERGE_SAME_SPEAKER_GAP_MS: u64 = 800;
pub const DEFAULT_MIN_UNIT_MS: u64 = 500;
pub const DEFAULT_MIN_RMS: f32 = 0.008;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiarizationSegment {
    pub speaker: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapped_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_confidence: Option<f32>,
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default)]
    pub overlap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsrUnitKind {
    Segment,
    Merged,
    Split,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrAudioUnit {
    pub unit_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapped_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    pub source_start_ms: u64,
    pub source_end_ms: u64,
    pub source_segment_ids: Vec<String>,
    pub overlap: bool,
    pub unit_kind: AsrUnitKind,
}

#[derive(Debug, Clone)]
pub struct AsrUnitPlannerConfig {
    pub merge_same_speaker_gap_ms: u64,
    pub max_unit_ms: u64,
    pub min_unit_ms: u64,
    pub min_rms: f32,
}

impl Default for AsrUnitPlannerConfig {
    fn default() -> Self {
        Self {
            merge_same_speaker_gap_ms: DEFAULT_MERGE_SAME_SPEAKER_GAP_MS,
            max_unit_ms: ASR_TASK_SEGMENT_MAX_MS,
            min_unit_ms: DEFAULT_MIN_UNIT_MS,
            min_rms: DEFAULT_MIN_RMS,
        }
    }
}

pub fn plan_asr_units(
    segments: &[DiarizationSegment],
    config: &AsrUnitPlannerConfig,
) -> Vec<AsrAudioUnit> {
    let mut ordered = segments
        .iter()
        .enumerate()
        .filter(|(_, segment)| segment.end_ms > segment.start_ms)
        .map(|(index, segment)| PlannedRange::from_segment(index, segment))
        .collect::<Vec<_>>();
    ordered.sort_by_key(|range| (range.start_ms, range.end_ms));

    let mut merged = Vec::<PlannedRange>::new();
    for range in ordered {
        let Some(last) = merged.last_mut() else {
            merged.push(range);
            continue;
        };
        if last.can_merge(&range, config.merge_same_speaker_gap_ms) {
            last.merge(range);
        } else {
            merged.push(range);
        }
    }

    let mut units = Vec::new();
    for range in merged {
        if range.duration_ms() < config.min_unit_ms {
            continue;
        }
        append_range_units(&mut units, range, config.max_unit_ms.max(1));
    }
    for (index, unit) in units.iter_mut().enumerate() {
        unit.unit_id = format!("asr_unit_{index:04}");
    }
    units
}

pub fn speakers_from_diarization_segments(segments: &[DiarizationSegment]) -> Vec<TimelineSpeaker> {
    type SpeakerMetadata = (
        String,
        Option<String>,
        Option<f32>,
        Option<String>,
        Option<String>,
        Option<f32>,
    );
    let mut speakers = std::collections::BTreeMap::<String, SpeakerMetadata>::new();
    for segment in segments {
        speakers.entry(segment.speaker.clone()).or_insert_with(|| {
            (
                segment.display_name.clone(),
                segment.mapped_profile_id.clone(),
                segment.confidence,
                segment.candidate_profile_id.clone(),
                segment.candidate_display_name.clone(),
                segment.candidate_confidence,
            )
        });
    }
    speakers
        .into_iter()
        .map(
            |(
                id,
                (
                    display_name,
                    mapped_profile_id,
                    confidence,
                    candidate_profile_id,
                    candidate_display_name,
                    candidate_confidence,
                ),
            )| TimelineSpeaker {
                id,
                display_name,
                mapped_profile_id,
                confidence,
                candidate_profile_id,
                candidate_display_name,
                candidate_confidence,
            },
        )
        .collect()
}

pub fn speaker_display_name(index: usize) -> String {
    if index < 26 {
        format!("用户{}", (b'A' + index as u8) as char)
    } else {
        format!("用户{}", index + 1)
    }
}

pub fn seconds_to_ms(seconds: f32) -> u64 {
    (seconds.max(0.0) * 1000.0).round() as u64
}

pub fn interval_overlap_ms(
    left_start: u64,
    left_end: u64,
    right_start: u64,
    right_end: u64,
) -> u64 {
    let start = left_start.max(right_start);
    let end = left_end.min(right_end);
    end.saturating_sub(start)
}

fn append_range_units(units: &mut Vec<AsrAudioUnit>, range: PlannedRange, max_unit_ms: u64) {
    let base_kind = if range.source_segment_ids.len() > 1 {
        AsrUnitKind::Merged
    } else {
        AsrUnitKind::Segment
    };
    if range.duration_ms() <= max_unit_ms {
        units.push(range.into_unit(String::new(), base_kind));
        return;
    }

    let mut start_ms = range.start_ms;
    while start_ms < range.end_ms {
        let end_ms = start_ms.saturating_add(max_unit_ms).min(range.end_ms);
        if end_ms <= start_ms {
            break;
        }
        let source_segment_ids = range
            .source_segment_ids
            .iter()
            .filter_map(|id| {
                let (segment_start, segment_end) = range.source_ranges.get(id)?;
                intervals_overlap(start_ms, end_ms, *segment_start, *segment_end)
                    .then(|| id.clone())
            })
            .collect::<Vec<_>>();
        units.push(AsrAudioUnit {
            unit_id: String::new(),
            speaker: Some(range.speaker.clone()),
            speaker_display_name: Some(range.display_name.clone()),
            mapped_profile_id: range.mapped_profile_id.clone(),
            confidence: range.confidence,
            source_start_ms: start_ms,
            source_end_ms: end_ms,
            source_segment_ids,
            overlap: range.overlap,
            unit_kind: AsrUnitKind::Split,
        });
        start_ms = end_ms;
    }
}

fn intervals_overlap(left_start: u64, left_end: u64, right_start: u64, right_end: u64) -> bool {
    left_start < right_end && right_start < left_end
}

#[derive(Debug, Clone)]
struct PlannedRange {
    speaker: String,
    display_name: String,
    mapped_profile_id: Option<String>,
    confidence: Option<f32>,
    start_ms: u64,
    end_ms: u64,
    source_segment_ids: Vec<String>,
    source_ranges: std::collections::BTreeMap<String, (u64, u64)>,
    overlap: bool,
}

impl PlannedRange {
    fn from_segment(index: usize, segment: &DiarizationSegment) -> Self {
        let source_segment_id = format!("diarization_segment_{index:04}");
        Self {
            speaker: segment.speaker.clone(),
            display_name: segment.display_name.clone(),
            mapped_profile_id: segment.mapped_profile_id.clone(),
            confidence: segment.confidence,
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            source_segment_ids: vec![source_segment_id.clone()],
            source_ranges: [(source_segment_id, (segment.start_ms, segment.end_ms))]
                .into_iter()
                .collect(),
            overlap: segment.overlap,
        }
    }

    fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }

    fn can_merge(&self, next: &Self, merge_gap_ms: u64) -> bool {
        self.speaker == next.speaker
            && self.mapped_profile_id == next.mapped_profile_id
            && next.start_ms >= self.end_ms
            && next.start_ms.saturating_sub(self.end_ms) <= merge_gap_ms
    }

    fn merge(&mut self, next: Self) {
        self.end_ms = self.end_ms.max(next.end_ms);
        self.overlap |= next.overlap;
        self.source_segment_ids.extend(next.source_segment_ids);
        self.source_ranges.extend(next.source_ranges);
    }

    fn into_unit(self, unit_id: String, unit_kind: AsrUnitKind) -> AsrAudioUnit {
        AsrAudioUnit {
            unit_id,
            speaker: Some(self.speaker),
            speaker_display_name: Some(self.display_name),
            mapped_profile_id: self.mapped_profile_id,
            confidence: self.confidence,
            source_start_ms: self.start_ms,
            source_end_ms: self.end_ms,
            source_segment_ids: self.source_segment_ids,
            overlap: self.overlap,
            unit_kind,
        }
    }
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
    fn planner_merges_same_speaker_gap() {
        let units = plan_asr_units(
            &[
                segment("speaker_00", 0, 1_000),
                segment("speaker_00", 1_500, 2_000),
            ],
            &AsrUnitPlannerConfig::default(),
        );

        assert_eq!(units.len(), 1);
        assert_eq!(units[0].source_start_ms, 0);
        assert_eq!(units[0].source_end_ms, 2_000);
        assert_eq!(units[0].unit_kind, AsrUnitKind::Merged);
        assert_eq!(units[0].source_segment_ids.len(), 2);
    }

    #[test]
    fn planner_splits_unit_over_30s() {
        let units = plan_asr_units(
            &[segment("speaker_00", 0, 65_000)],
            &AsrUnitPlannerConfig::default(),
        );

        assert_eq!(units.len(), 3);
        assert!(units
            .iter()
            .all(|unit| unit.source_end_ms - unit.source_start_ms <= 30_000));
        assert_eq!(units[0].unit_kind, AsrUnitKind::Split);
        assert_eq!(units[2].source_start_ms, 60_000);
        assert_eq!(units[2].source_end_ms, 65_000);
    }

    #[test]
    fn planner_skips_invalid_and_too_short_segments() {
        let units = plan_asr_units(
            &[
                segment("speaker_00", 1_000, 1_000),
                segment("speaker_00", 2_000, 2_300),
                segment("speaker_00", 4_000, 4_800),
            ],
            &AsrUnitPlannerConfig::default(),
        );

        assert_eq!(units.len(), 1);
        assert_eq!(units[0].source_start_ms, 4_000);
        assert_eq!(units[0].source_end_ms, 4_800);
    }
}
