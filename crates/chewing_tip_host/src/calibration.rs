// SPDX-License-Identifier: GPL-3.0-or-later

use chewing_tip_core::ipc::{d512::calibration::CalibrationRecord, varlink::MethodCall};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::Mutex,
};

pub(crate) struct CalibrationLog {
    sequence: Mutex<()>,
    enabled: Box<dyn Fn() -> bool + Send + Sync>,
    directory: Box<dyn Fn() -> Result<PathBuf, ()> + Send + Sync>,
}

fn enabled_from_registry() -> bool {
    windows_registry::CURRENT_USER
        .open("Software\\ChewingTextService")
        .and_then(|key| key.get_u32("D512LogCalibration"))
        .is_ok_and(|value| value == 1)
}
impl CalibrationLog {
    pub(crate) fn new() -> Self {
        Self::with_sources(enabled_from_registry, || {
            chewing_tip_core::shell::user_dir().map_err(|_| ())
        })
    }
    fn with_sources(
        enabled: impl Fn() -> bool + Send + Sync + 'static,
        directory: impl Fn() -> Result<PathBuf, ()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            sequence: Mutex::new(()),
            enabled: Box::new(enabled),
            directory: Box::new(directory),
        }
    }
    pub(crate) fn record_call(&self, call: MethodCall) {
        if self.write_call(call).is_err() {
            // Never format input, serde/IO errors, or paths here.
            log::error!("D512 calibration record could not be saved.");
        }
    }
    fn write_call(&self, call: MethodCall) -> Result<(), ()> {
        let _sequence = self.sequence.lock().map_err(|_| ())?;
        // Re-read inside the shared write sequence. Disabled means no path lookup or IO.
        if !(self.enabled)() {
            return Ok(());
        }
        if call.method != CalibrationRecord::METHOD
            || call.oneway != Some(true)
            || call.more == Some(true)
            || call.upgrade == Some(true)
        {
            return Err(());
        }
        let record: CalibrationRecord = serde_json::from_value(call.parameters).map_err(|_| ())?;
        let bytes = record.to_json_line().map_err(|_| ())?;
        let directory = (self.directory)()?;
        fs::create_dir_all(&directory).map_err(|_| ())?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join("calibration.jsonl"))
            .map_err(|_| ())?;
        file.write_all(&bytes).map_err(|_| ())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chewing_tip_core::ipc::d512::calibration::CalibrationRecord;
    use std::{
        fs,
        sync::{
            Arc, Once,
            atomic::{AtomicU64, Ordering},
        },
    };
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, RegOverridePredefKey};
    use windows_registry::{CURRENT_USER, Key};

    pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    pub(crate) struct TempDir(pub(crate) PathBuf);
    impl TempDir {
        pub(crate) fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "CalibrationLogTest-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct Registry {
        original: Key,
        root: Key,
        path: String,
    }
    impl Registry {
        fn new() -> Self {
            let original = CURRENT_USER.options().read().write().open("").unwrap();
            let path = format!(
                "Software\\CalibrationLogTest-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            );
            let root = original.create(&path).unwrap();
            unsafe {
                RegOverridePredefKey(HKEY_CURRENT_USER, Some(HKEY(root.as_raw())))
                    .ok()
                    .unwrap();
            }
            Self {
                original,
                root,
                path,
            }
        }
        fn key(&self) -> Key {
            self.root.create("Software\\ChewingTextService").unwrap()
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
    struct Capture(Mutex<Vec<String>>);
    static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));
    static INSTALL: Once = Once::new();
    impl log::Log for Capture {
        fn enabled(&self, m: &log::Metadata) -> bool {
            m.level() == log::Level::Error
        }
        fn log(&self, r: &log::Record) {
            if self.enabled(r.metadata()) {
                self.0.lock().unwrap().push(r.args().to_string());
            }
        }
        fn flush(&self) {}
    }
    fn captured() -> Vec<String> {
        INSTALL.call_once(|| {
            log::set_logger(&CAPTURE).unwrap();
            log::set_max_level(log::LevelFilter::Error);
        });
        CAPTURE
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.starts_with("D512 calibration"))
            .cloned()
            .collect()
    }
    pub(crate) fn records() -> Vec<CalibrationRecord> {
        include_str!("../tests/fixtures/calibration-mac.jsonl")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    pub(crate) fn always_enabled(path: PathBuf) -> CalibrationLog {
        CalibrationLog::with_sources(|| true, move || Ok(path.clone()))
    }
    fn log_at(path: PathBuf) -> CalibrationLog {
        CalibrationLog::with_sources(enabled_from_registry, move || Ok(path.clone()))
    }
    #[test]
    fn calibration_enabled_writes_mac_oracle_as_sorted_json_lines() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let registry = Registry::new();
        registry.key().set_u32("D512LogCalibration", 1).unwrap();
        let temp = TempDir::new();
        let logger = log_at(temp.0.clone());
        let expected = records();
        for record in &expected {
            logger.record_call(record.clone().into_call().unwrap());
        }
        let bytes = fs::read(temp.0.join("calibration.jsonl")).unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));
        let text = std::str::from_utf8(&bytes).unwrap();
        let actual: Vec<CalibrationRecord> = text
            .lines()
            .map(|line| {
                assert!(line.starts_with("{\"accepted\":"));
                assert!(line.contains("\"candidates\":[{\"fused\":"));
                serde_json::from_str(line).unwrap()
            })
            .collect();
        assert_eq!(actual, expected);
        // Explicitly requested offline compatibility artifact, synthetic data only.
        if let Some(export) = std::env::var_os("C2_CALIBRATION_TEST_EXPORT") {
            let export = PathBuf::from(export);
            fs::create_dir_all(export.parent().unwrap()).unwrap();
            fs::write(export, bytes).unwrap();
        }
    }
    #[test]
    fn calibration_disabled_touches_neither_existing_file_nor_new_directory() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let registry = Registry::new();
        registry.key().set_u32("D512LogCalibration", 0).unwrap();
        let temp = TempDir::new();
        let file = temp.0.join("calibration.jsonl");
        fs::write(&file, b"existing bytes\n").unwrap();
        log_at(temp.0.clone()).record_call(records()[0].clone().into_call().unwrap());
        assert_eq!(fs::read(&file).unwrap(), b"existing bytes\n");
        let absent = temp.0.join("absent");
        let logger = CalibrationLog::with_sources(enabled_from_registry, || {
            panic!("disabled logging resolved its directory")
        });
        logger.record_call(records()[0].clone().into_call().unwrap());
        assert!(!absent.exists());
    }
    #[test]
    fn calibration_rechecks_registry_for_each_record_and_only_one_enables_it() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let registry = Registry::new();
        let key = registry.key();
        let temp = TempDir::new();
        let logger = log_at(temp.0.clone());
        let call = || records()[0].clone().into_call().unwrap();
        logger.record_call(call());
        assert!(!temp.0.join("calibration.jsonl").exists());
        key.set_u32("D512LogCalibration", 1).unwrap();
        logger.record_call(call());
        let first = fs::read(temp.0.join("calibration.jsonl")).unwrap();
        for disabled in [0, 2, u32::MAX] {
            key.set_u32("D512LogCalibration", disabled).unwrap();
            logger.record_call(call());
            assert_eq!(fs::read(temp.0.join("calibration.jsonl")).unwrap(), first);
        }
        key.set_string("D512LogCalibration", "1").unwrap();
        logger.record_call(call());
        assert_eq!(fs::read(temp.0.join("calibration.jsonl")).unwrap(), first);
        key.set_u32("D512LogCalibration", 1).unwrap();
        logger.record_call(call());
        assert_eq!(
            fs::read(temp.0.join("calibration.jsonl")).unwrap(),
            first.repeat(2)
        );
    }
    #[test]
    fn calibration_write_failure_logs_once_without_content_and_host_keeps_working() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new();
        let blocked = temp.0.join("blocked");
        fs::write(&blocked, b"not a directory").unwrap();
        let root = blocked.clone();
        let logger = CalibrationLog::with_sources(|| true, move || Ok(root.clone()));
        let before = captured().len();
        logger.record_call(records()[0].clone().into_call().unwrap());
        assert_eq!(
            &captured()[before..],
            ["D512 calibration record could not be saved."]
        );
        fs::remove_file(&blocked).unwrap();
        logger.record_call(records()[0].clone().into_call().unwrap());
        assert_eq!(
            fs::read_to_string(blocked.join("calibration.jsonl"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert_eq!(captured().len(), before + 1);
    }
    #[test]
    fn calibration_concurrent_receivers_append_complete_serialized_records() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new();
        let root = temp.0.clone();
        let logger = Arc::new(CalibrationLog::with_sources(
            || true,
            move || Ok(root.clone()),
        ));
        let handles: Vec<_> = (0..8)
            .map(|thread| {
                let logger = logger.clone();
                std::thread::spawn(move || {
                    for i in 0..20 {
                        let mut record = records()[0].clone();
                        record.id = format!("synthetic-{thread}-{i}");
                        logger.record_call(record.into_call().unwrap());
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let text = fs::read_to_string(temp.0.join("calibration.jsonl")).unwrap();
        let ids: std::collections::HashSet<_> = text
            .lines()
            .map(|line| serde_json::from_str::<CalibrationRecord>(line).unwrap().id)
            .collect();
        assert_eq!(text.lines().count(), 160);
        assert_eq!(ids.len(), 160);
    }
    #[test]
    fn calibration_invalid_envelopes_and_payloads_never_echo_content_or_stop_host() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = TempDir::new();
        let root = temp.0.clone();
        let logger = CalibrationLog::with_sources(|| true, move || Ok(root.clone()));
        let before = captured().len();
        for kind in 0..4 {
            let mut call = records()[0].clone().into_call().unwrap();
            match kind {
                0 => call.oneway = Some(false),
                1 => call.more = Some(true),
                2 => call.upgrade = Some(true),
                _ => call.parameters["candidates"][0]["raw"] = "合成秘密不可回報".into(),
            }
            logger.record_call(call);
        }
        assert!(!temp.0.join("calibration.jsonl").exists());
        let errors = captured();
        assert_eq!(errors.len() - before, 4);
        assert!(
            errors[before..]
                .iter()
                .all(|line| line == "D512 calibration record could not be saved.")
        );
        logger.record_call(records()[0].clone().into_call().unwrap());
        assert_eq!(
            fs::read_to_string(temp.0.join("calibration.jsonl"))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }
}
