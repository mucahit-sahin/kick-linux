mod api;
mod config;
mod download;
mod fmt;
mod mpv;
mod ui;

use gtk::glib;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::ffi::c_char;
use std::rc::Rc;

extern "C" {
    fn setlocale(category: i32, locale: *const c_char) -> *mut c_char;
}

/// LC_NUMERIC = 1 on glibc
const LC_NUMERIC: i32 = 1;

const APP_ID: &str = "io.github.kicklite.KickLite";

fn main() -> glib::ExitCode {
    // mpv requires the numeric locale to be "C".
    unsafe {
        setlocale(LC_NUMERIC, b"C\0".as_ptr() as *const c_char);
    }

    // Handle command-line arguments ourselves so GApplication does not
    // interpret them as files.
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gtk::gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.connect_startup(|_| {
        load_css();
    });

    let created: Rc<RefCell<Option<Rc<ui::App>>>> = Rc::new(RefCell::new(None));
    let pending: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    app.connect_command_line({
        let pending = pending.clone();
        move |app, cmdline| {
            *pending.borrow_mut() = cmdline
                .arguments()
                .get(1)
                .and_then(|arg| arg.to_str())
                .filter(|arg| !arg.starts_with('-'))
                .map(str::to_string);
            app.activate();
            glib::ExitCode::SUCCESS
        }
    });

    app.connect_activate(move |app| {
        if let Some(existing) = created.borrow().as_ref() {
            if let Some(target) = pending.borrow_mut().take() {
                existing.handle_target(&target);
            }
            existing.window.present();
            return;
        }
        // GTK may change the locale during startup; reset the numeric locale
        // back to "C" before starting mpv.
        unsafe {
            setlocale(LC_NUMERIC, b"C\0".as_ptr() as *const c_char);
        }
        let cfg = config::load();
        match mpv::Player::new(cfg.silence_noise_db, cfg.silence_duration) {
            Ok(player) => {
                let window = ui::App::new(app, player, cfg);
                if let Some(target) = pending.borrow_mut().take() {
                    let a = window.clone();
                    glib::timeout_add_seconds_local(1, move || {
                        a.handle_target(&target);
                        glib::ControlFlow::Break
                    });
                }
                maybe_screenshot(&window.window);
                *created.borrow_mut() = Some(window);
            }
            Err(err) => show_player_error(app, &err.to_string()),
        }
    });

    app.run()
}

/// For tests and docs: KICK_LITE_SCREENSHOT=/path/to/image.png
/// (optional KICK_LITE_SCREENSHOT_DELAY in seconds)
fn maybe_screenshot(window: &adw::ApplicationWindow) {
    let Ok(path) = std::env::var("KICK_LITE_SCREENSHOT") else {
        return;
    };
    let delay = std::env::var("KICK_LITE_SCREENSHOT_DELAY")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(4);
    let win = window.clone();
    glib::timeout_add_seconds_local(delay, move || {
        take_screenshot(&win, &path);
        glib::ControlFlow::Break
    });
}

fn take_screenshot(window: &adw::ApplicationWindow, path: &str) {
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
    let Some(node) = snapshot.to_node() else {
        eprintln!("screenshot: could not create node");
        return;
    };
    let renderer = gtk::gsk::CairoRenderer::new();
    let surface = window.surface();
    if let Err(err) = renderer.realize(surface.as_ref()) {
        eprintln!("screenshot: could not set up renderer: {err}");
        return;
    }
    let texture = renderer.render_texture(&node, None);
    match texture.save_to_png(path) {
        Ok(()) => eprintln!("screenshot written: {path}"),
        Err(err) => eprintln!("screenshot could not be written: {err}"),
    }
    renderer.unrealize();
}

fn show_player_error(app: &adw::Application, message: &str) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Kick Lite")
        .default_width(520)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let title = gtk::Label::new(Some("Player could not be started"));
    title.add_css_class("title-2");
    title.set_xalign(0.0);

    let text = gtk::Label::new(Some(&format!(
        "{message}\n\nMake sure the 'mpv' package (libmpv) is installed for playback."
    )));
    text.set_wrap(true);
    text.set_xalign(0.0);

    let close = gtk::Button::with_label("Close");
    close.set_halign(gtk::Align::End);
    let win = window.clone();
    close.connect_clicked(move |_| win.close());

    content.append(&title);
    content.append(&text);
    content.append(&close);
    window.set_content(Some(&content));
    window.present();
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

const CSS: &str = r#"
.app-title { font-weight: bold; }

.sidebar-panel {
    background-color: alpha(@window_bg_color, 0.55);
}
.sidebar-heading {
    font-size: 11px;
    font-weight: bold;
    opacity: 0.62;
}
.sidebar-name { font-weight: bold; }
.live-dot { color: #2ecc71; font-size: 11px; }
.offline-dot { color: rgba(128, 128, 128, 0.5); font-size: 11px; }

.live-badge {
    background-color: #e11d48;
    color: #ffffff;
    border-radius: 999px;
    padding: 1px 8px;
    font-size: 10px;
    font-weight: bold;
}

.channel-card {
    padding: 0;
    border-radius: 12px;
    background-color: alpha(@window_fg_color, 0.045);
}
.channel-card:hover {
    background-color: alpha(@window_fg_color, 0.10);
}

.mini-player {
    min-width: 240px;
    min-height: 135px;
    border-radius: 10px;
    background-color: #000000;
}
.mini-player:hover {
    background-color: #111111;
}
.mini-close {
    min-width: 24px;
    min-height: 24px;
    padding: 2px;
    border-radius: 999px;
    background-color: rgba(0, 0, 0, 0.72);
    color: #ffffff;
}
.mini-close:hover {
    background-color: rgba(220, 40, 60, 0.92);
}

.hero-title {
    font-size: 34px;
    font-weight: bold;
}

.video-area { background-color: #000000; }

.video-topbar {
    background-image: linear-gradient(to bottom, rgba(0, 0, 0, 0.78), rgba(0, 0, 0, 0.0));
    padding: 8px 12px 24px 12px;
}
.video-bottombar {
    background-image: linear-gradient(to top, rgba(0, 0, 0, 0.84), rgba(0, 0, 0, 0.0));
    padding: 24px 12px 8px 12px;
}
.video-bottombar > box {
    min-height: 34px;
}
.video-bottombar > box > box {
    min-width: 0;
}
.video-topbar label,
.video-bottombar label { color: #ffffff; }
.video-topbar button,
.video-bottombar button { color: #ffffff; }
.video-topbar button:hover,
.video-bottombar button:hover { background-color: rgba(255, 255, 255, 0.18); }
.video-toggle:checked {
    background-color: rgba(255, 255, 255, 0.26);
    box-shadow: inset 0 -2px 0 #ffffff;
}

.video-title {
    font-weight: bold;
    font-size: 16px;
}
.video-subtitle {
    font-size: 12px;
    opacity: 0.82;
}
.video-time {
    font-size: 12px;
    opacity: 0.9;
}

.video-status {
    background-color: rgba(0, 0, 0, 0.68);
    border-radius: 14px;
    padding: 18px 26px;
}
.video-status-label {
    color: #ffffff;
    font-weight: bold;
}

.video-scale trough {
    min-height: 4px;
    border-radius: 999px;
    background-color: rgba(255, 255, 255, 0.28);
}
.video-scale highlight {
    background-color: #ffffff;
    border-radius: 999px;
}
.video-scale slider {
    background-color: #ffffff;
    border-radius: 999px;
}
"#;
