// SPDX-License-Identifier: GPL-3.0-or-later

use serde::{Deserialize, Serialize};
use std::fmt;

use super::super::varlink::MethodCall;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationCandidate {
    pub text: String,
    pub raw: f64,
    pub model_log_p: Option<f64>,
    pub personal_hits: i64,
    pub fused: Option<f64>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationRecord {
    pub id: String,
    pub phase: String,
    pub reading: Vec<String>,
    pub candidates: Vec<CalibrationCandidate>,
    pub default_index: i64,
    pub winner_index: i64,
    pub applied: bool,
    pub personal_conflict: bool,
    pub pom_conflict: bool,
    pub weight: f64,
    pub margin: f64,
    pub bonus: f64,
    pub mode: String,
    pub suggested: bool,
    pub accepted: Option<bool>,
    pub final_committed_text: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CalibrationError {
    InvalidRecord,
    OutOfLimits,
}

impl CalibrationRecord {
    pub const METHOD: &str = "im.chewing.d512.CalibrationLog";

    pub fn validate(&self) -> Result<(), CalibrationError> {
        let index_valid = |index: i64| index >= 0 && (index as usize) < self.candidates.len();
        if self.id.is_empty()
            || !matches!(self.phase.as_str(), "rerank" | "commit" | "cancel")
            // Matches the actual Mac record producer, including its "suggest" spelling.
            || !matches!(self.mode.as_str(), "automatic" | "suggest")
            || !index_valid(self.default_index)
            || !index_valid(self.winner_index)
            || !self.weight.is_finite() || self.weight < 0.
            || !self.margin.is_finite() || self.margin < 0.
            || self.bonus != 0. || self.personal_conflict || self.pom_conflict
            || self.candidates.iter().any(|candidate| {
                !candidate.raw.is_finite()
                    || candidate.model_log_p.is_some_and(|value| !value.is_finite())
                    || candidate.fused.is_some_and(|value| !value.is_finite())
                    || candidate.personal_hits != 0
            })
        {
            return Err(CalibrationError::InvalidRecord);
        }
        Ok(())
    }
    pub fn into_call(self) -> Result<MethodCall, CalibrationError> {
        self.validate()?;
        let call = MethodCall {
            method: Self::METHOD.into(),
            parameters: serde_json::to_value(self).map_err(|_| CalibrationError::InvalidRecord)?,
            oneway: Some(true),
            more: Some(false),
            upgrade: Some(false),
        };
        let bytes = serde_json::to_vec(&call).map_err(|_| CalibrationError::InvalidRecord)?;
        if bytes.len() >= super::MAX_FRAME_BYTES {
            return Err(CalibrationError::OutOfLimits);
        }
        Ok(call)
    }
    pub fn to_json_line(&self) -> Result<Vec<u8>, CalibrationError> {
        self.validate()?;
        let mut value = serde_json::to_value(self).map_err(|_| CalibrationError::InvalidRecord)?;
        value.sort_all_objects();
        let mut bytes = serde_json::to_vec(&value).map_err(|_| CalibrationError::InvalidRecord)?;
        if bytes.len() >= super::MAX_FRAME_BYTES {
            return Err(CalibrationError::OutOfLimits);
        }
        bytes.push(b'\n');
        Ok(bytes)
    }
}

impl fmt::Debug for CalibrationRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CalibrationRecord")
            .field("syllables", &self.reading.len())
            .field("candidates", &self.candidates.len())
            .finish_non_exhaustive()
    }
}
impl fmt::Debug for CalibrationCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CalibrationCandidate")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record() -> CalibrationRecord {
        CalibrationRecord {
            id: "synthetic-private-id".into(),
            phase: "rerank".into(),
            reading: vec!["ㄐㄧㄚˇ".into(), "ㄧˇ".into()],
            candidates: vec![CalibrationCandidate {
                text: "甲乙".into(),
                raw: -1.0,
                model_log_p: None,
                personal_hits: 0,
                fused: None,
            }],
            default_index: 0,
            winner_index: 0,
            applied: false,
            personal_conflict: false,
            pom_conflict: false,
            weight: 0.25,
            margin: 2.0,
            bonus: 0.0,
            mode: "suggest".into(),
            suggested: false,
            accepted: None,
            final_committed_text: None,
        }
    }
    #[test]
    fn calibration_oneway_roundtrip_preserves_all_mac_fields_and_phases() {
        for phase in ["rerank", "commit", "cancel"] {
            for mode in ["automatic", "suggest"] {
                let mut input = record();
                input.phase = phase.into();
                input.mode = mode.into();
                if phase == "commit" {
                    input.accepted = Some(true);
                    input.final_committed_text = Some("甲乙".into());
                }
                let call = input.clone().into_call().unwrap();
                assert_eq!(call.method, "im.chewing.d512.CalibrationLog");
                assert_eq!(call.oneway, Some(true));
                assert_ne!(call.more, Some(true));
                assert_ne!(call.upgrade, Some(true));
                let decoded: MethodCall =
                    serde_json::from_slice(&serde_json::to_vec(&call).unwrap()).unwrap();
                assert_eq!(
                    serde_json::from_value::<CalibrationRecord>(decoded.parameters).unwrap(),
                    input
                );
            }
        }
    }
    #[test]
    fn calibration_json_line_uses_mac_keys_null_scores_and_sorted_objects() {
        let bytes = record().to_json_line().unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.starts_with("{\"accepted\":null,\"applied\":false,\"bonus\":0.0,\"candidates\":[{\"fused\":null,\"modelLogP\":null,"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            json!({
                "id":"synthetic-private-id","phase":"rerank","reading":["ㄐㄧㄚˇ","ㄧˇ"],
                "candidates":[{"text":"甲乙","raw":-1.0,"modelLogP":null,"personalHits":0,"fused":null}],
                "defaultIndex":0,"winnerIndex":0,"applied":false,"personalConflict":false,"pomConflict":false,
                "weight":0.25,"margin":2.0,"bonus":0.0,"mode":"suggest","suggested":false,
                "accepted":null,"finalCommittedText":null
            })
        );
    }
    #[test]
    fn calibration_debug_never_reveals_id_reading_candidates_or_committed_text() {
        let mut input = record();
        input.final_committed_text = Some("合成送出字串".into());
        for debug in [format!("{input:?}"), format!("{:?}", input.candidates[0])] {
            for secret in [
                "synthetic-private-id",
                "ㄐㄧㄚˇ",
                "ㄧˇ",
                "甲乙",
                "合成送出字串",
            ] {
                assert!(!debug.contains(secret), "Calibration Debug exposed content");
            }
        }
        let debug = format!("{:?}", input.into_call().unwrap());
        assert!(!debug.contains("甲乙"));
        assert!(!debug.contains("合成送出字串"));
        assert!(debug.contains("<redacted>"));
    }
    #[test]
    fn calibration_rejects_nonfinite_values_and_non_windows_fields() {
        for kind in 0..10 {
            let mut invalid = record();
            match kind {
                0 => invalid.candidates[0].raw = f64::NAN,
                1 => invalid.candidates[0].model_log_p = Some(f64::INFINITY),
                2 => invalid.candidates[0].fused = Some(f64::NAN),
                3 => invalid.weight = f64::NEG_INFINITY,
                4 => invalid.margin = -1.0,
                5 => invalid.bonus = 1.0,
                6 => invalid.candidates[0].personal_hits = 1,
                7 => invalid.personal_conflict = true,
                8 => invalid.pom_conflict = true,
                _ => invalid.winner_index = 1,
            }
            assert_eq!(invalid.validate(), Err(CalibrationError::InvalidRecord));
        }
    }
    #[test]
    fn calibration_rejects_wrong_phase_mode_and_oversized_frames() {
        let mut invalid = record();
        invalid.phase = "other".into();
        assert!(invalid.validate().is_err());
        invalid = record();
        invalid.mode = "suggestion".into();
        assert!(invalid.validate().is_err());
        invalid = record();
        invalid.final_committed_text = Some("x".repeat(super::super::MAX_FRAME_BYTES));
        assert!(matches!(
            invalid.into_call(),
            Err(CalibrationError::OutOfLimits)
        ));
    }
}
