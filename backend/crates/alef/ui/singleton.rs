// SPDX-License-Identifier: MIT OR Apache-2.0
//! One runtime window (Applications / Settings) per kind, user and store
//! identity. A second `alef apps --ui` / `alef settings --ui` finds the
//! channel of the running window over an alef-broker IPC endpoint, sends a
//! raise frame and exits 0 instead of opening a new window. The endpoint is
//! bound before the window opens (winning launch races) and is released when
//! the window closes (the [`Guard`] is dropped).
//!
//! The raise is delivered to the window through a pending flag plus a focus
//! sink: the window arms the sink when its host is installed
//! ([`on_raise`]); a raise arriving earlier is remembered as pending and
//! fires as soon as the sink is armed.
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use alef_broker::ipc::{
    token::{load_existing_token, load_or_create_token},
    Client, Endpoint, Handler, ReplyTo, Server,
};
use alef_broker::protocol::message::{ClientMsg, Hello, PROTOCOL_VERSION};

/// Practical Unix socket path limit (`sun_path`, with a safety margin).
const MAX_SOCKET_PATH: usize = 100;
/// Deadline for the connect + handshake when raising the existing window.
const RAISE_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
/// Deadline for the raise reply of the existing window.
const RAISE_REPLY_TIMEOUT: Duration = Duration::from_millis(2000);
/// Delay before re-probing a channel that is neither free nor raising.
const RETRY_DELAY: Duration = Duration::from_millis(100);

/// Which runtime window this process wants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Apps,
    Settings,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::Apps => "apps",
            Kind::Settings => "settings",
        }
    }
}

/// Owns the raise channel while the window is open; dropping it releases the
/// endpoint. A guard with no slot means the channel was unavailable and the
/// window opens anyway (never lose the launch).
pub struct Guard {
    slot: Option<ServerSlot>,
}

struct ServerSlot {
    server: Arc<Server>,
    accept: JoinHandle<()>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            slot.server.stop();
            let _ = slot.accept.join();
            slot.server.join();
        }
    }
}

/// What the second launch observes at the channel of the first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Probe {
    /// The existing window answered the raise frame.
    Raised,
    /// No channel exists (no token, nothing listening).
    Absent,
    /// The channel exists but cannot be owned right now (launch race).
    Busy,
}

/// What the second launch does about it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    /// The existing window was raised; exit 0.
    Exit,
    /// Take the channel and open the window.
    Take,
    /// Probe and bind again after a delay.
    RetryThenTake,
}

fn decide(probe: Probe) -> Action {
    match probe {
        Probe::Raised => Action::Exit,
        Probe::Absent => Action::Take,
        Probe::Busy => Action::RetryThenTake,
    }
}

/// `true` = one of the bind attempts won the channel; `false` = give up and
/// open the window detached. Pure helper, exercised by the table test.
#[cfg(test)]
fn race_plan(binds: &[bool]) -> bool {
    binds.iter().any(|won| *won)
}

/// `None` = the existing window was raised; the caller must exit 0 without
/// opening anything.
pub fn enter(kind: Kind) -> Option<Guard> {
    let Some(ep) = ui_endpoint(kind) else {
        // The endpoint name does not fit; open the window anyway.
        return Some(Guard { slot: None });
    };
    for _ in 0..3 {
        match decide(probe(&ep)) {
            Action::Exit => return None,
            Action::Take | Action::RetryThenTake => {
                if let Ok(guard) = own_channel(&ep) {
                    return Some(guard);
                }
            }
        }
        std::thread::sleep(RETRY_DELAY);
    }
    Some(Guard { slot: None })
}

/// Classify the channel without taking it over.
fn probe(ep: &Endpoint) -> Probe {
    if raise_existing(ep) {
        return Probe::Raised;
    }
    // A present endpoint that refuses to raise is busy (a launch race or a
    // foreign listener); an absent one (no token) is free.
    if endpoint_absent(ep) {
        Probe::Absent
    } else {
        Probe::Busy
    }
}

/// Read-only "no channel here" check; never creates the token file.
fn endpoint_absent(ep: &Endpoint) -> bool {
    load_existing_token(ep).is_err()
}

/// Ask the running window to raise itself; `true` when it answered.
fn raise_existing(ep: &Endpoint) -> bool {
    let Ok(token) = load_existing_token(ep) else {
        return false;
    };
    let hello = Hello {
        protocol_version: PROTOCOL_VERSION,
        build: String::new(),
        alef_home: None,
        alef_app_home: None,
    };
    let mut client = match Client::connect_with_deadline(ep, &token, hello, RAISE_CONNECT_TIMEOUT) {
        Ok(client) => client,
        Err(_) => return false,
    };
    if client.send_raw(&raise_frame()).is_err() {
        return false;
    }
    let _ = client.set_read_timeout(Some(RAISE_REPLY_TIMEOUT));
    matches!(client.recv_raw(), Ok(reply) if decode_raised(&reply).is_some())
}

/// Bind the endpoint and serve raise frames until the window closes.
fn own_channel(ep: &Endpoint) -> io::Result<Guard> {
    let (token, _) = load_or_create_token(ep)?;
    let server = Arc::new(Server::bind(ep)?);
    let handler: Arc<dyn Handler> = Arc::new(raise_handler(Arc::new(|| fire_raise())));
    let accept = std::thread::Builder::new()
        .name("ui-singleton".to_owned())
        .spawn({
            let server = Arc::clone(&server);
            move || server.run(Arc::new(token), None, handler)
        })?;
    Ok(Guard {
        slot: Some(ServerSlot { server, accept }),
    })
}

/// The endpoint of the raise channel for `kind`, or `None` when the Unix
/// socket path does not fit the limit.
fn ui_endpoint(kind: Kind) -> Option<Endpoint> {
    let ep = endpoint_with(kind, &base_dir(), &ui_identity(kind));
    #[cfg(unix)]
    if ep.unix_socket.as_os_str().as_encoded_bytes().len() > MAX_SOCKET_PATH {
        return None;
    }
    Some(ep)
}

/// Pure endpoint naming: the base directory and identity are arguments.
fn endpoint_with(kind: Kind, base: &Path, identity: &str) -> Endpoint {
    Endpoint::for_session(base, &format!("ui-{}:{}", kind.tag(), identity))
}

fn ui_identity(kind: Kind) -> String {
    identity_for_tag(
        kind.tag(),
        std::env::var_os("ALEF_HOME").as_deref(),
        std::env::var_os("ALEF_APP_HOME").as_deref(),
    )
}

/// The store identity of this process: the exact values (or absence) of
/// `ALEF_HOME`/`ALEF_APP_HOME`, hex-encoded so the identity is exact on every
/// OS and never contains the raw values (mirrors `cli::warm::endpoint`).
fn identity_for_tag(tag: &str, home: Option<&OsStr>, app_home: Option<&OsStr>) -> String {
    format!(
        "alef/{} ui-{tag} ALEF_HOME={} ALEF_APP_HOME={}",
        env!("CARGO_PKG_VERSION"),
        identity_part(home),
        identity_part(app_home)
    )
}

fn identity_part(value: Option<&OsStr>) -> String {
    match value {
        None => "absent".to_owned(),
        Some(value) => hex(&os_bytes(value)),
    }
}

#[cfg(unix)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}

#[cfg(windows)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    value
        .encode_wide()
        .flat_map(|unit| unit.to_le_bytes())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The short per-user base directory of the raise channels (mirrors
/// `cli::warm::endpoint::session_base`).
fn base_dir() -> PathBuf {
    #[cfg(unix)]
    {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            let path = PathBuf::from(dir);
            if path.is_absolute() {
                return path.join("alef");
            }
        }
        // Invariant: getuid has no preconditions and cannot fail.
        PathBuf::from(format!("/tmp/alef-{}", unsafe { libc::getuid() }))
    }
    #[cfg(windows)]
    {
        dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("alef")
    }
}

fn raise_frame() -> Vec<u8> {
    br#"{"op":"raise","v":1}"#.to_vec()
}

fn raised_frame() -> &'static [u8] {
    b"{\"ok\":true,\"v\":1}"
}

fn decode_raise(body: &[u8]) -> Option<()> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    (value.get("op")?.as_str()? == "raise" && value.get("v")?.as_i64()? == 1).then_some(())
}

fn decode_raised(body: &[u8]) -> Option<()> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    (value.get("ok")?.as_bool()? && value.get("v")?.as_i64()? == 1).then_some(())
}

struct RaiseHandler {
    fire: Arc<dyn Fn() + Send + Sync>,
    #[cfg(feature = "e2e")]
    e2e: bool,
}

impl Handler for RaiseHandler {
    // Only raw raise frames are spoken here; a launcher message is not expected and is dropped.
    fn on_message(&self, _conn: &mut dyn ReplyTo, _msg: ClientMsg) {}

    fn on_raw_frame(&self, conn: &mut dyn ReplyTo, _conn_id: u64, body: &[u8]) -> io::Result<()> {
        if decode_raise(body).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a raise frame",
            ));
        }
        #[cfg(feature = "e2e")]
        if self.e2e {
            println!("ALEF_E2E raise-received");
        }
        (self.fire)();
        ReplyTo::send_raw(conn, raised_frame())
    }
}

fn raise_handler(fire: Arc<dyn Fn() + Send + Sync>) -> RaiseHandler {
    RaiseHandler {
        fire,
        #[cfg(feature = "e2e")]
        e2e: std::env::var("ALEF_E2E").is_ok_and(|value| value == "1"),
    }
}

/// The focus sink armed by the window host, if any.
static FOCUS: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);
/// A raise arrived before any sink was armed.
static PENDING: AtomicBool = AtomicBool::new(false);

/// Arms the focus sink of the running window; fires it at once when a raise
/// already arrived.
pub fn on_raise(sink: Arc<dyn Fn() + Send + Sync>) {
    let mut focus = FOCUS.lock().unwrap();
    if PENDING.swap(false, Ordering::SeqCst) {
        sink();
    }
    *focus = Some(sink);
}

/// Called by the IPC handler when a raise frame arrives.
fn fire_raise() {
    let sink = FOCUS.lock().unwrap().clone();
    match sink {
        Some(sink) => sink(),
        None => PENDING.store(true, Ordering::SeqCst),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raise_frames_round_trip_and_garbage_is_refused() {
        assert!(decode_raise(&raise_frame()).is_some());
        assert!(decode_raised(raised_frame()).is_some());
        assert!(decode_raise(b"not json").is_none());
        assert!(decode_raise(br#"{"op":"quit","v":1}"#).is_none());
        assert!(decode_raise(br#"{"op":"raise","v":2}"#).is_none());
        assert!(decode_raised(b"garbage").is_none());
        assert!(decode_raised(br#"{"ok":false,"v":1}"#).is_none());
    }

    #[test]
    fn endpoint_names_differ_by_kind_and_identity_and_are_safe() {
        let base = Path::new("/tmp/base");
        let apps = endpoint_with(Kind::Apps, base, "identity-1");
        let settings = endpoint_with(Kind::Settings, base, "identity-1");
        let other = endpoint_with(Kind::Apps, base, "identity-2");
        assert_ne!(apps.name, settings.name);
        assert_ne!(apps.name, other.name);
        assert!(alef_broker::ipc::endpoint::is_safe_name(&apps.name));
        assert!(apps.unix_socket.starts_with(base));
    }

    #[test]
    fn decide_routes_every_probe() {
        assert_eq!(decide(Probe::Raised), Action::Exit);
        assert_eq!(decide(Probe::Absent), Action::Take);
        assert_eq!(decide(Probe::Busy), Action::RetryThenTake);
    }

    #[test]
    fn race_plan_needs_one_won_bind() {
        assert!(!race_plan(&[false, false, false]));
        assert!(!race_plan(&[]));
        assert!(race_plan(&[false, true]));
        assert!(race_plan(&[true, true]));
    }

    #[test]
    fn identity_separates_kinds_absence_and_values() {
        let absent = identity_for_tag("apps", None, None);
        let set = identity_for_tag("apps", Some(OsStr::new("/home/u/.alef")), None);
        assert_ne!(absent, set);
        assert!(absent.contains("ALEF_HOME=absent"));
        assert!(!set.contains("/home/u/.alef"));
        #[cfg(unix)]
        assert!(set.contains("ALEF_HOME=2f686f6d652f752f2e616c6566"));
        assert_ne!(absent, identity_for_tag("settings", None, None));
    }

    #[test]
    fn a_real_channel_raises_and_replies() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = endpoint_with(Kind::Apps, tmp.path(), "test-identity");
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let handler = Arc::new(raise_handler(Arc::new(move || {
            let _ = tx.send(());
        })));
        let (token, _) = load_or_create_token(&ep).unwrap();
        let server = Arc::new(Server::bind(&ep).unwrap());
        let accept = {
            let server = Arc::clone(&server);
            std::thread::spawn(move || server.run(Arc::new(token), None, handler))
        };
        let token = load_existing_token(&ep).unwrap();
        let hello = Hello {
            protocol_version: PROTOCOL_VERSION,
            build: String::new(),
            alef_home: None,
            alef_app_home: None,
        };
        let mut client =
            Client::connect_with_deadline(&ep, &token, hello, Duration::from_millis(500)).unwrap();
        client.send_raw(&raise_frame()).unwrap();
        let _ = client.set_read_timeout(Some(Duration::from_secs(2)));
        assert_eq!(client.recv_raw().unwrap(), raised_frame());
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(()) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "no raise within deadline"
                    );
                }
                Err(e) => panic!("raise channel closed: {e}"),
            }
        }
        server.stop();
        let _ = accept.join();
        server.join();
    }
}
