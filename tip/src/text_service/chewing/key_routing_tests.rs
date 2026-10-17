// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use chewing::d512::{D512Error, D512Mode, D512Options, SentenceRequest, SentenceScorer};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize},
};
use windows::Win32::{
    Foundation::{E_FAIL, E_NOTIMPL, HWND},
    System::Registry::{HKEY, HKEY_CURRENT_USER, RegOverridePredefKey},
    UI::{Input::KeyboardAndMouse::*, TextServices::*},
};
use windows_core::{ComObjectInner, IUnknownImpl, implement};
use windows_registry::{CURRENT_USER, Key};

static REGISTRY_LOCK: Mutex<()> = Mutex::new(());
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Registry {
    original: Key,
    path: String,
}
impl Registry {
    fn new() -> Self {
        let original = CURRENT_USER.options().read().write().open("").unwrap();
        let path = format!(
            "Software\\ChewingKeyRouteTest-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        );
        let key = original.create(&path).unwrap();
        unsafe {
            RegOverridePredefKey(HKEY_CURRENT_USER, Some(HKEY(key.as_raw())))
                .ok()
                .unwrap();
        }
        CURRENT_USER.create("Software\\ChewingTextService").unwrap();
        Self { original, path }
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        unsafe {
            RegOverridePredefKey(HKEY_CURRENT_USER, None).ok().unwrap();
        }
        self.original.remove_tree(&self.path).unwrap();
    }
}

#[implement(ITfCompartment)]
struct Compartment;
impl ITfCompartment_Impl for Compartment_Impl {
    fn GetValue(&self) -> windows_core::Result<VARIANT> {
        Ok(0i32.into())
    }
    fn SetValue(&self, _tid: u32, _value: *const VARIANT) -> windows_core::Result<()> {
        Ok(())
    }
}
macro_rules! unsupported {
 ($(fn $name:ident(&self $(,$arg:ident:$ty:ty)*) -> $ret:ty;)*) => {$(fn $name(&self,$($arg:$ty),*)->$ret {$(let _=&$arg;)*Err(E_NOTIMPL.into())})*};
}
#[implement(
    ITfContext,
    ITfCompartmentMgr,
    ITfComposition,
    ITfCompositionSink,
    ITfThreadMgr,
    ITfUIElementMgr,
    ITfDocumentMgr
)]
struct Tsf {
    readonly: Cell<bool>,
    edits: Cell<u32>,
}

#[allow(unused_variables)]
impl ITfContext_Impl for Tsf_Impl {
    fn RequestEditSession(
        &self,
        tid: u32,
        pes: windows_core::Ref<ITfEditSession>,
        dwflags: TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    ) -> windows_core::Result<windows_core::HRESULT> {
        self.edits.set(self.edits.get() + 1);
        Err(E_FAIL.into())
    }
    fn GetStatus(&self) -> windows_core::Result<TS_STATUS> {
        Ok(TS_STATUS {
            dwDynamicFlags: if self.readonly.get() {
                TF_SD_READONLY
            } else {
                0
            },
            dwStaticFlags: 0,
        })
    }
    unsupported! {
        fn InWriteSession(&self, tid: u32) -> windows_core::Result<windows_core::BOOL>;
        fn GetSelection(&self, ec: u32, ulindex: u32, ulcount: u32, pselection: *mut TF_SELECTION, pcfetched: *mut u32) -> windows_core::Result<()>;
        fn SetSelection(&self, ec: u32, ulcount: u32, pselection: *const TF_SELECTION) -> windows_core::Result<()>;
        fn GetStart(&self, ec: u32) -> windows_core::Result<ITfRange>;
        fn GetEnd(&self, ec: u32) -> windows_core::Result<ITfRange>;
        fn GetActiveView(&self) -> windows_core::Result<ITfContextView>;
        fn EnumViews(&self) -> windows_core::Result<IEnumTfContextViews>;
        fn GetProperty(&self, guidprop: *const windows_core::GUID) -> windows_core::Result<ITfProperty>;
        fn GetAppProperty(&self, guidprop: *const windows_core::GUID) -> windows_core::Result<ITfReadOnlyProperty>;
        fn TrackProperties(&self, prgprop: *const *const windows_core::GUID, cprop: u32, prgappprop: *const *const windows_core::GUID, cappprop: u32) -> windows_core::Result<ITfReadOnlyProperty>;
        fn EnumProperties(&self) -> windows_core::Result<IEnumTfProperties>;
        fn GetDocumentMgr(&self) -> windows_core::Result<ITfDocumentMgr>;
        fn CreateRangeBackup(&self, ec: u32, prange: windows_core::Ref<ITfRange>) -> windows_core::Result<ITfRangeBackup>;
    }
}

#[allow(unused_variables)]
impl ITfCompartmentMgr_Impl for Tsf_Impl {
    fn GetCompartment(
        &self,
        rguid: *const windows_core::GUID,
    ) -> windows_core::Result<ITfCompartment> {
        Ok(Compartment.into_object().into_interface())
    }
    unsupported! {
        fn ClearCompartment(&self, tid: u32, rguid: *const windows_core::GUID) -> windows_core::Result<()>;
        fn EnumCompartments(&self) -> windows_core::Result<windows::Win32::System::Com::IEnumGUID>;
    }
}

#[allow(unused_variables)]
impl ITfComposition_Impl for Tsf_Impl {
    fn EndComposition(&self, ecwrite: u32) -> windows_core::Result<()> {
        Ok(())
    }
    unsupported! {
        fn GetRange(&self) -> windows_core::Result<ITfRange>;
        fn ShiftStart(&self, ecwrite: u32, pnewstart: windows_core::Ref<ITfRange>) -> windows_core::Result<()>;
        fn ShiftEnd(&self, ecwrite: u32, pnewend: windows_core::Ref<ITfRange>) -> windows_core::Result<()>;
    }
}

#[allow(unused_variables)]
impl ITfCompositionSink_Impl for Tsf_Impl {
    fn OnCompositionTerminated(
        &self,
        ecwrite: u32,
        pcomposition: windows_core::Ref<ITfComposition>,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    unsupported! {}
}

#[allow(unused_variables)]
impl ITfThreadMgr_Impl for Tsf_Impl {
    fn GetFocus(&self) -> windows_core::Result<ITfDocumentMgr> {
        Ok(self.to_interface())
    }
    unsupported! {
        fn Activate(&self) -> windows_core::Result<u32>;
        fn Deactivate(&self) -> windows_core::Result<()>;
        fn CreateDocumentMgr(&self) -> windows_core::Result<ITfDocumentMgr>;
        fn EnumDocumentMgrs(&self) -> windows_core::Result<IEnumTfDocumentMgrs>;
        fn SetFocus(&self, pdimfocus: windows_core::Ref<ITfDocumentMgr>) -> windows_core::Result<()>;
        fn AssociateFocus(&self, hwnd: windows::Win32::Foundation::HWND, pdimnew: windows_core::Ref<ITfDocumentMgr>) -> windows_core::Result<ITfDocumentMgr>;
        fn IsThreadFocus(&self) -> windows_core::Result<windows_core::BOOL>;
        fn GetFunctionProvider(&self, clsid: *const windows_core::GUID) -> windows_core::Result<ITfFunctionProvider>;
        fn EnumFunctionProviders(&self) -> windows_core::Result<IEnumTfFunctionProviders>;
        fn GetGlobalCompartment(&self) -> windows_core::Result<ITfCompartmentMgr>;
    }
}

#[allow(unused_variables)]
impl ITfUIElementMgr_Impl for Tsf_Impl {
    fn BeginUIElement(
        &self,
        pelement: windows_core::Ref<ITfUIElement>,
        pbshow: *mut windows_core::BOOL,
        pdwuielementid: *mut u32,
    ) -> windows_core::Result<()> {
        unsafe {
            *pbshow = false.into();
            *pdwuielementid = 1;
        }
        Ok(())
    }
    fn UpdateUIElement(&self, dwuielementid: u32) -> windows_core::Result<()> {
        Ok(())
    }
    fn EndUIElement(&self, dwuielementid: u32) -> windows_core::Result<()> {
        Ok(())
    }
    unsupported! {
        fn GetUIElement(&self, dwuielementid: u32) -> windows_core::Result<ITfUIElement>;
        fn EnumUIElements(&self) -> windows_core::Result<IEnumTfUIElements>;
    }
}

#[allow(unused_variables)]
impl ITfDocumentMgr_Impl for Tsf_Impl {
    fn GetTop(&self) -> windows_core::Result<ITfContext> {
        Ok(self.to_interface())
    }
    unsupported! {
        fn CreateContext(&self, tidowner: u32, dwflags: u32, punk: windows_core::Ref<windows_core::IUnknown>, ppic: windows_core::OutRef<ITfContext>, pectextstore: *mut u32) -> windows_core::Result<()>;
        fn Push(&self, pic: windows_core::Ref<ITfContext>) -> windows_core::Result<()>;
        fn Pop(&self, dwflags: u32) -> windows_core::Result<()>;
        fn GetBase(&self) -> windows_core::Result<ITfContext>;
        fn EnumContexts(&self) -> windows_core::Result<IEnumTfContexts>;
    }
}

struct Scores(Arc<AtomicUsize>);
impl SentenceScorer for Scores {
    fn score(
        &mut self,
        req: &SentenceRequest,
        _deadline: Instant,
    ) -> std::result::Result<Vec<f64>, D512Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(req
            .candidates
            .iter()
            .map(|s| if s == "代錶" { 100. } else { -100. })
            .collect())
    }
}
fn fixture(mode: i32, key: i32) -> (ChewingTextService, ComObject<Tsf>, Arc<AtomicUsize>) {
    let tsf = Tsf {
        readonly: Cell::new(false),
        edits: Cell::new(0),
    }
    .into_object();
    let thread: ITfThreadMgr = tsf.cast().unwrap();
    let button = || {
        LangBarButton::new(
            Default::default(),
            Default::default(),
            HICON::default(),
            HMENU::default(),
            0,
            thread.clone(),
        )
        .into_object()
    };
    let icon = super::super::icons::IconSet {
        dark: HICON::default(),
        light: HICON::default(),
        dark_dot: HICON::default(),
        light_dot: HICON::default(),
    };
    let mut cfg = Config::default();
    cfg.chewing_tsf.d512_mode = mode;
    cfg.chewing_tsf.d512_suggestion_accept_key = key;
    cfg.chewing_tsf.show_notification = false;
    cfg.chewing_tsf.enable_caps_lock = false;
    cfg.chewing_tsf.shift_key_sensitivity = i32::MAX;
    cfg.chewing_tsf.keybind.clear();
    cfg.save_reg();
    let cfg = Config::from_reg().unwrap();
    let mut editor = super::super::d512::tests::editor();
    let calls = Arc::new(AtomicUsize::new(0));
    editor
        .set_sentence_scorer(
            if mode == 0 {
                None
            } else {
                Some(Box::new(Scores(calls.clone())))
            },
            D512Options {
                mode: match mode {
                    0 => D512Mode::Off,
                    1 => D512Mode::Automatic,
                    _ => D512Mode::Suggestion,
                },
                weight: 0.25,
                margin: 2.,
                ..Default::default()
            },
        )
        .unwrap();
    editor.set_d512_context("前文");
    super::super::d512::tests::type_reading(&mut editor);
    let _ = editor.display();
    let service = ChewingTextService {
        thread_mgr: thread.clone(),
        tid: 1,
        input_da_atom: [VARIANT::default(), VARIANT::default()],
        _menu: Menu::default(),
        popup_menu: HMENU::default(),
        lang_icons: LangIconSet {
            tc: icon,
            sc: icon,
            en: icon,
            full_shape: HICON::default(),
            half_shape: HICON::default(),
        },
        lang_bar_buttons: vec![],
        composition_sink: tsf.cast().unwrap(),
        switch_lang_button: button(),
        switch_shape_button: button(),
        ime_mode_button: button(),
        lang_mode: Cell::new(TsfLangMode::Chinese),
        pending_lang_mode_change: Cell::new(false),
        has_focus: true,
        output_simp_chinese: false,
        shift_key_state: ShiftKeyState::Up,
        cfg,
        kbtype: KeyboardLayoutCompat::Default,
        keymap: KeymapOp::None,
        keybindings: vec![],
        chewing_editor: editor,
        d512_hint: Rc::new(super::super::d512_ui::HintSlot::new(Rc::new(Cell::new(0)))),
        d512_confirmed: "前文".into(),
        notification: None,
        candidate_list: None,
        composition: Rc::new(RefCell::new(Some(tsf.cast().unwrap()))),
        pending_edit: Weak::new(),
    };
    (service, tsf, calls)
}
fn event(vk: u16, scan: u16, ascii: u8, modifiers: u8) -> SystemKeyboardEvent {
    let mut key_state = [0; 256];
    for (mask, key) in [(1, VK_SHIFT), (2, VK_CONTROL), (4, VK_MENU), (8, VK_LWIN)] {
        if modifiers & mask != 0 {
            key_state[key.0 as usize] = 0x80;
        }
    }
    SystemKeyboardEvent {
        vk,
        scan_code: scan,
        ascii_code: ascii,
        key_state,
    }
}
#[test]
fn key_route_entries_preserve_native_shortcut_and_readonly_rejections() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let mut mismatches = vec![];
    for mode in [0, 1, 2] {
        for (name, ev, readonly, candidate, disabled) in [
            ("Alt+A", event(0x41, 0x1e, b'a', 4), false, false, false),
            ("Ctrl+C", event(0x43, 0x2e, b'c', 2), false, false, false),
            (
                "CapsLock disabled",
                event(VK_CAPITAL.0, 0x3a, 0, 0),
                false,
                false,
                false,
            ),
            (
                "readonly ordinary key",
                event(0x41, 0x1e, b'a', 0),
                true,
                false,
                false,
            ),
            (
                "candidate Ctrl+C",
                event(0x43, 0x2e, b'c', 2),
                false,
                true,
                false,
            ),
            (
                "keyboard closed",
                event(0x41, 0x1e, b'a', 0),
                false,
                false,
                true,
            ),
        ] {
            let (mut s, t, _) = fixture(mode, 0);
            t.readonly.set(readonly);
            if candidate {
                s.candidate_list =
                    Some(CandidateList::new(HWND::default(), t.cast().unwrap()).unwrap());
            }
            if disabled {
                s.lang_mode.set(TsfLangMode::DisabledChinese);
            }
            let context: ITfContext = t.cast().unwrap();
            let tested = s.on_test_keydown(&context, ev).unwrap();
            let handled = s.on_keydown(&context, ev).unwrap();
            if tested || handled {
                mismatches.push(format!("mode {mode} {name}: Test={tested},Key={handled}"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("; "));
}
#[test]
fn key_route_entries_follow_configured_acceptance_and_native_navigation() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let mut mismatches = vec![];
    for (key, ev, expected, middle) in [
        (0, event(VK_TAB.0, 0xf, 0, 0), true, false),
        (0, event(VK_TAB.0, 0xf, 0, 2), false, false),
        (1, event(VK_TAB.0, 0xf, 0, 2), true, false),
        (1, event(VK_TAB.0, 0xf, 0, 0), true, false),
        (1, event(VK_CONTROL.0, 0x1d, 0, 2), false, false),
        (2, event(VK_RIGHT.0, 0x4d, 0, 0), true, false),
        (2, event(VK_RIGHT.0, 0x4d, 0, 0), true, true),
    ] {
        let (mut s, t, _) = fixture(2, key);
        if middle {
            s.chewing_editor
                .process_keyevent(KeyboardEvent::builder().ksym(keysym::SYM_HOME).build());
        }
        let cursor = s.chewing_editor.cursor();
        if middle {
            assert_eq!(
                s.d512_gesture(ev.to_keyboard_event(s.keymap)),
                super::super::d512::AcceptanceGesture::None
            );
        }
        let context: ITfContext = t.cast().unwrap();
        let tested = s.on_test_keydown(&context, ev).unwrap();
        let handled = s.on_keydown(&context, ev).unwrap();
        if tested != expected || handled != expected {
            mismatches.push(format!(
                "key {key},vk {}: Test={tested},Key={handled},want={expected}",
                ev.vk
            ));
        }
        if middle {
            assert_eq!(s.chewing_editor.cursor(), cursor + 1);
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("; "));
}
#[test]
fn key_route_probe_keeps_report_text_and_pending_configuration() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t, calls) = fixture(2, 0);
    let context: ITfContext = t.cast().unwrap();
    let key = CURRENT_USER.create("Software\\ChewingTextService").unwrap();
    key.set_u32("DefFontSize", 24).unwrap();
    let before = calls.load(Ordering::SeqCst);
    let font = s.cfg.chewing_tsf.font_size;
    let epoch = s.d512_hint.epoch.get();
    for _ in 0..2 {
        assert!(
            s.on_test_keydown(&context, event(VK_TAB.0, 0xf, 0, 0))
                .unwrap()
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), before);
    assert_eq!(s.cfg.chewing_tsf.font_size, font);
    assert_eq!(s.d512_hint.epoch.get(), epoch);
    assert_eq!(t.edits.get(), 0);
    assert_eq!(
        s.chewing_editor
            .d512_report()
            .unwrap()
            .suggestion
            .unwrap()
            .text,
        "代錶"
    );
    assert_eq!(s.chewing_editor.display(), "代表");
    assert_eq!(s.chewing_editor.display_commit(), "");
}
#[test]
fn key_route_probe_uses_service_language_without_mutating_editor_options() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t, _) = fixture(2, 1);
    let context: ITfContext = t.cast().unwrap();
    s.chewing_editor
        .set_editor_options(|opt| opt.language_mode = LanguageMode::English);
    let ev = event(VK_TAB.0, 0xf, 0, 2);
    assert!(s.on_test_keydown(&context, ev).unwrap());
    assert!(s.on_test_keydown(&context, ev).unwrap());
    assert_eq!(
        s.chewing_editor.editor_options().language_mode,
        LanguageMode::English
    );
    assert!(s.on_keydown(&context, ev).unwrap());
    assert_eq!(
        s.chewing_editor.editor_options().language_mode,
        LanguageMode::Chinese
    );
}
#[test]
fn key_route_shift_probe_preserves_native_timer_and_toggle_without_reloading() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t, calls) = fixture(2, 0);
    let context: ITfContext = t.cast().unwrap();
    let before = calls.load(Ordering::SeqCst);
    let down = event(VK_SHIFT.0, 0x2a, 0, 1);
    assert!(!s.on_test_keydown(&context, down).unwrap());
    let ShiftKeyState::Down(start) = s.shift_key_state else {
        panic!("Shift timing was not preserved")
    };
    assert!(!s.on_test_keydown(&context, down).unwrap());
    assert!(matches!(s.shift_key_state,ShiftKeyState::Down(now) if now==start));
    assert_eq!(calls.load(Ordering::SeqCst), before);
    assert!(s.chewing_editor.d512_report().is_some());
    assert!(
        !s.on_test_keyup(&context, event(VK_SHIFT.0, 0x2a, 0, 0))
            .unwrap()
    );
    assert_eq!(s.lang_mode.get(), TsfLangMode::English);
}
