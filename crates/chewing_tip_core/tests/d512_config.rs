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

#[test]
fn d512_default_fields_exist() {
    assert_settings(&Config::default(), 1, 0.25, 0.25);
}

#[test]
fn d512_missing_registry_values_use_and_save_defaults() {
    let _lock = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let registry = IsolatedRegistry::new();
    assert_settings(&Config::from_reg().unwrap(), 1, 0.25, 0.25);

    Config::default().save_reg();
    let key = registry.settings();
    assert_eq!(key.get_u32("D512Mode").unwrap(), 1);
    assert_eq!(key.get_string("D512Weight").unwrap(), "0.25");
    assert_eq!(key.get_string("D512Margin").unwrap(), "0.25");
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
        assert_settings(&Config::from_reg().unwrap(), 0, 0.625, 0.25);
    }

    key.set_string("D512Mode", "0").unwrap();
    key.set_u32("D512Weight", 1).unwrap();
    key.set_u64("D512Margin", 1).unwrap();
    assert_settings(&Config::from_reg().unwrap(), 1, 0.25, 0.25);
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
    assert_settings(&Config::from_reg().unwrap(), 2, 0.25, 0.25);
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
    for (stored, effective) in [(0, 0), (1, 1), (2, 1), (3, 1), (-1, 1)] {
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
    assert_settings(&serde_json::from_value(value).unwrap(), 1, 0.25, 0.25);
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
