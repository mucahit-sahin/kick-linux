//! Video page: mpv rendering on a GLArea and the overlaid controls.

use crate::api;
use crate::config;
use crate::fmt;
use crate::mpv;
use crate::ui::{clear_container, App, Media, MediaKind};
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

pub struct PlayerPage {
    /// Stable handle for this page's player. It is never repointed: the mpv
    /// render context belongs to this player and this page's GLArea.
    pub player: mpv::SharedPlayer,
    pub page: gtk::Overlay,
    pub area: gtk::GLArea,
    pub controls: gtk::Box,
    pub top: gtk::Box,
    pub bottom: gtk::Box,
    pub back: gtk::Button,
    pub title: gtk::Label,
    pub subtitle: gtk::Label,
    pub live_badge: gtk::Label,
    pub play: gtk::Button,
    pub seek: gtk::Scale,
    pub seek_box: gtk::Box,
    pub live_controls_spacer: gtk::Box,
    pub pos: gtk::Label,
    pub dur: gtk::Label,
    pub vol_btn: gtk::Button,
    pub volume: gtk::Scale,
    pub quality_btn: gtk::MenuButton,
    pub quality_popover: gtk::Popover,
    pub quality_box: gtk::Box,
    pub speed_btn: gtk::MenuButton,
    pub speed_popover: gtk::Popover,
    pub speed_box: gtk::Box,
    pub skip_silence: gtk::ToggleButton,
    /// Opens the silence-detection settings (threshold and live level meter).
    pub silence_btn: gtk::Button,
    pub fs: gtk::Button,
    pub mini_close: gtk::Button,
    pub mini_click_area: gtk::DrawingArea,
    pub center: gtk::Box,
    pub spinner: gtk::Spinner,
    pub status_label: gtk::Label,
    /// Pending target when the user changes the progress bar, before it is sent to mpv.
    /// These are `Rc<Cell<_>>` so the `change_value` callback and the player tick
    /// share the same cell. A plain `Cell<T>: Clone` copies its contents instead
    /// of sharing them, which silently dropped every user seek.
    pub pending_seek: Rc<Cell<Option<f64>>>,
    pub last_user_change: Rc<Cell<Option<Instant>>>,
    /// Generation counter for delayed control hiding. A timeout is ignored if
    /// another one (or a page swap) has advanced the counter since it was set.
    /// Unlike removing a `SourceId`, this can never fail or panic.
    pub hide_generation: Cell<u64>,
    pub controls_visible: Cell<bool>,
    pub paused_icon: Cell<bool>,
    pub english: Cell<bool>,
}

impl PlayerPage {
    pub fn new(player: mpv::SharedPlayer) -> Self {
        let area = gtk::GLArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);
        // When mpv has a new frame, queue_render() is called via the render callback.
        // A continuous GTK render loop was keeping the CPU busy unnecessarily;
        // only draw when a new frame actually arrives.
        area.set_auto_render(false);
        area.set_has_depth_buffer(false);
        area.set_has_stencil_buffer(false);
        area.add_css_class("video-area");

        let page = gtk::Overlay::new();
        page.set_hexpand(true);
        page.set_vexpand(true);
        page.set_child(Some(&area));

        // --- top bar ---
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        top.add_css_class("video-topbar");
        top.set_valign(gtk::Align::Start);
        top.set_can_target(true);

        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        back.set_tooltip_text(Some("Back (Esc)"));
        back.set_has_frame(false);
        back.add_css_class("video-button");

        let title = gtk::Label::new(None);
        title.add_css_class("video-title");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let subtitle = gtk::Label::new(None);
        subtitle.add_css_class("video-subtitle");
        subtitle.set_xalign(0.0);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let titles = gtk::Box::new(gtk::Orientation::Vertical, 1);
        titles.append(&title);
        titles.append(&subtitle);
        titles.set_hexpand(true);
        titles.set_valign(gtk::Align::Center);

        let live_badge = gtk::Label::new(Some("LIVE"));
        live_badge.add_css_class("live-badge");
        live_badge.set_can_target(true);
        live_badge.set_tooltip_text(Some("Reduce latency by playing at 2x"));
        live_badge.set_visible(false);

        top.append(&back);
        top.append(&titles);
        top.append(&live_badge);

        // --- bottom bar ---
        // The seek bar spans the whole row. In live streams the position seek bar
        // is hidden, so play/pause and the actions sit at the two ends.
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bottom.add_css_class("video-bottombar");
        bottom.set_valign(gtk::Align::End);
        bottom.set_halign(gtk::Align::Fill);
        bottom.set_hexpand(true);
        bottom.set_can_target(true);

        let play = gtk::Button::from_icon_name("media-playback-start-symbolic");
        play.set_tooltip_text(Some("Play / Pause (Space)"));
        play.set_has_frame(false);
        play.add_css_class("video-button");

        let pos = gtk::Label::new(Some("0:00"));
        pos.add_css_class("video-time");
        pos.set_width_chars(5);
        pos.set_xalign(1.0);

        let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
        seek.set_draw_value(false);
        seek.set_hexpand(true);
        seek.set_sensitive(false);
        seek.add_css_class("video-scale");
        seek.set_tooltip_text(Some("Playback position"));

        let dur = gtk::Label::new(Some("0:00"));
        dur.add_css_class("video-time");
        dur.set_width_chars(5);
        dur.set_xalign(0.0);

        let seek_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        seek_box.append(&pos);
        seek_box.append(&seek);
        seek_box.append(&dur);
        seek_box.set_hexpand(true);
        seek_box.set_halign(gtk::Align::Fill);

        let vol_btn = gtk::Button::from_icon_name("audio-volume-high-symbolic");
        vol_btn.set_tooltip_text(Some("Mute / unmute (M)"));
        vol_btn.set_has_frame(false);
        vol_btn.add_css_class("video-button");

        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        volume.set_draw_value(false);
        volume.set_width_request(72);
        volume.set_tooltip_text(Some("Volume"));
        volume.add_css_class("video-scale");

        let quality_popover = gtk::Popover::new();
        let quality_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        quality_box.set_margin_top(6);
        quality_box.set_margin_bottom(6);
        quality_box.set_margin_start(6);
        quality_box.set_margin_end(6);
        quality_popover.set_child(Some(&quality_box));

        let quality_btn = gtk::MenuButton::new();
        quality_btn.set_label("Quality");
        quality_btn.set_popover(Some(&quality_popover));
        quality_btn.set_tooltip_text(Some("Stream quality"));
        quality_btn.set_has_frame(false);
        quality_btn.add_css_class("video-button");

        let speed_popover = gtk::Popover::new();
        let speed_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        speed_box.set_margin_top(6);
        speed_box.set_margin_bottom(6);
        speed_box.set_margin_start(6);
        speed_box.set_margin_end(6);
        speed_popover.set_child(Some(&speed_box));
        let speed_btn = gtk::MenuButton::new();
        speed_btn.set_label("Speed");
        speed_btn.set_popover(Some(&speed_popover));
        speed_btn.set_tooltip_text(Some("Playback speed"));
        speed_btn.set_has_frame(false);
        speed_btn.add_css_class("video-button");

        let skip_silence = gtk::ToggleButton::with_label("Skip silence");
        skip_silence.set_has_frame(false);
        skip_silence.add_css_class("video-button");
        skip_silence.add_css_class("video-toggle");
        skip_silence.set_tooltip_text(Some("Skip silent parts in replays"));

        let silence_btn = gtk::Button::from_icon_name("preferences-system-symbolic");
        silence_btn.set_tooltip_text(Some("Silence settings"));
        silence_btn.set_has_frame(false);
        silence_btn.add_css_class("video-button");

        let fs = gtk::Button::from_icon_name("view-fullscreen-symbolic");
        fs.set_tooltip_text(Some("Fullscreen (F)"));
        fs.set_has_frame(false);
        fs.add_css_class("video-button");

        let mini_close = gtk::Button::from_icon_name("window-close-symbolic");
        mini_close.set_tooltip_text(Some("Close stream / video"));
        mini_close.set_has_frame(false);
        mini_close.add_css_class("mini-close");
        mini_close.set_halign(gtk::Align::End);
        mini_close.set_valign(gtk::Align::Start);
        mini_close.set_margin_top(6);
        mini_close.set_margin_end(6);
        mini_close.set_visible(true);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.append(&vol_btn);
        actions.append(&volume);
        actions.append(&skip_silence);
        actions.append(&silence_btn);
        actions.append(&speed_btn);
        actions.append(&quality_btn);
        actions.append(&fs);

        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bar.set_hexpand(true);
        bar.set_halign(gtk::Align::Fill);
        let live_controls_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        live_controls_spacer.set_hexpand(true);
        live_controls_spacer.set_visible(false);
        live_controls_spacer.set_can_target(false);
        bar.append(&play);
        bar.append(&live_controls_spacer);
        bar.append(&seek_box);
        bar.append(&actions);
        bottom.append(&bar);

        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        spacer.set_can_target(false);

        let controls = gtk::Box::new(gtk::Orientation::Vertical, 0);
        controls.set_valign(gtk::Align::Fill);
        controls.set_halign(gtk::Align::Fill);
        controls.set_vexpand(true);
        controls.set_hexpand(true);
        // The control bars must be able to receive events. The spacer between them
        // still does not, so the video gesture can play/pause in the empty area.
        controls.set_can_target(true);
        controls.append(&top);
        controls.append(&spacer);
        controls.append(&bottom);

        // --- central status indicator ---
        let center = gtk::Box::new(gtk::Orientation::Vertical, 10);
        center.set_halign(gtk::Align::Center);
        center.set_valign(gtk::Align::Center);
        center.add_css_class("video-status");
        center.set_visible(false);
        let spinner = gtk::Spinner::new();
        let status_label = gtk::Label::new(None);
        status_label.add_css_class("video-status-label");
        center.append(&spinner);
        center.append(&status_label);

        // The GLArea may not receive click events in some compositor/GTK states.
        // An overlay covering the whole mini-player surface catches the enlarge click.
        let mini_click_area = gtk::DrawingArea::new();
        mini_click_area.set_hexpand(true);
        mini_click_area.set_vexpand(true);
        mini_click_area.set_can_target(true);
        mini_click_area.set_visible(false);
        page.add_overlay(&controls);
        page.add_overlay(&center);
        page.add_overlay(&mini_click_area);
        page.add_overlay(&mini_close);

        Self {
            player,
            page,
            area,
            controls,
            top,
            bottom,
            back,
            title,
            subtitle,
            live_badge,
            play,
            seek,
            seek_box,
            live_controls_spacer,
            pos,
            dur,
            vol_btn,
            volume,
            quality_btn,
            quality_popover,
            quality_box,
            speed_btn,
            speed_popover,
            speed_box,
            skip_silence,
            silence_btn,
            fs,
            mini_close,
            mini_click_area,
            center,
            spinner,
            status_label,
            pending_seek: Rc::new(Cell::new(None)),
            last_user_change: Rc::new(Cell::new(None)),
            hide_generation: Cell::new(0),
            controls_visible: Cell::new(true),
            paused_icon: Cell::new(false),
            english: Cell::new(false),
        }
    }

    /// Each page is wired to its own stable player handle, so its render
    /// context always matches the GLArea it draws into.
    pub fn wire(self: &Rc<Self>, app: Rc<App>) {
        let player = self.player.clone();
        let weak_page = Rc::downgrade(self);
        let volume = self.volume.clone();
        let pending_seek = self.pending_seek.clone();
        let last_user_change = self.last_user_change.clone();

        // GLArea lifecycle: this page always renders with the handle it was
        // wired to, so its mpv render context keeps its GL context.
        {
            let p = player.clone();
            let a = app.clone();
            self.area.connect_realize(move |area| {
                if !p.attach(area) {
                    a.toast("Could not start video output (OpenGL)");
                } else {
                    area.queue_render();
                }
            });
        }
        {
            let p = player.clone();
            self.area.connect_render(move |area, _ctx| {
                if !p.attach(area) {
                    return glib::Propagation::Stop;
                }
                p.render(area);
                glib::Propagation::Proceed
            });
        }
        {
            let p = player.clone();
            self.area.connect_unrealize(move |_| p.detach());
        }

        // Clicking a mini tile promotes its stream into the full slot.
        {
            let a = app.clone();
            let weak = weak_page.clone();
            let click = gtk::GestureClick::new();
            click.set_button(1);
            click.connect_pressed(move |_, _, _, _| {
                if let Some(page) = weak.upgrade() {
                    a.activate_page(&page);
                }
            });
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            self.mini_click_area.add_controller(click);
        }
        // Click video: play/pause, double-click: fullscreen
        {
            let a = app.clone();
            let p = player.clone();
            let weak = weak_page.clone();
            let click = gtk::GestureClick::new();
            click.set_button(1);
            click.connect_pressed(move |_gesture, n_press, _x, _y| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                if page.page.has_css_class("mini-player")
                    || a.stack.visible_child_name().as_deref() != Some("player")
                {
                    a.activate_page(&page);
                    return;
                }
                if n_press == 2 {
                    a.toggle_fullscreen();
                } else {
                    p.toggle_pause();
                    a.sync_pause_icon();
                    a.poke_controls();
                }
            });
            self.area.add_controller(click);
        }
        // In live streams, adjust volume with vertical scroll over the video area.
        {
            let a = app.clone();
            let weak = weak_page.clone();
            let volume = volume.clone();
            let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
            scroll.connect_scroll(move |_, _dx, dy| {
                if weak
                    .upgrade()
                    .is_some_and(|page| page.page.has_css_class("mini-player"))
                {
                    return glib::Propagation::Proceed;
                }
                let is_live = a
                    .current_media
                    .borrow()
                    .as_ref()
                    .is_some_and(|media| media.kind == MediaKind::Live);
                if !is_live {
                    return glib::Propagation::Proceed;
                }
                let step = if dy < 0.0 { 5.0 } else { -5.0 };
                let value = (volume.value() + step).clamp(0.0, 100.0);
                volume.set_value(value);
                a.poke_controls();
                glib::Propagation::Stop
            });
            self.area.add_controller(scroll);
        }

        // Mouse motion: show controls
        {
            let a = app.clone();
            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion(move |_, _, _| {
                if a.stack.visible_child_name().as_deref() == Some("player") {
                    a.poke_controls();
                }
            });
            let a = app.clone();
            motion.connect_leave(move |_| {
                if a.stack.visible_child_name().as_deref() == Some("player") {
                    a.schedule_hide_controls();
                }
            });
            self.page.add_controller(motion);
        }

        // Top bar
        {
            let a = app.clone();
            self.back.connect_clicked(move |_| a.go_back());
        }
        {
            let a = app.clone();
            let click = gtk::GestureClick::new();
            click.set_button(1);
            click.connect_pressed(move |_, _, _, _| a.start_live_catch_up());
            self.live_badge.add_controller(click);
        }

        // Bottom bar
        {
            let a = app.clone();
            let p = player.clone();
            self.play.connect_clicked(move |_| {
                p.toggle_pause();
                a.sync_pause_icon();
                a.poke_controls();
            });
        }
        {
            let a = app.clone();
            self.seek.connect_change_value(move |_scale, _scroll, value| {
                if a.current_media.borrow().is_some() {
                    pending_seek.set(Some(value));
                    last_user_change.set(Some(Instant::now()));
                }
                glib::Propagation::Proceed
            });
        }
        {
            let a = app.clone();
            let p = player.clone();
            self.volume.connect_value_changed(move |scale| {
                let v = scale.value();
                p.set_volume(v);
                p.set_mute(false);
                {
                    let mut cfg = a.cfg.borrow_mut();
                    cfg.volume = v;
                    cfg.muted = false;
                }
                a.update_volume_icon();
            });
        }
        {
            let a = app.clone();
            let p = player.clone();
            let weak = weak_page.clone();
            self.vol_btn.connect_clicked(move |_| {
                let muted = !p.is_muted();
                p.set_mute(muted);
                let en = a.is_english();
                if let Some(page) = weak.upgrade() {
                    page.vol_btn.set_tooltip_text(Some(if muted {
                        if en { "Unmute (M)" } else { "Sesi aç (M)" }
                    } else if en {
                        "Mute / unmute (M)"
                    } else {
                        "Sesi aç / kapat (M)"
                    }));
                }
                a.cfg.borrow_mut().muted = muted;
                a.update_volume_icon();
                a.poke_controls();
            });
        }
        {
            let a = app.clone();
            self.fs.connect_clicked(move |_| a.toggle_fullscreen());
        }
        {
            let a = app.clone();
            let weak = weak_page.clone();
            self.mini_close.connect_clicked(move |_| {
                if let Some(page) = weak.upgrade() {
                    a.close_page(&page);
                }
            });
        }

        // Do not hide controls while the quality menu is open.
        {
            let a = app.clone();
            self.quality_popover
                .connect_visible_notify(move |popover| {
                    if popover.is_visible() {
                        a.cancel_hide_controls();
                    } else {
                        a.schedule_hide_controls();
                    }
                });
        }

        // Silence skipping: the on/off state is stored in settings.
        {
            let initial = app.cfg.borrow().skip_silence;
            player.set_skip_silence(initial);
            self.skip_silence.set_active(initial);
            let p = player.clone();
            let a = app.clone();
            self.skip_silence.connect_toggled(move |button| {
                let enabled = button.is_active();
                p.set_skip_silence(enabled);
                a.cfg.borrow_mut().skip_silence = enabled;
                let cfg = a.cfg.borrow().clone();
                let _ = config::save(&cfg);
            });
        }

        // The settings button reuses the same dialog as the app menu, but can be
        // reached directly while watching a replay.
        {
            let a = app.clone();
            self.silence_btn.connect_clicked(move |_| a.show_silence_settings());
        }

        // Initial volume settings
        {
            let cfg = app.cfg.borrow().clone();
            player.set_volume(cfg.volume);
            player.set_mute(cfg.muted);
            self.volume.set_value(cfg.volume);
            app.update_volume_icon();
        }
        self.set_controls_visible(true);
        self.set_speed_options(&player);
    }

    /// Shows/hides the controls (with a smooth transition).
    pub fn set_controls_visible(&self, visible: bool) {
        self.controls_visible.set(visible);
        self.controls.set_opacity(if visible { 1.0 } else { 0.0 });
        self.top.set_can_target(visible);
        self.bottom.set_can_target(visible);
    }

    pub fn set_language(&self, english: bool) {
        self.english.set(english);
        self.back.set_tooltip_text(Some(if english { "Back (Esc)" } else { "Geri (Esc)" }));
        self.play.set_tooltip_text(Some(if english { "Play / Pause (Space)" } else { "Oynat / Duraklat (Boşluk)" }));
        self.live_badge.set_text(if english { "LIVE" } else { "CANLI" });
        self.live_badge.set_tooltip_text(Some(if english { "Reduce latency by playing at 2x" } else { "Gecikmeyi azaltmak için 2x hızda oynat" }));
        self.seek.set_tooltip_text(Some(if english { "Playback position" } else { "Yayın konumu" }));
        self.vol_btn.set_tooltip_text(Some(if english { "Mute / unmute (M)" } else { "Sesi aç / kapat (M)" }));
        self.volume.set_tooltip_text(Some(if english { "Volume" } else { "Ses düzeyi" }));
        self.quality_btn.set_tooltip_text(Some(if english { "Stream quality" } else { "Yayın kalitesi" }));
        self.speed_btn.set_tooltip_text(Some(if english { "Playback speed" } else { "Oynatma hızı" }));
        self.skip_silence.set_label(if english { "Skip silence" } else { "Sessiz atla" });
        self.skip_silence.set_tooltip_text(Some(if english { "Skip silent parts in replays" } else { "Yayın tekrarlarında sessiz kısımları atla" }));
        self.silence_btn.set_tooltip_text(Some(if english { "Silence settings" } else { "Sessizlik ayarları" }));
        self.fs.set_tooltip_text(Some(if english { "Fullscreen (F)" } else { "Tam ekran (F)" }));
        self.mini_close.set_tooltip_text(Some(if english { "Close stream / video" } else { "Yayını / videoyu kapat" }));
        self.quality_btn.set_label(if english { "Quality" } else { "Kalite" });
        self.speed_btn.set_label(if english { "Speed" } else { "Hız" });
        if self.status_label.is_visible() {
            let status = self.status_label.text();
            let translated = match status.as_str() {
                "Canlı yayına bağlanılıyor…" => if english { "Connecting to live stream…" } else { "Canlı yayına bağlanılıyor…" },
                "Yayın yükleniyor…" => if english { "Loading video…" } else { "Yayın yükleniyor…" },
                "Yayın açılamadı" => if english { "Could not open stream" } else { "Yayın açılamadı" },
                "Kalite değiştiriliyor…" => if english { "Changing quality…" } else { "Kalite değiştiriliyor…" },
                _ => return,
            };
            self.status_label.set_text(translated);
        }
    }

    /// Updates the metadata shown in the player without starting a loading
    /// state. Used when a running stream is swapped in from a mini player.
    pub fn set_media_info(&self, media: &Media) {
        self.title.set_text(&media.title);
        self.subtitle.set_text(&media.subtitle);
        self.subtitle.set_visible(!media.subtitle.is_empty());
        let live = media.kind == MediaKind::Live;
        self.live_badge.set_visible(live);
        self.seek_box.set_visible(!live);
        self.live_controls_spacer.set_visible(live);
        self.seek.set_sensitive(!live);
        // Silence skipping only makes sense in replays (VODs).
        self.skip_silence.set_visible(!live);
        self.silence_btn.set_visible(!live);
        self.pos.set_text("0:00");
        self.dur
            .set_text(&media.duration.map(fmt::duration).unwrap_or_default());
        let upper = media.duration.unwrap_or(1.0).max(1.0);
        self.seek.adjustment().set_upper(upper);
        self.seek.set_value(0.0);
        self.pending_seek.set(None);
        self.quality_btn.set_label(if self.english.get() { "Quality" } else { "Kalite" });
        clear_container(&self.quality_box);
    }

    pub fn set_media(&self, media: &Media) {
        self.set_media_info(media);
        self.spinner.start();
        self.show_status(if media.kind == MediaKind::Live {
            if self.english.get() { "Connecting to live stream…" } else { "Canlı yayına bağlanılıyor…" }
        } else if self.english.get() { "Loading video…" } else { "Yayın yükleniyor…" });
    }

    pub fn set_mini_click_enabled(&self, enabled: bool) {
        self.mini_click_area.set_visible(enabled);
    }

    pub fn set_variants(&self, app: &Rc<App>, variants: Vec<api::Variant>) {
        clear_container(&self.quality_box);

        let auto = gtk::Button::with_label(app.text("Otomatik (en iyi)", "Auto (best)"));
        auto.set_has_frame(false);
        auto.set_halign(gtk::Align::Fill);
        auto.add_css_class("flat");
        {
            let a = app.clone();
            let auto_label = app.text("Kalite", "Quality").to_string();
            auto.connect_clicked(move |_| a.switch_quality(None, auto_label.clone()));
        }
        self.quality_box.append(&auto);

        if variants.is_empty() {
            let message = if self.quality_btn.is_sensitive() {
                app.text("Kalite seçeneği yok", "No quality options")
            } else {
                app.text("Çevrimdışı indirme", "Offline download")
            };
            let label = gtk::Label::new(Some(message));
            label.add_css_class("dim-label");
            label.set_margin_top(4);
            label.set_margin_bottom(4);
            self.quality_box.append(&label);
            return;
        }

        for variant in variants {
            let label = variant.label();
            let button = gtk::Button::with_label(&label);
            button.set_has_frame(false);
            button.set_halign(gtk::Align::Fill);
            button.add_css_class("flat");
            button.set_width_request(120);
            let url = variant.url.clone();
            let display = label.clone();
            let a = app.clone();
            button.connect_clicked(move |_| a.switch_quality(Some(url.clone()), display.clone()));
            self.quality_box.append(&button);
        }
    }

    pub fn set_speed_options(&self, player: &mpv::SharedPlayer) {
        clear_container(&self.speed_box);
        for speed in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0] {
            let label = if speed == 1.0 { "1x".to_string() } else { format!("{speed}x") };
            let button = gtk::Button::with_label(&label);
            button.set_has_frame(false);
            button.set_halign(gtk::Align::Fill);
            button.add_css_class("flat");
            let p = player.clone();
            let selected = label.clone();
            let speed_btn = self.speed_btn.clone();
            let speed_popover = self.speed_popover.clone();
            button.connect_clicked(move |_| {
                p.set_speed(speed);
                speed_btn.set_label(&selected);
                speed_popover.popdown();
            });
            self.speed_box.append(&button);
        }
    }

    pub fn set_paused(&self, paused: bool) {
        if self.paused_icon.get() == paused {
            return;
        }
        self.paused_icon.set(paused);
        self.play.set_icon_name(if paused {
            "media-playback-start-symbolic"
        } else {
            "media-playback-pause-symbolic"
        });
        self.play.set_tooltip_text(Some(if paused { "Play (Space)" } else { "Pause (Space)" }));
    }

    pub fn show_status(&self, text: &str) {
        self.status_label.set_text(text);
        self.status_label.set_visible(!text.is_empty());
        self.center.set_visible(!text.is_empty());
    }

    pub fn hide_status(&self) {
        self.spinner.stop();
        self.center.set_visible(false);
    }
}
