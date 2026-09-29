use crate::phrase::time::Seconds;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentationReason {
    SpeakerChange,
    PunctuationPause,
    LongPause,
    SyntaxPause,
    OptimalDpSplit,
    OversizeExceeded,
    TurnEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpeechSpan {
    pub start: Seconds,
    pub end: Seconds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AmbiguityStatus {
    Clear,
    LowOverlapRatio,
    MultipleSpeakersOverlap,
    GapInherited,
    GapNearestAssigned,
    UnknownSpeaker,
}

pub trait TextUnit: Send + Sync {
    fn text(&self) -> &str;
    fn start(&self) -> Seconds;
    fn end(&self) -> Seconds;
    fn can_split_after(&self) -> bool;
    fn is_whisper_boundary(&self) -> bool;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AsrUnit {
    pub text: String,
    pub start: Seconds,
    pub end: Seconds,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub is_whisper_boundary: bool,
    /// Разрешена ли граница фразы ПОСЛЕ данного юнита
    #[serde(default = "default_true")]
    pub can_split_after: bool,
}

impl TextUnit for AsrUnit {
    #[inline]
    fn text(&self) -> &str {
        &self.text
    }

    #[inline]
    fn start(&self) -> Seconds {
        self.start
    }

    #[inline]
    fn end(&self) -> Seconds {
        self.end
    }

    #[inline]
    fn can_split_after(&self) -> bool {
        self.can_split_after
    }

    #[inline]
    fn is_whisper_boundary(&self) -> bool {
        self.is_whisper_boundary
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WhisperSegment {
    pub start: Seconds,
    pub end: Seconds,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AsrResult {
    pub units: Vec<AsrUnit>,
    pub raw_segments: Vec<WhisperSegment>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiarInterval {
    pub speaker: String,
    pub start: Seconds,
    pub end: Seconds,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct DiarizationResult {
    pub intervals: Vec<DiarInterval>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlignedUnit {
    pub text: String,
    pub start: Seconds,
    pub end: Seconds,
    pub speaker: String,
    pub word_confidence: Option<f64>,
    pub speaker_confidence: f64,
    pub overlap_ratio: f64,
    pub ambiguity: AmbiguityStatus,
    pub alternative_speaker: Option<String>,
    pub alternative_overlap_ratio: f64,
    pub is_whisper_boundary: bool,
    pub can_split_after: bool,
}

impl TextUnit for AlignedUnit {
    #[inline]
    fn text(&self) -> &str {
        &self.text
    }

    #[inline]
    fn start(&self) -> Seconds {
        self.start
    }

    #[inline]
    fn end(&self) -> Seconds {
        self.end
    }

    #[inline]
    fn can_split_after(&self) -> bool {
        self.can_split_after
    }

    #[inline]
    fn is_whisper_boundary(&self) -> bool {
        self.is_whisper_boundary
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InternalPause {
    /// Локальный 0-based индекс юнита внутри реплики Utterance, ПОСЛЕ которого находится пауза
    pub after_unit_index: usize,
    pub start: Seconds,
    pub end: Seconds,
    pub duration: Seconds,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeakerTurn {
    pub id: String,
    pub speaker: String,
    pub start: Seconds,
    pub end: Seconds,
    pub units: Vec<AlignedUnit>,
    pub is_overlap: bool,
    pub context_group_id: Option<String>,
    pub continues_from_turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentationMeta {
    pub reason: SegmentationReason,
    pub confidence: f64,
    pub is_oversize: bool,
    pub is_soft_max_exceeded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoundaryCandidate {
    pub after_unit_index: usize,
    pub time_offset: Seconds,
    pub boundary_score: f64,
    pub can_split_after: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DpCostBreakdown {
    pub segment_penalty: f64,
    pub length_penalty: f64,
    pub boundary_reward: f64,
    pub total_cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Utterance {
    pub id: String,
    pub speaker: String,
    pub source_language: String,
    pub speaker_turn_id: Option<String>,
    /// Выставляется на конкретной реплике при связывании бэкчанелов (cg_N)
    pub context_group_id: Option<String>,
    pub continues_from: Option<String>,
    pub start: Seconds,
    pub end: Seconds,
    pub text: String,
    pub units: Vec<AlignedUnit>,
    pub internal_pauses: Vec<InternalPause>,
    pub segmentation: SegmentationMeta,
    pub overlap: bool,
    pub boundary_candidates: Vec<BoundaryCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dp_cost_breakdown: Option<DpCostBreakdown>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_types_serde_roundtrip() {
        let asr_unit = AsrUnit {
            text: "Hello".to_string(),
            start: Seconds(1.0),
            end: Seconds(1.5),
            confidence: Some(0.95),
            is_whisper_boundary: false,
            can_split_after: true,
        };
        let json = serde_json::to_string(&asr_unit).unwrap();
        let de: AsrUnit = serde_json::from_str(&json).unwrap();
        assert_eq!(asr_unit, de);
        assert_eq!(de.text(), "Hello");
        assert_eq!(de.start(), Seconds(1.0));
        assert_eq!(de.end(), Seconds(1.5));
        assert!(de.can_split_after());
        assert!(!de.is_whisper_boundary());

        let aligned = AlignedUnit {
            text: "World".to_string(),
            start: Seconds(1.6),
            end: Seconds(2.0),
            speaker: "SPEAKER_00".to_string(),
            word_confidence: Some(0.9),
            speaker_confidence: 0.85,
            overlap_ratio: 0.0,
            ambiguity: AmbiguityStatus::Clear,
            alternative_speaker: None,
            alternative_overlap_ratio: 0.0,
            is_whisper_boundary: true,
            can_split_after: true,
        };
        let json = serde_json::to_string(&aligned).unwrap();
        let de_aligned: AlignedUnit = serde_json::from_str(&json).unwrap();
        assert_eq!(aligned, de_aligned);
        assert_eq!(de_aligned.text(), "World");

        let utt = Utterance {
            id: "utt_001".to_string(),
            speaker: "SPEAKER_00".to_string(),
            source_language: "ru".to_string(),
            speaker_turn_id: Some("turn_001".to_string()),
            context_group_id: Some("cg_1".to_string()),
            continues_from: None,
            start: Seconds(0.0),
            end: Seconds(2.0),
            text: "Привет мир.".to_string(),
            units: vec![aligned],
            internal_pauses: vec![InternalPause {
                after_unit_index: 0,
                start: Seconds(0.5),
                end: Seconds(0.7),
                duration: Seconds(0.2),
            }],
            segmentation: SegmentationMeta {
                reason: SegmentationReason::PunctuationPause,
                confidence: 0.95,
                is_oversize: false,
                is_soft_max_exceeded: false,
            },
            overlap: false,
            boundary_candidates: vec![BoundaryCandidate {
                after_unit_index: 0,
                time_offset: Seconds(0.5),
                boundary_score: 1.2,
                can_split_after: true,
            }],
            dp_cost_breakdown: Some(DpCostBreakdown {
                segment_penalty: 2.5,
                length_penalty: 0.5,
                boundary_reward: 1.8,
                total_cost: 1.2,
            }),
        };

        let json_utt = serde_json::to_string(&utt).unwrap();
        let de_utt: Utterance = serde_json::from_str(&json_utt).unwrap();
        assert_eq!(utt, de_utt);
    }

    #[test]
    fn test_asr_unit_default_can_split_after() {
        // When can_split_after is omitted from JSON, it should default to true
        let raw_json = r#"{"text":"test","start":0.0,"end":1.0}"#;
        let unit: AsrUnit = serde_json::from_str(raw_json).unwrap();
        assert!(unit.can_split_after);
        assert_eq!(unit.confidence, None);
        assert!(!unit.is_whisper_boundary);
    }
}
