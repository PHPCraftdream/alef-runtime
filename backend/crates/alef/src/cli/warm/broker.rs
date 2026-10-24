// SPDX-License-Identifier: MIT OR Apache-2.0
//! `alef broker` (docs/stages/m11-warm-start.md, «Архитектура реализации»):
//! the constant per-user process. The main thread owns the tray (or waits
//! for the stop signal without one); a worker thread drives the [`Broker`]
//! pump loop and the shutdown, and the IPC server runs on its own thread.
//! The tray is rebuilt when the stored interface language changes (polled
//! by the tray dispatcher); an admin shutdown or a tray quit ends the run.
//! One broker per session: a second start reports "already running" and
//! exits successfully.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alef_broker::broker::{Broker, ProcessSpawner};
use alef_broker::ipc::server::{ExpectedIdentity, Handler, Server};
use alef_broker::ipc::token::load_or_create_token;
use alef_broker::ipc::ReplyTo;
use alef_broker::pool::{Jitter, PoolConfig, SystemClock};
use alef_broker::protocol::message::{ClientMsg, OsBytes};
use alef_broker::protocol::warm::{decode_warm_admin_msg, WarmAdminMsg};
use alef_core::security::permissions::language::{Language, SettingsStore};
use alef_runtime::{BrokerTray, BrokerTrayCommand, BrokerTrayConfig, BrokerTrayHandle};

use super::control::spawn_detached;
use super::endpoint::endpoint;
use crate::cli::failure::Failure;

/// The pump loop sleeps on the pool's wake; the channel is the real wakeup
/// for shutdown, this is only a backstop.
const NO_WAKE: Duration = Duration::from_secs(3600);

type AppBroker = Broker<ProcessSpawner, SystemClock, NoJitter>;

/// Deterministic jitter: the wiring provides one; none is fine.
struct NoJitter;
impl Jitter for NoJitter {
    fn fraction(&mut self) -> f64 {
        0.0
    }
}

/// Admin shutdown arrives through `on_raw_frame`; the broker's own handler
/// answers it, this wrapper only tells the pump loop to finish the process.
struct BrokerHandler {
    broker: Arc<AppBroker>,
    stop: Sender<()>,
    /// Set with `stop` on an admin shutdown; tells the tray loop apart from
    /// a language-change rebuild.
    quit: Arc<AtomicBool>,
}

impl Handler for BrokerHandler {
    fn on_message(&self, conn: &mut dyn ReplyTo, msg: ClientMsg) {
        self.broker.on_message(conn, msg)
    }

    fn on_raw_frame(&self, conn: &mut dyn ReplyTo, conn_id: u64, body: &[u8]) -> io::Result<()> {
        if matches!(
            decode_warm_admin_msg(body).as_ref(),
            Ok(WarmAdminMsg::Shutdown(_))
        ) {
            let _ = self.stop.send(());
            self.quit.store(true, Ordering::SeqCst);
        }
        self.broker.on_raw_frame(conn, conn_id, body)
    }
}

/// The tray strings of one language.
struct TrayLabels {
    tooltip: &'static str,
    apps: &'static str,
    settings: &'static str,
    quit: &'static str,
}

fn tray_labels(language: Language) -> TrayLabels {
    TrayLabels {
        tooltip: language.tray_tooltip(),
        apps: language.tray_apps(),
        settings: language.tray_settings(),
        quit: language.tray_quit(),
    }
}

/// Runs the broker; blocks the caller until the tray quits or an admin
/// shutdown arrives. The socket file is removed when the server is dropped.
pub(crate) fn run_broker(no_tray: bool) -> Result<u8, Failure> {
    let exe = std::env::current_exe()
        .map_err(|e| Failure::Runtime(format!("current executable: {e}")))?;
    let Some(ep) = endpoint() else {
        return Err(Failure::Runtime(
            "WARM_UNAVAILABLE: the broker entry point does not fit the system limits; \
             warm start is unavailable on this system"
                .to_owned(),
        ));
    };
    // One broker per session: an answer means one is already there.
    if super::control::broker_is_running() {
        println!("already running");
        return Ok(0);
    }
    #[cfg(unix)]
    let socket_path = ep.unix_socket.clone();
    let (token, _) =
        load_or_create_token(&ep).map_err(|e| Failure::Runtime(format!("broker token: {e}")))?;
    let server = Arc::new(
        Server::bind(&ep).map_err(|e| Failure::Runtime(format!("WARM_BIND_FAILED: {e}")))?,
    );
    let version = env!("CARGO_PKG_VERSION").to_owned();
    let identity = ExpectedIdentity {
        build: version.clone(),
        alef_home: std::env::var_os("ALEF_HOME").map(|v| OsBytes::from_os_string(&v)),
        alef_app_home: std::env::var_os("ALEF_APP_HOME").map(|v| OsBytes::from_os_string(&v)),
    };
    let broker: Arc<AppBroker> = Arc::new(Broker::new(
        // The windowless slot is off in this version: the measured gain was
        // ≈ 5–16 ms (docs, «Замер холодного запуска»).
        PoolConfig {
            level: alef_broker::pool::Level::Full,
            windowless: false,
            ..PoolConfig::default()
        },
        ProcessSpawner::new(exe.clone(), Vec::new(), ep),
        SystemClock::new(),
        NoJitter,
        version,
    ));
    // `ALEF_WARM_LOG=<file>`: one line per spawn and per exit of a spare (with what it wrote), and
    // exits are polled twice a second, so a spare that died at once shows up in the file.
    let log_path = std::env::var_os("ALEF_WARM_LOG").map(std::path::PathBuf::from);
    if let Some(path) = log_path.clone() {
        broker.set_debug(Arc::new(move |line: &str| {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = writeln!(file, "{line}");
            }
        }));
    }
    let poll_cap = log_path.map(|_| Duration::from_millis(500));
    let (stop_tx, stop_rx) = channel::<()>();
    let (done_tx, done_rx) = channel::<()>();
    let quit = Arc::new(AtomicBool::new(false));

    // The IPC server: one endpoint serves clients, spares and the admin.
    let server_thread = {
        let server = Arc::clone(&server);
        let token = Arc::new(token);
        let handler = Arc::new(BrokerHandler {
            broker: Arc::clone(&broker),
            stop: stop_tx.clone(),
            quit: Arc::clone(&quit),
        });
        std::thread::Builder::new()
            .name("broker-ipc".into())
            .spawn(move || server.run(token, Some(identity), handler))
            .map_err(|e| Failure::Runtime(format!("broker thread: {e}")))?
    };

    // The tray is created on the main thread and rebuilt when the language
    // changes; the command channel and the shared handle cell outlive any
    // single tray instance.
    let tray_available = !no_tray;
    let (commands, tray_rx) = crossbeam_channel::bounded::<BrokerTrayCommand>(16);
    let tray_handle: Arc<Mutex<Option<BrokerTrayHandle>>> = Arc::new(Mutex::new(None));
    let mut language = if tray_available {
        SettingsStore::new(alef_launch::consent::runtime_home()).language()
    } else {
        Language::English
    };
    if tray_available {
        // The tray commands: Apps, Settings and Quit.
        let dispatcher_exe = exe.clone();
        let dispatcher_stop = stop_tx.clone();
        let dispatcher_quit = Arc::clone(&quit);
        let dispatcher_tray = Arc::clone(&tray_handle);
        let _ = std::thread::Builder::new()
            .name("broker-tray".into())
            .spawn(move || {
                // The language the tray on the main thread was built with.
                let mut built = language;
                loop {
                    match tray_rx.recv_timeout(Duration::from_secs(2)) {
                        Ok(command) => match command {
                            BrokerTrayCommand::ShowApps => {
                                // The broker's own trusted list: argv array,
                                // no shell, detached from this process.
                                let _ = spawn_detached(&dispatcher_exe, &["apps", "--ui"]);
                            }
                            BrokerTrayCommand::ShowSettings => {
                                let _ = spawn_detached(&dispatcher_exe, &["settings", "--ui"]);
                            }
                            BrokerTrayCommand::Quit => {
                                dispatcher_quit.store(true, Ordering::SeqCst);
                                let _ = dispatcher_stop.send(());
                                break;
                            }
                        },
                        // Poll the stored language; a change asks the main
                        // thread to rebuild the tray. exit() is called only
                        // when the language actually differs, so run()
                        // returning Ok always means quit or a rebuild.
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                            let current =
                                SettingsStore::new(alef_launch::consent::runtime_home()).language();
                            let mut guard =
                                dispatcher_tray.lock().unwrap_or_else(|e| e.into_inner());
                            if current != built {
                                if let Some(handle) = guard.as_ref() {
                                    handle.exit();
                                }
                                built = current;
                            }
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
    }

    // The pump loop: pool wakeups only, no timers of its own. After the
    // stop: the pool shutdown (running applications are untouched), then
    // the server, and the tray goes away with it.
    let worker = {
        let broker = Arc::clone(&broker);
        let server = Arc::clone(&server);
        let worker_tray = Arc::clone(&tray_handle);
        std::thread::Builder::new()
            .name("broker-pump".into())
            .spawn(move || {
                loop {
                    let wake = broker.pump();
                    let wait = wake.unwrap_or(NO_WAKE).min(poll_cap.unwrap_or(NO_WAKE));
                    match stop_rx.recv_timeout(wait) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                }
                broker.shutdown();
                server.stop();
                if let Some(handle) = worker_tray
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                {
                    handle.exit();
                }
                let _ = done_tx.send(());
            })
            .map_err(|e| Failure::Runtime(format!("broker thread: {e}")))?
    };

    // The main thread: the tray blocks here and is rebuilt on a language
    // change (the icon disappears and reappears: the platform backends are
    // not taught to relabel the tray in place), or the process waits quietly.
    if tray_available {
        loop {
            let labels = tray_labels(language);
            let config = BrokerTrayConfig {
                icon_png: crate::cli::DEFAULT_ICON.to_vec(),
                tooltip: labels.tooltip.to_owned(),
                show_apps_label: labels.apps.to_owned(),
                show_settings_label: labels.settings.to_owned(),
                quit_label: labels.quit.to_owned(),
                commands: commands.clone(),
            };
            match BrokerTray::new(config) {
                Ok(tray) => {
                    // Shared with the pump worker and the dispatcher; exit
                    // is idempotent.
                    *tray_handle.lock().unwrap_or_else(|e| e.into_inner()) = Some(tray.handle());
                    match tray.run() {
                        // An Ok return means a quit or a language change:
                        // the dispatcher exits the tray for nothing else.
                        Ok(()) => {}
                        Err(error) => {
                            eprintln!(
                                "alef: the tray is unavailable ({error}); waiting for `alef warm stop`"
                            );
                            let _ = done_rx.recv();
                            break;
                        }
                    }
                    if quit.load(Ordering::SeqCst) {
                        break;
                    }
                    // A language change: reload for the next build.
                    language = SettingsStore::new(alef_launch::consent::runtime_home()).language();
                }
                // Creation failure (an invalid icon, say) is not a language
                // issue: give up instead of spinning.
                Err(error) => {
                    eprintln!(
                        "alef: the tray is unavailable ({error}); waiting for `alef warm stop`"
                    );
                    let _ = done_rx.recv();
                    break;
                }
            }
        }
    } else {
        if no_tray {
            eprintln!("alef: the tray is disabled with --no-tray; waiting for `alef warm stop`");
        }
        let _ = done_rx.recv();
    }
    let _ = stop_tx.send(());
    let _ = worker.join();
    let _ = server_thread.join();
    drop(server);
    // A connection thread that outlives the stop keeps the listener (and its unlink on drop)
    // alive past the end of the process: the socket file goes here, by name.
    #[cfg(unix)]
    let _ = std::fs::remove_file(&socket_path);
    Ok(0)
}
