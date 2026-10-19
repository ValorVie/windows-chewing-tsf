// SPDX-License-Identifier: GPL-3.0-or-later

use chewing::{
    d512::{D512Calibration, D512Error, D512Mode, SentenceRequest, SentenceScorer},
    editor::Editor,
};
use chewing_tip_core::{
    config::ChewingTsfConfig,
    ipc::d512::calibration::{CalibrationCandidate, CalibrationRecord},
};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

// Same lifecycle as Mac D512SentenceRerank: keep all evaluation receipts until
// commit/cancel; acceptance changes only its associated current receipt.
pub(super) struct Collector {
    pub(super) attempts: Arc<AtomicU64>,
    enabled: Cell<bool>,
    epoch: Cell<u64>,
    consent_epoch: Cell<u64>,
    seen: Cell<u64>,
    current: RefCell<Option<String>>,
    pending: RefCell<Vec<CalibrationRecord>>,
    sink: Box<dyn Fn(Vec<CalibrationRecord>) -> bool>,
}
impl Collector {
    pub(super) fn new() -> Self {
        Self::with_sink(chewing_tip_core::ipc::calibration_client::try_send)
    }
    pub(super) fn with_sink(sink: impl Fn(Vec<CalibrationRecord>) -> bool + 'static) -> Self {
        Self {
            attempts: Arc::new(AtomicU64::new(0)),
            enabled: Cell::new(false),
            epoch: Cell::new(0),
            consent_epoch: Cell::new(0),
            seen: Cell::new(0),
            current: RefCell::new(None),
            pending: RefCell::new(vec![]),
            sink: Box::new(sink),
        }
    }
    pub(super) fn configure(&self, cfg: &ChewingTsfConfig) {
        let enabled = cfg.effective_d512_mode() != 0 && cfg.d512_log_calibration == 1;
        if self.enabled.replace(enabled) != enabled {
            self.seen.set(self.attempts.load(Ordering::Relaxed));
        }
        if !enabled {
            // Opt-out also invalidates already detached asynchronous receipts.
            self.consent_epoch
                .set(self.consent_epoch.get().wrapping_add(1));
            self.pending.borrow_mut().clear();
            self.current.replace(None);
        }
    }
    pub(super) fn track(
        &self,
        scorer: Box<dyn SentenceScorer + Send>,
    ) -> Box<dyn SentenceScorer + Send> {
        if !self.enabled.get() {
            return scorer;
        }
        Box::new(TrackedScorer {
            inner: scorer,
            attempts: self.attempts.clone(),
        })
    }
    pub(super) fn capture(&self, editor: &Editor) {
        self.capture_with(self.attempts.load(Ordering::Relaxed), || {
            editor.d512_calibration()
        });
    }
    fn capture_with(&self, attempt: u64, source: impl FnOnce() -> Option<D512Calibration>) {
        // Do not even call the engine getter when disabled or already collected.
        if !self.enabled.get() || attempt == self.seen.get() {
            return;
        }
        self.seen.set(attempt);
        self.current.replace(None);
        let Some(data) = source() else {
            return;
        };
        let (mode, accepted) = match data.mode {
            D512Mode::Automatic => ("automatic", None),
            D512Mode::Suggestion => ("suggest", Some(false)),
            D512Mode::Off => return,
        };
        let Ok(random) = getrandom::u64().and_then(|hi| getrandom::u64().map(|lo| (hi, lo))) else {
            log::error!("D512 calibration record could not be collected.");
            return;
        };
        let record = CalibrationRecord {
            id: format!("{:016x}{:016x}", random.0, random.1),
            phase: "rerank".into(),
            reading: data.reading,
            candidates: data
                .candidates
                .into_iter()
                .map(|c| CalibrationCandidate {
                    text: c.text,
                    raw: c.raw,
                    model_log_p: c.model_logp,
                    personal_hits: c.personal_hits as i64,
                    fused: c.fused,
                })
                .collect(),
            default_index: data.default_index as i64,
            winner_index: data.winner_index as i64,
            applied: data.applied,
            personal_conflict: false,
            pom_conflict: false,
            weight: data.weight,
            margin: data.margin,
            bonus: data.bonus,
            mode: mode.into(),
            suggested: data.suggested,
            accepted,
            final_committed_text: None,
        };
        self.current.replace(Some(record.id.clone()));
        self.pending.borrow_mut().push(record.clone());
        (self.sink)(vec![record]);
    }
    pub(super) fn mark_accepted(&self) {
        if !self.enabled.get() {
            return;
        }
        let current = self.current.borrow();
        for r in self.pending.borrow_mut().iter_mut() {
            if current.as_ref() == Some(&r.id) && r.mode == "suggest" && r.suggested {
                r.accepted = Some(true);
                r.applied = true;
            }
        }
    }
    pub(super) fn leave_client(&self) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
        self.cancel();
    }
    pub(super) fn cancel(&self) {
        self.current.replace(None);
        let records = std::mem::take(&mut *self.pending.borrow_mut());
        self.finish(records, None);
    }
    fn finish(&self, mut records: Vec<CalibrationRecord>, text: Option<&str>) {
        if !self.enabled.get() || records.is_empty() {
            return;
        }
        for r in &mut records {
            r.phase = if text.is_some() { "commit" } else { "cancel" }.into();
            r.final_committed_text = text.map(str::to_owned);
        }
        (self.sink)(records);
    }
    pub(super) fn take_commit(self: &Rc<Self>) -> Rc<CommitReceipt> {
        self.current.replace(None);
        Rc::new(CommitReceipt {
            owner: Rc::downgrade(self),
            epoch: self.epoch.get(),
            consent_epoch: self.consent_epoch.get(),
            records: RefCell::new(std::mem::take(&mut *self.pending.borrow_mut())),
        })
    }
}
struct TrackedScorer {
    inner: Box<dyn SentenceScorer + Send>,
    attempts: Arc<AtomicU64>,
}
impl SentenceScorer for TrackedScorer {
    fn score(&mut self, req: &SentenceRequest, deadline: Instant) -> Result<Vec<f64>, D512Error> {
        let result = self.inner.score(req, deadline);
        self.attempts.fetch_add(1, Ordering::Relaxed);
        result
    }
}
pub(super) struct CommitReceipt {
    owner: Weak<Collector>,
    epoch: u64,
    consent_epoch: u64,
    records: RefCell<Vec<CalibrationRecord>>,
}
impl CommitReceipt {
    pub(super) fn confirm(&self, text: &str) {
        self.finish(Some(text));
    }
    fn finish(&self, text: Option<&str>) {
        let records = std::mem::take(&mut *self.records.borrow_mut());
        if let Some(c) = self.owner.upgrade()
            && c.consent_epoch.get() == self.consent_epoch
        {
            c.finish(records, text.filter(|_| c.epoch.get() == self.epoch));
        }
    }
}
impl Drop for CommitReceipt {
    fn drop(&mut self) {
        self.finish(None);
    }
}

#[cfg(test)]
mod tests {
    use super::super::d512::tests::{configure, editor, parameter_config, type_reading};
    use super::*;
    use chewing::d512::D512Error;

    fn setup(mode: i32, flag: i32) -> (Rc<Collector>, Rc<RefCell<Vec<CalibrationRecord>>>) {
        let rows = Rc::new(RefCell::new(Vec::new()));
        let output = rows.clone();
        let collector = Rc::new(Collector::with_sink(move |records| {
            output.borrow_mut().extend(records);
            true
        }));
        let mut cfg = parameter_config(mode, (0.25, 0.5), (0.25, 2.));
        cfg.d512_log_calibration = flag;
        collector.configure(&cfg);
        (collector, rows)
    }
    fn sample(mode: i32, failure: bool) -> D512Calibration {
        let mut e = editor();
        configure(
            &mut e,
            &parameter_config(mode, (0.25, 0.5), (0.25, 2.)),
            failure.then_some(D512Error::Timeout),
            false,
        );
        type_reading(&mut e);
        let _ = e.display();
        e.d512_calibration().unwrap()
    }
    #[test]
    fn calibration_disabled_never_reads_engine_or_sends() {
        for (mode, flag) in [(0, 1), (1, 0), (2, 0), (2, 2), (2, -1)] {
            let (c, rows) = setup(mode, flag);
            c.capture_with(1, || panic!("disabled collector called calibration getter"));
            c.mark_accepted();
            c.cancel();
            c.take_commit().confirm("合成送出");
            assert!(rows.borrow().is_empty());
        }
    }
    #[test]
    fn calibration_retains_each_attempt_and_finishes_all_receipts() {
        for (mode, name, applied, accepted) in [
            (1, "automatic", true, None),
            (2, "suggest", false, Some(false)),
        ] {
            let (c, rows) = setup(mode, 1);
            c.capture_with(1, || Some(sample(mode, false)));
            c.capture_with(1, || panic!("duplicate getter must not be read"));
            c.capture_with(2, || Some(sample(mode, false)));
            let r = rows.borrow();
            assert_eq!(r.len(), 2);
            assert_ne!(r[0].id, r[1].id);
            assert_eq!(r[0].phase, "rerank");
            assert_eq!(r[0].mode, name);
            assert_eq!(r[0].applied, applied);
            assert_eq!(r[0].accepted, accepted);
            assert_eq!(r[0].bonus, 0.);
            assert!(!r[0].personal_conflict);
            assert!(!r[0].pom_conflict);
            assert!(r[0].candidates.iter().all(|x| x.personal_hits == 0));
            drop(r);
            c.take_commit().confirm("代錶");
            let r = rows.borrow();
            assert_eq!(r.len(), 4);
            for i in 0..2 {
                assert_eq!(r[i].id, r[i + 2].id);
                assert_eq!(r[i + 2].phase, "commit");
                assert_eq!(r[i + 2].final_committed_text.as_deref(), Some("代錶"));
            }
        }
    }
    #[test]
    fn calibration_acceptance_marks_only_the_current_suggestion() {
        let (c, rows) = setup(2, 1);
        c.capture_with(1, || Some(sample(2, false)));
        c.capture_with(2, || Some(sample(2, false)));
        c.mark_accepted();
        c.take_commit().confirm("代錶");
        let r = rows.borrow();
        assert_eq!(r.len(), 4);
        assert_eq!(r[0].accepted, Some(false));
        assert_eq!(r[1].accepted, Some(false));
        assert_eq!(r[2].accepted, Some(false));
        assert_eq!(r[3].accepted, Some(true));
        assert!(r[3].applied);
        assert!(r[3].suggested);
    }
    #[test]
    fn calibration_cancel_and_delayed_commit_respect_client_epoch_and_disable() {
        let (c, rows) = setup(2, 1);
        c.capture_with(1, || Some(sample(2, false)));
        c.cancel();
        c.capture_with(2, || Some(sample(2, false)));
        let receipt = c.take_commit();
        c.leave_client();
        receipt.confirm("不可誤記送出");
        c.capture_with(3, || Some(sample(2, false)));
        drop(c.take_commit());
        assert_eq!(
            rows.borrow().iter().filter(|r| r.phase == "cancel").count(),
            3
        );
        assert!(
            rows.borrow()
                .iter()
                .all(|r| r.final_committed_text.is_none())
        );
        c.capture_with(4, || Some(sample(2, false)));
        let receipt = c.take_commit();
        let mut cfg = parameter_config(2, (0.25, 0.5), (0.25, 2.));
        c.configure(&cfg);
        cfg.d512_log_calibration = 1;
        c.configure(&cfg);
        receipt.confirm("關閉後不得補寫");
        assert_eq!(rows.borrow().len(), 7);
    }
    #[test]
    fn calibration_failure_preserves_null_model_scores_and_redacted_wire_debug() {
        let (c, rows) = setup(1, 1);
        c.capture_with(1, || Some(sample(1, true)));
        let r = rows.borrow();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].winner_index, 0);
        assert!(!r[0].applied);
        assert!(
            r[0].candidates
                .iter()
                .all(|x| x.model_log_p.is_none() && x.fused.is_none())
        );
        let call = r[0].clone().into_call().unwrap();
        let debug = format!("{call:?}");
        assert!(!debug.contains("代表"));
        assert!(!debug.contains("代錶"));
    }

    #[test]
    fn personal_u2_collector_maps_engine_hits_and_bonus_through_commit() {
        let (c, rows) = setup(1, 1);
        let mut data = sample(1, false);
        data.bonus = 3.5;
        data.candidates[0].personal_hits = 2;
        c.capture_with(1, || Some(data));
        c.take_commit().confirm("合成送出");
        let rows = rows.borrow();
        assert_eq!(rows.len(), 2);
        for row in rows.iter() {
            assert_eq!(row.bonus, 3.5);
            assert_eq!(row.candidates[0].personal_hits, 2);
            assert!(!row.personal_conflict && !row.pom_conflict);
            assert!(row.clone().into_call().is_ok());
        }
    }

    #[test]
    fn calibration_tip_wire_fixture_for_host() {
        let mut frames = vec![];
        for mode in [1, 2] {
            let (c, rows) = setup(mode, 1);
            c.capture_with(1, || Some(sample(mode, false)));
            if mode == 2 {
                c.mark_accepted();
            }
            c.take_commit().confirm("代錶");
            let rows = rows.borrow();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].phase, "rerank");
            assert_eq!(rows[1].phase, "commit");
            for record in rows.iter() {
                // This is the same bounded oneway encoding used by the worker.
                let call = record.clone().into_call().unwrap();
                assert_eq!(call.oneway, Some(true));
                frames.extend(serde_json::to_vec(&call).unwrap());
                frames.push(0);
            }
        }
        assert_eq!(frames.iter().filter(|b| **b == 0).count(), 4);
        // Optional test artifact only; production never writes from the TIP.
        if let Some(path) = std::env::var_os("C2_TIP_CALIBRATION_WIRE") {
            std::fs::write(path, frames).unwrap();
        }
    }
}
