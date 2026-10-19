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
        send_calls(&client, calls)
    })
}
fn send_calls(client: &ChewingIpcClient, calls: Vec<MethodCall>) -> Result<(), ()> {
    // Calibration has its own worker and connection. Wait for buffer space here:
    // PIPE_NOWAIT + write_all can fail after the first terminal record, losing
    // the rest of a commit/cancel batch. UI and scoring never wait on this worker.
    client
        .pipe
        .try_borrow_mut()
        .map_err(|_| ())?
        .as_mut()
        .ok_or(())?
        .set_nonblocking(false)
        .map_err(|_| ())?;
    for call in calls {
        client.send(call).map_err(|_| ())?;
    }
    Ok(())
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
    use interprocess::os::windows::named_pipe::{
        DuplexPipeStream, PipeListenerOptions, pipe_mode::Bytes,
    };
    use std::{
        io::{BufRead, BufReader},
        sync::mpsc,
        time::Duration,
    };
    fn record() -> CalibrationRecord {
        serde_json::from_str(r#"{"id":"synthetic","phase":"rerank","reading":["ㄐㄧㄚˇ"],"candidates":[{"text":"甲","raw":-1,"modelLogP":-2,"personalHits":0,"fused":-1.5}],"defaultIndex":0,"winnerIndex":0,"applied":false,"personalConflict":false,"pomConflict":false,"weight":0.25,"margin":0.5,"bonus":0,"mode":"automatic","suggested":false,"accepted":null,"finalCommittedText":null}"#).unwrap()
    }
    fn terminal_batch_with_slow_reader(phase: &str) {
        let name = format!(
            r"\\.\pipe\chewing-calibration-test-{}",
            uuid::Uuid::new_v4()
        );
        let listener = PipeListenerOptions::new()
            .path(name.as_str())
            .create_duplex::<Bytes>()
            .unwrap();
        let (drain, gate) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let mut reader = BufReader::new(listener.accept().unwrap());
            // Hold the reader until send_calls returns or the bounded gate
            // expires. Blocking writes must wait here; PIPE_NOWAIT loses frames.
            let _ = gate.recv_timeout(Duration::from_millis(250));
            let mut records = vec![];
            loop {
                let mut bytes = vec![];
                if reader.read_until(0, &mut bytes).unwrap() == 0 {
                    break;
                }
                if bytes.pop() != Some(0) {
                    break; // A failed write can leave an incomplete final frame.
                }
                let call: MethodCall = serde_json::from_slice(&bytes).unwrap();
                records.push(serde_json::from_value::<CalibrationRecord>(call.parameters).unwrap());
            }
            records
        });
        let pipe = DuplexPipeStream::connect_by_path(name.as_str()).unwrap();
        pipe.set_nonblocking(true).unwrap(); // connect_for_scoring's initial mode
        let client = ChewingIpcClient::new();
        client.pipe.replace(Some(pipe));
        let expected: Vec<_> = (0..32)
            .map(|index| {
                let mut row = record();
                row.id = format!("synthetic-{index}");
                row.phase = phase.into();
                if index > 0 {
                    // Still below MAX_FRAME_BYTES, but larger than the pipe buffer.
                    row.candidates[0].text = "合".repeat(7000);
                }
                row.final_committed_text = (phase == "commit").then(|| "合成送出".into());
                row
            })
            .collect();
        let calls = expected
            .iter()
            .cloned()
            .map(|r| r.into_call().unwrap())
            .collect();
        let result = send_calls(&client, calls);
        let _ = drain.send(());
        drop(client);
        let actual = server.join().unwrap();
        assert_eq!(actual.len(), expected.len(), "batch lost terminal records");
        assert!(result.is_ok());
        assert_eq!(actual, expected);
    }
    #[test]
    fn calibration_commit_batch_survives_pipe_backpressure() {
        terminal_batch_with_slow_reader("commit");
    }
    #[test]
    fn calibration_cancel_batch_survives_pipe_backpressure() {
        terminal_batch_with_slow_reader("cancel");
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
