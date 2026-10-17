// SPDX-License-Identifier: GPL-3.0-or-later

use chewing::{
    d512::{D512Error, SentenceScorer},
    editor::Editor,
};
use chewing_tip_core::{config::ChewingTsfConfig, ipc::d512_client::IpcSentenceScorer};

pub(super) fn configure_editor(
    editor: &mut Editor,
    cfg: &ChewingTsfConfig,
) -> Result<(), D512Error> {
    configure_with(editor, cfg, || Box::new(IpcSentenceScorer))
}
fn configure_with(
    editor: &mut Editor,
    cfg: &ChewingTsfConfig,
    factory: impl FnOnce() -> Box<dyn SentenceScorer + Send>,
) -> Result<(), D512Error> {
    use chewing::d512::{D512Mode, D512Options};
    let mode = cfg.effective_d512_mode();
    let enabled = mode != 0;
    let (weight, margin) = if mode == 2 {
        (cfg.d512_suggest_weight, cfg.d512_suggest_margin)
    } else {
        (cfg.d512_weight, cfg.d512_margin)
    };
    let options = D512Options {
        mode: match mode {
            0 => D512Mode::Off,
            2 => D512Mode::Suggestion,
            _ => D512Mode::Automatic,
        },
        weight,
        margin,
        ..D512Options::default()
    };
    let scorer = if enabled { Some(factory()) } else { None };
    editor.set_sentence_scorer(scorer, options)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AcceptanceGesture {
    None,
    Dedicated,
    Forward,
}

pub(super) fn acceptance_gesture(
    editor: &Editor,
    cfg: &ChewingTsfConfig,
    event: chewing::input::KeyboardEvent,
) -> AcceptanceGesture {
    use chewing::{
        editor::{ConversionEngineKind, LanguageMode},
        input::{
            KeyState,
            keysym::{SYM_RIGHT, SYM_TAB},
        },
    };
    if cfg.effective_d512_mode() != 2
        || !editor.is_entering()
        || editor.is_empty()
        || editor.entering_syllable()
        || editor.editor_options().language_mode != LanguageMode::Chinese
        || editor.editor_options().conversion_engine != ConversionEngineKind::ChewingEngine
        || [KeyState::Shift, KeyState::Alt, KeyState::Super]
            .iter()
            .any(|key| event.is_state_on(*key))
    {
        return AcceptanceGesture::None;
    }
    let control = event.is_state_on(KeyState::Control);
    match cfg.effective_d512_suggestion_accept_key() {
        0 if event.ksym == SYM_TAB && !control => AcceptanceGesture::Dedicated,
        1 if event.ksym == SYM_TAB && control => AcceptanceGesture::Dedicated,
        2 if event.ksym == SYM_RIGHT && !control && editor.cursor() == editor.len() => {
            AcceptanceGesture::Forward
        }
        _ => AcceptanceGesture::None,
    }
}

pub(super) fn accept_current(
    editor: &mut Editor,
    expected: &chewing::d512::D512Suggestion,
) -> bool {
    let Some(report) = editor.d512_report() else {
        return false;
    };
    let Some(current) = report.suggestion.filter(|_| report.fallback.is_none()) else {
        return false;
    };
    current.text == expected.text
        && current.outcome_index == expected.outcome_index
        && editor.accept_d512_suggestion()
}

pub(super) fn confirmed_tail(units: &[u16]) -> String {
    // A 128-unit backwards range may start in the middle of a surrogate pair.
    let units = if units
        .first()
        .is_some_and(|unit| (0xDC00..=0xDFFF).contains(unit))
    {
        &units[1..]
    } else {
        units
    };
    let Ok(text) = String::from_utf16(units) else {
        return String::new();
    };
    let mut chars: Vec<_> = text.chars().rev().take(64).collect();
    chars.reverse();
    chars.into_iter().collect()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use chewing::{
        d512::{D512Error, SentenceRequest},
        editor::{BasicEditor, EditorBuilder},
        input::keymap::{QWERTY_MAP, map_ascii},
        syl,
        zhuyin::Bopomofo::*,
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Instant,
    };
    struct Fixture {
        calls: Arc<AtomicUsize>,
        error: Option<D512Error>,
        nonfinite: bool,
    }
    impl SentenceScorer for Fixture {
        fn score(
            &mut self,
            request: &SentenceRequest,
            _deadline: Instant,
        ) -> Result<Vec<f64>, D512Error> {
            assert_eq!(request.context, "前文");
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.error {
                return Err(error);
            }
            Ok(request
                .candidates
                .iter()
                .map(|text| {
                    if self.nonfinite {
                        f64::NAN
                    } else if text == "代錶" {
                        100.
                    } else {
                        -100.
                    }
                })
                .collect())
        }
    }
    pub(crate) fn editor() -> Editor {
        let editor = EditorBuilder::new().build();
        editor.user_dict().insert(&[syl![D, AI, TONE4]], "代");
        editor.user_dict().insert(&[syl![B, I, AU, TONE3]], "表");
        editor.user_dict().insert(&[syl![B, I, AU, TONE3]], "錶");
        editor
            .user_dict()
            .insert(&[syl![D, AI, TONE4], syl![B, I, AU, TONE3]], "代表");
        editor
            .user_dict()
            .insert(&[syl![D, AI, TONE4], syl![B, I, AU, TONE3]], "代錶");
        editor
            .user_dict()
            .boost(&[syl![D, AI, TONE4], syl![B, I, AU, TONE3]], "代表");
        editor
    }
    pub(crate) fn type_reading(editor: &mut Editor) {
        for key in b"2941ul3" {
            editor.process_keyevent(map_ascii(&QWERTY_MAP, *key));
        }
    }
    pub(crate) fn configure(
        editor: &mut Editor,
        cfg: &ChewingTsfConfig,
        error: Option<D512Error>,
        nonfinite: bool,
    ) -> Arc<AtomicUsize> {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        configure_with(editor, cfg, || {
            Box::new(Fixture {
                calls: count,
                error,
                nonfinite,
            })
        })
        .unwrap();
        editor.set_d512_context("前文");
        calls
    }
    #[test]
    fn d512_off_does_not_construct_or_call_a_transport() {
        let mut editor = editor();
        let cfg = ChewingTsfConfig {
            d512_mode: 0,
            ..Default::default()
        };
        configure_with(&mut editor, &cfg, || panic!("off mode constructed IPC")).unwrap();
        type_reading(&mut editor);
        assert_eq!(editor.display(), "代表");
    }
    #[test]
    fn d512_automatic_scores_real_editor_candidates() {
        let mut editor = editor();
        let calls = configure(&mut editor, &ChewingTsfConfig::default(), None, false);
        type_reading(&mut editor);
        assert_eq!(editor.display(), "代錶");
        assert!(calls.load(Ordering::SeqCst) > 0);
        assert!(editor.d512_report().unwrap().fallback.is_none());
    }
    #[test]
    fn d512_suggestion_preserves_native_until_explicit_acceptance() {
        let mut editor = editor();
        let cfg = ChewingTsfConfig {
            d512_mode: 2,
            ..Default::default()
        };
        let calls = configure(&mut editor, &cfg, None, false);
        type_reading(&mut editor);
        assert_eq!(editor.display(), "代表");
        let suggestion = editor.d512_report().unwrap().suggestion.unwrap();
        let before = calls.load(Ordering::SeqCst);
        let cursor = editor.cursor();
        assert!(accept_current(&mut editor, &suggestion));
        assert_eq!(calls.load(Ordering::SeqCst), before);
        assert_eq!(editor.cursor(), cursor);
        assert_eq!(editor.display_commit(), "");
        assert_eq!(editor.display(), "代錶");
        assert!(!accept_current(&mut editor, &suggestion));
        editor.process_keyevent(
            chewing::input::KeyboardEvent::builder()
                .ksym(chewing::input::keysym::SYM_RETURN)
                .build(),
        );
        assert_eq!(editor.display_commit(), "代錶");
        assert!(calls.load(Ordering::SeqCst) > 0);
        assert_eq!(cfg.d512_mode, 2);
    }
    #[test]
    fn d512_errors_and_invalid_scores_keep_the_entire_native_order() {
        let mut native = editor();
        type_reading(&mut native);
        let original = native.hypotheses();
        assert_eq!(native.display(), "代表");
        for error in [
            D512Error::MissingFiles,
            D512Error::RuntimeUnavailable,
            D512Error::Timeout,
            D512Error::Busy,
            D512Error::Inference,
        ] {
            let mut editor = editor();
            let calls = configure(
                &mut editor,
                &ChewingTsfConfig::default(),
                Some(error),
                false,
            );
            type_reading(&mut editor);
            assert_eq!(editor.hypotheses(), original);
            assert!(calls.load(Ordering::SeqCst) > 0);
            assert_eq!(editor.d512_report().unwrap().fallback, Some(error));
        }
        let mut editor = editor();
        configure(&mut editor, &ChewingTsfConfig::default(), None, true);
        type_reading(&mut editor);
        assert_eq!(editor.hypotheses(), original);
        assert_eq!(
            editor.d512_report().unwrap().fallback,
            Some(D512Error::NonFinite)
        );
    }
    #[test]
    fn d512_weight_and_margin_reach_the_engine() {
        for cfg in [
            ChewingTsfConfig {
                d512_weight: 0.,
                ..Default::default()
            },
            ChewingTsfConfig {
                d512_margin: 1000.,
                ..Default::default()
            },
        ] {
            let mut editor = editor();
            let calls = configure(&mut editor, &cfg, None, false);
            type_reading(&mut editor);
            assert_eq!(editor.display(), "代表");
            assert!(calls.load(Ordering::SeqCst) > 0);
        }
    }
    #[test]
    fn d512_context_is_limited_by_unicode_scalars_not_utf16_units() {
        let text = "甲".repeat(3) + &"𠮷".repeat(64);
        let units: Vec<_> = text.encode_utf16().collect();
        assert_eq!(confirmed_tail(&units), "𠮷".repeat(64));
        assert_eq!(
            confirmed_tail(&"前文".encode_utf16().collect::<Vec<_>>()),
            "前文"
        );
    }
    #[test]
    fn d512_context_discards_a_split_leading_surrogate_and_invalid_text() {
        assert_eq!(confirmed_tail(&[0xDFB7, 0x524D, 0x6587]), "前文");
        assert_eq!(confirmed_tail(&[0x524D, 0xD842]), "");
    }
    pub(crate) fn parameter_config(
        mode: i32,
        automatic: (f64, f64),
        suggest: (f64, f64),
    ) -> ChewingTsfConfig {
        let mut json = serde_json::to_value(ChewingTsfConfig::default()).unwrap();
        json["d512_mode"] = mode.into();
        json["d512_weight"] = automatic.0.into();
        json["d512_margin"] = automatic.1.into();
        json["d512_suggest_weight"] = suggest.0.into();
        json["d512_suggest_margin"] = suggest.1.into();
        serde_json::from_value(json).unwrap()
    }
    #[test]
    fn d512_each_mode_uses_only_its_own_parameters() {
        for (mode, auto, suggest, has_suggestion) in [
            (1, (0.25, 0.25), (0., 1000.), true),
            (1, (0., 1000.), (0.25, 0.25), false),
            (2, (0., 1000.), (0.25, 0.25), true),
            (2, (0.25, 0.25), (0., 1000.), false),
        ] {
            let mut e = editor();
            configure(&mut e, &parameter_config(mode, auto, suggest), None, false);
            type_reading(&mut e);
            assert_eq!(
                e.display(),
                if mode == 1 && has_suggestion {
                    "代錶"
                } else {
                    "代表"
                }
            );
            assert_eq!(
                e.d512_report().unwrap().suggestion.is_some(),
                has_suggestion
            );
        }
    }
    fn event(key: chewing::input::keysym::Keysym, modifiers: u8) -> chewing::input::KeyboardEvent {
        chewing::input::KeyboardEvent::builder()
            .ksym(key)
            .shift_if(modifiers & 1 != 0)
            .control_if(modifiers & 2 != 0)
            .alt_if(modifiers & 4 != 0)
            .super_if(modifiers & 8 != 0)
            .build()
    }
    #[test]
    fn d512_gesture_matches_only_the_configured_key_and_modifiers() {
        use AcceptanceGesture::*;
        use chewing::input::keysym::{SYM_RIGHT, SYM_TAB};
        let mut e = editor();
        type_reading(&mut e);
        for key in 0..=2 {
            let cfg = ChewingTsfConfig {
                d512_mode: 2,
                d512_suggestion_accept_key: key,
                ..Default::default()
            };
            for modifiers in 0..16 {
                assert_eq!(
                    acceptance_gesture(&e, &cfg, event(SYM_TAB, modifiers)),
                    if (key == 0 && modifiers == 0) || (key == 1 && modifiers == 2) {
                        Dedicated
                    } else {
                        None
                    }
                );
                assert_eq!(
                    acceptance_gesture(&e, &cfg, event(SYM_RIGHT, modifiers)),
                    if key == 2 && modifiers == 0 {
                        Forward
                    } else {
                        None
                    }
                );
            }
        }
    }
    #[test]
    fn d512_gesture_probe_does_not_mutate_or_score() {
        use chewing::input::keysym::SYM_TAB;
        let mut e = editor();
        let cfg = parameter_config(2, (0.25, 0.25), (0.25, 0.25));
        let calls = configure(&mut e, &cfg, None, false);
        type_reading(&mut e);
        let display = e.display();
        let before = calls.load(Ordering::SeqCst);
        let suggestion = e.d512_report().unwrap().suggestion.unwrap().text;
        for _ in 0..2 {
            assert_eq!(
                acceptance_gesture(&e, &cfg, event(SYM_TAB, 0)),
                AcceptanceGesture::Dedicated
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), before);
        assert_eq!(
            e.d512_report().unwrap().suggestion.unwrap().text,
            suggestion
        );
        assert_eq!(e.display(), display);
    }
    #[test]
    fn d512_gesture_excludes_partial_input_candidates_and_other_modes() {
        use chewing::input::keysym::{SYM_LEFT, SYM_RIGHT, SYM_TAB};
        let mut e = editor();
        let mut cfg = ChewingTsfConfig {
            d512_mode: 2,
            ..Default::default()
        };
        assert_eq!(
            acceptance_gesture(&e, &cfg, event(SYM_TAB, 0)),
            AcceptanceGesture::None
        );
        type_reading(&mut e);
        for mode in [0, 1] {
            cfg.d512_mode = mode;
            assert_eq!(
                acceptance_gesture(&e, &cfg, event(SYM_TAB, 0)),
                AcceptanceGesture::None
            );
        }
        cfg.d512_mode = 2;
        e.start_selecting().unwrap();
        assert_eq!(
            acceptance_gesture(&e, &cfg, event(SYM_TAB, 0)),
            AcceptanceGesture::None
        );
        e.cancel_selecting().unwrap();
        e.process_keyevent(map_ascii(&QWERTY_MAP, b'2'));
        assert_eq!(
            acceptance_gesture(&e, &cfg, event(SYM_TAB, 0)),
            AcceptanceGesture::None
        );
        e.cancel_entering_syllable();
        cfg.d512_suggestion_accept_key = 2;
        e.process_keyevent(event(SYM_LEFT, 0));
        assert_eq!(
            acceptance_gesture(&e, &cfg, event(SYM_RIGHT, 0)),
            AcceptanceGesture::None
        );
    }
    #[test]
    fn d512_acceptance_rejects_a_different_or_stale_report() {
        let mut e = editor();
        configure(
            &mut e,
            &parameter_config(2, (0.25, 0.25), (0.25, 0.25)),
            None,
            false,
        );
        type_reading(&mut e);
        let _ = e.display();
        let mut suggestion = e.d512_report().unwrap().suggestion.unwrap();
        suggestion.text = "錯字".into();
        assert!(!accept_current(&mut e, &suggestion));
        let suggestion = e.d512_report().unwrap().suggestion.unwrap();
        e.set_d512_context("改變");
        assert!(!accept_current(&mut e, &suggestion));
    }
}
