// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    fs,
    io::{BufRead, BufReader},
    process::Command,
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chewing_tip_core::ipc::{named_pipe::create_pipe_listener, varlink::MethodCall};

static PIPE_TEST: Mutex<()> = Mutex::new(());

#[test]
fn temporary_tsfreg_delivers_content_free_stop() {
    let _guard = PIPE_TEST.lock().unwrap();
    // Exclusive ownership prevents this test from stopping an existing host.
    let listener = create_pipe_listener().expect("test must own the host pipe exclusively");
    let (frame_tx, frame_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let pipe = listener.accept().unwrap();
        let mut frame = Vec::new();
        let mut reader = BufReader::new(pipe);
        let result = reader.read_until(0, &mut frame);
        frame_tx.send((result, frame)).unwrap();
        let _ = release_rx.recv_timeout(Duration::from_secs(5));
    });
    let directory = std::env::temp_dir().join(format!(
        "c2-stop-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&directory).unwrap();
    let executable = directory.join("tsfreg.exe");
    fs::copy(env!("CARGO_BIN_EXE_tsfreg"), &executable).unwrap();
    let output = Command::new(&executable).arg("-s").output().unwrap();
    let received = frame_rx.recv_timeout(Duration::from_secs(5));
    let _ = release_tx.send(());
    fs::remove_dir_all(directory).unwrap();
    assert!(output.status.success(), "Stop must remain best-effort");
    let (read, mut frame) = received.expect("temporary tsfreg did not send Stop");
    read.unwrap();
    assert_eq!(frame.pop(), Some(0), "Stop frame was not delivered");
    let call: MethodCall = serde_json::from_slice(&frame).unwrap();
    assert_eq!(call.method, "im.chewing.ui.Stop");
    assert_eq!(call.parameters, serde_json::Value::Null);
    assert_eq!(call.oneway, Some(true));
    assert_eq!(call.more, None);
    assert_eq!(call.upgrade, None);
    server.join().unwrap();
}

#[test]
fn absent_host_keeps_best_effort_exit_status() {
    let _guard = PIPE_TEST.lock().unwrap();
    let listener = create_pipe_listener().expect("test requires no running host");
    drop(listener);
    let output = Command::new(env!("CARGO_BIN_EXE_tsfreg"))
        .arg("-s")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "no host is normal during installation"
    );
}
