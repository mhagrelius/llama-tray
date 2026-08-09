//! The seam. Everything outside this process is reached from here.
//!
//! # Why the probes are synchronous
//!
//! They are only ever run while the menu is on screen, and this process has no
//! UI of its own to stutter. Measured on the target machine: the systemd
//! property read is ~1 ms, the socket round-trip to `llama-server` ~2 ms, and
//! `nvidia-smi` ~17 ms. Three async chains feeding a menu that is only polled
//! while it is visible would buy back nothing for that. Every call carries an
//! explicit timeout so a wedged server cannot hold the loop.

use gio::prelude::*;

use crate::lifetime::{Counters, Lifetime};
use crate::{ENDPOINT, UNIT};

const SYSTEMD_NAME: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER_IFACE: &str = "org.freedesktop.systemd1.Manager";
const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";

/// How long any one external call may take before we give up on it.
const TIMEOUT_MS: i32 = 2000;

/// What systemd says about the unit, reduced to the states the menu draws
/// differently. `reloading` folds into [`Running`](UnitState::Running) because
/// the server is still answering while it happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitState {
    Running,
    Starting,
    Stopping,
    Stopped,
    Failed,
}

impl UnitState {
    /// Map systemd's `ActiveState`. An unrecognised value reads as stopped:
    /// the safe direction, since it offers to start rather than to stop.
    pub fn from_active_state(state: &str) -> Self {
        match state {
            "active" | "reloading" => UnitState::Running,
            "activating" => UnitState::Starting,
            "deactivating" => UnitState::Stopping,
            "failed" => UnitState::Failed,
            _ => UnitState::Stopped,
        }
    }

    /// Is the server up enough to be worth asking for numbers?
    pub fn is_up(self) -> bool {
        matches!(self, UnitState::Running)
    }
}

/// The handful of `/metrics` counters the menu shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub tokens_per_second: f64,
    pub requests_processing: u64,
    /// Cumulative, but only since *this* server process started. Banking them
    /// across restarts is [`crate::lifetime`]'s job.
    pub totals: Counters,
}

/// Card memory, in mebibytes, as `nvidia-smi` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vram {
    pub used_mib: u64,
    pub total_mib: u64,
}

/// systemd's object path for a unit name.
///
/// Escaping this locally rather than round-tripping through `LoadUnit` keeps
/// the refresh to a single bus call. The rule is systemd's `bus_label_escape`:
/// letters pass through, digits pass except as the first character, and
/// everything else — including `_` — becomes `_` followed by two hex digits.
pub fn unit_object_path(unit: &str) -> String {
    let mut path = String::from(SYSTEMD_PATH);
    path.push_str("/unit/");
    for (index, byte) in unit.bytes().enumerate() {
        let plain = byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit());
        if plain {
            path.push(byte as char);
        } else {
            path.push_str(&format!("_{byte:02x}"));
        }
    }
    path
}

/// A live connection to the session bus, which is also where the *user*
/// systemd manager lives.
pub struct Session {
    connection: gio::DBusConnection,
    unit_path: String,
}

impl Session {
    pub fn new() -> Result<Self, glib::Error> {
        Ok(Self {
            connection: gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?,
            unit_path: unit_object_path(UNIT),
        })
    }

    pub fn connection(&self) -> &gio::DBusConnection {
        &self.connection
    }

    /// Read `ActiveState` off the unit.
    ///
    /// A unit systemd has never loaded has no object, so the call fails rather
    /// than answering "inactive"; that reads as [`UnitState::Stopped`], which
    /// is what the user means by it.
    pub fn unit_state(&self) -> UnitState {
        let reply = self.connection.call_sync(
            Some(SYSTEMD_NAME),
            &self.unit_path,
            "org.freedesktop.DBus.Properties",
            "Get",
            Some(&(UNIT_IFACE, "ActiveState").to_variant()),
            Some(glib::VariantTy::new("(v)").expect("valid type")),
            gio::DBusCallFlags::NONE,
            TIMEOUT_MS,
            gio::Cancellable::NONE,
        );

        let state = reply
            .ok()
            .and_then(|reply| reply.try_child_value(0))
            .and_then(|boxed| boxed.as_variant())
            .and_then(|value| value.get::<String>());

        match state {
            Some(state) => UnitState::from_active_state(&state),
            None => UnitState::Stopped,
        }
    }

    /// Ask systemd to start or stop the unit.
    ///
    /// Fire-and-forget: the job takes ~20 s to land 20 GB on the card, far
    /// longer than a menu click should wait, and the next refresh reports the
    /// outcome anyway.
    pub fn set_unit_running(&self, running: bool) {
        let method = if running { "StartUnit" } else { "StopUnit" };
        self.connection.call(
            Some(SYSTEMD_NAME),
            SYSTEMD_PATH,
            MANAGER_IFACE,
            method,
            Some(&(UNIT, "replace").to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            TIMEOUT_MS,
            gio::Cancellable::NONE,
            move |result| {
                if let Err(error) = result {
                    glib::g_warning!("llama-tray", "{method} failed: {error}");
                }
            },
        );
    }
}

/// `GET` a path from the server and hand back the response body.
///
/// `None` covers every way the server can be unavailable — not listening,
/// still loading the model, wedged past the timeout — because to this app they
/// are all just "no numbers to show".
fn get(path: &str) -> Option<String> {
    let client = gio::SocketClient::new();
    client.set_timeout(TIMEOUT_MS as u32 / 1000);

    let connection = client
        .connect_to_host(ENDPOINT, 0, gio::Cancellable::NONE)
        .ok()?;

    // `Connection: close` so the read ends at EOF instead of on a length we
    // would otherwise have to parse out of the headers.
    let request = format!("GET {path} HTTP/1.1\r\nHost: {ENDPOINT}\r\nConnection: close\r\n\r\n");
    connection
        .output_stream()
        .write_all(request.as_bytes(), gio::Cancellable::NONE)
        .ok()?;

    let mut buffer = vec![0u8; 64 * 1024];
    let (read, _) = connection
        .input_stream()
        .read_all(&mut buffer, gio::Cancellable::NONE)
        .ok()?;
    buffer.truncate(read);

    let response = String::from_utf8(buffer).ok()?;
    Some(split_body(&response)?.to_string())
}

/// The body of an HTTP response, or `None` if the headers never ended.
fn split_body(response: &str) -> Option<&str> {
    response.split_once("\r\n\r\n").map(|(_, body)| body)
}

/// Read the two counters the menu shows out of Prometheus text format.
///
/// Absent counters are not an error: llama.cpp only exposes these when it was
/// started with `--metrics`, and a build without it should still show a
/// working toggle rather than nothing at all.
pub fn parse_metrics(body: &str) -> Metrics {
    Metrics {
        tokens_per_second: metric(body, "llamacpp:predicted_tokens_seconds").unwrap_or(0.0),
        requests_processing: metric(body, "llamacpp:requests_processing").unwrap_or(0.0) as u64,
        totals: Counters {
            prompt_tokens: metric(body, "llamacpp:prompt_tokens_total").unwrap_or(0.0) as u64,
            predicted_tokens: metric(body, "llamacpp:tokens_predicted_total").unwrap_or(0.0) as u64,
            generating_seconds: metric(body, "llamacpp:tokens_predicted_seconds_total")
                .unwrap_or(0.0),
        },
    }
}

/// One `name value` line of Prometheus text format. Comments start with `#`.
fn metric(body: &str, name: &str) -> Option<f64> {
    body.lines()
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let (key, value) = line.split_once(char::is_whitespace)?;
            (key == name).then(|| value.trim().parse().ok())?
        })
}

/// Current throughput and queue depth, or `None` when the server is unreachable.
pub fn metrics() -> Option<Metrics> {
    get("/metrics").map(|body| parse_metrics(&body))
}

/// The alias the server was launched with (`-a qwen3.6-27b`).
pub fn model_name() -> Option<String> {
    parse_model_name(&get("/v1/models")?)
}

/// Pull the first model id out of a `/v1/models` reply.
pub fn parse_model_name(body: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    let id = parsed.get("data")?.get(0)?.get("id")?.as_str()?;
    (!id.is_empty()).then(|| id.to_string())
}

/// Card memory right now. `None` when there is no NVIDIA driver to ask.
///
/// Deliberately not scoped to the server's own allocation: the number that
/// matters before launching a game is how much of the card is spoken for in
/// total, whatever is holding it.
pub fn vram() -> Option<Vram> {
    let process = gio::Subprocess::newv(
        &[
            "nvidia-smi".as_ref(),
            "--query-gpu=memory.used,memory.total".as_ref(),
            "--format=csv,noheader,nounits".as_ref(),
        ],
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    )
    .ok()?;

    let (stdout, _) = process
        .communicate_utf8(None, gio::Cancellable::NONE)
        .ok()?;

    parse_vram(stdout?.as_str())
}

/// Parse one `used, total` row of `nvidia-smi --format=csv,noheader,nounits`.
///
/// Only the first row is read: a second card would need the menu to say which
/// one it meant, and this machine has one.
pub fn parse_vram(output: &str) -> Option<Vram> {
    let line = output.lines().next()?;
    let (used, total) = line.split_once(',')?;
    Some(Vram {
        used_mib: used.trim().parse().ok()?,
        total_mib: total.trim().parse().ok()?,
    })
}

/// Where the banked totals live between logins.
fn lifetime_path() -> std::path::PathBuf {
    let state = glib::user_state_dir().join("llama-tray");
    state.join("lifetime.json")
}

/// Read the banked totals. A missing or unreadable file starts from zero
/// rather than failing: losing the history is a shame, not a reason to have no
/// tray icon.
pub fn load_lifetime() -> Lifetime {
    std::fs::read_to_string(lifetime_path())
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Write the banked totals back.
///
/// Through a temporary file and a rename, so that being killed mid-write
/// leaves the previous totals intact instead of a truncated file that would
/// silently reset the history to zero.
pub fn save_lifetime(lifetime: &Lifetime) {
    let path = lifetime_path();
    let Some(directory) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(directory).is_err() {
        return;
    }

    let Ok(json) = serde_json::to_string(lifetime) else {
        return;
    };

    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, json).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
}

/// Open the server's own web UI in the default browser.
pub fn open_web_ui() {
    let uri = format!("http://{ENDPOINT}/");
    if let Err(error) = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE) {
        glib::g_warning!("llama-tray", "could not open {uri}: {error}");
    }
}

/// Follow the unit's journal in a terminal.
///
/// There is no GUI log viewer that will show a *user* unit's journal, so this
/// is a terminal or nothing.
pub fn open_logs() {
    let Some(argv) = terminal_argv(
        |program| glib::find_program_in_path(program).is_some(),
        &["journalctl", "--user", "-u", UNIT, "-f", "-n", "200"],
    ) else {
        glib::g_warning!("llama-tray", "no terminal emulator found to show logs in");
        return;
    };

    let argv: Vec<&std::ffi::OsStr> = argv.iter().map(|arg| arg.as_ref()).collect();
    if let Err(error) = gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
        glib::g_warning!("llama-tray", "could not start a terminal: {error}");
    }
}

/// Terminals that can run a command, and the flag each needs to be told where
/// its own options stop. First one installed wins.
const TERMINALS: &[(&str, &str)] = &[
    ("ptyxis", "--"),
    ("kgx", "--"),
    ("gnome-terminal", "--"),
    ("xterm", "-e"),
];

/// Build the argv that runs `command` inside whichever terminal is installed.
///
/// `exists` is injected so this stays a pure function; the caller passes a
/// `PATH` lookup.
pub fn terminal_argv(exists: impl Fn(&str) -> bool, command: &[&str]) -> Option<Vec<String>> {
    let (terminal, separator) = TERMINALS
        .iter()
        .find(|(terminal, _)| exists(terminal))
        .copied()?;

    let mut argv = vec![terminal.to_string(), separator.to_string()];
    argv.extend(command.iter().map(|arg| arg.to_string()));
    Some(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_path_matches_what_systemd_actually_uses() {
        // Confirmed against `busctl --user get-property` on the real unit.
        assert_eq!(
            unit_object_path("llama-server.service"),
            "/org/freedesktop/systemd1/unit/llama_2dserver_2eservice"
        );
    }

    #[test]
    fn a_leading_digit_is_escaped_but_a_later_one_is_not() {
        // systemd escapes digits only in first position, so that a path
        // component never starts with one.
        assert!(unit_object_path("2fa.service").ends_with("_32fa_2eservice"));
        assert!(unit_object_path("a2b.service").ends_with("a2b_2eservice"));
    }

    #[test]
    fn underscores_are_escaped_rather_than_passed_through() {
        // The escape is not reversible if `_` is left alone.
        assert!(unit_object_path("a_b.service").ends_with("a_5fb_2eservice"));
    }

    #[test]
    fn active_states_map_onto_what_the_menu_draws() {
        assert_eq!(UnitState::from_active_state("active"), UnitState::Running);
        assert_eq!(
            UnitState::from_active_state("reloading"),
            UnitState::Running,
            "still answering requests"
        );
        assert_eq!(
            UnitState::from_active_state("activating"),
            UnitState::Starting
        );
        assert_eq!(
            UnitState::from_active_state("deactivating"),
            UnitState::Stopping
        );
        assert_eq!(UnitState::from_active_state("failed"), UnitState::Failed);
        assert_eq!(UnitState::from_active_state("inactive"), UnitState::Stopped);
    }

    #[test]
    fn an_unknown_active_state_offers_to_start_rather_than_to_stop() {
        assert_eq!(UnitState::from_active_state("nonsense"), UnitState::Stopped);
        assert_eq!(UnitState::from_active_state(""), UnitState::Stopped);
    }

    /// A trimmed copy of the real `/metrics` output.
    const METRICS: &str = "\
# HELP llamacpp:prompt_tokens_total Number of prompt tokens processed.
# TYPE llamacpp:prompt_tokens_total counter
llamacpp:prompt_tokens_total 77690
llamacpp:predicted_tokens_seconds 93.634
llamacpp:requests_processing 2
llamacpp:requests_deferred 0
";

    #[test]
    fn metrics_are_read_out_of_prometheus_text() {
        let metrics = parse_metrics(METRICS);
        assert_eq!(metrics.tokens_per_second, 93.634);
        assert_eq!(metrics.requests_processing, 2);
    }

    #[test]
    fn comment_lines_are_not_mistaken_for_values() {
        // `# TYPE llamacpp:x counter` would otherwise parse as the metric `#`
        // and, worse, a `# HELP` line naming a metric could shadow it.
        assert_eq!(metric(METRICS, "#"), None);
        assert_eq!(
            metric(METRICS, "llamacpp:prompt_tokens_total"),
            Some(77690.0)
        );
    }

    #[test]
    fn a_metric_that_is_only_a_prefix_of_another_is_not_returned() {
        assert_eq!(
            metric("llamacpp:requests_processing 5\n", "llamacpp:requests"),
            None
        );
    }

    #[test]
    fn missing_metrics_read_as_zero_rather_than_failing() {
        // A build without --metrics still gets a working toggle.
        let metrics = parse_metrics("");
        assert_eq!(metrics.tokens_per_second, 0.0);
        assert_eq!(metrics.requests_processing, 0);
    }

    #[test]
    fn the_model_alias_comes_out_of_the_models_reply() {
        let body = r#"{"object":"list","data":[{"id":"qwen3.6-27b","object":"model"}]}"#;
        assert_eq!(parse_model_name(body), Some("qwen3.6-27b".to_string()));
    }

    #[test]
    fn a_models_reply_with_nothing_in_it_yields_no_name() {
        assert_eq!(parse_model_name(r#"{"data":[]}"#), None);
        assert_eq!(parse_model_name(r#"{"data":[{"id":""}]}"#), None);
        assert_eq!(parse_model_name("not json"), None);
    }

    #[test]
    fn vram_is_read_from_the_csv_row() {
        assert_eq!(
            parse_vram("30106, 32607\n"),
            Some(Vram {
                used_mib: 30106,
                total_mib: 32607
            })
        );
    }

    #[test]
    fn a_machine_with_no_nvidia_driver_reports_no_vram() {
        // nvidia-smi prints an error to stdout on some failures rather than
        // exiting quietly, so the parser has to reject prose.
        assert_eq!(parse_vram(""), None);
        assert_eq!(
            parse_vram("NVIDIA-SMI has failed because it couldn't\n"),
            None
        );
    }

    #[test]
    fn the_body_is_taken_from_after_the_headers() {
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nllamacpp:x 1\n";
        assert_eq!(split_body(response), Some("llamacpp:x 1\n"));
    }

    #[test]
    fn a_truncated_response_has_no_body() {
        assert_eq!(split_body("HTTP/1.1 200 OK\r\nContent-Ty"), None);
    }

    #[test]
    fn the_first_installed_terminal_is_used() {
        // ptyxis is the one on this machine; the rest are there so the app is
        // not useless on a differently-provisioned desktop.
        let argv = terminal_argv(|program| program == "ptyxis", &["journalctl", "-f"]);
        assert_eq!(
            argv,
            Some(vec![
                "ptyxis".to_string(),
                "--".to_string(),
                "journalctl".to_string(),
                "-f".to_string()
            ])
        );
    }

    #[test]
    fn each_terminal_gets_the_separator_it_needs() {
        // `xterm -e` and `gnome-terminal --` are not interchangeable; passing
        // the wrong one makes the terminal swallow the command's own flags.
        let argv = terminal_argv(|program| program == "xterm", &["journalctl"]).unwrap();
        assert_eq!(argv[..2], ["xterm".to_string(), "-e".to_string()]);
    }

    #[test]
    fn terminals_are_tried_in_order_of_preference() {
        // Everything installed: the GNOME-native one should win over xterm.
        let argv = terminal_argv(|_| true, &["journalctl"]).unwrap();
        assert_eq!(argv[0], "ptyxis");
    }

    #[test]
    fn no_terminal_installed_yields_no_command_rather_than_a_broken_one() {
        assert_eq!(terminal_argv(|_| false, &["journalctl"]), None);
    }
}
