// SPDX-License-Identifier: GPL-3.0-or-later

use std::{fmt, time::Instant};

use chewing::d512::{D512Error, SentenceRequest};
use serde::{Deserialize, Serialize};

use super::varlink::{MethodCall, MethodReply};

pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_BUDGET_MICROS: u64 = 5_000_000;

#[derive(Clone, Serialize, Deserialize)]
pub struct D512Score {
    pub context: String,
    pub reading: Vec<String>,
    pub candidates: Vec<String>,
    pub remaining_micros: u64,
}

impl D512Score {
    pub const METHOD: &str = "im.chewing.d512.Score";

    pub fn from_request(request: &SentenceRequest, deadline: Instant) -> Result<Self, D512Error> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(D512Error::Timeout)?;
        let message = Self {
            context: request.context.clone(),
            reading: request.reading.clone(),
            candidates: request.candidates.clone(),
            remaining_micros: remaining
                .as_micros()
                .try_into()
                .map_err(|_| D512Error::InvalidConfig)?,
        };
        message.validate()?;
        Ok(message)
    }

    pub fn validate(&self) -> Result<(), D512Error> {
        if self.remaining_micros == 0 {
            return Err(D512Error::Timeout);
        }
        if self.remaining_micros > MAX_BUDGET_MICROS {
            return Err(D512Error::InvalidConfig);
        }
        if self.context.chars().count() > 64
            || self.reading.is_empty()
            || self.reading.len() > 32
            || self.candidates.is_empty()
            || self.candidates.len() > 16
        {
            return Err(D512Error::OutOfLimits);
        }
        if self
            .reading
            .iter()
            .any(|r| r.is_empty() || r.chars().count() > 8)
        {
            return Err(D512Error::BadReading);
        }
        for (index, candidate) in self.candidates.iter().enumerate() {
            if candidate.chars().count() != self.reading.len()
                || self.candidates[..index].contains(candidate)
            {
                return Err(D512Error::OutOfLimits);
            }
        }
        Ok(())
    }

    pub fn into_request(self) -> SentenceRequest {
        SentenceRequest {
            context: self.context,
            reading: self.reading,
            candidates: self.candidates,
        }
    }

    pub fn into_call(self) -> Result<MethodCall, D512Error> {
        self.validate()?;
        Ok(MethodCall {
            method: Self::METHOD.to_string(),
            parameters: serde_json::to_value(self).map_err(|_| D512Error::Inference)?,
            oneway: Some(false),
            more: Some(false),
            upgrade: Some(false),
        })
    }
}

impl fmt::Debug for D512Score {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("D512Score")
            .field("syllables", &self.reading.len())
            .field("candidates", &self.candidates.len())
            .field("remaining_micros", &self.remaining_micros)
            .finish_non_exhaustive()
    }
}

fn check_scores(scores: Vec<f64>, expected: usize) -> Result<Vec<f64>, D512Error> {
    if expected == 0 || expected > 16 || scores.len() != expected {
        return Err(D512Error::InvalidConfig);
    }
    if scores.iter().any(|s| !s.is_finite()) {
        return Err(D512Error::NonFinite);
    }
    Ok(scores)
}

pub fn scoring_reply(result: Result<Vec<f64>, D512Error>, expected: usize) -> MethodReply {
    match result.and_then(|scores| check_scores(scores, expected)) {
        Ok(scores) => MethodReply {
            parameters: serde_json::json!({"scores": scores}),
            continues: None,
            error: None,
        },
        Err(error) => MethodReply {
            parameters: serde_json::Value::Null,
            continues: None,
            error: Some(format!("im.chewing.d512.{error:?}")),
        },
    }
}

pub fn scores_from_reply(reply: MethodReply, expected: usize) -> Result<Vec<f64>, D512Error> {
    if reply.continues == Some(true) {
        return Err(D512Error::InvalidConfig);
    }
    if let Some(error) = reply.error {
        return Err(match error.strip_prefix("im.chewing.d512.") {
            Some("InvalidConfig") => D512Error::InvalidConfig,
            Some("MissingFiles") => D512Error::MissingFiles,
            Some("InvalidModel") => D512Error::InvalidModel,
            Some("RuntimeUnavailable") => D512Error::RuntimeUnavailable,
            Some("TelemetryNotDisabled") => D512Error::TelemetryNotDisabled,
            Some("OutOfLimits") => D512Error::OutOfLimits,
            Some("Oov") => D512Error::Oov,
            Some("BadReading") => D512Error::BadReading,
            Some("ProtectedRange") => D512Error::ProtectedRange,
            Some("Timeout") => D512Error::Timeout,
            Some("Busy") => D512Error::Busy,
            Some("NonFinite") => D512Error::NonFinite,
            _ => D512Error::Inference,
        });
    }
    let scores: Vec<f64> = serde_json::from_value(
        reply
            .parameters
            .get("scores")
            .cloned()
            .ok_or(D512Error::InvalidConfig)?,
    )
    .map_err(|_| D512Error::InvalidConfig)?;
    check_scores(scores, expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn wire() -> D512Score {
        D512Score {
            context: "前文".into(),
            reading: vec!["ㄐㄧㄚˇ".into(), "ㄧˇ".into()],
            candidates: vec!["甲乙".into(), "假乙".into()],
            remaining_micros: 100_000,
        }
    }

    #[test]
    fn request_round_trip_preserves_candidate_order() {
        let input = wire().into_request();
        let message =
            D512Score::from_request(&input, Instant::now() + Duration::from_millis(100)).unwrap();
        let call = message.into_call().unwrap();
        assert_eq!(call.method, "im.chewing.d512.Score");
        assert_eq!(call.parameters["candidates"], json!(["甲乙", "假乙"]));
        let decoded: D512Score = serde_json::from_value(call.parameters).unwrap();
        assert!(decoded.remaining_micros > 0 && decoded.remaining_micros <= 100_000);
        let decoded = decoded.into_request();
        assert_eq!(decoded.context, "前文");
        assert_eq!(decoded.reading, ["ㄐㄧㄚˇ", "ㄧˇ"]);
    }

    #[test]
    fn expired_request_has_no_wire_message() {
        assert!(matches!(
            D512Score::from_request(
                &wire().into_request(),
                Instant::now() - Duration::from_millis(1)
            ),
            Err(D512Error::Timeout)
        ));
    }

    #[test]
    fn maximum_documented_shape_is_valid() {
        let mut message = wire();
        message.context = "甲".repeat(64);
        message.reading = vec!["ㄐㄧㄚˇ".into(); 32];
        message.candidates = "甲乙丙丁戊己庚辛壬癸子丑寅卯辰巳"
            .chars()
            .map(|c| format!("{c}{}", "甲".repeat(31)))
            .collect();
        message.remaining_micros = MAX_BUDGET_MICROS;
        assert_eq!(message.validate(), Ok(()));
    }

    #[test]
    fn invalid_shapes_are_rejected() {
        let mut invalid = vec![];
        let mut m = wire();
        m.context = "甲".repeat(65);
        invalid.push(m);
        let mut m = wire();
        m.reading.clear();
        invalid.push(m);
        let mut m = wire();
        m.reading = vec!["ㄐㄧㄚˇ".into(); 33];
        invalid.push(m);
        let mut m = wire();
        m.candidates.clear();
        invalid.push(m);
        let mut m = wire();
        m.candidates = vec!["甲乙".into(); 17];
        invalid.push(m);
        let mut m = wire();
        m.candidates[1] = "甲乙".into();
        invalid.push(m);
        let mut m = wire();
        m.candidates[1] = "甲".into();
        invalid.push(m);
        let mut m = wire();
        m.reading[0].clear();
        invalid.push(m);
        for message in invalid {
            assert!(message.validate().is_err());
        }
    }

    #[test]
    fn invalid_time_budgets_are_rejected() {
        let mut message = wire();
        message.remaining_micros = 0;
        assert_eq!(message.validate(), Err(D512Error::Timeout));
        message.remaining_micros = MAX_BUDGET_MICROS + 1;
        assert_eq!(message.validate(), Err(D512Error::InvalidConfig));
    }

    #[test]
    fn request_debug_omits_text_and_reading() {
        let output = format!("{:?}", wire());
        for text in ["前文", "甲乙", "假乙", "ㄐㄧㄚˇ"] {
            assert!(!output.contains(text));
        }
    }

    #[test]
    fn score_reply_preserves_natural_log_scores_in_order() {
        let reply = scoring_reply(Ok(vec![-10.0, -2.0]), 2);
        assert_eq!(scores_from_reply(reply, 2), Ok(vec![-10.0, -2.0]));
    }

    #[test]
    fn wrong_shape_or_nonfinite_scores_do_not_become_partial_results() {
        for scores in [
            vec![-10.0],
            vec![-10.0, f64::NAN],
            vec![f64::INFINITY, -2.0],
        ] {
            let reply = scoring_reply(Ok(scores), 2);
            assert!(reply.error.is_some());
            assert!(scores_from_reply(reply, 2).is_err());
        }
    }

    #[test]
    fn typed_errors_cross_the_wire_without_text() {
        for error in [
            D512Error::Timeout,
            D512Error::Busy,
            D512Error::Oov,
            D512Error::BadReading,
            D512Error::MissingFiles,
        ] {
            assert_eq!(
                scores_from_reply(scoring_reply(Err(error), 2), 2),
                Err(error)
            );
        }
    }

    #[test]
    fn unsolicited_streaming_and_unknown_errors_are_rejected() {
        let mut reply = scoring_reply(Ok(vec![-1.0, -2.0]), 2);
        reply.continues = Some(true);
        assert_eq!(scores_from_reply(reply, 2), Err(D512Error::InvalidConfig));
        reply = MethodReply {
            parameters: json!({"scores": [-1.0, -2.0]}),
            continues: None,
            error: Some("untrusted error text".into()),
        };
        assert_eq!(scores_from_reply(reply, 2), Err(D512Error::Inference));
    }
}
