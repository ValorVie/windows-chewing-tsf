// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use chewing::d512::{D512Error, D512Mode, D512Options, SentenceRequest, SentenceScorer};
use chewing::zhuyin::Bopomofo::{AI, D, TONE4};
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
    ITfDocumentMgr,
    ITfInsertAtSelection
)]
struct Tsf {
    readonly: Cell<bool>,
    edits: Cell<u32>,
    execute_sessions: Cell<bool>,
    defer_sessions: Cell<bool>,
    queued_sessions: RefCell<Vec<ITfEditSession>>,
    document: Rc<Document>,
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
        if self.defer_sessions.get() && dwflags.0 & TF_ES_ASYNC.0 != 0 {
            self.queued_sessions.borrow_mut().push(pes.ok()?.clone());
            return Ok(windows_core::HRESULT(0x00040300)); // TF_S_ASYNC: scheduled only
        }
        if self.execute_sessions.get() {
            return Ok(match unsafe { pes.ok()?.DoEditSession(0) } {
                Ok(()) => windows_core::HRESULT(0),
                Err(error) => error.code(),
            });
        }
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
    fn GetRange(&self) -> windows_core::Result<ITfRange> {
        Ok(Range {
            document: self.document.clone(),
        }
        .into_object()
        .into_interface())
    }
    fn ShiftStart(
        &self,
        _ecwrite: u32,
        _range: windows_core::Ref<ITfRange>,
    ) -> windows_core::Result<()> {
        if self.document.fail_commit.get() {
            return Err(E_FAIL.into());
        }
        self.document.committed.set(true);
        Ok(())
    }
    fn EndComposition(&self, ecwrite: u32) -> windows_core::Result<()> {
        Ok(())
    }
    unsupported! {
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

// Only the external document boundary is synthetic. These cases execute the
// production edit sessions and inject failures after/before real mock writes.
#[derive(Default)]
struct Document {
    text: RefCell<String>,
    fail_write: Cell<bool>,
    committed: Cell<bool>,
    shifted: Cell<i32>,
    shift_limit: Cell<Option<i32>>,
    fail_commit: Cell<bool>,
}
#[implement(ITfRange)]
struct Range {
    document: Rc<Document>,
}
impl ITfRange_Impl for Range_Impl {
    fn SetText(
        &self,
        _ec: u32,
        _flags: u32,
        text: &windows_core::PCWSTR,
        len: i32,
    ) -> windows_core::Result<()> {
        if self.document.fail_write.get() {
            return Err(E_FAIL.into());
        }
        self.document.text.replace(
            String::from_utf16(unsafe { std::slice::from_raw_parts(text.0, len as usize) })
                .unwrap(),
        );
        Ok(())
    }
    fn ShiftStart(
        &self,
        _ec: u32,
        count: i32,
        moved: *mut i32,
        _halt: *const TF_HALTCOND,
    ) -> windows_core::Result<()> {
        let count = self
            .document
            .shift_limit
            .get()
            .map_or(count, |limit| count.min(limit));
        self.document.shifted.set(count);
        unsafe {
            *moved = count;
        }
        Ok(())
    }
    fn Collapse(&self, _ec: u32, _anchor: TfAnchor) -> windows_core::Result<()> {
        Err(E_FAIL.into())
    }
    unsupported! {
    fn GetText(&self, ec: u32, dwflags: u32, pchtext: windows_core::PWSTR, cchmax: u32, pcch: *mut u32) -> windows_core::Result<()>;
    fn GetFormattedText(&self, ec: u32) -> windows_core::Result<windows::Win32::System::Com::IDataObject>;
    fn GetEmbedded(&self, ec: u32, rguidservice: *const windows_core::GUID, riid: *const windows_core::GUID) -> windows_core::Result<windows_core::IUnknown>;
    fn InsertEmbedded(&self, ec: u32, dwflags: u32, pdataobject: windows_core::Ref<windows::Win32::System::Com::IDataObject>) -> windows_core::Result<()>;
    fn ShiftEnd(&self, ec: u32, cchreq: i32, pcch: *mut i32, phalt: *const TF_HALTCOND) -> windows_core::Result<()>;
    fn ShiftStartToRange(&self, ec: u32, prange: windows_core::Ref<ITfRange>, apos: TfAnchor) -> windows_core::Result<()>;
    fn ShiftEndToRange(&self, ec: u32, prange: windows_core::Ref<ITfRange>, apos: TfAnchor) -> windows_core::Result<()>;
    fn ShiftStartRegion(&self, ec: u32, dir: TfShiftDir) -> windows_core::Result<windows_core::BOOL>;
    fn ShiftEndRegion(&self, ec: u32, dir: TfShiftDir) -> windows_core::Result<windows_core::BOOL>;
    fn IsEmpty(&self, ec: u32) -> windows_core::Result<windows_core::BOOL>;
    fn IsEqualStart(&self, ec: u32, pwith: windows_core::Ref<ITfRange>, apos: TfAnchor) -> windows_core::Result<windows_core::BOOL>;
    fn IsEqualEnd(&self, ec: u32, pwith: windows_core::Ref<ITfRange>, apos: TfAnchor) -> windows_core::Result<windows_core::BOOL>;
    fn CompareStart(&self, ec: u32, pwith: windows_core::Ref<ITfRange>, apos: TfAnchor) -> windows_core::Result<i32>;
    fn CompareEnd(&self, ec: u32, pwith: windows_core::Ref<ITfRange>, apos: TfAnchor) -> windows_core::Result<i32>;
    fn AdjustForInsert(&self, ec: u32, cchinsert: u32) -> windows_core::Result<windows_core::BOOL>;
    fn GetGravity(&self, pgstart: *mut TfGravity, pgend: *mut TfGravity) -> windows_core::Result<()>;
    fn SetGravity(&self, ec: u32, gstart: TfGravity, gend: TfGravity) -> windows_core::Result<()>;
    fn Clone(&self) -> windows_core::Result<ITfRange>;
    fn GetContext(&self) -> windows_core::Result<ITfContext>;
    }
}
impl ITfInsertAtSelection_Impl for Tsf_Impl {
    fn InsertTextAtSelection(
        &self,
        _ec: u32,
        _flags: INSERT_TEXT_AT_SELECTION_FLAGS,
        text: &windows_core::PCWSTR,
        len: i32,
    ) -> windows_core::Result<ITfRange> {
        if self.document.fail_write.get() {
            return Err(E_FAIL.into());
        }
        self.document.text.replace(
            String::from_utf16(unsafe { std::slice::from_raw_parts(text.0, len as usize) })
                .unwrap(),
        );
        Ok(Range {
            document: self.document.clone(),
        }
        .into_object()
        .into_interface())
    }
    unsupported! {
        fn InsertEmbeddedAtSelection(&self, ec:u32, flags:u32, object:windows_core::Ref<windows::Win32::System::Com::IDataObject>)->windows_core::Result<ITfRange>;
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
        execute_sessions: Cell::new(false),
        defer_sessions: Cell::new(false),
        queued_sessions: RefCell::new(vec![]),
        document: Rc::new(Document::default()),
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
    cfg.chewing_tsf.use_personal_candidate_preference = false;
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
        calibration: Rc::new(super::super::d512_calibration::Collector::new()),
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
    for (mask, key) in [
        (1, VK_SHIFT),
        (2, VK_CONTROL),
        (4, VK_MENU),
        (8, VK_LWIN),
        (16, VK_RWIN),
    ] {
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

const CTRL_SYMBOLS: [(u16, u16, u8, char); 6] = [
    (0xbc, 0x33, b',', '，'),
    (0xbe, 0x34, b'.', '。'),
    (0xba, 0x27, b';', '；'),
    (0xde, 0x28, b'\'', '、'),
    (0xdb, 0x1a, b'[', '【'),
    (0xdd, 0x1b, b']', '】'),
];

fn input_habits_fixture(enabled: bool, content: u8) -> (ChewingTextService, ComObject<Tsf>) {
    let (mut service, tsf, _) = fixture(0, 0);
    service.cfg.chewing_tsf.ctrl_symbol_new_phonetic = enabled;
    service.cfg.save_reg();
    service.cfg = Config::from_reg().unwrap();
    service.chewing_editor = service
        .configure_loaded_editor(
            &service.cfg.chewing_tsf,
            super::super::d512::tests::editor(),
        )
        .unwrap();
    match content {
        0 => {
            service.composition.replace(None);
        }
        1 => super::super::d512::tests::type_reading(&mut service.chewing_editor),
        2 => {
            service
                .chewing_editor
                .process_keyevent(chewing::input::keymap::map_ascii(
                    &chewing::input::keymap::QWERTY_MAP,
                    b'2',
                ));
            assert!(!service.chewing_editor.syllable_buffer().is_empty());
        }
        3 => {} // A stale TSF composition with both engine buffers empty.
        _ => unreachable!(),
    }
    (service, tsf)
}

#[test]
fn input_habits_ctrl_symbols_route_both_entries_and_insert_once() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    for (vk, scan, ascii, symbol) in CTRL_SYMBOLS {
        let (mut s, t) = input_habits_fixture(true, 1);
        let ctx = t.cast().unwrap();
        let ev = event(vk, scan, ascii, 2);
        let text = s.chewing_editor.display();
        for _ in 0..2 {
            assert!(
                s.on_test_keydown(&ctx, ev).unwrap(),
                "Ctrl+{}",
                ascii as char
            );
        }
        assert_eq!(s.chewing_editor.display(), text);
        assert_eq!(t.edits.get(), 0);
        assert!(s.on_keydown(&ctx, ev).unwrap());
        assert_eq!(s.chewing_editor.display(), format!("{text}{symbol}"));
        assert_eq!(s.chewing_editor.display_commit(), "");
    }
}

#[test]
fn input_habits_unfinished_syllable_is_consumed_without_insertion() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    for (vk, scan, ascii, _) in CTRL_SYMBOLS {
        let (mut s, t) = input_habits_fixture(true, 2);
        let ctx = t.cast().unwrap();
        let before = s.chewing_editor.syllable_buffer();
        let ev = event(vk, scan, ascii, 2);
        assert!(s.on_test_keydown(&ctx, ev).unwrap());
        assert!(s.on_keydown(&ctx, ev).unwrap());
        assert_eq!(
            s.chewing_editor.last_key_behavior(),
            EditorKeyBehavior::Bell
        );
        assert_eq!(s.chewing_editor.syllable_buffer(), before);
        assert!(s.chewing_editor.is_empty());
        assert_eq!(s.chewing_editor.display_commit(), "");
    }
}

#[test]
fn input_habits_native_shortcuts_stay_native() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    for (enabled, content, modifiers, ascii) in [
        (true, 0, 2, b'['),
        (true, 3, 2, b'['),
        (false, 1, 2, b'['),
        (true, 1, 2, b'/'),
        (true, 1, 2 | 1, b'['),
        (true, 1, 2 | 4, b'['),
        (true, 1, 2 | 8, b'['),
        (true, 1, 2 | 16, b'['),
    ] {
        let (mut s, t) = input_habits_fixture(enabled, content);
        let ctx = t.cast().unwrap();
        let before = s.chewing_editor.display();
        let ev = event(
            if ascii == b'/' { 0xbf } else { 0xdb },
            0x1a,
            ascii,
            modifiers,
        );
        assert!(!s.on_test_keydown(&ctx, ev).unwrap());
        assert!(!s.on_keydown(&ctx, ev).unwrap());
        assert_eq!(s.chewing_editor.display(), before);
        assert_eq!(t.edits.get(), 0);
    }
}

#[test]
fn input_habits_existing_ctrl_digit_and_easy_symbol_routing_is_preserved() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t) = input_habits_fixture(true, 1);
    let ctx = t.cast().unwrap();
    assert!(s.on_test_keydown(&ctx, event(0x31, 2, b'1', 2)).unwrap());
    s.cfg.chewing_tsf.easy_symbols_with_shift_ctrl = true;
    assert!(s.on_test_keydown(&ctx, event(0x41, 0x1e, b'A', 3)).unwrap());
}

#[test]
fn input_habits_ctrl_probe_preserves_pending_config_and_d512_state() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t, calls) = fixture(2, 0);
    let ctx = t.cast().unwrap();
    CURRENT_USER
        .create("Software\\ChewingTextService")
        .unwrap()
        .set_u32("CtrlSymbolNewPhonetic", 0)
        .unwrap();
    let before = calls.load(Ordering::SeqCst);
    let epoch = s.d512_hint.epoch.get();
    let cursor = s.chewing_editor.cursor();
    let options = s.chewing_editor.editor_options().ctrl_symbol_new_phonetic;
    for _ in 0..2 {
        assert!(s.on_test_keydown(&ctx, event(0xdb, 0x1a, b'[', 2)).unwrap());
    }
    assert!(s.cfg.chewing_tsf.ctrl_symbol_new_phonetic);
    assert_eq!(
        s.chewing_editor.editor_options().ctrl_symbol_new_phonetic,
        options
    );
    assert_eq!(s.chewing_editor.cursor(), cursor);
    assert_eq!(s.chewing_editor.display(), "代表");
    assert_eq!(
        s.chewing_editor
            .d512_report()
            .unwrap()
            .suggestion
            .unwrap()
            .text,
        "代錶"
    );
    assert_eq!(calls.load(Ordering::SeqCst), before);
    assert_eq!(s.d512_hint.epoch.get(), epoch);
    assert_eq!(t.edits.get(), 0);
}

#[test]
fn input_habits_cursor_wrap_setting_reaches_configured_editor() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (s, _, _) = fixture(0, 0);
    for wrap in [false, true] {
        let cfg = ChewingTsfConfig {
            d512_mode: 0,
            cursor_wrap: wrap,
            ..Default::default()
        };
        let mut editor = s
            .configure_loaded_editor(&cfg, super::super::d512::tests::editor())
            .unwrap();
        super::super::d512::tests::type_reading(&mut editor);
        editor.process_keyevent(KeyboardEvent::builder().ksym(keysym::SYM_HOME).build());
        editor.process_keyevent(KeyboardEvent::builder().ksym(keysym::SYM_LEFT).build());
        assert_eq!(editor.cursor(), if wrap { editor.len() } else { 0 });
    }
}

#[test]
fn input_habits_punctuation_keysyms_follow_layout_not_oem_vk() {
    for (vk, scan, ascii, _) in CTRL_SYMBOLS {
        let ev = event(vk, scan, ascii, 2).to_keyboard_event(KeymapOp::None);
        assert_eq!(ev.ksym.0, ascii as u32);
        assert!(ev.is_state_on(KeyState::Control));
    }
    // Simulated Dvorak maps physical US minus/equal to brackets. Their VKs
    // are not VK_OEM_4/6; the translated keysym is the engine input.
    for (vk, scan, ascii, want) in [(0xbd, 0x0c, b'-', '['), (0xbb, 0x0d, b'=', ']')] {
        let ev = event(vk, scan, ascii, 2).to_keyboard_event(KeymapOp::Code(&INVERTED_DVORAK_MAP));
        assert_eq!(ev.ksym.0, want as u32);
        assert!(ev.is_state_on(KeyState::Control));
    }
    // A keyboard layout can use the US bracket VK for another character.
    assert_eq!(
        event(0xdb, 0x1a, b'@', 2)
            .to_keyboard_event(KeymapOp::None)
            .ksym
            .0,
        b'@' as u32
    );
}

#[test]
fn input_habits_right_win_is_a_chord_modifier() {
    let ev = event(0xdb, 0x1a, b'[', 2 | 16).to_keyboard_event(KeymapOp::None);
    assert!(ev.is_state_on(KeyState::Super));
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

#[test]
fn accepted_preedit_entry_does_not_rescore() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut service, tsf, calls) = fixture(2, 0);
    let proposal = service
        .chewing_editor
        .d512_report()
        .unwrap()
        .suggestion
        .unwrap();
    let before = calls.load(Ordering::SeqCst);
    assert!(super::super::d512::accept_current(
        &mut service.chewing_editor,
        &proposal
    ));
    assert_eq!(calls.load(Ordering::SeqCst), before);
    // Real redraw entry, including cursor/syllable/interval getters; only the
    // external TSF document write is denied by the synthetic context.
    service
        .update_preedit(&tsf.cast().unwrap(), String::new())
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        before,
        "accepted update_preedit rescored"
    );
    assert_eq!(service.chewing_editor.display_commit(), "");
    assert_eq!(calls.load(Ordering::SeqCst), before);
}

fn calibration_fixture() -> (
    ChewingTextService,
    ComObject<Tsf>,
    Rc<RefCell<Vec<chewing_tip_core::ipc::d512::calibration::CalibrationRecord>>>,
) {
    let (mut s, t, calls) = fixture(2, 0);
    let rows = Rc::new(RefCell::new(vec![]));
    let output = rows.clone();
    s.calibration = Rc::new(super::super::d512_calibration::Collector::with_sink(
        move |records| {
            output.borrow_mut().extend(records);
            true
        },
    ));
    s.cfg.chewing_tsf.d512_log_calibration = 1;
    s.cfg.save_reg();
    s.cfg = Config::from_reg().unwrap();
    s.calibration.configure(&s.cfg.chewing_tsf);
    s.chewing_editor
        .set_sentence_scorer(
            Some(s.calibration.track(Box::new(Scores(calls)))),
            D512Options {
                mode: D512Mode::Suggestion,
                weight: 0.25,
                margin: 2.,
                ..Default::default()
            },
        )
        .unwrap();
    (s, t, rows)
}

fn calibration_multisyllable_keys(commit: bool) {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    for mode in [D512Mode::Automatic, D512Mode::Suggestion] {
        let (mut s, t, rows) = calibration_fixture();
        s.cfg.chewing_tsf.d512_mode = if mode == D512Mode::Automatic { 1 } else { 2 };
        s.cfg.chewing_tsf.esc_clean_all_buf = true;
        s.cfg.save_reg();
        s.cfg = Config::from_reg().unwrap();
        s.chewing_editor = super::super::d512::tests::editor();
        s.chewing_editor
            .user_dict()
            .insert(&[chewing::syl![D, AI, TONE4]], "帶");
        s.chewing_editor.set_editor_options(|options| {
            options.esc_clear_all_buffer = true;
            options.disable_auto_learn_phrase = true;
        });
        s.chewing_editor
            .set_sentence_scorer(
                Some(
                    s.calibration
                        .track(Box::new(Scores(Arc::new(AtomicUsize::new(0))))),
                ),
                D512Options {
                    mode,
                    weight: 0.25,
                    margin: 2.,
                    ..Default::default()
                },
            )
            .unwrap();
        t.execute_sessions.set(true);
        let ctx = t.cast().unwrap();
        // Three synthetic syllables, through the actual Test/Key entry points.
        for (index, syllable) in [b"294".as_slice(), b"1ul3", b"294"].iter().enumerate() {
            for &ascii in *syllable {
                let scan = match ascii {
                    b'1' => 2,
                    b'2' => 3,
                    b'3' => 4,
                    b'4' => 5,
                    b'9' => 10,
                    b'u' => 0x16,
                    b'l' => 0x26,
                    _ => unreachable!(),
                };
                let ev = event(ascii.to_ascii_uppercase() as u16, scan, ascii, 0);
                assert!(s.on_test_keydown(&ctx, ev).unwrap());
                assert!(s.on_keydown(&ctx, ev).unwrap());
            }
            assert!(
                rows.borrow()
                    .iter()
                    .any(|r| r.phase == "rerank" && r.reading.len() == index + 1),
                "missing syllable {}",
                index + 1
            );
        }
        let ids: Vec<_> = rows.borrow().iter().map(|r| r.id.clone()).collect();
        assert!(ids.len() >= 3);
        let committed = s.chewing_editor.display();
        let ev = if commit {
            event(VK_RETURN.0, 0x1c, 0, 0)
        } else {
            event(VK_ESCAPE.0, 1, 0, 0)
        };
        assert!(s.on_test_keydown(&ctx, ev).unwrap());
        assert!(s.on_keydown(&ctx, ev).unwrap());
        let rows = rows.borrow();
        for id in &ids {
            let ends: Vec<_> = rows
                .iter()
                .filter(|r| &r.id == id && r.phase != "rerank")
                .collect();
            assert_eq!(
                ends.len(),
                1,
                "each rerank needs exactly one terminal receipt"
            );
            assert_eq!(ends[0].phase, if commit { "commit" } else { "cancel" });
            assert_eq!(
                ends[0].final_committed_text.as_deref(),
                commit.then_some(committed.as_str())
            );
        }
        assert!(s.chewing_editor.is_empty());
    }
}

#[test]
fn calibration_multisyllable_enter_finishes_every_rerank() {
    calibration_multisyllable_keys(true);
}

#[test]
fn calibration_multisyllable_escape_cancels_every_rerank() {
    calibration_multisyllable_keys(false);
}

#[test]
fn calibration_entry_redraw_then_escape_finishes_with_cancel() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t, rows) = calibration_fixture();
    let ctx = t.cast().unwrap();
    s.chewing_editor
        .set_editor_options(|o| o.esc_clear_all_buffer = true);
    s.update_preedit(&ctx, String::new()).unwrap();
    assert_eq!(rows.borrow().len(), 1);
    assert_eq!(rows.borrow()[0].phase, "rerank");
    assert!(s.on_keydown(&ctx, event(VK_ESCAPE.0, 1, 0, 0)).unwrap());
    let r = rows.borrow();
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].id, r[1].id);
    assert_eq!(r[1].phase, "cancel");
    assert!(r[1].final_committed_text.is_none());
}
#[test]
fn calibration_entry_leaving_focus_cancels_pending_receipts() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t, rows) = calibration_fixture();
    s.update_preedit(&t.cast().unwrap(), String::new()).unwrap();
    assert_eq!(rows.borrow().len(), 1);
    s.on_kill_focus(None).unwrap();
    let r = rows.borrow();
    assert_eq!(r.len(), 2);
    assert_eq!(r[1].phase, "cancel");
    assert_eq!(r[0].id, r[1].id);
}

#[test]
fn calibration_focus_invalidation_survives_reentrant_service_borrow() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t, rows) = calibration_fixture();
    s.update_preedit(&t.cast().unwrap(), String::new()).unwrap();
    assert_eq!(rows.borrow().len(), 1);
    let outer = TextService::new();
    outer.calibration.replace(Rc::downgrade(&s.calibration));
    let _borrow = outer.inner.borrow_mut();
    outer.invalidate_d512_hint();
    assert_eq!(rows.borrow().len(), 2);
    assert_eq!(rows.borrow()[1].phase, "cancel");
}

#[test]
fn calibration_insert_receipt_tracks_text_write_even_when_selection_fails() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    for fail in [false, true] {
        let (mut s, t, rows) = calibration_fixture();
        s.update_preedit(&t.cast().unwrap(), String::new()).unwrap();
        t.document.fail_write.set(fail);
        let receipt = s.calibration.take_commit();
        let session: ITfEditSession = InsertText::new(
            t.cast().unwrap(),
            HSTRING::from("合成🧪"),
            Rc::new(move || receipt.confirm("合成🧪")),
        )
        .into_object()
        .into_interface();
        assert!(unsafe { session.DoEditSession(0) }.is_err());
        drop(session);
        let r = rows.borrow();
        assert_eq!(r.len(), 2);
        assert_eq!(
            t.document.text.borrow().as_str(),
            if fail { "" } else { "合成🧪" }
        );
        assert_eq!(r[1].phase, if fail { "cancel" } else { "commit" });
        assert_eq!(
            r[1].final_committed_text.as_deref(),
            if fail { None } else { Some("合成🧪") }
        );
    }
}
#[test]
fn calibration_prefix_receipt_tracks_commit_even_when_property_lookup_fails() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    for fail in [false, true] {
        let (mut s, t, rows) = calibration_fixture();
        s.update_preedit(&t.cast().unwrap(), String::new()).unwrap();
        t.document.fail_write.set(fail);
        let receipt = s.calibration.take_commit();
        let pending = Rc::new(RefCell::new(Some(CompositionString {
            commit: "合成🧪".into(),
            preedit: String::new(),
            segments: vec![],
            cursor: 0,
            on_committed: Some(Rc::new(move || receipt.confirm("合成🧪"))),
            on_applied: None,
        })));
        let session: ITfEditSession = SetCompositionString::new(
            t.cast().unwrap(),
            s.composition.clone(),
            t.cast().unwrap(),
            [VARIANT::default(), VARIANT::default()],
            pending.clone(),
        )
        .into_object()
        .into_interface();
        assert!(unsafe { session.DoEditSession(0) }.is_err());
        drop(session);
        drop(pending);
        let r = rows.borrow();
        assert_eq!(r.len(), 2);
        assert_eq!(t.document.committed.get(), !fail);
        if !fail {
            assert_eq!(t.document.shifted.get(), 4);
        }
        assert_eq!(r[1].phase, if fail { "cancel" } else { "commit" });
        assert_eq!(
            r[1].final_committed_text.as_deref(),
            if fail { None } else { Some("合成🧪") }
        );
    }
}
#[test]
fn calibration_enter_captures_preedit_before_editor_and_ignores_hidden_rescore() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t, rows) = calibration_fixture();
    let _ = s.chewing_editor.display(); // completed preedit evaluation, not yet collected
    assert!(rows.borrow().is_empty());
    assert!(
        s.on_keydown(&t.cast().unwrap(), event(VK_RETURN.0, 0x1c, 0, 0))
            .unwrap()
    );
    let r = rows.borrow();
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].phase, "rerank");
    // This context denies the document write. Only the saved preedit ID ends.
    assert_eq!(r[1].phase, "cancel");
    assert_eq!(r[0].id, r[1].id);
}

#[test]
fn enter_entry_reuses_displayed_result_when_scorer_becomes_unavailable() {
    struct Once(Arc<AtomicUsize>);
    impl SentenceScorer for Once {
        fn score(
            &mut self,
            req: &SentenceRequest,
            _: Instant,
        ) -> std::result::Result<Vec<f64>, D512Error> {
            if self.0.fetch_add(1, Ordering::SeqCst) != 0 {
                return Err(D512Error::Timeout);
            }
            Ok(req
                .candidates
                .iter()
                .map(|text| if text == "代錶" { 100. } else { -100. })
                .collect())
        }
    }
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t, _) = fixture(1, 0);
    let calls = Arc::new(AtomicUsize::new(0));
    s.chewing_editor
        .set_sentence_scorer(
            Some(Box::new(Once(calls.clone()))),
            D512Options {
                mode: D512Mode::Automatic,
                weight: 0.25,
                margin: 0.5,
                ..Default::default()
            },
        )
        .unwrap();
    s.chewing_editor
        .set_editor_options(|options| options.disable_auto_learn_phrase = true);
    // The synthetic context denies the confirmed-left read, which means empty
    // context on both display and Enter; a changed context must remain a miss.
    s.chewing_editor.set_d512_context("");
    assert_eq!(s.chewing_editor.display(), "代錶");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    t.execute_sessions.set(true);
    assert!(
        s.on_keydown(&t.cast().unwrap(), event(VK_RETURN.0, 0x1c, 0, 0))
            .unwrap()
    );
    assert_eq!(
        (
            t.document.text.borrow().clone(),
            calls.load(Ordering::SeqCst)
        ),
        ("代錶".to_owned(), 1),
        "Enter changed displayed output or rescored"
    );
}

fn personal_u2_fixture() -> (ChewingTextService, ComObject<Tsf>) {
    let (mut s, t, _) = fixture(0, 0);
    s.cfg.chewing_tsf.use_personal_candidate_preference = true;
    s.cfg.chewing_tsf.enable_auto_learn = false;
    s.cfg.save_reg();
    s.cfg = Config::from_reg().unwrap();
    s.chewing_editor = super::super::d512::tests::editor();
    s.chewing_editor.set_editor_options(|o| {
        o.personal_candidate_preference = true;
        o.phrase_choice_rearward = true;
        o.disable_auto_learn_phrase = true;
    });
    super::super::d512::tests::type_reading(&mut s.chewing_editor);
    s.chewing_editor.start_selecting().unwrap();
    let index = s
        .chewing_editor
        .all_candidates()
        .unwrap()
        .iter()
        .position(|s| s == "代錶")
        .unwrap();
    s.chewing_editor.select(index).unwrap();
    assert_eq!(s.chewing_editor.display(), "代錶");
    (s, t)
}
fn personal_u2_assert_learned(s: &mut ChewingTextService, learned: bool) {
    s.chewing_editor.clear();
    super::super::d512::tests::type_reading(&mut s.chewing_editor);
    assert_eq!(
        s.chewing_editor.display(),
        if learned { "代錶" } else { "代表" }
    );
}
fn personal_u2_drain(t: &ComObject<Tsf>) {
    let sessions = std::mem::take(&mut *t.queued_sessions.borrow_mut());
    for session in sessions {
        let _ = unsafe { session.DoEditSession(0) };
    }
}

fn personal_fix1_rebuild_under_lock(route: &str) {
    use chewing::input::keymap::{QWERTY_MAP, map_ascii};
    use std::{fs, process::Command, sync::mpsc, thread, time::Duration};

    // A subprocess gives the real builder private factory/user paths without
    // changing process-wide environment variables while other tests are running.
    if std::env::var("C2_PERSONAL_FIX1_ROUTE").as_deref() != Ok(route) {
        use chewing::{
            dictionary::StringTableBuilder,
            lm::{StaticDictBuilder, StaticLmCompiler},
        };
        let root = std::env::temp_dir().join(format!(
            "chewing-personal-fix1-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let factory = root.join("factory");
        fs::create_dir_all(&factory).unwrap();
        // Empty factory dictionaries are sufficient for the two synthetic user
        // entries below; both rebuilds still use Editor::chewing(None, None).
        for name in ["static_dict.bin", "rare_dict.bin"] {
            StaticDictBuilder::new()
                .to_writer(fs::File::create(factory.join(name)).unwrap())
                .unwrap();
        }
        StringTableBuilder::new()
            .to_writer(fs::File::create(factory.join("static_words.bin")).unwrap())
            .unwrap();
        StaticLmCompiler::new()
            .to_writer(fs::File::create(factory.join("static_lm.bin")).unwrap())
            .unwrap();
        let test = format!(
            "text_service::chewing::key_routing_tests::personal_fix1_{route}_rebuild_returns_while_preference_file_is_locked"
        );
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test, "--nocapture"])
            .env("C2_PERSONAL_FIX1_ROUTE", route)
            .env("CHEWING_PATH", factory)
            .env("CHEWING_USER_PATH", root.join("user"))
            .output()
            .unwrap();
        fs::remove_dir_all(root).unwrap();
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{route} child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t, _) = fixture(0, 0);
    s.cfg.chewing_tsf.use_personal_candidate_preference = true;
    s.cfg.chewing_tsf.enable_auto_learn = false;
    s.cfg.chewing_tsf.phrase_choice_rearward = true;
    s.cfg.save_reg();
    s.cfg = Config::from_reg().unwrap();
    s.chewing_editor = s.build_editor_from_cfg(&s.cfg.chewing_tsf).unwrap();
    for word in ["代", "帶"] {
        s.chewing_editor
            .user_dict()
            .insert(&[chewing::syl![D, AI, TONE4]], word);
    }
    s.chewing_editor.flush(); // Test setup only: finish initial loading before locking.
    let dir = chewing::path::SearchPath::from_env()
        .user_versioned_path()
        .unwrap();
    let lock_path = dir.join("candidate-ranking.json.lock");
    let (ready, wait_ready) = mpsc::channel();
    let (start, wait_start) = mpsc::channel();
    let (done, wait_done) = mpsc::channel();
    let blocker = thread::spawn(move || {
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path)
            .unwrap();
        lock.lock().unwrap();
        ready.send(()).unwrap();
        // Always release the lock, including a failure/panic in the test caller.
        let quick = wait_start.recv_timeout(Duration::from_secs(5)).is_ok()
            && wait_done.recv_timeout(Duration::from_millis(200)).is_ok();
        drop(lock);
        quick
    });
    wait_ready.recv_timeout(Duration::from_secs(5)).unwrap();
    for key in b"294" {
        s.chewing_editor
            .process_keyevent(map_ascii(&QWERTY_MAP, *key));
    }
    s.chewing_editor.start_selecting().unwrap();
    let index = s
        .chewing_editor
        .all_candidates()
        .unwrap()
        .iter()
        .position(|word| word == "帶")
        .unwrap();
    s.chewing_editor.select(index).unwrap();
    t.execute_sessions.set(true);
    s.on_keydown(&t.cast().unwrap(), event(VK_RETURN.0, 0x1c, 0, 0))
        .unwrap();
    assert_eq!(*t.document.text.borrow(), "帶");

    // Only the held-lock observation is timed, excluding setup and final-owner
    // teardown. This is a blocking regression, not a normal latency benchmark.
    let changed_wrap = !s.cfg.chewing_tsf.cursor_wrap;
    if route == "reload" {
        let mut changed = Config::from_reg().unwrap();
        changed.chewing_tsf.cursor_wrap = changed_wrap;
        changed.save_reg();
    } else {
        // A no-op focus handler must fail the reloaded-options assertion too.
        s.chewing_editor
            .set_editor_options(|opt| opt.cursor_wrap = changed_wrap);
    }
    start.send(()).unwrap();
    let result = if route == "reload" {
        // Unhandled F12 still traverses the real next-key configuration reload.
        s.on_keydown(&t.cast().unwrap(), event(VK_F12.0, 0x58, 0, 0))
            .map(|handled| assert!(!handled))
    } else {
        s.on_thread_focus()
    };
    let _ = done.send(());
    let quick = blocker.join().unwrap();
    result.unwrap();
    if route == "reload" {
        assert_eq!(s.cfg.chewing_tsf.cursor_wrap, changed_wrap);
    }
    assert_eq!(
        s.chewing_editor.editor_options().cursor_wrap,
        s.cfg.chewing_tsf.cursor_wrap
    );
    drop(s); // Final owner may wait; the lock has already been released.
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("candidate-ranking.json")).unwrap()).unwrap();
    assert_eq!(json["groups"][0]["counts"]["帶"], 1);
    assert!(quick, "{route} rebuild waited for the preference file lock");
}

#[test]
fn personal_fix1_reload_rebuild_returns_while_preference_file_is_locked() {
    personal_fix1_rebuild_under_lock("reload");
}

#[test]
fn personal_fix1_focus_rebuild_returns_while_preference_file_is_locked() {
    personal_fix1_rebuild_under_lock("focus");
}

#[test]
fn personal_u2_tip_config_follows_preference_switch() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (s, _, _) = fixture(0, 0);
    for enabled in [true, false] {
        let cfg = ChewingTsfConfig {
            d512_mode: 0,
            use_personal_candidate_preference: enabled,
            ..Default::default()
        };
        let e = s
            .configure_loaded_editor(&cfg, super::super::d512::tests::editor())
            .unwrap();
        assert_eq!(e.editor_options().personal_candidate_preference, enabled);
    }
}
#[test]
fn personal_u2_enter_learns_only_after_actual_deferred_insertion() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t) = personal_u2_fixture();
    t.defer_sessions.set(true);
    let ctx = t.cast().unwrap();
    assert!(s.on_keydown(&ctx, event(VK_RETURN.0, 0x1c, 0, 0)).unwrap());
    assert!(!t.document.committed.get());
    personal_u2_assert_learned(&mut s, false);
    personal_u2_drain(&t);
    assert!(t.document.committed.get());
    assert_eq!(t.document.text.borrow().as_str(), "代錶");
    personal_u2_assert_learned(&mut s, true);
}
#[test]
fn personal_u2_immediate_callback_learns_despite_later_selection_failure() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t) = personal_u2_fixture();
    t.execute_sessions.set(true);
    s.chewing_editor.commit().unwrap();
    let text = s.chewing_editor.display_commit().to_owned();
    s.insert_text(&t.cast().unwrap(), &text).unwrap();
    assert_eq!(t.document.text.borrow().as_str(), text);
    personal_u2_assert_learned(&mut s, true);
}
#[test]
fn personal_u2_composition_wrapper_preserves_receipt_before_ack() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t) = personal_u2_fixture();
    t.execute_sessions.set(true);
    s.chewing_editor.commit().unwrap();
    let text = s.chewing_editor.display_commit().to_owned();
    s.set_composition_string(&t.cast().unwrap(), &text, "", vec![], 0)
        .unwrap();
    assert!(t.document.committed.get());
    personal_u2_assert_learned(&mut s, true);
}
#[test]
fn personal_u2_scheduled_but_failed_partial_or_missing_client_never_learns() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    for failure in 0..4 {
        let (mut s, t) = personal_u2_fixture();
        t.defer_sessions.set(true);
        // Enter transfers this cell to EndComposition and replaces the service's
        // cell. Invalidate the one retained by the queued edit sessions.
        let composition = s.composition.clone();
        assert!(
            s.on_keydown(&t.cast().unwrap(), event(VK_RETURN.0, 0x1c, 0, 0))
                .unwrap()
        );
        match failure {
            0 => t.document.fail_write.set(true),
            1 => t.document.shift_limit.set(Some(1)),
            2 => t.document.fail_commit.set(true),
            _ => {
                composition.replace(None);
            }
        }
        personal_u2_drain(&t);
        personal_u2_assert_learned(&mut s, false);
    }
}
#[test]
fn personal_u2_rejected_schedule_and_changed_output_never_learn() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    for changed in [false, true] {
        let (mut s, t) = personal_u2_fixture();
        t.execute_sessions.set(changed);
        s.chewing_editor.commit().unwrap();
        let mut text = s.chewing_editor.display_commit().to_owned();
        if changed {
            text.push('。');
        }
        s.insert_text(&t.cast().unwrap(), &text).unwrap();
        personal_u2_assert_learned(&mut s, false);
    }
    let (mut s, t) = personal_u2_fixture();
    t.execute_sessions.set(true);
    s.output_simp_chinese = true;
    assert!(
        s.on_keydown(&t.cast().unwrap(), event(VK_RETURN.0, 0x1c, 0, 0))
            .unwrap()
    );
    assert_ne!(t.document.text.borrow().as_str(), "代錶");
    personal_u2_assert_learned(&mut s, false);
}
#[test]
fn personal_u2_disabled_and_torn_down_editors_discard_late_callbacks() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t) = personal_u2_fixture();
    t.defer_sessions.set(true);
    assert!(
        s.on_keydown(&t.cast().unwrap(), event(VK_RETURN.0, 0x1c, 0, 0))
            .unwrap()
    );
    s.chewing_editor
        .set_editor_options(|o| o.personal_candidate_preference = false);
    personal_u2_drain(&t);
    s.chewing_editor
        .set_editor_options(|o| o.personal_candidate_preference = true);
    personal_u2_assert_learned(&mut s, false);
    let (mut s, t) = personal_u2_fixture();
    t.defer_sessions.set(true);
    assert!(
        s.on_keydown(&t.cast().unwrap(), event(VK_RETURN.0, 0x1c, 0, 0))
            .unwrap()
    );
    drop(s); // The callbacks retain receipts, never the Editor/store owner.
    personal_u2_drain(&t);
    assert!(t.queued_sessions.borrow().is_empty());
}

#[test]
fn personal_u2_overflow_confirms_prefix_while_tail_stays_composing() {
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t) = personal_u2_fixture();
    t.execute_sessions.set(true);
    s.chewing_editor
        .set_editor_options(|o| o.auto_commit_threshold = 2);
    for &key in b"294" {
        s.chewing_editor
            .process_keyevent(chewing::input::keymap::map_ascii(
                &chewing::input::keymap::QWERTY_MAP,
                key,
            ));
    }
    let prefix = s.chewing_editor.display_commit().to_owned();
    assert_eq!(prefix, "代錶");
    assert!(!s.chewing_editor.is_empty());
    s.update_preedit(&t.cast().unwrap(), prefix).unwrap();
    assert!(t.document.committed.get());
    personal_u2_assert_learned(&mut s, true);
}
#[test]
fn personal_u2_accepted_suggestion_learns_changed_word_after_insertion_only() {
    use super::super::{
        d512::AcceptanceGesture,
        d512_ui::{AcceptanceResult, DocumentSnapshot, dispatch_acceptance},
    };
    let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _registry = Registry::new();
    let (mut s, t, _) = fixture(2, 0);
    s.cfg.chewing_tsf.use_personal_candidate_preference = true;
    s.cfg.save_reg();
    s.cfg = Config::from_reg().unwrap();
    let mut e = chewing::editor::EditorBuilder::new().build();
    for word in ["代", "帶"] {
        e.user_dict().insert(&[chewing::syl![D, AI, TONE4]], word);
    }
    for word in ["表", "錶"] {
        e.user_dict().insert(
            &[chewing::syl![
                chewing::zhuyin::Bopomofo::B,
                chewing::zhuyin::Bopomofo::I,
                chewing::zhuyin::Bopomofo::AU,
                chewing::zhuyin::Bopomofo::TONE3
            ]],
            word,
        );
    }
    e.user_dict().boost(&[chewing::syl![D, AI, TONE4]], "代");
    e.user_dict().boost(
        &[chewing::syl![
            chewing::zhuyin::Bopomofo::B,
            chewing::zhuyin::Bopomofo::I,
            chewing::zhuyin::Bopomofo::AU,
            chewing::zhuyin::Bopomofo::TONE3
        ]],
        "表",
    );
    e.set_editor_options(|o| {
        o.personal_candidate_preference = true;
        o.disable_auto_learn_phrase = true;
    });
    e.set_sentence_scorer(
        Some(Box::new(Scores(Arc::new(AtomicUsize::new(0))))),
        D512Options {
            mode: D512Mode::Suggestion,
            ..Default::default()
        },
    )
    .unwrap();
    super::super::d512::tests::type_reading(&mut e);
    assert_eq!(e.display(), "代表");
    let proposal = e.d512_report().unwrap().suggestion.unwrap();
    assert_eq!(proposal.changes.len(), 1);
    assert_eq!(proposal.changes[0].text, "錶");
    let document = DocumentSnapshot {
        preedit: "代表".into(),
        confirmed: String::new(),
        cursor: 2,
    };
    assert_eq!(
        dispatch_acceptance(
            &mut e,
            AcceptanceGesture::Dedicated,
            Some((&document, &proposal)),
            Some(&document),
            true
        ),
        AcceptanceResult::Accepted
    );
    s.chewing_editor = e;
    t.defer_sessions.set(true);
    assert!(
        s.on_keydown(&t.cast().unwrap(), event(VK_RETURN.0, 0x1c, 0, 0))
            .unwrap()
    );
    assert!(!t.document.committed.get());
    personal_u2_drain(&t);
    assert!(t.document.committed.get());
    // Use public engine calibration to inspect learned intervals, without
    // exposing a test-only store API or touching personal files.
    for (keys, word, hits) in [(b"294".as_slice(), "代", 0), (b"1ul3".as_slice(), "錶", 1)] {
        s.chewing_editor.clear();
        for &key in keys {
            s.chewing_editor
                .process_keyevent(chewing::input::keymap::map_ascii(
                    &chewing::input::keymap::QWERTY_MAP,
                    key,
                ));
        }
        let _ = s.chewing_editor.display();
        let data = s.chewing_editor.d512_calibration().unwrap();
        assert_eq!(
            data.candidates
                .iter()
                .find(|c| c.text == word)
                .unwrap()
                .personal_hits,
            hits
        );
    }
}
