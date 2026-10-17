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
    let enabled = cfg.effective_d512_mode() != 0;
    let options = D512Options {
        mode: if enabled {
            D512Mode::Automatic
        } else {
            D512Mode::Off
        },
        weight: cfg.d512_weight,
        margin: cfg.d512_margin,
        ..D512Options::default()
    };
    let scorer = if enabled { Some(factory()) } else { None };
    editor.set_sentence_scorer(scorer, options)
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
mod tests {
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
    fn editor() -> Editor {
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
    fn type_reading(editor: &mut Editor) {
        for key in b"2941ul3" {
            editor.process_keyevent(map_ascii(&QWERTY_MAP, *key));
        }
    }
    fn configure(
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
    fn d512_suggestion_setting_uses_effective_mode_without_rewriting_it() {
        let mut editor = editor();
        let cfg = ChewingTsfConfig {
            d512_mode: 2,
            ..Default::default()
        };
        let calls = configure(&mut editor, &cfg, None, false);
        type_reading(&mut editor);
        assert_eq!(editor.display(), "代錶");
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
}
