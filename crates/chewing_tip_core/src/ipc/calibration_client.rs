// SPDX-License-Identifier: GPL-3.0-or-later

use super::{client::ChewingIpcClient, d512::calibration::CalibrationRecord, varlink::MethodCall};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};

/// A separate bounded worker: file/pipe/attestation work never runs on the UI
/// thread, and a stalled calibration operation cannot hold up scoring.
pub fn try_send(records: Vec<CalibrationRecord>) -> bool {
    if records.is_empty() {
        return false;
    }
    static WORKER: OnceLock<Result<Worker, ()>> = OnceLock::new();
    WORKER
        .get_or_init(production_worker)
        .as_ref()
        .is_ok_and(|w| w.try_send(records))
}
fn production_worker() -> Result<Worker, ()> {
    use windows::{
        Win32::{
            Foundation::HMODULE,
            System::LibraryLoader::{
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN,
                GetModuleHandleExW,
            },
        },
        core::PCWSTR,
    };
    // As with the scorer worker, TSF must not unload a DLL with executing code.
    let mut module = HMODULE::default();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            PCWSTR(production_worker as *const () as *const u16),
            &mut module,
        )
    }
    .map_err(|_| ())?;
    Worker::spawn(|calls| {
        let client = ChewingIpcClient::new();
        client
            .connect_for_scoring(Instant::now() + Duration::from_millis(100))
            .map_err(|_| ())?;
        // The connection is nonblocking, each call is oneway and already bounded.
        // A short/failed write discards this client; no partial-frame retry.
        for call in calls {
            client.send(call).map_err(|_| ())?;
        }
        Ok(())
    })
}
struct Worker {
    sender: SyncSender<Vec<CalibrationRecord>>,
    busy: Arc<AtomicBool>,
}
impl Worker {
    fn spawn(
        mut operation: impl FnMut(Vec<MethodCall>) -> Result<(), ()> + Send + 'static,
    ) -> Result<Self, ()> {
        let (sender, receiver) = mpsc::sync_channel::<Vec<CalibrationRecord>>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let worker_busy = busy.clone();
        std::thread::Builder::new()
            .name("chewing-calibration-ipc".into())
            .spawn(move || {
                while let Ok(records) = receiver.recv() {
                    let calls = records
                        .into_iter()
                        .filter_map(|r| r.into_call().ok())
                        .collect::<Vec<_>>();
                    if !calls.is_empty()
                        && catch_unwind(AssertUnwindSafe(|| operation(calls)))
                            .unwrap_or(Err(()))
                            .is_err()
                    {
                        log::error!("D512 calibration records could not be sent.");
                    }
                    worker_busy.store(false, Ordering::Release);
                }
            })
            .map_err(|_| ())?;
        Ok(Self { sender, busy })
    }
    fn try_send(&self, records: Vec<CalibrationRecord>) -> bool {
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        if self.sender.try_send(records).is_err() {
            self.busy.store(false, Ordering::Release);
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};
    fn record() -> CalibrationRecord {
        serde_json::from_str(r#"{"id":"synthetic","phase":"rerank","reading":["ㄐㄧㄚˇ"],"candidates":[{"text":"甲","raw":-1,"modelLogP":-2,"personalHits":0,"fused":-1.5}],"defaultIndex":0,"winnerIndex":0,"applied":false,"personalConflict":false,"pomConflict":false,"weight":0.25,"margin":0.5,"bonus":0,"mode":"automatic","suggested":false,"accepted":null,"finalCommittedText":null}"#).unwrap()
    }
    #[test]
    fn calibration_busy_sender_drops_without_waiting_or_retrying() {
        let (entered, receiver) = mpsc::sync_channel(1);
        let (release, gate) = mpsc::sync_channel(1);
        let worker = Worker::spawn(move |calls| {
            entered.send(calls).unwrap();
            gate.recv().unwrap();
            Ok(())
        })
        .unwrap();
        assert!(worker.try_send(vec![record()]));
        let calls = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].oneway, Some(true));
        // Must return while the operation is blocked; release is sent only later.
        assert!(!worker.try_send(vec![record()]));
        release.send(()).unwrap();
    }
    #[test]
    fn calibration_worker_discards_oversized_record_and_keeps_following_valid_record() {
        let (tx, rx) = mpsc::sync_channel(1);
        let worker = Worker::spawn(move |calls| {
            tx.send(calls).unwrap();
            Ok(())
        })
        .unwrap();
        let mut huge = record();
        huge.final_committed_text = Some("x".repeat(65536));
        assert!(worker.try_send(vec![huge, record()]));
        let calls = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].parameters["id"], "synthetic");
    }
}
