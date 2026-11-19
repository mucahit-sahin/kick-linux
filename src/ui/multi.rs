//! Extra picture-in-picture players for watching several streams at once.
//!
//! Each tile owns an independent libmpv instance and renders into its own
//! `GLArea`. The full player stays the "focused" stream; clicking a tile swaps
//! it into that slot and moves the focused stream back into a tile.

use crate::mpv;
use crate::ui::App;
use crate::ui::Media;
use gtk::glib;
use gtk::prelude::*;
use std::rc::Rc;
use std::time::Duration;

pub struct MiniPlayer {
    pub id: u64,
    pub player: Rc<mpv::Player>,
    pub media: Media,
    pub tile: gtk::Overlay,
}

impl MiniPlayer {
    /// Creates a muted mini player for `media`. Returns `None` when libmpv
    /// cannot be initialized (the caller has already been notified by a toast).
    pub fn new(app: &Rc<App>, id: u64, media: Media) -> Option<Rc<Self>> {
        let cfg = app.cfg.borrow();
        let player = match mpv::Player::new_mini(cfg.silence_noise_db, cfg.silence_duration) {
            Ok(player) => Rc::new(player),
            Err(err) => {
                drop(cfg);
                app.toast(&format!(
                    "{}: {err}",
                    app.text("Oynatıcı açılamadı", "Could not start player")
                ));
                return None;
            }
        };
        drop(cfg);

        // Several streams playing sound at once is unusable; minis start muted
        // and can be unmuted individually.
        player.set_mute(true);

        let area = gtk::GLArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);
        // Like the main player, only draw when mpv reports a new frame.
        area.set_auto_render(false);
        area.set_has_depth_buffer(false);
        area.set_has_stencil_buffer(false);
        area.add_css_class("video-area");

        let tile = gtk::Overlay::new();
        tile.add_css_class("mini-player");
        tile.add_css_class("multi-tile");
        tile.set_child(Some(&area));
        tile.set_size_request(260, 156);

        // A transparent overlay catches the "promote to main player" click. It
        // is added first so the bars below stay on top and receive their own
        // button presses.
        let click_area = gtk::DrawingArea::new();
        click_area.set_hexpand(true);
        click_area.set_vexpand(true);
        click_area.set_can_target(true);
        click_area.set_tooltip_text(Some(app.text(
            "Ana oynatıcıda aç",
            "Open in the main player",
        )));

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        top.add_css_class("multi-bar");
        top.set_valign(gtk::Align::Start);
        top.set_halign(gtk::Align::Fill);
        top.set_hexpand(true);
        let title = gtk::Label::new(Some(&media.title));
        title.add_css_class("multi-title");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let mute_btn = gtk::Button::from_icon_name("audio-volume-muted-symbolic");
        mute_btn.set_has_frame(false);
        mute_btn.add_css_class("video-button");
        mute_btn.set_tooltip_text(Some(app.text("Sesi aç", "Unmute")));
        let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
        close_btn.set_has_frame(false);
        close_btn.add_css_class("mini-close");
        close_btn.set_margin_top(4);
        close_btn.set_margin_end(4);
        top.append(&title);
        top.append(&mute_btn);
        top.append(&close_btn);

        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        bottom.add_css_class("multi-bar-bottom");
        bottom.set_valign(gtk::Align::End);
        bottom.set_halign(gtk::Align::Start);
        let play_btn = gtk::Button::from_icon_name("media-playback-pause-symbolic");
        play_btn.set_has_frame(false);
        play_btn.add_css_class("video-button");
        play_btn.set_tooltip_text(Some(app.text("Duraklat / Oynat", "Pause / Play")));
        bottom.append(&play_btn);

        tile.add_overlay(&click_area);
        tile.add_overlay(&top);
        tile.add_overlay(&bottom);

        let mini = Rc::new(Self {
            id,
            player,
            media,
            tile,
        });

        // ---- wiring (weak references keep the tile from leaking) ----
        {
            let weak_app = Rc::downgrade(app);
            let click = gtk::GestureClick::new();
            click.set_button(1);
            click.connect_pressed(move |_, _, _, _| {
                // Defer removal of the tile until after this event handler
                // returns; GTK does not like widgets being unparented mid-event.
                let weak_app = weak_app.clone();
                glib::timeout_add_local(Duration::from_millis(0), move || {
                    if let Some(app) = weak_app.upgrade() {
                        app.promote_multi(id);
                    }
                    glib::ControlFlow::Break
                });
            });
            click_area.add_controller(click);
        }
        {
            let p = mini.player.clone();
            let weak_btn = play_btn.downgrade();
            play_btn.connect_clicked(move |_| {
                p.toggle_pause();
                let paused = p.is_paused();
                if let Some(btn) = weak_btn.upgrade() {
                    btn.set_icon_name(if paused {
                        "media-playback-start-symbolic"
                    } else {
                        "media-playback-pause-symbolic"
                    });
                }
            });
        }
        {
            let p = mini.player.clone();
            let en = app.is_english();
            mute_btn.connect_clicked(move |btn| {
                let muted = !p.is_muted();
                p.set_mute(muted);
                btn.set_icon_name(if muted {
                    "audio-volume-muted-symbolic"
                } else {
                    "audio-volume-high-symbolic"
                });
                btn.set_tooltip_text(Some(if en {
                    if muted { "Unmute" } else { "Mute" }
                } else if muted {
                    "Sesi aç"
                } else {
                    "Sesi kapat"
                }));
            });
        }
        {
            let weak_app = Rc::downgrade(app);
            close_btn.connect_clicked(move |_| {
                let weak_app = weak_app.clone();
                glib::timeout_add_local(Duration::from_millis(0), move || {
                    if let Some(app) = weak_app.upgrade() {
                        app.close_multi(id);
                    }
                    glib::ControlFlow::Break
                });
            });
        }
        {
            let p = mini.player.clone();
            let weak_app = Rc::downgrade(app);
            area.connect_realize(move |area| {
                if !p.attach(area) {
                    if let Some(app) = weak_app.upgrade() {
                        app.toast("Could not start video output (OpenGL)");
                    }
                } else {
                    area.queue_render();
                }
            });
        }
        {
            let p = mini.player.clone();
            area.connect_render(move |area, _ctx| {
                if !p.attach(area) {
                    return glib::Propagation::Stop;
                }
                p.render(area);
                glib::Propagation::Proceed
            });
        }
        {
            let p = mini.player.clone();
            area.connect_unrealize(move |_| p.detach());
        }

        if let Err(err) = mini.player.load(&mini.media.url, None) {
            app.toast(&format!("{err:#}"));
            return None;
        }

        Some(mini)
    }
}
