// SPDX-License-Identifier: GPL-3.0-or-later

#![cfg(windows)]

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use chewing_tip_core::config::Config;
use serde_json::json;
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, RegOverridePredefKey};
use windows_registry::{CURRENT_USER, Key};

// HKCU overrides affect this test process, so all registry tests share this lock.
static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

struct IsolatedRegistry {
    original_user: Key,
    root: Key,
    path: String,
    overridden: bool,
}

impl IsolatedRegistry {
    fn new() -> Self {
        let original_user = CURRENT_USER.options().read().write().open("").unwrap();
        let path = format!(
            "Software\\ChewingTsfConfigTest-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = original_user.create(&path).unwrap();
        let mut registry = Self {
            original_user,
            root,
            path,
            overridden: false,
        };
        // Keep production from_reg/save_reg calls inside this temporary subtree.
        unsafe {
            RegOverridePredefKey(HKEY_CURRENT_USER, Some(HKEY(registry.root.as_raw())))
                .ok()
                .unwrap();
        }
        registry.overridden = true;
        CURRENT_USER.create("Software\\ChewingTextService").unwrap();
        registry
    }

    fn settings(&self) -> Key {
        self.root.create("Software\\ChewingTextService").unwrap()
    }
}

impl Drop for IsolatedRegistry {
    fn drop(&mut self) {
        if self.overridden {
            unsafe { RegOverridePredefKey(HKEY_CURRENT_USER, None).ok().unwrap() };
        }
        self.original_user.remove_tree(&self.path).unwrap();
    }
}

fn assert_settings(cfg: &Config, mode: i32, weight: f64, margin: f64) {
    let value = serde_json::to_value(&cfg.chewing_tsf).unwrap();
    assert_eq!(value["d512_mode"], json!(mode));
    assert_eq!(value["d512_weight"], json!(weight));
    assert_eq!(value["d512_margin"], json!(margin));
}

fn assert_input_habits(cfg: &Config, ctrl_symbol: bool, cursor_wrap: bool) {
    let value = serde_json::to_value(&cfg.chewing_tsf).unwrap();
    assert_eq!(value["ctrl_symbol_new_phonetic"], json!(ctrl_symbol));
    assert_eq!(value["cursor_wrap"], json!(cursor_wrap));
}

fn assert_personal_pref(cfg: &Config, enabled: bool, automatic: f64, suggest: f64) {
    let value = serde_json::to_value(&cfg.chewing_tsf).unwrap();
    assert_eq!(value["use_personal_candidate_preference"], json!(enabled));
    assert_eq!(value["d512_personal_bonus"], json!(automatic));
    assert_eq!(value["d512_suggest_personal_bonus"], json!(suggest));
}

#[test]
fn personal_pref_missing_registry_values_use_and_save_defaults() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    assert_personal_pref(&Config::default(), true, 2., 2.);
    let cfg = Config::from_reg().unwrap();
    assert_personal_pref(&cfg, true, 2., 2.);
    assert!(key.get_u32("UsePersonalCandidatePreference").is_err());
    assert!(key.get_string("D512PersonalBonus").is_err());
    assert!(key.get_string("D512SuggestPersonalBonus").is_err());
    cfg.save_reg();
    assert_eq!(key.get_u32("UsePersonalCandidatePreference").unwrap(), 1);
    assert_eq!(key.get_string("D512PersonalBonus").unwrap(), "2");
    assert_eq!(key.get_string("D512SuggestPersonalBonus").unwrap(), "2");
}

#[test]
fn personal_pref_reads_dword_and_independent_decimal_bonuses() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    for (enabled, automatic, suggest) in [(0, "0", "1.25"), (1, "0.5", "0"), (7, "3.75", "4.125")] {
        key.set_u32("UsePersonalCandidatePreference", enabled)
            .unwrap();
        key.set_string("D512PersonalBonus", automatic).unwrap();
        key.set_string("D512SuggestPersonalBonus", suggest).unwrap();
        assert_personal_pref(
            &Config::from_reg().unwrap(),
            enabled > 0,
            automatic.parse().unwrap(),
            suggest.parse().unwrap(),
        );
    }
}

#[test]
fn personal_pref_save_roundtrips_without_overwriting_existing_settings() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    for (enabled, automatic, suggest) in [(false, 0., 3.75), (true, 4.125, 0.)] {
        let mut value = serde_json::to_value(Config::default()).unwrap();
        value["chewing_tsf"]["use_personal_candidate_preference"] = json!(enabled);
        value["chewing_tsf"]["d512_personal_bonus"] = json!(automatic);
        value["chewing_tsf"]["d512_suggest_personal_bonus"] = json!(suggest);
        let mut cfg: Config = serde_json::from_value(value).unwrap();
        cfg.chewing_tsf.d512_mode = 2;
        cfg.chewing_tsf.d512_weight = 0.375;
        cfg.chewing_tsf.d512_margin = 0.625;
        cfg.chewing_tsf.d512_suggest_weight = 0.5;
        cfg.chewing_tsf.d512_suggest_margin = 3.;
        cfg.chewing_tsf.d512_log_calibration = 1;
        cfg.chewing_tsf.d512_suggestion_accept_key = 2;
        cfg.chewing_tsf.cursor_wrap = true;
        cfg.save_reg();
        assert_eq!(
            key.get_u32("UsePersonalCandidatePreference").unwrap(),
            u32::from(enabled)
        );
        assert_eq!(
            key.get_string("D512PersonalBonus").unwrap(),
            automatic.to_string()
        );
        assert_eq!(
            key.get_string("D512SuggestPersonalBonus").unwrap(),
            suggest.to_string()
        );
        let loaded = Config::from_reg().unwrap();
        assert_personal_pref(&loaded, enabled, automatic, suggest);
        assert_settings(&loaded, 2, 0.375, 0.625);
        assert_eq!(loaded.chewing_tsf.d512_suggest_weight, 0.5);
        assert_eq!(loaded.chewing_tsf.d512_suggest_margin, 3.);
        assert_eq!(loaded.chewing_tsf.d512_log_calibration, 1);
        assert_eq!(loaded.chewing_tsf.d512_suggestion_accept_key, 2);
        assert_input_habits(&loaded, false, true);
    }
}

#[test]
fn personal_pref_invalid_registry_values_fall_back_independently() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    key.set_u32("UsePersonalCandidatePreference", 0).unwrap();
    for invalid in ["", "invalid", "NaN", "inf", "-inf", "1e999", "-0.01", "-1"] {
        key.set_string("D512PersonalBonus", invalid).unwrap();
        key.set_string("D512SuggestPersonalBonus", "3.75").unwrap();
        assert_personal_pref(&Config::from_reg().unwrap(), false, 2., 3.75);
        key.set_string("D512PersonalBonus", "4.125").unwrap();
        key.set_string("D512SuggestPersonalBonus", invalid).unwrap();
        assert_personal_pref(&Config::from_reg().unwrap(), false, 4.125, 2.);
    }
    key.set_string("UsePersonalCandidatePreference", "0")
        .unwrap();
    key.set_u32("D512PersonalBonus", 3).unwrap();
    key.set_u64("D512SuggestPersonalBonus", 4).unwrap();
    assert_personal_pref(&Config::from_reg().unwrap(), true, 2., 2.);
}

#[test]
fn personal_pref_survives_unrelated_config_save() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    key.set_u32("UsePersonalCandidatePreference", 0).unwrap();
    key.set_string("D512PersonalBonus", "0.75").unwrap();
    key.set_string("D512SuggestPersonalBonus", "3.5").unwrap();
    let mut cfg = Config::from_reg().unwrap();
    cfg.chewing_tsf.font_size = 19;
    cfg.chewing_tsf.d512_mode = 0;
    cfg.chewing_tsf.esc_clean_all_buf = true;
    cfg.save_reg();
    assert_personal_pref(&Config::from_reg().unwrap(), false, 0.75, 3.5);
    assert_eq!(key.get_u32("UsePersonalCandidatePreference").unwrap(), 0);
    assert_eq!(key.get_string("D512PersonalBonus").unwrap(), "0.75");
    assert_eq!(key.get_string("D512SuggestPersonalBonus").unwrap(), "3.5");
    assert_eq!(key.get_u32("DefFontSize").unwrap(), 19);
    assert_eq!(key.get_u32("D512Mode").unwrap(), 0);
    assert_eq!(key.get_u32("EscCleanAllBuf").unwrap(), 1);
}

#[test]
fn personal_pref_older_json_defaults_only_missing_fields() {
    for missing in [
        &["use_personal_candidate_preference"][..],
        &["d512_personal_bonus"][..],
        &["d512_suggest_personal_bonus"][..],
        &[
            "use_personal_candidate_preference",
            "d512_personal_bonus",
            "d512_suggest_personal_bonus",
        ][..],
    ] {
        let mut value = serde_json::to_value(Config::default()).unwrap();
        value["chewing_tsf"]["use_personal_candidate_preference"] = json!(false);
        value["chewing_tsf"]["d512_personal_bonus"] = json!(0.75);
        value["chewing_tsf"]["d512_suggest_personal_bonus"] = json!(3.5);
        for field in missing {
            value["chewing_tsf"].as_object_mut().unwrap().remove(*field);
        }
        assert_personal_pref(
            &serde_json::from_value(value).unwrap(),
            missing.contains(&"use_personal_candidate_preference"),
            if missing.contains(&"d512_personal_bonus") {
                2.
            } else {
                0.75
            },
            if missing.contains(&"d512_suggest_personal_bonus") {
                2.
            } else {
                3.5
            },
        );
    }
}

#[test]
fn input_habits_missing_registry_values_use_and_save_tsf_defaults() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    assert_input_habits(&Config::default(), false, false);
    let cfg = Config::from_reg().unwrap();
    assert_input_habits(&cfg, false, false);
    assert!(key.get_u32("CtrlSymbolNewPhonetic").is_err());
    assert!(key.get_u32("CursorWrap").is_err());
    cfg.save_reg();
    assert_eq!(key.get_u32("CtrlSymbolNewPhonetic").unwrap(), 0);
    assert_eq!(key.get_u32("CursorWrap").unwrap(), 0);
}

#[test]
fn input_habits_reads_dword_booleans_independently() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    for (ctrl_symbol, cursor_wrap) in [(0, 0), (0, 1), (1, 0), (1, 1), (7, u32::MAX)] {
        key.set_u32("CtrlSymbolNewPhonetic", ctrl_symbol).unwrap();
        key.set_u32("CursorWrap", cursor_wrap).unwrap();
        assert_input_habits(
            &Config::from_reg().unwrap(),
            ctrl_symbol > 0,
            cursor_wrap > 0,
        );
    }
}

#[test]
fn input_habits_save_roundtrips_all_boolean_pairs() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    for (ctrl_symbol, cursor_wrap) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut value = serde_json::to_value(Config::default()).unwrap();
        value["chewing_tsf"]["ctrl_symbol_new_phonetic"] = json!(ctrl_symbol);
        value["chewing_tsf"]["cursor_wrap"] = json!(cursor_wrap);
        let cfg: Config = serde_json::from_value(value).unwrap();
        cfg.save_reg();
        assert_eq!(
            key.get_u32("CtrlSymbolNewPhonetic").unwrap(),
            u32::from(ctrl_symbol)
        );
        assert_eq!(key.get_u32("CursorWrap").unwrap(), u32::from(cursor_wrap));
        assert_input_habits(&Config::from_reg().unwrap(), ctrl_symbol, cursor_wrap);
    }
}

#[test]
fn input_habits_survive_unrelated_config_save() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    for (ctrl_symbol, cursor_wrap) in [(0, 1), (1, 0)] {
        key.set_u32("CtrlSymbolNewPhonetic", ctrl_symbol).unwrap();
        key.set_u32("CursorWrap", cursor_wrap).unwrap();
        key.set_string("D512Weight", "0.375").unwrap();
        let mut cfg = Config::from_reg().unwrap();
        cfg.chewing_tsf.esc_clean_all_buf = true;
        cfg.chewing_tsf.font_size = 19;
        cfg.chewing_tsf.d512_mode = 2;
        cfg.save_reg();
        assert_eq!(key.get_u32("CtrlSymbolNewPhonetic").unwrap(), ctrl_symbol);
        assert_eq!(key.get_u32("CursorWrap").unwrap(), cursor_wrap);
        assert_eq!(key.get_u32("EscCleanAllBuf").unwrap(), 1);
        assert_eq!(key.get_u32("DefFontSize").unwrap(), 19);
        assert_eq!(key.get_u32("D512Mode").unwrap(), 2);
        assert_eq!(key.get_string("D512Weight").unwrap(), "0.375");
        assert_input_habits(
            &Config::from_reg().unwrap(),
            ctrl_symbol > 0,
            cursor_wrap > 0,
        );
    }
}

#[test]
fn input_habits_older_json_uses_defaults_for_missing_fields_only() {
    for (missing, ctrl_symbol, cursor_wrap) in [
        (&["ctrl_symbol_new_phonetic"][..], false, true),
        (&["cursor_wrap"][..], true, false),
        (
            &["ctrl_symbol_new_phonetic", "cursor_wrap"][..],
            false,
            false,
        ),
    ] {
        let mut value = serde_json::to_value(Config::default()).unwrap();
        value["chewing_tsf"]["ctrl_symbol_new_phonetic"] = json!(true);
        value["chewing_tsf"]["cursor_wrap"] = json!(true);
        for field in missing {
            value["chewing_tsf"].as_object_mut().unwrap().remove(*field);
        }
        assert_input_habits(
            &serde_json::from_value(value).unwrap(),
            ctrl_symbol,
            cursor_wrap,
        );
    }
}

#[test]
fn d512_default_fields_exist() {
    assert_settings(&Config::default(), 1, 0.25, 0.5);
}

#[test]
fn d512_missing_registry_values_use_and_save_defaults() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    assert_settings(&Config::from_reg().unwrap(), 1, 0.25, 0.5);

    Config::default().save_reg();
    let key = registry.settings();
    assert_eq!(key.get_u32("D512Mode").unwrap(), 1);
    assert_eq!(key.get_string("D512Weight").unwrap(), "0.25");
    assert_eq!(key.get_string("D512Margin").unwrap(), "0.5");
}

#[test]
fn d512_registry_modes_preserve_user_choice() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    key.set_string("D512Weight", "0.5").unwrap();
    key.set_string("D512Margin", "0.125").unwrap();

    for (stored, loaded) in [(0, 0), (1, 1), (2, 2), (3, 1), (u32::MAX, 1)] {
        key.set_u32("D512Mode", stored).unwrap();
        assert_settings(&Config::from_reg().unwrap(), loaded, 0.5, 0.125);
    }
}

#[test]
fn d512_save_uses_dword_mode_and_decimal_strings() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();

    for stored in [0, 1, 2] {
        let mut value = serde_json::to_value(Config::default()).unwrap();
        value["chewing_tsf"]["d512_mode"] = json!(stored);
        value["chewing_tsf"]["d512_weight"] = json!(0.75);
        value["chewing_tsf"]["d512_margin"] = json!(0.125);
        let cfg: Config = serde_json::from_value(value).unwrap();
        cfg.save_reg();

        assert_eq!(key.get_u32("D512Mode").unwrap(), stored);
        assert_eq!(key.get_string("D512Weight").unwrap(), "0.75");
        assert_eq!(key.get_string("D512Margin").unwrap(), "0.125");
        assert_settings(&Config::from_reg().unwrap(), stored as i32, 0.75, 0.125);
    }
}

#[test]
fn d512_invalid_values_fall_back_independently() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    key.set_u32("D512Mode", 0).unwrap();

    for invalid in ["", "invalid", "NaN", "inf", "-inf", "1e999", "-0.01", "-1"] {
        key.set_string("D512Weight", invalid).unwrap();
        key.set_string("D512Margin", "0.375").unwrap();
        assert_settings(&Config::from_reg().unwrap(), 0, 0.25, 0.375);

        key.set_string("D512Weight", "0.625").unwrap();
        key.set_string("D512Margin", invalid).unwrap();
        assert_settings(&Config::from_reg().unwrap(), 0, 0.625, 0.5);
    }

    key.set_string("D512Mode", "0").unwrap();
    key.set_u32("D512Weight", 1).unwrap();
    key.set_u64("D512Margin", 1).unwrap();
    assert_settings(&Config::from_reg().unwrap(), 1, 0.25, 0.5);
}

#[test]
fn d512_suggestion_choice_survives_unrelated_config_save() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    key.set_u32("D512Mode", 2).unwrap();

    let mut cfg = Config::from_reg().unwrap();
    cfg.chewing_tsf.font_size = 18;
    cfg.save_reg();

    assert_eq!(key.get_u32("D512Mode").unwrap(), 2);
    assert_settings(&Config::from_reg().unwrap(), 2, 0.25, 0.5);
}

#[test]
fn d512_zero_weight_and_margin_are_valid() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    key.set_string("D512Weight", "0").unwrap();
    key.set_string("D512Margin", "0").unwrap();
    assert_settings(&Config::from_reg().unwrap(), 1, 0.0, 0.0);
}

#[test]
fn d512_effective_mode_does_not_rewrite_user_choice() {
    let mut cfg = Config::default();
    for (stored, effective) in [(0, 0), (1, 1), (2, 2), (3, 1), (-1, 1)] {
        cfg.chewing_tsf.d512_mode = stored;
        assert_eq!(cfg.chewing_tsf.effective_d512_mode(), effective);
        assert_eq!(cfg.chewing_tsf.d512_mode, stored);
    }
}

#[test]
fn d512_older_json_uses_defaults_for_new_fields() {
    let mut value = serde_json::to_value(Config::default()).unwrap();
    let fields = value["chewing_tsf"].as_object_mut().unwrap();
    for field in ["d512_mode", "d512_weight", "d512_margin"] {
        fields.remove(field);
    }
    assert_settings(&serde_json::from_value(value).unwrap(), 1, 0.25, 0.5);
}

fn assert_accept_key(cfg: &Config, expected: i32) {
    let value = serde_json::to_value(&cfg.chewing_tsf).unwrap();
    assert_eq!(value["d512_suggestion_accept_key"], json!(expected));
}

#[test]
fn d512_accept_key_defaults_to_tab() {
    assert_accept_key(&Config::default(), 0);
}

#[test]
fn d512_accept_key_missing_registry_value_uses_and_saves_default() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    let cfg = Config::from_reg().unwrap();
    assert_accept_key(&cfg, 0);
    assert!(key.get_u32("D512SuggestionAcceptKey").is_err());
    cfg.save_reg();
    assert_eq!(key.get_u32("D512SuggestionAcceptKey").unwrap(), 0);
}

#[test]
fn d512_accept_key_older_json_uses_default() {
    let mut value = serde_json::to_value(Config::default()).unwrap();
    value["chewing_tsf"]
        .as_object_mut()
        .unwrap()
        .remove("d512_suggestion_accept_key");
    assert_accept_key(&serde_json::from_value(value).unwrap(), 0);
}

#[test]
fn d512_accept_key_registry_choices_survive_unrelated_save() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    for stored in [0, 1, 2] {
        key.set_u32("D512SuggestionAcceptKey", stored).unwrap();
        let mut cfg = Config::from_reg().unwrap();
        assert_accept_key(&cfg, stored as i32);
        cfg.chewing_tsf.font_size = 18;
        cfg.save_reg();
        assert_eq!(key.get_u32("D512SuggestionAcceptKey").unwrap(), stored);
    }
}

#[test]
fn d512_accept_key_save_writes_the_selected_dword() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    for stored in [0, 1, 2] {
        let mut value = serde_json::to_value(Config::default()).unwrap();
        value["chewing_tsf"]["d512_suggestion_accept_key"] = json!(stored);
        let cfg: Config = serde_json::from_value(value).unwrap();
        cfg.save_reg();
        assert_eq!(key.get_u32("D512SuggestionAcceptKey").unwrap(), stored);
        assert_accept_key(&Config::from_reg().unwrap(), stored as i32);
    }
}

#[test]
fn d512_accept_key_out_of_range_dwords_are_preserved() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    for stored in [3, 42, i32::MAX as u32, 0x8000_0000, u32::MAX] {
        key.set_u32("D512SuggestionAcceptKey", stored).unwrap();
        let mut cfg = Config::from_reg().unwrap();
        assert_accept_key(&cfg, stored as i32);
        assert_eq!(key.get_u32("D512SuggestionAcceptKey").unwrap(), stored);
        cfg.chewing_tsf.font_size = 19;
        cfg.save_reg();
        assert_eq!(key.get_u32("D512SuggestionAcceptKey").unwrap(), stored);
        assert_accept_key(&Config::from_reg().unwrap(), stored as i32);
    }
}

#[test]
fn d512_accept_key_wrong_registry_type_uses_default_without_writing() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    key.set_string("D512SuggestionAcceptKey", "1").unwrap();
    assert_accept_key(&Config::from_reg().unwrap(), 0);
    assert_eq!(key.get_string("D512SuggestionAcceptKey").unwrap(), "1");
}

#[test]
fn d512_accept_key_effective_value_does_not_rewrite_user_choice() {
    let mut cfg = Config::default();
    for mode in [0, 1, 2] {
        cfg.chewing_tsf.d512_mode = mode;
        for (stored, effective) in [
            (0, 0),
            (1, 1),
            (2, 2),
            (3, 0),
            (42, 0),
            (i32::MAX, 0),
            (i32::MIN, 0),
            (-1, 0),
        ] {
            cfg.chewing_tsf.d512_suggestion_accept_key = stored;
            assert_eq!(
                cfg.chewing_tsf.effective_d512_suggestion_accept_key(),
                effective
            );
            assert_accept_key(&cfg, stored);
            assert_eq!(cfg.chewing_tsf.d512_mode, mode);
        }
    }
}

fn assert_suggest_params(cfg: &Config, weight: f64, margin: f64) {
    let value = serde_json::to_value(&cfg.chewing_tsf).unwrap();
    assert_eq!(value["d512_suggest_weight"], json!(weight));
    assert_eq!(value["d512_suggest_margin"], json!(margin));
}

#[test]
fn d512_suggest_params_defaults_and_older_json() {
    assert_suggest_params(&Config::default(), 0.25, 2.0);
    let mut value = serde_json::to_value(Config::default()).unwrap();
    let fields = value["chewing_tsf"].as_object_mut().unwrap();
    fields.remove("d512_suggest_weight");
    fields.remove("d512_suggest_margin");
    assert_suggest_params(&serde_json::from_value(value).unwrap(), 0.25, 2.0);
}

#[test]
fn d512_suggest_params_registry_roundtrip_is_independent() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    key.set_string("D512Weight", "0.375").unwrap();
    key.set_string("D512Margin", "0.5").unwrap();
    key.set_string("D512SuggestWeight", "0.625").unwrap();
    key.set_string("D512SuggestMargin", "1.125").unwrap();
    let cfg = Config::from_reg().unwrap();
    assert_settings(&cfg, 1, 0.375, 0.5);
    assert_suggest_params(&cfg, 0.625, 1.125);
    cfg.save_reg();
    assert_eq!(key.get_string("D512SuggestWeight").unwrap(), "0.625");
    assert_eq!(key.get_string("D512SuggestMargin").unwrap(), "1.125");
    assert_settings(&Config::from_reg().unwrap(), 1, 0.375, 0.5);
}

#[test]
fn d512_suggest_params_missing_and_invalid_values_use_defaults() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    let cfg = Config::from_reg().unwrap();
    assert_suggest_params(&cfg, 0.25, 2.0);
    assert!(key.get_string("D512SuggestWeight").is_err());
    assert!(key.get_string("D512SuggestMargin").is_err());
    cfg.save_reg();
    assert_eq!(key.get_string("D512SuggestWeight").unwrap(), "0.25");
    assert_eq!(key.get_string("D512SuggestMargin").unwrap(), "2");
    for invalid in ["", "invalid", "NaN", "inf", "-inf", "1e999", "-0.01"] {
        key.set_string("D512SuggestWeight", invalid).unwrap();
        key.set_string("D512SuggestMargin", "0.75").unwrap();
        assert_suggest_params(&Config::from_reg().unwrap(), 0.25, 0.75);
        key.set_string("D512SuggestWeight", "0.5").unwrap();
        key.set_string("D512SuggestMargin", invalid).unwrap();
        assert_suggest_params(&Config::from_reg().unwrap(), 0.5, 2.0);
    }
    key.set_u32("D512SuggestWeight", 1).unwrap();
    key.set_u64("D512SuggestMargin", 1).unwrap();
    assert_suggest_params(&Config::from_reg().unwrap(), 0.25, 2.0);
}

#[test]
fn d512_suggest_params_save_decimal_strings_and_zero() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    for (weight, margin) in [(0.625, 1.125), (0.0, 0.0)] {
        let mut value = serde_json::to_value(Config::default()).unwrap();
        value["chewing_tsf"]["d512_suggest_weight"] = json!(weight);
        value["chewing_tsf"]["d512_suggest_margin"] = json!(margin);
        let cfg: Config = serde_json::from_value(value).unwrap();
        cfg.save_reg();
        assert_eq!(
            key.get_string("D512SuggestWeight").unwrap(),
            weight.to_string()
        );
        assert_eq!(
            key.get_string("D512SuggestMargin").unwrap(),
            margin.to_string()
        );
        assert_suggest_params(&Config::from_reg().unwrap(), weight, margin);
        assert_settings(&cfg, 1, 0.25, 0.5);
    }
}

#[test]
fn d512_saved_values_survive_default_changes() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    // Preserve both former defaults and explicit choices, including disabled mode.
    for mode in [0, 1, 2] {
        for (auto_margin, suggest_margin) in [("0.25", "4.288167167322101"), ("0", "0")] {
            key.set_u32("D512Mode", mode).unwrap();
            key.set_string("D512Weight", "0.25").unwrap();
            key.set_string("D512Margin", auto_margin).unwrap();
            key.set_string("D512SuggestWeight", "0.25").unwrap();
            key.set_string("D512SuggestMargin", suggest_margin).unwrap();
            let mut cfg = Config::from_reg().unwrap();
            assert_settings(&cfg, mode as i32, 0.25, auto_margin.parse().unwrap());
            assert_suggest_params(&cfg, 0.25, suggest_margin.parse().unwrap());
            cfg = serde_json::from_value(serde_json::to_value(cfg).unwrap()).unwrap();
            cfg.chewing_tsf.font_size = 18;
            cfg.save_reg();
            assert_eq!(key.get_string("D512Weight").unwrap(), "0.25");
            assert_eq!(key.get_string("D512Margin").unwrap(), auto_margin);
            assert_eq!(key.get_string("D512SuggestWeight").unwrap(), "0.25");
            assert_eq!(key.get_string("D512SuggestMargin").unwrap(), suggest_margin);
        }
    }
}

fn assert_log_calibration(cfg: &Config, expected: i32) {
    let value = serde_json::to_value(&cfg.chewing_tsf).unwrap();
    assert_eq!(value["d512_log_calibration"], json!(expected));
}

#[test]
fn d512_log_calibration_defaults_to_off() {
    assert_log_calibration(&Config::default(), 0);
}

#[test]
fn d512_log_calibration_older_json_defaults_to_off() {
    let mut value = serde_json::to_value(Config::default()).unwrap();
    value["chewing_tsf"]
        .as_object_mut()
        .unwrap()
        .remove("d512_log_calibration");
    assert_log_calibration(&serde_json::from_value(value).unwrap(), 0);
}

#[test]
fn d512_log_calibration_missing_registry_value_uses_and_saves_default() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    let cfg = Config::from_reg().unwrap();
    assert_log_calibration(&cfg, 0);
    assert!(key.get_u32("D512LogCalibration").is_err());
    cfg.save_reg();
    assert_eq!(key.get_u32("D512LogCalibration").unwrap(), 0);
}

#[test]
fn d512_log_calibration_preserves_dword_choices_and_bits() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    for stored in [0, 1, 2, 42, i32::MAX as u32, 0x8000_0000, u32::MAX] {
        let mut value = serde_json::to_value(Config::default()).unwrap();
        value["chewing_tsf"]["d512_log_calibration"] = json!(stored as i32);
        let cfg: Config = serde_json::from_value(value).unwrap();
        cfg.save_reg();
        assert_eq!(key.get_u32("D512LogCalibration").unwrap(), stored);
        let mut loaded = Config::from_reg().unwrap();
        assert_log_calibration(&loaded, stored as i32);
        loaded.chewing_tsf.font_size = 19;
        loaded.save_reg();
        assert_eq!(key.get_u32("D512LogCalibration").unwrap(), stored);
    }
}

#[test]
fn d512_log_calibration_wrong_registry_type_uses_default_without_writing() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    let key = registry.settings();
    key.set_string("D512LogCalibration", "1").unwrap();
    assert_log_calibration(&Config::from_reg().unwrap(), 0);
    assert_eq!(key.get_string("D512LogCalibration").unwrap(), "1");
}
