#![windows_subsystem = "windows"]

use std::{path::PathBuf, process::Command, sync::Arc, thread};

use chewing_tip_core::ipc::named_pipe::{create_pipe_listener, named_pipe_path};
use log::{error, info};
use logforth::record::{Level, LevelFilter};
use scoped_error::{ErrorExt, expect_error};
use windows::Win32::System::{
    Console::{ATTACH_PARENT_PROCESS, AttachConsole},
    Recovery::{REGISTER_APPLICATION_RESTART_FLAGS, RegisterApplicationRestart},
};

use crate::{ipc::run_ipc_listener, ui::event_loop::MainLoop};

mod ipc;
mod scoring;
mod text_service;
mod ui;
mod ui_elements;

fn main() -> Result<(), scoped_error::Error> {
    // Windows permits environment mutation; do this before ORT or our threads,
    // including launches through the MSI Run entry rather than the TIP launcher.
    unsafe {
        std::env::set_var("ORT_DISABLE_TELEMETRY", "1");
    }
    expect_error("Running chewing_tip_host failed", || {
        if std::env::args().any(|arg| arg == "-d") {
            // daemonize
            let self_exe = std::env::current_exe()?;
            let _ = Command::new(self_exe).spawn()?;
            return Ok(());
        }
        if std::env::args().any(|arg| arg == "-a") {
            unsafe {
                let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
        logforth::starter_log::stdout()
            .filter(LevelFilter::MoreSevereEqual(Level::Debug))
            .apply();

        info!("Register application for automatic restart");
        unsafe {
            if let Err(error) =
                RegisterApplicationRestart(None, REGISTER_APPLICATION_RESTART_FLAGS(0))
            {
                error!(
                    "Failed to register application for restart: {}",
                    error.report()
                );
            }
        }

        info!("Create NamedPipe listener");
        let listener = create_pipe_listener().inspect_err(|_| {
            if let Ok(path) = named_pipe_path() {
                let pipe_path = PathBuf::from(path);
                if pipe_path.exists() {
                    info!("Another chewing_tip_host is already running.");
                }
            }
        })?;

        // Acquire the exclusive listener before loading the single model copy.
        let executable = std::env::current_exe()?;
        let directory = executable
            .parent()
            .ok_or("missing host installation directory")?;
        let scorer = Arc::new(scoring::SharedScorer::load_from_directory(directory));

        info!("Setup main loop");
        let mut main_loop = MainLoop::new();
        let mh = main_loop.get_handle();

        info!("Spawn IPC thread");
        thread::spawn(move || run_ipc_listener(listener, mh, scorer));

        info!("Starting main loop");
        main_loop.run();

        Ok(())
    })
}
