//! Turning what the probes found into the rows of the menu. No I/O here.

use crate::lifetime::{self, Counters};
use crate::server::{Metrics, UnitState, Vram};
use crate::tray::MenuRow;

/// Detailed action names, matched in `main`.
pub const ACTION_TOGGLE: &str = "toggle";
pub const ACTION_WEB_UI: &str = "web-ui";
pub const ACTION_LOGS: &str = "logs";
pub const ACTION_QUIT: &str = "quit";

/// One id per row, fixed for the life of the process.
///
/// Rows come and go — the throughput line only exists while a server does — but
/// an id always means the same row, and always the same *kind* of row. Numbering
/// the rows by position instead is what broke the menu the first time the server
/// was stopped from it: see [`MenuRow`].
const ROW_HEADING: i32 = 1;
const ROW_THROUGHPUT: i32 = 2;
const ROW_VRAM: i32 = 3;
const ROW_LIFETIME_RULE: i32 = 4;
const ROW_LIFETIME_TOKENS: i32 = 5;
const ROW_LIFETIME_WORDS: i32 = 6;
const ROW_ACTIONS_RULE: i32 = 7;
const ROW_TOGGLE: i32 = 8;
const ROW_WEB_UI: i32 = 9;
const ROW_LOGS: i32 = 10;
const ROW_QUIT_RULE: i32 = 11;
const ROW_QUIT: i32 = 12;

/// Everything one refresh learned. Any field may be absent: the server is down
/// for most of the interesting cases, which is precisely when the menu is open.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub state: UnitState,
    pub model: Option<String>,
    pub metrics: Option<Metrics>,
    pub vram: Option<Vram>,
    /// Everything the card has processed, across every run of the server.
    pub lifetime: Counters,
}

impl Snapshot {
    /// What the menu shows before the first probe has returned.
    pub fn unknown() -> Self {
        Self {
            state: UnitState::Stopped,
            model: None,
            metrics: None,
            vram: None,
            lifetime: Counters::default(),
        }
    }
}

/// `30106` MiB as `29.4`, because a panel menu should not make anyone divide
/// by 1024 in their head.
fn gib(mib: u64) -> f64 {
    mib as f64 / 1024.0
}

/// The heading: what is loaded, and whether it is up.
fn heading(snapshot: &Snapshot) -> String {
    // Falling back to the unit's own name keeps the row honest while the server
    // is down, when there is nothing to ask for a model name.
    let subject = snapshot.model.as_deref().unwrap_or("llama-server");
    let state = match snapshot.state {
        UnitState::Running => "running",
        UnitState::Starting => "starting",
        UnitState::Stopping => "stopping",
        UnitState::Stopped => "stopped",
        UnitState::Failed => "failed",
    };
    format!("{subject} · {state}")
}

/// The throughput row, present only when there is a server to have produced it.
fn throughput(snapshot: &Snapshot) -> Option<String> {
    let metrics = snapshot.metrics.filter(|_| snapshot.state.is_up())?;
    let active = metrics.requests_processing;
    Some(format!(
        "{:.1} tok/s · {active} active",
        metrics.tokens_per_second
    ))
}

/// The label and enabled-ness of the start/stop row.
///
/// Mid-transition the row is disabled rather than hidden: a row that vanishes
/// moves everything under it just as the pointer arrives.
fn toggle(state: UnitState) -> MenuRow {
    match state {
        UnitState::Running => MenuRow::item(ROW_TOGGLE, "Stop Server", ACTION_TOGGLE),
        UnitState::Stopped | UnitState::Failed => {
            MenuRow::item(ROW_TOGGLE, "Start Server", ACTION_TOGGLE)
        }
        UnitState::Starting => MenuRow::disabled(ROW_TOGGLE, "Starting…", ACTION_TOGGLE),
        UnitState::Stopping => MenuRow::disabled(ROW_TOGGLE, "Stopping…", ACTION_TOGGLE),
    }
}

/// The whole menu, top to bottom.
pub fn menu(snapshot: &Snapshot) -> Vec<MenuRow> {
    let mut rows = vec![MenuRow::info(ROW_HEADING, &heading(snapshot))];

    if let Some(line) = throughput(snapshot) {
        rows.push(MenuRow::info(ROW_THROUGHPUT, &line));
    }

    if let Some(vram) = snapshot.vram {
        rows.push(MenuRow::info(
            ROW_VRAM,
            &format!(
                "VRAM {:.1} / {:.1} GiB",
                gib(vram.used_mib),
                gib(vram.total_mib)
            ),
        ));
    }

    // Only once there is something to boast about. Two rows of zeroes on a
    // fresh install say nothing and cost the same space as the real thing.
    if let Some([tokens, words]) = lifetime_rows(&snapshot.lifetime) {
        rows.push(MenuRow::separator(ROW_LIFETIME_RULE));
        rows.push(MenuRow::info(ROW_LIFETIME_TOKENS, &tokens));
        rows.push(MenuRow::info(ROW_LIFETIME_WORDS, &words));
    }

    rows.push(MenuRow::separator(ROW_ACTIONS_RULE));
    rows.push(toggle(snapshot.state));

    // Nothing answers on the port until the model has finished loading, so the
    // browser would land on a connection refused.
    if snapshot.state.is_up() {
        rows.push(MenuRow::item(ROW_WEB_UI, "Open Web UI", ACTION_WEB_UI));
    } else {
        rows.push(MenuRow::disabled(ROW_WEB_UI, "Open Web UI", ACTION_WEB_UI));
    }

    // Always live: the logs are most wanted exactly when the unit failed.
    rows.push(MenuRow::item(ROW_LOGS, "View Logs", ACTION_LOGS));
    rows.push(MenuRow::separator(ROW_QUIT_RULE));
    rows.push(MenuRow::item(ROW_QUIT, "Quit", ACTION_QUIT));

    rows
}

/// The two "how much has this thing done" rows, or `None` before the card has
/// generated anything at all.
///
/// Read tokens are separated from written ones because they are not the same
/// achievement: the prompt side is mostly the KV cache being re-fed, while the
/// generated side is what the GPU actually thought up.
fn lifetime_rows(total: &Counters) -> Option<[String; 2]> {
    if total.predicted_tokens == 0 {
        return None;
    }

    Some([
        format!(
            "All time: {} written · {} read",
            lifetime::compact(total.predicted_tokens),
            lifetime::compact(total.prompt_tokens)
        ),
        format!(
            "≈ {} words in {} of thinking",
            lifetime::compact(lifetime::words(total.predicted_tokens)),
            lifetime::duration(total.generating_seconds)
        ),
    ])
}

/// The rows and the icon together: everything the tray draws.
pub fn view(snapshot: &Snapshot) -> crate::tray::View {
    crate::tray::View {
        rows: menu(snapshot),
        icon_name: icon_name(snapshot.state),
    }
}

/// Which panel icon to wear.
///
/// State is carried by the icon rather than by the item's `Status`, because
/// GNOME's appindicator extension hides a `Passive` item outright — which
/// would remove the start button at the one moment it is wanted.
pub fn icon_name(state: UnitState) -> String {
    if state.is_up() {
        format!("{}-symbolic", crate::APP_ID)
    } else {
        format!("{}-inactive-symbolic", crate::APP_ID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::MenuEntry;
    use std::collections::HashMap;

    fn running() -> Snapshot {
        Snapshot {
            state: UnitState::Running,
            model: Some("qwen3.6-27b".to_string()),
            metrics: Some(Metrics {
                tokens_per_second: 93.634,
                requests_processing: 0,
                totals: Counters::default(),
            }),
            vram: Some(Vram {
                used_mib: 30106,
                total_mib: 32607,
            }),
            lifetime: Counters {
                prompt_tokens: 3_240_000,
                predicted_tokens: 9_234_567,
                generating_seconds: 83_700.0,
            },
        }
    }

    /// Every menu this app can draw: each unit state against a card that may or
    /// may not answer, a server that may or may not, and totals that may still
    /// be empty. What the id tests have to hold across.
    fn every_menu() -> Vec<Vec<MenuRow>> {
        let mut menus = Vec::new();
        for state in [
            UnitState::Running,
            UnitState::Starting,
            UnitState::Stopping,
            UnitState::Stopped,
            UnitState::Failed,
        ] {
            for vram in [running().vram, None] {
                for metrics in [running().metrics, None] {
                    for lifetime in [running().lifetime, Counters::default()] {
                        menus.push(menu(&Snapshot {
                            state,
                            vram,
                            metrics,
                            lifetime,
                            ..running()
                        }));
                    }
                }
            }
        }
        menus
    }

    /// The labels of every row, so a test can talk about the menu the way the
    /// user sees it.
    fn labels(snapshot: &Snapshot) -> Vec<String> {
        menu(snapshot)
            .iter()
            .map(|row| match &row.entry {
                MenuEntry::Item { label, .. } | MenuEntry::Info { label } => label.clone(),
                MenuEntry::Separator => "---".to_string(),
            })
            .collect()
    }

    #[test]
    fn a_running_server_shows_its_model_throughput_and_memory() {
        assert_eq!(
            labels(&running()),
            vec![
                "qwen3.6-27b · running",
                "93.6 tok/s · 0 active",
                "VRAM 29.4 / 31.8 GiB",
                "---",
                "All time: 9.2M written · 3.2M read",
                "≈ 6.9M words in 23h 15m of thinking",
                "---",
                "Stop Server",
                "Open Web UI",
                "View Logs",
                "---",
                "Quit",
            ]
        );
    }

    #[test]
    fn a_stopped_server_offers_to_start_and_still_shows_the_card() {
        // The VRAM row is the point of stopping it, so it has to survive the
        // server going away.
        let snapshot = Snapshot {
            state: UnitState::Stopped,
            model: None,
            metrics: None,
            ..running()
        };
        assert_eq!(
            labels(&snapshot),
            vec![
                "llama-server · stopped",
                "VRAM 29.4 / 31.8 GiB",
                "---",
                "All time: 9.2M written · 3.2M read",
                "≈ 6.9M words in 23h 15m of thinking",
                "---",
                "Start Server",
                "Open Web UI",
                "View Logs",
                "---",
                "Quit",
            ]
        );
    }

    #[test]
    fn an_id_always_means_the_same_kind_of_row() {
        // The bug this guards. Ids were positions, so stopping the server took
        // the throughput row away and slid every id below it up by one. A host
        // builds one widget per id and settles its kind the first time it sees
        // it, so the menu came back with a separator wearing "Start Server",
        // which cannot be clicked, and stale labels where the rules should be.
        let mut seen: HashMap<i32, &str> = HashMap::new();
        for rows in every_menu() {
            for row in rows {
                let kind = match row.entry {
                    MenuEntry::Item { .. } => "item",
                    MenuEntry::Info { .. } => "info",
                    MenuEntry::Separator => "separator",
                };
                if let Some(before) = seen.insert(row.id, kind) {
                    assert_eq!(before, kind, "row {} changed kind", row.id);
                }
            }
        }
    }

    #[test]
    fn no_row_shares_an_id_or_claims_the_root_s() {
        for rows in every_menu() {
            let mut ids: Vec<i32> = rows.iter().map(|row| row.id).collect();
            assert!(!ids.contains(&0), "dbusmenu keeps 0 for the root");
            let drawn = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), drawn, "two rows answer to the same id");
        }
    }

    #[test]
    fn stopping_the_server_moves_no_other_row() {
        // The transition from the bug report: the throughput row goes, and
        // everything under it has to stay on the id it already had.
        let before = menu(&running());
        let after = menu(&Snapshot {
            state: UnitState::Stopped,
            model: None,
            metrics: None,
            ..running()
        });

        assert!(before.iter().any(|row| row.id == ROW_THROUGHPUT));
        assert!(!after.iter().any(|row| row.id == ROW_THROUGHPUT));
        assert_eq!(
            after.iter().find(|row| row.id == ROW_TOGGLE),
            Some(&MenuRow::item(ROW_TOGGLE, "Start Server", ACTION_TOGGLE))
        );
    }

    #[test]
    fn throughput_is_dropped_when_the_unit_is_not_up() {
        // /metrics can answer for a moment after the stop is issued; showing
        // "93.6 tok/s" next to "stopping" would be a lie.
        let snapshot = Snapshot {
            state: UnitState::Stopping,
            ..running()
        };
        assert!(!labels(&snapshot).iter().any(|row| row.contains("tok/s")));
    }

    #[test]
    fn mid_transition_the_toggle_is_disabled_but_still_there() {
        for (state, label) in [
            (UnitState::Starting, "Starting…"),
            (UnitState::Stopping, "Stopping…"),
        ] {
            assert_eq!(
                toggle(state),
                MenuRow::disabled(ROW_TOGGLE, label, ACTION_TOGGLE),
                "{state:?} must not be clickable"
            );
        }
    }

    #[test]
    fn a_failed_unit_offers_to_start_it_again() {
        assert_eq!(
            toggle(UnitState::Failed),
            MenuRow::item(ROW_TOGGLE, "Start Server", ACTION_TOGGLE)
        );
    }

    #[test]
    fn the_web_ui_is_only_reachable_while_the_server_is_up() {
        let up = menu(&running());
        assert!(up.contains(&MenuRow::item(ROW_WEB_UI, "Open Web UI", ACTION_WEB_UI)));

        let down = menu(&Snapshot {
            state: UnitState::Starting,
            ..running()
        });
        assert!(
            down.contains(&MenuRow::disabled(ROW_WEB_UI, "Open Web UI", ACTION_WEB_UI)),
            "the port is not listening until the model has loaded"
        );
    }

    #[test]
    fn the_logs_stay_reachable_when_the_unit_has_failed() {
        let snapshot = Snapshot {
            state: UnitState::Failed,
            ..running()
        };
        assert!(menu(&snapshot).contains(&MenuRow::item(ROW_LOGS, "View Logs", ACTION_LOGS)));
    }

    #[test]
    fn a_menu_with_nothing_known_yet_is_still_usable() {
        // What the very first paint gets if every probe failed.
        assert_eq!(
            labels(&Snapshot::unknown()),
            vec![
                "llama-server · stopped",
                "---",
                "Start Server",
                "Open Web UI",
                "View Logs",
                "---",
                "Quit",
            ]
        );
    }

    #[test]
    fn quit_is_always_the_last_row() {
        for state in [UnitState::Running, UnitState::Stopped, UnitState::Failed] {
            assert_eq!(
                menu(&Snapshot { state, ..running() }).last(),
                Some(&MenuRow::item(ROW_QUIT, "Quit", ACTION_QUIT)),
                "{state:?}"
            );
        }
    }

    #[test]
    fn the_all_time_rows_are_hidden_until_something_has_been_generated() {
        // On a fresh install "All time: 0 written · 0 read" is just noise, and
        // it would push the toggle two rows further from the pointer.
        let fresh = Snapshot {
            lifetime: Counters::default(),
            ..running()
        };
        assert_eq!(lifetime_rows(&fresh.lifetime), None);
        assert!(!labels(&fresh).iter().any(|row| row.contains("All time")));
    }

    #[test]
    fn the_all_time_rows_survive_the_server_being_stopped() {
        // They are banked totals, not live ones — going away when the server
        // does would defeat the point of accumulating them.
        let stopped = Snapshot {
            state: UnitState::Stopped,
            metrics: None,
            ..running()
        };
        assert!(labels(&stopped)
            .iter()
            .any(|row| row == "All time: 9.2M written · 3.2M read"));
    }

    #[test]
    fn the_icon_distinguishes_a_running_server_from_a_stopped_one() {
        assert_eq!(
            icon_name(UnitState::Running),
            "us.hagreli.LlamaTray-symbolic"
        );
        assert_eq!(
            icon_name(UnitState::Stopped),
            "us.hagreli.LlamaTray-inactive-symbolic"
        );
        assert_eq!(
            icon_name(UnitState::Starting),
            "us.hagreli.LlamaTray-inactive-symbolic",
            "not serving yet"
        );
    }

    #[test]
    fn the_view_carries_both_the_rows_and_the_icon() {
        let view = view(&running());
        assert_eq!(view.rows, menu(&running()));
        assert_eq!(view.icon_name, "us.hagreli.LlamaTray-symbolic");
    }

    #[test]
    fn memory_is_shown_in_gibibytes_not_raw_mebibytes() {
        // nvidia-smi reports MiB; dividing by 1000 would overstate the card by
        // about 2 GiB, which is most of the headroom the number exists to show.
        assert_eq!(format!("{:.1}", gib(30106)), "29.4");
        assert_eq!(format!("{:.1}", gib(32607)), "31.8");
    }
}
