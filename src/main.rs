//! Wiring: one bus connection, one tray icon, one main loop.

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;

use llama_tray::lifetime::Lifetime;
use llama_tray::server::{self, Session};
use llama_tray::status::{self, Snapshot};
use llama_tray::tray::{Tray, WATCHER_NAME};

/// Ask every source once. Called when the menu is about to be drawn and every
/// couple of seconds while it stays up — never in the background.
fn probe(session: &Session, lifetime: &RefCell<Lifetime>) -> Snapshot {
    let state = session.unit_state();

    // Only bother the server when systemd says there is one. While it is
    // starting the port is not listening yet, and each probe would otherwise
    // sit out its full connect timeout.
    let (model, metrics) = if state.is_up() {
        (server::model_name(), server::metrics())
    } else {
        (None, None)
    };

    // Fold this reading into the running totals before anything is drawn. A
    // server that has gone away banks its final numbers here, which is the
    // only chance to do it — the counters are gone with the process.
    let totals = {
        let mut lifetime = lifetime.borrow_mut();
        lifetime.observe(metrics.map(|metrics| metrics.totals));
        server::save_lifetime(&lifetime);
        lifetime.total()
    };

    Snapshot {
        state,
        model,
        metrics,
        // Always read: how much of the card is spoken for is the whole reason
        // this exists, and it matters most when the server is *not* the one
        // holding it.
        vram: server::vram(),
        lifetime: totals,
    }
}

/// Build a tray icon bound to this session.
fn make_tray(
    session: &Rc<Session>,
    lifetime: &Rc<RefCell<Lifetime>>,
    main_loop: &glib::MainLoop,
) -> Option<Tray> {
    Tray::new(
        session.connection().clone(),
        {
            let session = session.clone();
            let lifetime = lifetime.clone();
            move || status::view(&probe(&session, &lifetime))
        },
        {
            let session = session.clone();
            let main_loop = main_loop.clone();
            move |action| match action {
                status::ACTION_TOGGLE => {
                    // Read the state again rather than trusting the label the
                    // row was drawn with: the menu may have been sitting open
                    // while something else started or stopped the unit.
                    let running = session.unit_state().is_up();
                    session.set_unit_running(!running);
                }
                status::ACTION_WEB_UI => server::open_web_ui(),
                status::ACTION_LOGS => server::open_logs(),
                status::ACTION_QUIT => main_loop.quit(),
                other => glib::g_warning!("llama-tray", "unknown action: {other}"),
            }
        },
    )
}

fn main() -> ExitCode {
    let session = match Session::new() {
        Ok(session) => Rc::new(session),
        Err(error) => {
            eprintln!("llama-tray: no session bus: {error}");
            return ExitCode::FAILURE;
        }
    };

    // Totals from previous logins, so the panel does not forget everything
    // the card has done each time the session restarts.
    let lifetime = Rc::new(RefCell::new(server::load_lifetime()));

    let main_loop = glib::MainLoop::new(None, false);

    // Follow the watcher's bus name rather than checking for it once at
    // startup. This runs from login, so at that point GNOME Shell has usually
    // not claimed it yet — and the same subscription puts the icon back after
    // a shell restart, which otherwise leaves the panel with a dead item and
    // this process with no way to be reached.
    let tray: Rc<RefCell<Option<Tray>>> = Rc::new(RefCell::new(None));
    gio::bus_watch_name_on_connection(
        session.connection(),
        WATCHER_NAME,
        gio::BusNameWatcherFlags::NONE,
        {
            let session = session.clone();
            let lifetime = lifetime.clone();
            let main_loop = main_loop.clone();
            let tray = tray.clone();
            move |_conn, _name, _owner| {
                let new_tray = make_tray(&session, &lifetime, &main_loop);
                if new_tray.is_none() {
                    glib::g_warning!("llama-tray", "could not export the tray interfaces");
                }
                tray.replace(new_tray);
            }
        },
        {
            let tray = tray.clone();
            move |_conn, _name| {
                // Drop it so the interfaces are unexported; the appeared
                // handler will build a fresh one when the shell comes back.
                tray.replace(None);
            }
        },
    );

    main_loop.run();
    ExitCode::SUCCESS
}
