use std::{
    cell::RefCell,
    io::{BufRead, BufReader, Write},
    rc::Rc,
    time::Duration,
};

use interprocess::{
    TryClone,
    os::windows::named_pipe::{DuplexPipeStream, pipe_mode::Bytes},
};
use scoped_error::{expect_error, impl_context_error};

use crate::ipc::{
    messages::{Ping, PingReply},
    named_pipe::{connect_and_attest, named_pipe_path},
    varlink::{MethodCall, MethodReply},
};

#[derive(Clone, Default)]
pub struct ChewingIpcClient {
    pub(super) pipe: Rc<RefCell<Option<DuplexPipeStream<Bytes>>>>,
}

impl ChewingIpcClient {
    pub fn new() -> ChewingIpcClient {
        Self::default()
    }
    pub fn connect(&self) -> Result<(), IpcOpError> {
        expect_error("Unable to connect to chewing_tip_host", || {
            let pipe_path = named_pipe_path()?;
            let pipe = connect_and_attest(&pipe_path, Duration::from_millis(100))?;
            self.pipe.replace(Some(pipe));
            Ok(())
        })
    }
    pub fn send(&self, method_call: MethodCall) -> Result<MethodReply, IpcOpError> {
        expect_error("Failed to call IPC method", || {
            let mut bytes = serde_json::to_vec(&method_call)?;
            bytes.push(0);
            self.pipe
                .try_borrow_mut()?
                .as_mut()
                .map(|pipe| pipe.write_all(&bytes))
                .ok_or("IPC Client was not connected")??;
            if matches!(method_call.oneway, Some(true)) {
                return Ok(MethodReply {
                    parameters: serde_json::Value::Null,
                    continues: None,
                    error: None,
                });
            }
            let mut buffer = vec![];
            let mut reader = BufReader::new(
                self.pipe
                    .try_borrow()?
                    .as_ref()
                    .ok_or("Broken Pipe")?
                    .try_clone()?,
            );
            reader.read_until(0, &mut buffer)?;
            if buffer.last().is_none_or(|b| *b != 0) {
                log::debug!("EOF - server exited");
                return Err("EOF - server exited".into());
            }
            buffer.pop();
            Ok(serde_json::from_slice::<MethodReply>(&buffer)?)
        })
    }
    pub(super) fn connect_for_scoring(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(), IpcOpError> {
        expect_error("Unable to connect to scoring host", || {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or("scoring deadline elapsed")?;
            let pipe = connect_and_attest(&named_pipe_path()?, remaining)?;
            if std::time::Instant::now() >= deadline {
                return Err("scoring deadline elapsed".into());
            }
            pipe.set_nonblocking(true)?;
            self.pipe.replace(Some(pipe));
            Ok(())
        })
    }

    pub fn score_sentence(
        &self,
        request: &chewing::d512::SentenceRequest,
        deadline: std::time::Instant,
    ) -> Result<Vec<f64>, chewing::d512::D512Error> {
        use super::d512::{D512Score, MAX_FRAME_BYTES, scores_from_reply};
        use chewing::d512::D512Error;
        use std::io::{ErrorKind, Read};
        let result = (|| {
            let mut owned = self.pipe.try_borrow_mut().map_err(|_| D512Error::Busy)?;
            let pipe = owned.as_mut().ok_or(D512Error::RuntimeUnavailable)?;
            pipe.set_nonblocking(true)
                .map_err(|_| D512Error::Inference)?;
            let message = D512Score::from_request(request, deadline)?;
            let mut outgoing =
                serde_json::to_vec(&message.into_call()?).map_err(|_| D512Error::Inference)?;
            outgoing.push(0);
            if outgoing.len() > MAX_FRAME_BYTES {
                return Err(D512Error::OutOfLimits);
            }
            let mut sent = 0;
            while sent < outgoing.len() {
                check_deadline(deadline)?;
                match pipe.write(&outgoing[sent..]) {
                    Ok(n) if n > 0 => sent += n,
                    Ok(_) => wait_for_pipe(deadline)?,
                    Err(error) if pipe_would_block(&error) => wait_for_pipe(deadline)?,
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => return Err(D512Error::Inference),
                }
            }
            // No flush: FlushFileBuffers has no deadline and waits for the peer.
            let mut incoming = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                check_deadline(deadline)?;
                // interprocess downgrades Windows ERROR_NO_DATA to Ok(0).
                // Peek first to distinguish a connected empty pipe from EOF.
                use std::os::windows::io::AsRawHandle;
                use windows::Win32::{Foundation::HANDLE, System::Pipes::PeekNamedPipe};
                let mut available = 0;
                unsafe {
                    PeekNamedPipe(
                        HANDLE(pipe.as_raw_handle()),
                        None,
                        0,
                        None,
                        Some(&mut available),
                        None,
                    )
                }
                .map_err(|_| D512Error::Inference)?;
                if available == 0 {
                    wait_for_pipe(deadline)?;
                    continue;
                }
                match pipe.read(&mut chunk) {
                    Ok(0) => return Err(D512Error::Inference),
                    Ok(n) => {
                        if incoming.len() + n > MAX_FRAME_BYTES {
                            return Err(D512Error::OutOfLimits);
                        }
                        if let Some(end) = chunk[..n].iter().position(|b| *b == 0) {
                            if end + 1 != n {
                                return Err(D512Error::Inference);
                            }
                            incoming.extend_from_slice(&chunk[..end]);
                            let reply = serde_json::from_slice(&incoming)
                                .map_err(|_| D512Error::Inference)?;
                            let scores = scores_from_reply(reply, request.candidates.len())?;
                            check_deadline(deadline)?;
                            return Ok(scores);
                        }
                        incoming.extend_from_slice(&chunk[..n]);
                    }
                    Err(error) if pipe_would_block(&error) => wait_for_pipe(deadline)?,
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => return Err(D512Error::Inference),
                }
            }
        })();
        // Each scoring connection is disposable. In particular, timeout/error
        // must never trigger synchronous or background flushing of stale text.
        if let Ok(mut pipe) = self.pipe.try_borrow_mut() {
            if let Some(pipe) = pipe.take() {
                pipe.evade_limbo();
            }
        }
        result
    }

    pub fn ping(&self) -> Result<String, IpcOpError> {
        expect_error("Cannot ping server", || {
            let reply = self.send(MethodCall {
                method: Ping::METHOD.to_string(),
                parameters: serde_json::to_value(Ping::new())?,
                oneway: Some(false),
                more: Some(false),
                upgrade: Some(false),
            })?;
            let params: PingReply = serde_json::from_value(reply.parameters)?;
            Ok(params.uuid)
        })
    }
}

fn check_deadline(deadline: std::time::Instant) -> Result<(), chewing::d512::D512Error> {
    if std::time::Instant::now() >= deadline {
        Err(chewing::d512::D512Error::Timeout)
    } else {
        Ok(())
    }
}
fn pipe_would_block(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == Some(windows::Win32::Foundation::ERROR_NO_DATA.0 as i32)
}
fn wait_for_pipe(deadline: std::time::Instant) -> Result<(), chewing::d512::D512Error> {
    check_deadline(deadline)?;
    std::thread::sleep(
        deadline
            .saturating_duration_since(std::time::Instant::now())
            .min(Duration::from_millis(1)),
    );
    Ok(())
}

impl Drop for ChewingIpcClient {
    fn drop(&mut self) {
        if let Some(pipe) = self.pipe.borrow_mut().as_mut() {
            let _ = pipe.flush();
        }
    }
}

impl_context_error!(pub IpcOpError);
