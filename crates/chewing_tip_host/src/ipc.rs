use std::{
    io::{BufRead, BufReader, Read, Write},
    ops::ControlFlow,
    sync::Arc,
    time::Instant,
};

use chewing_tip_core::ipc::{
    messages::{CheckUpdate, HideCandidateList, ShowCandidateList, ShowNotification, Stop},
    varlink::{MethodCall, MethodReply},
};
use chewing_tip_core::{
    ipc::messages::{OnTestKeyDown, OnTestKeyDownReply, Ping, PingReply},
    update::check_for_update,
};
use interprocess::os::windows::named_pipe::{PipeListener, PipeStream, pipe_mode::Bytes};
use log::{error, warn};
use scoped_error::{ErrorExt, expect_error, impl_context_error};

use crate::{
    scoring::SharedScorer, text_service::chewing::TipSession, ui::event_loop::MainLoopHandle,
};
use chewing::d512::D512Error;
use chewing_tip_core::ipc::d512::{D512Score, MAX_FRAME_BYTES, scoring_reply};

pub(crate) fn run_ipc_listener(
    listener: PipeListener<Bytes, Bytes>,
    mh: MainLoopHandle,
    scorer: Arc<SharedScorer>,
) -> Result<(), HandleIpcError> {
    expect_error("IPC listener failed", || {
        for pipe in listener.incoming() {
            let mh = mh.clone();
            let scorer = scorer.clone();
            let pipe = pipe?;
            std::thread::spawn(move || {
                if let Err(error) = ipc_loop(pipe, mh, scorer) {
                    error!("{}", error.report());
                }
            });
        }
        Ok(())
    })
}

fn ipc_loop(
    pipe: PipeStream<Bytes, Bytes>,
    mh: MainLoopHandle,
    scorer: Arc<SharedScorer>,
) -> Result<(), HandleIpcError> {
    expect_error("Failed to initialize IPC loop", || {
        let (receiver_inner, mut sender) = pipe.split();
        let mut receiver = BufReader::new(receiver_inner);
        // D512 scoring never constructs an editor or accesses a user directory.
        let mut tip_session: Option<TipSession> = None;
        loop {
            match ipc_loop_once(&mut receiver, &mut sender, &mh, &mut tip_session, &scorer) {
                Ok(ControlFlow::Continue(_)) => continue,
                Ok(ControlFlow::Break(_)) => break,
                Err(error) => {
                    error!("{}", error.report());
                    break;
                }
            }
        }
        Ok(())
    })
}

fn ipc_loop_once(
    mut receiver: impl BufRead,
    mut sender: impl Write,
    mh: &MainLoopHandle,
    tip_session: &mut Option<TipSession>,
    scorer: &SharedScorer,
) -> Result<ControlFlow<()>, HandleIpcError> {
    expect_error("Failed to handle one IPC message", || {
        let Some(call) = read_call_frame(&mut receiver)? else {
            return Ok(ControlFlow::Break(()));
        };
        let received = Instant::now();
        let oneway = call.oneway.is_some_and(|v| v);

        match call.method.as_str() {
            D512Score::METHOD => {
                let reply = match serde_json::from_value::<D512Score>(call.parameters) {
                    Ok(message)
                        if !oneway && call.more != Some(true) && call.upgrade != Some(true) =>
                    {
                        scorer.score(message, received)
                    }
                    _ => scoring_reply(Err(D512Error::InvalidConfig), 0),
                };
                if !oneway {
                    sender.write_all(&reply.to_bytes()?)?;
                }
            }
            Ping::METHOD => {
                if !oneway {
                    let ping: Ping = serde_json::from_value(call.parameters)?;
                    let reply = MethodReply {
                        parameters: serde_json::to_value(PingReply::from(ping))?,
                        continues: None,
                        error: None,
                    };
                    sender.write_all(&reply.to_bytes()?)?;
                }
            }
            ShowNotification::METHOD
            | ShowCandidateList::METHOD
            | HideCandidateList::METHOD
            | Stop::METHOD => {
                mh.send(call)?;
                if !oneway {
                    sender.write_all(c"{}".to_bytes_with_nul())?;
                }
            }
            CheckUpdate::METHOD => {
                std::thread::spawn(|| check_for_update());
                if !oneway {
                    sender.write_all(c"{}".to_bytes_with_nul())?;
                }
            }
            OnTestKeyDown::METHOD => {
                let params: OnTestKeyDown = serde_json::from_value(call.parameters)?;
                if tip_session.is_none() {
                    *tip_session = Some(TipSession::new()?);
                }
                let handled = tip_session
                    .as_mut()
                    .ok_or("missing legacy session")?
                    .on_test_keydown(
                        params.is_context_mutable,
                        params.is_composing,
                        params.shift_key_state,
                        params.event.try_into()?,
                    )?;
                let reply = MethodReply {
                    parameters: serde_json::to_value(OnTestKeyDownReply { handled })?,
                    continues: None,
                    error: None,
                };
                if !oneway {
                    sender.write_all(&reply.to_bytes()?)?;
                }
            }
            _ => {
                warn!("Unknown IPC method");
            }
        }
        Ok(ControlFlow::Continue(()))
    })
}

fn read_call_frame(mut receiver: impl BufRead) -> Result<Option<MethodCall>, HandleIpcError> {
    expect_error("Failed to read IPC frame", || {
        let mut buffer = vec![];
        (&mut receiver)
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(0, &mut buffer)?;
        if buffer.is_empty() {
            return Ok(None);
        }
        if buffer.len() > MAX_FRAME_BYTES {
            return Err("IPC frame exceeds size limit".into());
        }
        if buffer.pop() != Some(0) {
            return Err("truncated IPC frame".into());
        }
        let call = serde_json::from_slice(&buffer).map_err(|_| "invalid IPC message")?;
        Ok(Some(call))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn d512_frame_accepts_a_complete_call() {
        let wire = b"{\"method\":\"im.chewing.d512.Score\",\"parameters\":{}}\0";
        assert_eq!(
            read_call_frame(Cursor::new(wire)).unwrap().unwrap().method,
            "im.chewing.d512.Score"
        );
    }
    #[test]
    fn d512_frame_rejects_oversized_json_before_dispatch() {
        let mut wire=serde_json::to_vec(&serde_json::json!({"method":"im.chewing.d512.Score","parameters":{"context":"x".repeat(65536)}})).unwrap();
        wire.push(0);
        assert!(read_call_frame(Cursor::new(wire)).is_err());
    }
    #[test]
    fn d512_frame_rejects_truncated_json() {
        assert!(read_call_frame(Cursor::new(b"{\"method\":\"im.chewing.d512.Score\"}")).is_err());
    }
}

impl_context_error!(HandleIpcError);
