// SPDX-License-Identifier: GPL-3.0-or-later

use super::d512::{AcceptanceGesture, accept_current};
use chewing::{d512::D512Suggestion, editor::Editor};
use std::{
    cell::{Cell, Ref, RefCell},
    rc::Rc,
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct DocumentSnapshot {
    pub(super) preedit: String,
    pub(super) confirmed: String,
    pub(super) cursor: usize,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum AcceptanceResult {
    Native,
    Consumed,
    Accepted,
}

pub(super) fn acceptance_prefix(
    event: chewing::input::KeyboardEvent,
    tab_gesture: AcceptanceGesture,
) -> bool {
    use chewing::input::keysym::{SYM_LEFTCTRL, SYM_RIGHTCTRL};
    matches!(event.ksym, SYM_LEFTCTRL | SYM_RIGHTCTRL)
        && tab_gesture == AcceptanceGesture::Dedicated
}

pub(super) fn dispatch_acceptance(
    editor: &mut Editor,
    gesture: AcceptanceGesture,
    pending: Option<(&DocumentSnapshot, &D512Suggestion)>,
    observed: Option<&DocumentSnapshot>,
    owner_current: bool,
) -> AcceptanceResult {
    if gesture == AcceptanceGesture::None {
        return AcceptanceResult::Native;
    }
    let accepted = owner_current
        && pending.is_some_and(|(expected, proposal)| {
            observed.is_some_and(|actual| actual == expected) && accept_current(editor, proposal)
        });
    if accepted {
        AcceptanceResult::Accepted
    } else if gesture == AcceptanceGesture::Dedicated {
        AcceptanceResult::Consumed
    } else {
        AcceptanceResult::Native
    }
}

pub(super) struct HintSlot<T> {
    pub(super) epoch: Rc<Cell<u64>>,
    value: RefCell<Option<(u64, T)>>,
}
impl<T> HintSlot<T> {
    pub(super) fn new(epoch: Rc<Cell<u64>>) -> Self {
        Self {
            epoch,
            value: RefCell::new(None),
        }
    }
    pub(super) fn withdraw(&self) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
        self.value.replace(None);
    }
    pub(super) fn prune_stale(&self) {
        let stale = self
            .value
            .borrow()
            .as_ref()
            .is_some_and(|(epoch, _)| *epoch != self.epoch.get());
        if stale {
            self.value.replace(None);
        }
    }
    pub(super) fn publish(&self, epoch: u64, value: T, visible: bool) -> bool {
        if epoch != self.epoch.get() || !visible {
            return false;
        }
        self.value.replace(Some((epoch, value)));
        true
    }
    pub(super) fn current(&self) -> Option<Ref<'_, T>> {
        Ref::filter_map(self.value.borrow(), |item| {
            item.as_ref()
                .filter(|(epoch, _)| *epoch == self.epoch.get())
                .map(|(_, value)| value)
        })
        .ok()
    }
}
pub(super) fn accept_key_label(key: i32) -> &'static str {
    match key {
        1 => "Ctrl+Tab",
        2 => "→",
        _ => "Tab",
    }
}
pub(super) fn utf16_offset(text: &str, scalar: usize) -> i32 {
    text.chars()
        .take(scalar)
        .map(char::len_utf16)
        .sum::<usize>() as i32
}
pub(super) fn hint_width(work_width: i32, dpi: f32) -> f32 {
    ((work_width as f32 - 60.) * 96. / dpi - 20.).clamp(1., 480.)
}

use super::{
    edit_session::{CompositionApplied, read_suggestion_document},
    ui_elements::{Notification, NotificationModel},
};
use crate::ui::gfx::{color_s, get_dpi_for_point};
use chewing_tip_core::config::ChewingTsfConfig;
use windows::{
    Win32::{
        Foundation::POINT,
        Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint},
        UI::TextServices::{ITfComposition, ITfContext, ITfThreadMgr},
    },
    core::{ComObject, HSTRING, IUnknown, Interface},
};

#[derive(Clone)]
pub(super) struct PendingHint {
    pub(super) context: ITfContext,
    pub(super) composition: ITfComposition,
    pub(super) document: DocumentSnapshot,
    pub(super) proposal: D512Suggestion,
    pub(super) epoch: u64,
}
pub(super) struct DisplayedHint {
    pub(super) pending: PendingHint,
    notification: ComObject<Notification>,
}
impl Drop for DisplayedHint {
    fn drop(&mut self) {
        self.notification.set_timer(std::time::Duration::ZERO);
        self.notification.end_ui_element();
    }
}
impl HintSlot<DisplayedHint> {
    pub(super) fn has_visible_current(&self) -> bool {
        self.current()
            .is_some_and(|item| item.notification.is_visible())
    }
    pub(super) fn visible_snapshot(&self) -> Option<PendingHint> {
        self.current()
            .filter(|item| item.notification.is_visible())
            .map(|item| item.pending.clone())
    }
}
pub(super) fn same_object<T: Interface>(left: &T, right: &T) -> bool {
    match (left.cast::<IUnknown>(), right.cast::<IUnknown>()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub(super) fn publication(
    slot: &Rc<HintSlot<DisplayedHint>>,
    context: ITfContext,
    thread_mgr: ITfThreadMgr,
    cfg: &ChewingTsfConfig,
    simplified: bool,
    document: DocumentSnapshot,
    proposal: D512Suggestion,
) -> CompositionApplied {
    let epoch = slot.epoch.get();
    let slot = Rc::downgrade(slot);
    let text = if simplified {
        zhconv::zhconv(&proposal.text, zhconv::Variant::ZhHans)
    } else {
        proposal.text.clone()
    };
    let model = NotificationModel {
        text: HSTRING::from(format!(
            "{} 接受：{}",
            accept_key_label(cfg.effective_d512_suggestion_accept_key()),
            text
        )),
        font_family: HSTRING::from(&cfg.font_family),
        font_size: cfg.font_size as f32,
        fg_color: color_s(&cfg.notify_fg_color),
        bg_color: color_s(&cfg.notify_bg_color),
        border_color: color_s(&cfg.notify_border_color),
        max_text_width: None,
    };
    Rc::new(move |ec, composition| {
        let Some(slot) = slot.upgrade() else {
            return;
        };
        if slot.epoch.get() != epoch {
            return;
        }
        let Ok(Some((actual, rect))) = read_suggestion_document(ec, &context, composition) else {
            return;
        };
        if actual != document {
            return;
        }
        let result = (|| -> windows::core::Result<()> {
            let hwnd = unsafe { context.GetActiveView()?.GetWnd().unwrap_or_default() };
            let notification = Notification::new(hwnd, thread_mgr.clone())
                .map_err(|_| windows::Win32::Foundation::E_FAIL)?;
            // Own every successful BeginUIElement immediately, including suppression/error paths.
            let displayed = DisplayedHint {
                notification,
                pending: PendingHint {
                    context: context.clone(),
                    composition: composition.clone(),
                    document: document.clone(),
                    proposal: proposal.clone(),
                    epoch,
                },
            };
            let point = POINT {
                x: rect.left,
                y: rect.bottom,
            };
            let mut monitor = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !unsafe {
                GetMonitorInfoW(
                    MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST),
                    &mut monitor,
                )
            }
            .as_bool()
            {
                return Ok(());
            }
            let mut model = model.clone();
            model.max_text_width = Some(hint_width(
                monitor.rcWork.right - monitor.rcWork.left,
                get_dpi_for_point(point),
            ));
            displayed.notification.set_model(model);
            if slot.epoch.get() != epoch {
                return Ok(());
            }
            displayed.notification.set_timer(std::time::Duration::ZERO);
            displayed.notification.set_position(rect.left, rect.bottom);
            displayed.notification.set_position(rect.left, rect.bottom);
            displayed.notification.show();
            let visible = displayed.notification.is_visible();
            slot.publish(epoch, displayed, visible);
            Ok(())
        })();
        if result.is_err() {
            log::debug!("Suggestion UI unavailable");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::d512::tests::{configure, editor, parameter_config, type_reading};
    use super::*;
    use std::sync::atomic::Ordering;
    fn document() -> DocumentSnapshot {
        DocumentSnapshot {
            preedit: "代表".into(),
            confirmed: "前文".into(),
            cursor: 2,
        }
    }
    #[test]
    fn d512_document_and_epoch_must_match_before_acceptance() {
        for kind in 0..5 {
            let mut e = editor();
            let cfg = parameter_config(2, (0.25, 0.25), (0.25, 0.25));
            let calls = configure(&mut e, &cfg, None, false);
            type_reading(&mut e);
            assert_eq!(e.display(), "代表");
            let proposal = e.d512_report().unwrap().suggestion.unwrap();
            let expected = document();
            let mut actual = expected.clone();
            match kind {
                0 => actual.preedit = "別字".into(),
                1 => actual.confirmed = "別處".into(),
                2 => actual.cursor = 1,
                _ => {}
            }
            let before = calls.load(Ordering::SeqCst);
            assert_eq!(
                dispatch_acceptance(
                    &mut e,
                    AcceptanceGesture::Dedicated,
                    Some((&expected, &proposal)),
                    if kind == 3 { None } else { Some(&actual) },
                    kind != 4
                ),
                AcceptanceResult::Consumed
            );
            assert_eq!(calls.load(Ordering::SeqCst), before);
            assert_eq!(e.display_commit(), "");
            assert_eq!(e.display(), "代表");
        }
    }
    #[test]
    fn d512_dedicated_empty_keys_consume_while_forward_falls_through() {
        for (gesture, result) in [
            (AcceptanceGesture::Dedicated, AcceptanceResult::Consumed),
            (AcceptanceGesture::Forward, AcceptanceResult::Native),
            (AcceptanceGesture::None, AcceptanceResult::Native),
        ] {
            let mut e = editor();
            type_reading(&mut e);
            assert_eq!(
                dispatch_acceptance(&mut e, gesture, None, None, false),
                result
            );
            assert_eq!(e.display(), "代表");
            assert_eq!(e.display_commit(), "");
        }
    }
    #[test]
    fn d512_accepted_gesture_calls_engine_once_without_commit_or_rescore() {
        let mut e = editor();
        let calls = configure(
            &mut e,
            &parameter_config(2, (0.25, 0.25), (0.25, 0.25)),
            None,
            false,
        );
        type_reading(&mut e);
        let _ = e.display();
        let proposal = e.d512_report().unwrap().suggestion.unwrap();
        let snapshot = document();
        let before = calls.load(Ordering::SeqCst);
        assert_eq!(
            dispatch_acceptance(
                &mut e,
                AcceptanceGesture::Dedicated,
                Some((&snapshot, &proposal)),
                Some(&snapshot),
                true
            ),
            AcceptanceResult::Accepted
        );
        assert_eq!(calls.load(Ordering::SeqCst), before);
        assert_eq!(e.cursor(), 2);
        assert_eq!(e.display_commit(), "");
        assert_eq!(
            dispatch_acceptance(
                &mut e,
                AcceptanceGesture::Dedicated,
                Some((&snapshot, &proposal)),
                Some(&snapshot),
                true
            ),
            AcceptanceResult::Consumed
        );
        assert_eq!(e.display(), "代錶");
    }
    #[test]
    fn d512_other_keys_never_accept_an_available_suggestion() {
        let mut e = editor();
        configure(
            &mut e,
            &parameter_config(2, (0.25, 0.25), (0.25, 0.25)),
            None,
            false,
        );
        type_reading(&mut e);
        let _ = e.display();
        let proposal = e.d512_report().unwrap().suggestion.unwrap();
        let snapshot = document();
        assert_eq!(
            dispatch_acceptance(
                &mut e,
                AcceptanceGesture::None,
                Some((&snapshot, &proposal)),
                Some(&snapshot),
                true
            ),
            AcceptanceResult::Native
        );
        assert_eq!(e.display(), "代表");
    }
    #[test]
    fn d512_physical_ctrl_then_tab_preserves_the_visible_suggestion() {
        use chewing::input::{KeyboardEvent, keysym::*};
        for control in [SYM_LEFTCTRL, SYM_RIGHTCTRL] {
            let mut e = editor();
            let mut cfg = parameter_config(2, (0.25, 0.25), (0.25, 0.25));
            cfg.d512_suggestion_accept_key = 1;
            let calls = configure(&mut e, &cfg, None, false);
            type_reading(&mut e);
            let _ = e.display();
            let proposal = e.d512_report().unwrap().suggestion.unwrap();
            let snapshot = document();
            let slot = HintSlot::new(Rc::new(Cell::new(0)));
            slot.publish(0, (snapshot.clone(), proposal), true);
            let before = calls.load(Ordering::SeqCst);
            let event = KeyboardEvent::builder().ksym(control).control().build();
            let mut tab = event;
            tab.ksym = SYM_TAB;
            let gesture = super::super::d512::acceptance_gesture(&e, &cfg, tab);
            if !acceptance_prefix(event, gesture) {
                slot.withdraw();
            }
            assert!(
                slot.current().is_some(),
                "Ctrl keydown must retain the hint"
            );
            assert_eq!(e.display_commit(), "");
            assert_eq!(calls.load(Ordering::SeqCst), before);
            let pending = slot.current().unwrap();
            assert_eq!(
                dispatch_acceptance(
                    &mut e,
                    gesture,
                    Some((&pending.0, &pending.1)),
                    Some(&snapshot),
                    true
                ),
                AcceptanceResult::Accepted
            );
            assert_eq!(calls.load(Ordering::SeqCst), before);
            assert_eq!(e.display(), "代錶");
            assert_eq!(e.display_commit(), "");
            for key in [SYM_TAB, SYM_LEFTSHIFT, SYM_LEFTALT, SYM_RIGHT] {
                let mut other = event;
                other.ksym = key;
                assert!(!acceptance_prefix(other, gesture));
            }
            assert!(!acceptance_prefix(event, AcceptanceGesture::None));
        }
    }
    struct Tracked(Rc<Cell<usize>>);
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    #[test]
    fn d512_stale_async_publication_and_ui_suppression_release_the_owner() {
        let epoch = Rc::new(Cell::new(0));
        let drops = Rc::new(Cell::new(0));
        let slot = HintSlot::new(epoch.clone());
        assert!(!slot.publish(0, Tracked(drops.clone()), false));
        assert!(slot.current().is_none());
        assert_eq!(drops.get(), 1);
        assert!(slot.publish(0, Tracked(drops.clone()), true));
        slot.withdraw();
        assert!(slot.current().is_none());
        assert_eq!(drops.get(), 2);
        assert!(!slot.publish(0, Tracked(drops.clone()), true));
        assert_eq!(drops.get(), 3);
        assert!(slot.publish(epoch.get(), Tracked(drops.clone()), true));
        epoch.set(epoch.get() + 1);
        assert!(slot.current().is_none());
        slot.withdraw();
        assert_eq!(drops.get(), 4);
    }
    #[test]
    fn d512_labels_follow_the_actual_accept_key() {
        assert_eq!(accept_key_label(0), "Tab");
        assert_eq!(accept_key_label(1), "Ctrl+Tab");
        assert_eq!(accept_key_label(2), "→");
        assert_eq!(accept_key_label(99), "Tab");
    }
    #[test]
    fn d512_utf16_caret_and_hint_width_are_bounded() {
        assert_eq!(utf16_offset("甲𠮷乙", 2), 3);
        assert_eq!(utf16_offset("甲𠮷乙", 3), 4);
        assert_eq!(hint_width(1920, 96.), 480.);
        assert!(hint_width(240, 192.) <= 80.);
    }
}
