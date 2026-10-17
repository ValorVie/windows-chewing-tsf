// SPDX-License-Identifier: GPL-3.0-or-later

use super::{client::ChewingIpcClient, d512::D512Score};
use chewing::d512::{D512Error, SentenceRequest, SentenceScorer};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant},
};
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

type ScoreResult = Result<Vec<f64>, D512Error>;

/// Cheap to create: neither connects nor starts a worker until the first score.
pub struct IpcSentenceScorer;
impl SentenceScorer for IpcSentenceScorer {
    fn score(&mut self, request: &SentenceRequest, deadline: Instant) -> ScoreResult {
        if Instant::now() >= deadline {
            return Err(D512Error::Timeout);
        }
        static WORKER: OnceLock<Result<Worker, D512Error>> = OnceLock::new();
        match WORKER.get_or_init(production_worker) {
            Ok(worker) => worker.score(request, deadline),
            Err(error) => Err(*error),
        }
    }
}

fn production_worker() -> Result<Worker, D512Error> {
    // A background attestation may outlive the editor's deadline. Keep its DLL
    // loaded for the process lifetime so TSF cannot unload executing worker code.
    let mut module = HMODULE::default();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            PCWSTR(production_worker as *const () as *const u16),
            &mut module,
        )
    }
    .map_err(|_| D512Error::RuntimeUnavailable)?;
    let mut last_launch: Option<Instant> = None;
    Worker::spawn(move |message, deadline| {
        let client = ChewingIpcClient::new();
        if client.connect_for_scoring(deadline).is_err() {
            if Instant::now() < deadline
                && last_launch.is_none_or(|last| last.elapsed() >= Duration::from_secs(30))
            {
                last_launch = Some(Instant::now());
                let _ = crate::shell::launch_tip_host();
            }
            return Err(if Instant::now() >= deadline {
                D512Error::Timeout
            } else {
                D512Error::RuntimeUnavailable
            });
        }
        client.score_sentence(&message.into_request(), deadline)
    })
}

struct Job {
    message: D512Score,
    deadline: Instant,
    reply: SyncSender<ScoreResult>,
}
struct Worker {
    sender: SyncSender<Job>,
    busy: Arc<AtomicBool>,
}
impl Worker {
    fn spawn(
        mut operation: impl FnMut(D512Score, Instant) -> ScoreResult + Send + 'static,
    ) -> Result<Self, D512Error> {
        let (sender, receiver) = mpsc::sync_channel::<Job>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let worker_busy = busy.clone();
        std::thread::Builder::new()
            .name("chewing-d512-ipc".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    let result = if Instant::now() >= job.deadline {
                        Err(D512Error::Timeout)
                    } else {
                        catch_unwind(AssertUnwindSafe(|| operation(job.message, job.deadline)))
                            .unwrap_or(Err(D512Error::Inference))
                    };
                    // Clear before waking the caller; no queue of expired requests.
                    worker_busy.store(false, Ordering::Release);
                    let _ = job.reply.send(if Instant::now() >= job.deadline {
                        Err(D512Error::Timeout)
                    } else {
                        result
                    });
                }
            })
            .map_err(|_| D512Error::RuntimeUnavailable)?;
        Ok(Self { sender, busy })
    }
    fn score(&self, request: &SentenceRequest, deadline: Instant) -> ScoreResult {
        let message = D512Score::from_request(request, deadline)?;
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| D512Error::Busy)?;
        let (reply, receiver) = mpsc::sync_channel(1);
        if self
            .sender
            .try_send(Job {
                message,
                deadline,
                reply,
            })
            .is_err()
        {
            self.busy.store(false, Ordering::Release);
            return Err(D512Error::Busy);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let result = receiver
            .recv_timeout(remaining)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => D512Error::Timeout,
                mpsc::RecvTimeoutError::Disconnected => D512Error::RuntimeUnavailable,
            })?;
        if Instant::now() >= deadline {
            Err(D512Error::Timeout)
        } else {
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{client::ChewingIpcClient, d512::MAX_FRAME_BYTES};
    use interprocess::os::windows::named_pipe::{
        DuplexPipeStream, PipeListenerOptions, pipe_mode::Bytes,
    };
    use std::{
        io::{BufRead, BufReader, Write},
        sync::mpsc,
        thread,
        time::Duration,
    };

    fn request() -> SentenceRequest {
        SentenceRequest {
            context: "測試".into(),
            reading: vec!["ㄉㄞˋ".into(), "ㄅㄧㄠˇ".into()],
            candidates: vec!["代表".into(), "代錶".into()],
        }
    }

    #[test]
    fn worker_returns_ordered_scores() {
        let worker = Worker::spawn(|wire, deadline| {
            assert_eq!(wire.context, "測試");
            assert_eq!(wire.candidates, ["代表", "代錶"]);
            assert!(Instant::now() < deadline);
            Ok(vec![-10., -2.])
        })
        .unwrap();
        assert_eq!(
            worker
                .score(&request(), Instant::now() + Duration::from_secs(1))
                .unwrap(),
            [-10., -2.]
        );
    }
    #[test]
    fn worker_enforces_deadline_even_when_operation_stalls() {
        let worker = Worker::spawn(|_, _| {
            thread::sleep(Duration::from_millis(180));
            Ok(vec![-10., -2.])
        })
        .unwrap();
        let start = Instant::now();
        assert_eq!(
            worker.score(&request(), start + Duration::from_millis(25)),
            Err(D512Error::Timeout)
        );
        assert!(start.elapsed() < Duration::from_millis(140));
    }
    #[test]
    fn expired_request_does_not_start_work() {
        let called = Arc::new(AtomicBool::new(false));
        let seen = called.clone();
        let worker = Worker::spawn(move |_, _| {
            seen.store(true, Ordering::SeqCst);
            Ok(vec![-10., -2.])
        })
        .unwrap();
        assert_eq!(
            worker.score(&request(), Instant::now()),
            Err(D512Error::Timeout)
        );
        assert!(!called.load(Ordering::SeqCst));
    }
    #[test]
    fn timed_out_work_keeps_capacity_bounded_until_it_finishes() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = Worker::spawn(move |_, _| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(vec![-10., -2.])
        })
        .unwrap();
        assert_eq!(
            worker.score(&request(), Instant::now() + Duration::from_millis(30)),
            Err(D512Error::Timeout)
        );
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            worker.score(&request(), Instant::now() + Duration::from_secs(1)),
            Err(D512Error::Busy)
        );
        release_tx.send(()).unwrap();
    }
    #[test]
    fn worker_contains_operation_panics() {
        let worker = Worker::spawn(|_, _| panic!("synthetic operation failure")).unwrap();
        assert_eq!(
            worker.score(&request(), Instant::now() + Duration::from_secs(1)),
            Err(D512Error::Inference)
        );
    }

    fn with_pipe(
        body: impl FnOnce(DuplexPipeStream<Bytes>) + Send + 'static,
        check: impl FnOnce(ChewingIpcClient),
    ) {
        let name = format!(r"\\.\pipe\c2-d512-{}", uuid::Uuid::new_v4());
        let listener = PipeListenerOptions::new()
            .path(name.as_str())
            .create_duplex::<Bytes>()
            .unwrap();
        let server = thread::spawn(move || body(listener.accept().unwrap()));
        let pipe = DuplexPipeStream::connect_by_path(name.as_str()).unwrap();
        let client = ChewingIpcClient::new();
        client.pipe.replace(Some(pipe));
        check(client);
        server.join().unwrap();
    }
    fn read_call(pipe: &DuplexPipeStream<Bytes>) {
        let mut bytes = vec![];
        BufReader::new(pipe).read_until(0, &mut bytes).unwrap();
        let call: serde_json::Value = serde_json::from_slice(&bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(call["method"], "im.chewing.d512.Score");
        assert_eq!(
            call["parameters"]["candidates"],
            serde_json::json!(["代表", "代錶"])
        );
    }
    #[test]
    fn scoring_method_uses_real_pipe_and_preserves_score_order() {
        with_pipe(
            |mut pipe| {
                read_call(&pipe);
                pipe.write_all(b"{\"parameters\":{\"scores\":[-10.0,-2.0]}}\0")
                    .unwrap();
                pipe.flush().unwrap();
            },
            |client| {
                assert_eq!(
                    client
                        .score_sentence(&request(), Instant::now() + Duration::from_secs(1))
                        .unwrap(),
                    [-10., -2.]
                );
            },
        );
    }
    #[test]
    fn real_pipe_read_has_a_deadline() {
        with_pipe(
            |pipe| {
                read_call(&pipe);
                thread::sleep(Duration::from_millis(180));
            },
            |client| {
                let start = Instant::now();
                assert_eq!(
                    client.score_sentence(&request(), start + Duration::from_millis(25)),
                    Err(D512Error::Timeout)
                );
                assert!(start.elapsed() < Duration::from_millis(140));
            },
        );
    }
    #[test]
    fn oversized_and_unterminated_pipe_frames_are_rejected() {
        with_pipe(
            |mut pipe| {
                read_call(&pipe);
                let _ = pipe.write_all(&vec![b'x'; MAX_FRAME_BYTES + 1]);
                let _ = pipe.flush();
            },
            |client| {
                assert_eq!(
                    client.score_sentence(&request(), Instant::now() + Duration::from_secs(1)),
                    Err(D512Error::OutOfLimits)
                );
            },
        );
    }
    #[test]
    fn malformed_reply_and_disconnect_are_fallback_errors() {
        with_pipe(
            |mut pipe| {
                read_call(&pipe);
                pipe.write_all(b"not json\0").unwrap();
                pipe.flush().unwrap();
            },
            |client| {
                assert_eq!(
                    client.score_sentence(&request(), Instant::now() + Duration::from_secs(1)),
                    Err(D512Error::Inference)
                );
            },
        );
        with_pipe(
            |pipe| {
                read_call(&pipe);
            },
            |client| {
                assert_eq!(
                    client.score_sentence(&request(), Instant::now() + Duration::from_secs(1)),
                    Err(D512Error::Inference)
                );
            },
        );
    }
}
