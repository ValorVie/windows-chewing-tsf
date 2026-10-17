// SPDX-License-Identifier: GPL-3.0-or-later

use chewing::d512::{D512Error, OrtSentenceScorer, SentenceScorer};
use chewing_tip_core::ipc::{
    d512::{D512Score, scoring_reply},
    varlink::MethodReply,
};
use std::{path::Path, sync::Mutex, time::Instant};

pub(crate) struct SharedScorer {
    scorer: Mutex<Result<Box<dyn SentenceScorer + Send>, D512Error>>,
}
impl SharedScorer {
    fn new(scorer: Result<Box<dyn SentenceScorer + Send>, D512Error>) -> Self {
        Self {
            scorer: Mutex::new(scorer),
        }
    }
    pub(crate) fn load_from_directory(directory: &Path) -> Self {
        Self::new(
            OrtSentenceScorer::load(directory, &directory.join("onnxruntime.dll"))
                .map(|s| Box::new(s) as Box<dyn SentenceScorer + Send>),
        )
    }
    pub(crate) fn score(&self, message: D512Score, received: Instant) -> MethodReply {
        let expected = message.candidates.len();
        let result = (|| {
            message.validate()?;
            let deadline = received
                .checked_add(std::time::Duration::from_micros(message.remaining_micros))
                .ok_or(D512Error::InvalidConfig)?;
            if Instant::now() >= deadline {
                return Err(D512Error::Timeout);
            }
            let mut state = self.scorer.try_lock().map_err(|_| D512Error::Busy)?;
            let scorer = state.as_mut().map_err(|error| *error)?;
            if Instant::now() >= deadline {
                return Err(D512Error::Timeout);
            }
            let scores = scorer.score(&message.into_request(), deadline)?;
            if Instant::now() >= deadline {
                return Err(D512Error::Timeout);
            }
            Ok(scores)
        })();
        scoring_reply(result, expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chewing::d512::SentenceRequest;
    use chewing_tip_core::ipc::d512::scores_from_reply;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    struct Fixture {
        calls: Arc<AtomicUsize>,
        result: Result<Vec<f64>, D512Error>,
        delay: Duration,
    }
    impl SentenceScorer for Fixture {
        fn score(
            &mut self,
            request: &SentenceRequest,
            deadline: Instant,
        ) -> Result<Vec<f64>, D512Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.context, "測試");
            assert_eq!(request.reading, ["ㄉㄞˋ", "ㄅㄧㄠˇ"]);
            assert_eq!(request.candidates, ["代表", "代錶"]);
            assert!(deadline <= Instant::now() + Duration::from_millis(101));
            std::thread::sleep(self.delay);
            self.result.clone()
        }
    }
    fn message() -> D512Score {
        D512Score {
            context: "測試".into(),
            reading: vec!["ㄉㄞˋ".into(), "ㄅㄧㄠˇ".into()],
            candidates: vec!["代表".into(), "代錶".into()],
            remaining_micros: 100_000,
        }
    }
    fn state(
        result: Result<Vec<f64>, D512Error>,
        delay: Duration,
    ) -> (SharedScorer, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            SharedScorer::new(Ok(Box::new(Fixture {
                calls: calls.clone(),
                result,
                delay,
            }))),
            calls,
        )
    }
    #[test]
    fn scores_share_one_backend_and_keep_native_candidate_order() {
        let (state, calls) = state(Ok(vec![-10., -2.]), Duration::ZERO);
        for _ in 0..2 {
            assert_eq!(
                scores_from_reply(state.score(message(), Instant::now()), 2).unwrap(),
                [-10., -2.]
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn load_failure_is_returned_without_constructing_an_editor() {
        let state = SharedScorer::new(Err(D512Error::MissingFiles));
        assert_eq!(
            scores_from_reply(state.score(message(), Instant::now()), 2),
            Err(D512Error::MissingFiles)
        );
    }
    #[test]
    fn occupied_backend_returns_busy_without_queuing() {
        let (state, calls) = state(Ok(vec![-10., -2.]), Duration::ZERO);
        let _held = state.scorer.lock().unwrap();
        assert_eq!(
            scores_from_reply(state.score(message(), Instant::now()), 2),
            Err(D512Error::Busy)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn expired_budget_never_starts_inference() {
        let (state, calls) = state(Ok(vec![-10., -2.]), Duration::ZERO);
        assert_eq!(
            scores_from_reply(
                state.score(message(), Instant::now() - Duration::from_secs(1)),
                2
            ),
            Err(D512Error::Timeout)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn malformed_request_never_starts_inference() {
        let (state, calls) = state(Ok(vec![-10., -2.]), Duration::ZERO);
        let mut bad = message();
        bad.candidates[1] = "錯".into();
        assert_eq!(
            scores_from_reply(state.score(bad, Instant::now()), 2),
            Err(D512Error::OutOfLimits)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn late_success_and_nonfinite_scores_are_errors() {
        let (late, _) = state(Ok(vec![-10., -2.]), Duration::from_millis(20));
        let mut short = message();
        short.remaining_micros = 5_000;
        assert_eq!(
            scores_from_reply(late.score(short, Instant::now()), 2),
            Err(D512Error::Timeout)
        );
        let (nan, _) = state(Ok(vec![-10., f64::NAN]), Duration::ZERO);
        assert_eq!(
            scores_from_reply(nan.score(message(), Instant::now()), 2),
            Err(D512Error::NonFinite)
        );
    }
    #[test]
    #[ignore = "requires immutable model and absolute ORT 1.31 DLL in C2 scratch"]
    fn native_ort_scores_without_host_editor() {
        let directory =
            std::path::PathBuf::from(std::env::var_os("C2_D512_ASSETS").expect("C2_D512_ASSETS"));
        assert!(directory.is_absolute());
        let state = SharedScorer::load_from_directory(&directory);
        let mut wire = message();
        wire.remaining_micros = 5_000_000;
        let scores = scores_from_reply(state.score(wire, Instant::now()), 2).unwrap();
        assert_eq!(scores.len(), 2);
        assert!(scores.iter().all(|s| s.is_finite()));
    }
}
