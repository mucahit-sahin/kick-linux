//! Application state, main window, and navigation between pages.

pub mod channel;
pub mod home;
pub mod player;

use crate::{api, config, fmt, mpv};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MediaKind {
    Live,
    Vod,
}

#[derive(Clone, Debug)]
pub struct Media {
    pub kind: MediaKind,
    pub title: String,
    pub subtitle: String,
    /// Master URL, regardless of the selected quality.
    pub master: String,
    /// Currently played URL (changes when the quality changes).
    pub url: String,
    pub duration: Option<f64>,
    pub start: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Loading,
    Playing,
    Ended,
}

struct SidebarRow {
    _row: gtk::ListBoxRow,
    dot: gtk::Label,
    info: gtk::Label,
}

pub struct App {
    pub window: adw::ApplicationWindow,
    pub toolbar: adw::ToolbarView,
    pub toast_overlay: adw::ToastOverlay,
    pub sidebar: gtk::Box,
    pub sidebar_revealer: gtk::Revealer,
    pub sidebar_list: gtk::ListBox,
    pub sidebar_title: gtk::Label,
    pub sidebar_empty: gtk::Label,
    pub search: gtk::SearchEntry,
    pub refresh_btn: gtk::Button,
    pub add_btn: gtk::Button,
    pub stack: gtk::Stack,
    pub home_box: gtk::Box,
    pub home_scroll: gtk::ScrolledWindow,
    pub channel_box: gtk::Box,
    pub channel_scroll: gtk::ScrolledWindow,
    pub player_page: player::PlayerPage,
    pub player: mpv::Player,
    pub language: RefCell<String>,

    pub cfg: RefCell<config::Config>,
    pub status: RefCell<HashMap<String, api::Channel>>,
    follow_views: RefCell<HashMap<String, SidebarRow>>,

    pub current_channel: RefCell<Option<api::Channel>>,
    pub current_vods: RefCell<Vec<api::Vod>>,
    pub vods_loaded: Cell<bool>,
    pub vods_error: RefCell<Option<String>>,

    pub current_media: RefCell<Option<Media>>,
    pub media_seq: Cell<u64>,
    channel_seq: Cell<u64>,
    return_page: Cell<&'static str>,
    refreshing: Cell<bool>,
    phase: Cell<Phase>,
    load_started: Cell<Option<Instant>>,
    resume_pending: Cell<Option<f64>>,
    live_catch_up: Cell<bool>,
}

impl App {
    pub fn new(gtk_app: &adw::Application, player: mpv::Player, cfg: config::Config) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(gtk_app)
            .title("Kick Lite")
            .default_width(1180)
            .default_height(740)
            .build();
        window.set_size_request(820, 520);
        window.set_icon_name(Some("io.github.kicklite.KickLite"));

        // --- top bar ---
        let header = adw::HeaderBar::new();
        let app_title = gtk::Label::new(Some("Kick Lite"));
        app_title.add_css_class("app-title");
        header.pack_start(&app_title);

        let sidebar_toggle = gtk::Button::from_icon_name("view-sidebar-symbolic");
        sidebar_toggle.set_tooltip_text(Some("Show/hide followed channels"));
        sidebar_toggle.set_has_frame(false);
        header.pack_start(&sidebar_toggle);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Channel name or Kick link…"));
        search.set_width_request(290);
        search.set_tooltip_text(Some(
            "Enter a channel name (e.g. xqc) or Kick URL and press Enter",
        ));

        let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some("Refresh followed channels"));
        refresh_btn.set_has_frame(false);

        let home_btn = gtk::Button::from_icon_name("go-home-symbolic");
        home_btn.set_tooltip_text(Some("Go to home"));
        home_btn.set_has_frame(false);

        let menu_model = gio::Menu::new();
        menu_model.append(Some("About / Hakkında"), Some("app.about"));
        menu_model.append(
            Some("Silence settings / Sessizlik ayarları"),
            Some("app.silence-settings"),
        );
        let language_menu = gio::Menu::new();
        language_menu.append(Some("Sistem dili / System"), Some("app.language-auto"));
        language_menu.append(Some("Türkçe"), Some("app.language-tr"));
        language_menu.append(Some("English"), Some("app.language-en"));
        menu_model.append_submenu(Some("Language / Dil"), &language_menu);
        menu_model.append(Some("Quit / Çıkış"), Some("app.quit"));
        let menu_btn = gtk::MenuButton::new();
        menu_btn.set_icon_name("open-menu-symbolic");
        menu_btn.set_tooltip_text(Some("Menu / Menü"));
        menu_btn.set_menu_model(Some(&menu_model));
        menu_btn.set_has_frame(false);

        header.pack_end(&menu_btn);
        header.pack_end(&refresh_btn);
        header.pack_start(&home_btn);
        header.pack_end(&search);

        // --- sidebar ---
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 4);
        sidebar.add_css_class("sidebar-panel");
        sidebar.set_width_request(238);
        // The "+" button in the header expands horizontally; we explicitly disable
        // the sidebar's horizontal expansion so it does not spread to parent boxes.
        sidebar.set_hexpand(false);

        let sidebar_head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        sidebar_head.set_margin_top(10);
        sidebar_head.set_margin_bottom(2);
        sidebar_head.set_margin_start(14);
        sidebar_head.set_margin_end(8);
        let sidebar_title = gtk::Label::new(Some("Following"));
        sidebar_title.add_css_class("sidebar-heading");
        sidebar_title.set_xalign(0.0);
        sidebar_title.set_hexpand(true);
        let add_btn = gtk::Button::from_icon_name("list-add-symbolic");
        add_btn.set_tooltip_text(Some("Add channel"));
        add_btn.set_has_frame(false);
        sidebar_head.append(&sidebar_title);
        sidebar_head.append(&add_btn);

        let sidebar_list = gtk::ListBox::new();
        sidebar_list.set_selection_mode(gtk::SelectionMode::None);
        sidebar_list.add_css_class("navigation-sidebar");
        let side_scroll = gtk::ScrolledWindow::new();
        side_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        side_scroll.set_vexpand(true);
        side_scroll.set_child(Some(&sidebar_list));

        let sidebar_empty = gtk::Label::new(Some("You are not following any channels yet.\nSearch for a channel above."));
        sidebar_empty.set_wrap(true);
        sidebar_empty.set_justify(gtk::Justification::Center);
        sidebar_empty.add_css_class("dim-label");
        sidebar_empty.add_css_class("caption");
        sidebar_empty.set_margin_start(18);
        sidebar_empty.set_margin_end(18);
        sidebar_empty.set_margin_top(18);
        sidebar_empty.set_visible(false);

        sidebar.append(&sidebar_head);
        sidebar.append(&side_scroll);
        sidebar.append(&sidebar_empty);

        // --- pages ---
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(120);

        let home_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
        home_box.set_margin_top(28);
        home_box.set_margin_bottom(28);
        home_box.set_margin_start(30);
        home_box.set_margin_end(30);
        let home_scroll = gtk::ScrolledWindow::new();
        home_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        home_scroll.set_child(Some(&home_box));
        home_scroll.set_vexpand(true);

        let channel_box = gtk::Box::new(gtk::Orientation::Vertical, 16);
        channel_box.set_margin_top(24);
        channel_box.set_margin_bottom(30);
        channel_box.set_margin_start(28);
        channel_box.set_margin_end(28);
        let channel_scroll = gtk::ScrolledWindow::new();
        channel_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        channel_scroll.set_child(Some(&channel_box));
        channel_scroll.set_vexpand(true);

        let player_page = player::PlayerPage::new();

        stack.add_named(&home_scroll, Some("home"));
        stack.add_named(&channel_scroll, Some("channel"));
        // The player is a persistent overlay; it does not close when switching
        // pages, it is only resized between full and mini size.
        let player_placeholder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        stack.add_named(&player_placeholder, Some("player"));
        stack.set_visible_child_name("home");

        let sidebar_revealer = gtk::Revealer::new();
        sidebar_revealer.set_transition_type(gtk::RevealerTransitionType::SlideRight);
        sidebar_revealer.set_reveal_child(true);
        let sidebar_shell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        sidebar_shell.append(&sidebar);
        sidebar_shell.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        sidebar_revealer.set_child(Some(&sidebar_shell));

        let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        content.append(&sidebar_revealer);
        let stack_overlay = gtk::Overlay::new();
        stack_overlay.set_hexpand(true);
        stack_overlay.set_vexpand(true);
        stack_overlay.set_child(Some(&stack));
        player_page.page.set_visible(false);
        stack_overlay.add_overlay(&player_page.page);
        content.append(&stack_overlay);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&content));

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&toolbar));
        window.set_content(Some(&toast_overlay));

        setup_actions(gtk_app, &window);

        let language = if cfg.language == "auto" || cfg.language.is_empty() {
            system_language()
        } else {
            cfg.language.clone()
        };
        let app = Rc::new(Self {
            window,
            toolbar,
            toast_overlay,
            sidebar,
            sidebar_revealer,
            sidebar_list,
            sidebar_title,
            sidebar_empty,
            search,
            refresh_btn,
            add_btn,
            stack,
            home_box,
            home_scroll,
            channel_box,
            channel_scroll,
            player_page,
            player,
            language: RefCell::new(language),
            cfg: RefCell::new(cfg),
            status: RefCell::new(HashMap::new()),
            follow_views: RefCell::new(HashMap::new()),
            current_channel: RefCell::new(None),
            current_vods: RefCell::new(Vec::new()),
            vods_loaded: Cell::new(false),
            vods_error: RefCell::new(None),
            current_media: RefCell::new(None),
            media_seq: Cell::new(0),
            channel_seq: Cell::new(0),
            return_page: Cell::new("home"),
            refreshing: Cell::new(false),
            phase: Cell::new(Phase::Idle),
            load_started: Cell::new(None),
            resume_pending: Cell::new(None),
            live_catch_up: Cell::new(false),
        });
        app.apply_language();

        {
            let a = app.clone();
            home_btn.connect_clicked(move |_| a.go_home());
        }

        app.clone().wire();
        install_language_actions(gtk_app, &app);
        install_silence_settings_action(gtk_app, &app);
        {
            let a = app.clone();
            sidebar_toggle.connect_clicked(move |_| {
                let visible = !a.sidebar_revealer.reveals_child();
                a.sidebar_revealer.set_reveal_child(visible);
            });
        }
        app.rebuild_sidebar();
        home::render(&app);
        app.present();
        app
    }

    pub fn is_english(&self) -> bool {
        self.language.borrow().as_str() == "en"
    }

    pub fn text<'a>(&self, tr: &'a str, en: &'a str) -> &'a str {
        if self.is_english() { en } else { tr }
    }

    fn change_language(self: &Rc<Self>, selected: &str) {
        let lang = if selected == "auto" { system_language() } else { selected.to_string() };
        *self.language.borrow_mut() = lang;
        self.cfg.borrow_mut().language = selected.to_string();
        let cfg = self.cfg.borrow().clone();
        let _ = config::save(&cfg);
        self.apply_language();
    }

    /// Window for editing the silence-detection thresholds.
    fn show_silence_settings(self: &Rc<Self>) {
        let cfg = self.cfg.borrow().clone();
        let dialog = adw::PreferencesDialog::builder()
            .title(self.text("Sessizlik algılama", "Silence detection"))
            .build();
        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::builder()
            .title(self.text("Yayın tekrarları", "Replays"))
            .description(self.text(
                "Bu eşikler yalnızca VOD'larda sessiz kısımları atlamak için kullanılır.",
                "These thresholds are only used for skipping silence in replays.",
            ))
            .build();

        let noise = adw::SpinRow::builder()
            .title(self.text("Ses eşiği (dB)", "Noise threshold (dB)"))
            .subtitle(self.text(
                "Altındaki ses düzeyleri sessiz sayılır",
                "Audio levels below this are treated as silence",
            ))
            .build();
        noise.set_range(-80.0, 0.0);
        noise.set_digits(0);
        noise.set_value(cfg.silence_noise_db as f64);

        let duration = adw::SpinRow::builder()
            .title(self.text("En kısa sessizlik (sn)", "Minimum silence (s)"))
            .subtitle(self.text(
                "Bu süreden kısa boşluklar atlanmaz",
                "Gaps shorter than this are ignored",
            ))
            .build();
        duration.set_range(0.1, 10.0);
        duration.set_digits(1);
        duration.set_value(cfg.silence_duration);

        // Save the setting and reapply the filter when the value changes. Use a
        // short delay to avoid changing the filter too often while the spin button
        // is held down.
        let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        for row in [noise.clone(), duration.clone()] {
            let a = self.clone();
            let pending = pending.clone();
            let noise = noise.clone();
            let duration = duration.clone();
            row.connect_value_notify(move |_| {
                {
                    let mut cfg = a.cfg.borrow_mut();
                    cfg.silence_noise_db = noise.value().round() as i32;
                    cfg.silence_duration = duration.value();
                }
                if let Some(id) = pending.borrow_mut().take() {
                    id.remove();
                }
                let a = a.clone();
                let id = glib::timeout_add_local(std::time::Duration::from_millis(350), move || {
                    let cfg = a.cfg.borrow().clone();
                    if let Err(err) =
                        a.player.apply_silence_filter(cfg.silence_noise_db, cfg.silence_duration)
                    {
                        eprintln!("Could not apply silence filter: {err}");
                    }
                    let _ = config::save(&cfg);
                    glib::ControlFlow::Break
                });
                pending.replace(Some(id));
            });
        }

        let reset = adw::ActionRow::builder()
            .title(self.text("Varsayılan değerler", "Default values"))
            .build();
        let reset_btn = gtk::Button::with_label(self.text("Sıfırla", "Reset"));
        reset_btn.set_valign(gtk::Align::Center);
        let defaults = config::Config::default();
        let noise_reset = noise.clone();
        let duration_reset = duration.clone();
        reset_btn.connect_clicked(move |_| {
            noise_reset.set_value(defaults.silence_noise_db as f64);
            duration_reset.set_value(defaults.silence_duration);
        });
        reset.add_suffix(&reset_btn);

        group.add(&noise);
        group.add(&duration);
        group.add(&reset);
        page.add(&group);
        dialog.add(&page);
        dialog.present(Some(&self.window));
    }

    fn apply_language(self: &Rc<Self>) {
        let en = self.is_english();
        self.window.set_title(Some(if en { "Kick Lite" } else { "Kick Lite" }));
        self.search.set_placeholder_text(Some(if en { "Channel name or Kick link…" } else { "Kanal adı veya Kick bağlantısı…" }));
        self.sidebar_title.set_text(if en { "Following" } else { "Takip edilenler" });
        self.sidebar_empty.set_text(if en { "You are not following any channels yet.\nSearch for a channel above." } else { "Henüz kanal takip etmiyorsun.\nÜstteki arama alanına bir kanal adı yaz." });
        self.refresh_btn.set_tooltip_text(Some(if en { "Refresh followed channels" } else { "Takip edilenleri yenile" }));
        self.add_btn.set_tooltip_text(Some(if en { "Add channel" } else { "Kanal ekle" }));
        self.search.set_tooltip_text(Some(if en { "Enter a channel name or Kick link" } else { "Kanal adı veya Kick bağlantısı gir" }));
        if let Some(header) = self.sidebar_title.parent().and_then(|p| p.parent()) {
            let _ = header;
        }
        self.sidebar_empty.set_text(if en {
            "You are not following any channels yet.\nSearch for a channel above."
        } else {
            "Henüz kanal takip etmiyorsun.\nÜstteki arama alanına bir kanal adı yaz."
        });
        self.player_page.set_language(en);
        self.rebuild_sidebar();
        home::render(self);
        if self.current_channel.borrow().is_some() {
            channel::render(self);
        }
    }

    pub fn present(self: &Rc<Self>) {
        self.window.present();
        // Refresh the followed list on startup.
        // (Independently of the timer in wire(), after the window is shown.)
        self.refresh_follows();
        self.refresh_resume_videos();
    }

    fn wire(self: &Rc<Self>) {
        self.player_page.wire(self.clone());

        // Search
        {
            let a = self.clone();
            self.search.connect_activate(move |entry| {
                let text = entry.text().to_string();
                a.handle_target(&text);
            });
        }

        // Refresh
        {
            let a = self.clone();
            self.refresh_btn.connect_clicked(move |_| {
                if a.stack.visible_child_name().as_deref() == Some("channel") {
                    a.refresh_channel();
                }
                a.refresh_follows();
            });
        }

        // Sidebar "+" button
        {
            let a = self.clone();
            self.add_btn.connect_clicked(move |_| {
                a.search.grab_focus();
            });
        }

        // Sidebar selection
        {
            let a = self.clone();
            self.sidebar_list.connect_row_activated(move |_, row| {
                let slug = row.widget_name().to_string();
                if !slug.is_empty() {
                    a.open_channel(slug, false);
                }
            });
        }

        // Keyboard shortcuts
        {
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            let a = self.clone();
            keys.connect_key_pressed(move |_, keyval, _code, _mods| {
                use gtk::gdk::Key;

                // In GTK4 the window has no "focus" property; querying it with property()
                // causes a runtime panic. Since the only text input in the app is the
                // SearchEntry, checking it directly is sufficient.
                if a.search.has_focus() {
                    return glib::Propagation::Proceed;
                }

                if keyval == Key::Escape {
                    if a.window.is_fullscreen() {
                        a.window.unfullscreen();
                        return glib::Propagation::Stop;
                    }
                    if a.stack.visible_child_name().as_deref() == Some("player") {
                        a.go_back();
                        return glib::Propagation::Stop;
                    }
                    return glib::Propagation::Proceed;
                }

                if a.stack.visible_child_name().as_deref() != Some("player") {
                    return glib::Propagation::Proceed;
                }
                if a.current_media.borrow().is_none() {
                    return glib::Propagation::Proceed;
                }

                if keyval == Key::space {
                    a.player.toggle_pause();
                    a.sync_pause_icon();
                    a.poke_controls();
                    return glib::Propagation::Stop;
                }
                if keyval == Key::f || keyval == Key::F {
                    a.toggle_fullscreen();
                    return glib::Propagation::Stop;
                }
                if keyval == Key::m || keyval == Key::M {
                    let muted = !a.player.is_muted();
                    a.player.set_mute(muted);
                    a.cfg.borrow_mut().muted = muted;
                    a.update_volume_icon();
                    a.poke_controls();
                    return glib::Propagation::Stop;
                }
                if keyval == Key::Left || keyval == Key::Right {
                    let is_vod = a
                        .current_media
                        .borrow()
                        .as_ref()
                        .map(|m| m.kind == MediaKind::Vod)
                        .unwrap_or(false);
                    if is_vod {
                        a.player.clear_silence();
                        a.player
                            .seek_relative(if keyval == Key::Left { -10.0 } else { 10.0 });
                        a.poke_controls();
                        return glib::Propagation::Stop;
                    }
                }
                if keyval == Key::Up || keyval == Key::Down {
                    let step = if keyval == Key::Up { 5.0 } else { -5.0 };
                    let v = (a.player_page.volume.value() + step).clamp(0.0, 100.0);
                    a.player_page.volume.set_value(v);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            self.window.add_controller(keys);
        }

        // Fullscreen changes
        {
            let a = self.clone();
            self.window
                .connect_notify_local(Some("fullscreened"), move |win, _| {
                    let fs = win.is_fullscreen();
                    a.toolbar.set_reveal_top_bars(!fs);
                    a.sidebar.set_visible(!fs);
                    a.player_page.fs.set_icon_name(if fs {
                        "view-restore-symbolic"
                    } else {
                        "view-fullscreen-symbolic"
                    });
                    a.poke_controls();
                });
        }

        // Shutdown: write settings, shut down mpv
        {
            let a = self.clone();
            self.window.connect_close_request(move |_| {
                a.save_current_position();
                let cfg = a.cfg.borrow().clone();
                let _ = config::save(&cfg);
                a.player.shutdown();
                // Timeouts hold the App with an Rc and can keep the main loop alive.
                // End the application when the last window closes.
                if let Some(app) = a.window.application() {
                    app.quit();
                }
                glib::Propagation::Proceed
            });
        }

        // Player state loop
        {
            let a = self.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                a.tick();
                glib::ControlFlow::Continue
            });
        }

        // Periodically refresh the live status of followed channels
        {
            let a = self.clone();
            glib::timeout_add_seconds_local(120, move || {
                if a.window.is_active() {
                    a.refresh_follows();
                }
                glib::ControlFlow::Continue
            });
        }
    }

    // ---- sidebar ----

    fn rebuild_sidebar(self: &Rc<Self>) {
        while let Some(row) = self.sidebar_list.row_at_index(0) {
            self.sidebar_list.remove(&row);
        }
        let followed = self.cfg.borrow().followed.clone();
        {
            let mut views = self.follow_views.borrow_mut();
            views.clear();
            for f in &followed {
                let row = gtk::ListBoxRow::new();
                row.set_widget_name(&f.slug);
                let h = gtk::Box::new(gtk::Orientation::Horizontal, 9);
                h.set_margin_top(7);
                h.set_margin_bottom(7);
                h.set_margin_start(10);
                h.set_margin_end(10);
                let dot = gtk::Label::new(Some("●"));
                dot.add_css_class("offline-dot");
                let v = gtk::Box::new(gtk::Orientation::Vertical, 1);
                v.set_hexpand(true);
                let name = gtk::Label::new(Some(&f.name));
                name.set_xalign(0.0);
                name.add_css_class("sidebar-name");
                name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                let info = gtk::Label::new(Some(self.text("çevrimdışı", "offline")));
                info.set_xalign(0.0);
                info.add_css_class("dim-label");
                info.add_css_class("caption");
                info.set_ellipsize(gtk::pango::EllipsizeMode::End);
                v.append(&name);
                v.append(&info);
                h.append(&dot);
                h.append(&v);
                row.set_child(Some(&h));
                self.sidebar_list.append(&row);
                views.insert(
                    f.slug.clone(),
                    SidebarRow {
                        _row: row,
                        dot,
                        info,
                    },
                );
            }
        }
        self.sidebar_empty.set_visible(followed.is_empty());
        self.update_sidebar_title();
        for f in &followed {
            if let Some(ch) = self.status.borrow().get(&f.slug).cloned() {
                self.update_sidebar_row(&ch);
            }
        }
    }

    fn update_sidebar_row(&self, ch: &api::Channel) {
        let views = self.follow_views.borrow();
        let Some(v) = views.get(&ch.slug) else {
            return;
        };
        if ch.is_live() {
            v.dot.remove_css_class("offline-dot");
            v.dot.add_css_class("live-dot");
            let text = if self.is_english() { format!("{} viewers", fmt::human_count(ch.viewers())) } else { format!("{} izleyici", fmt::human_count(ch.viewers())) };
            v.info.set_text(&text);
        } else {
            v.dot.remove_css_class("live-dot");
            v.dot.add_css_class("offline-dot");
            v.info.set_text(self.text("çevrimdışı", "offline"));
        }
    }

    fn update_sidebar_title(&self) {
        let cfg = self.cfg.borrow();
        if cfg.followed.is_empty() {
            self.sidebar_title.set_text(self.text("Takip edilenler", "Following"));
            return;
        }
        let status = self.status.borrow();
        let live = status
            .iter()
            .filter(|(slug, ch)| ch.is_live() && cfg.is_following(slug))
            .count();
        self.sidebar_title
            .set_text(&if self.is_english() { format!("Following · {live} live") } else { format!("Takip edilenler · {live} canlı") });
    }

    pub fn refresh_follows(self: &Rc<Self>) {
        if self.refreshing.replace(true) {
            return;
        }
        if self.stack.visible_child_name().as_deref() == Some("home") {
            home::render(self);
        }
        let slugs: Vec<String> = self
            .cfg
            .borrow()
            .followed
            .iter()
            .map(|f| f.slug.clone())
            .collect();
        if slugs.is_empty() {
            self.refreshing.set(false);
            if self.stack.visible_child_name().as_deref() == Some("home") {
                home::render(self);
            }
            return;
        }
        let a = self.clone();
        spawn_task(
            move || {
                let mut out = Vec::new();
                for slug in slugs {
                    out.push(api::channel(&slug).ok());
                }
                out
            },
            move |list| {
                for ch in list.into_iter().flatten() {
                    a.status.borrow_mut().insert(ch.slug.clone(), ch.clone());
                    a.update_sidebar_row(&ch);
                }
                a.refreshing.set(false);
                a.update_sidebar_title();
                home::render(&a);
            },
        );
    }

    /// Older versions stored only the URL and position. On startup we match
    /// them against the VOD lists of followed channels to build metadata for the
    /// home page.
    fn refresh_resume_videos(self: &Rc<Self>) {
        let followed = self.cfg.borrow().followed.clone();
        let positions = self.cfg.borrow().resume_positions.clone();
        if positions.is_empty() || followed.is_empty() {
            return;
        }
        let a = self.clone();
        spawn_task(
            move || {
                let mut found = Vec::new();
                for followed_channel in followed {
                    let Ok(vods) = api::vods(&followed_channel.slug) else {
                        continue;
                    };
                    for vod in vods {
                        let Some(url) = vod.url() else { continue };
                        let Some(position) = positions.get(url).copied() else { continue };
                        found.push(config::ResumeVideo {
                            url: url.to_string(),
                            title: vod.title().to_string(),
                            channel: followed_channel.name.clone(),
                            subtitle: vod_subtitle(&vod),
                            position,
                            duration: vod.duration_secs(),
                        });
                    }
                }
                found
            },
            move |found| {
                if found.is_empty() {
                    return;
                }
                let mut cfg = a.cfg.borrow_mut();
                for video in found {
                    if let Some(existing) = cfg
                        .resume_videos
                        .iter_mut()
                        .find(|existing| existing.url == video.url)
                    {
                        *existing = video;
                    } else {
                        cfg.resume_videos.push(video);
                    }
                }
                let snapshot = cfg.clone();
                drop(cfg);
                let _ = config::save(&snapshot);
                home::render(&a);
            },
        );
    }

    // ---- channel ----

    pub fn open_channel(self: &Rc<Self>, slug: String, follow: bool) {
        // When switching to another page, keep playing the current media and
        // preserve the mini view. When a new live/VOD is selected, play_media will
        // already stop the previous media.
        let move_to_mini = self.stack.visible_child_name().as_deref() == Some("player")
            && self.current_media.borrow().is_some();
        if self.current_media.borrow().is_none() {
            self.stop_player();
        } else if move_to_mini {
            self.show_mini_player();
        }
        self.channel_seq.set(self.channel_seq.get() + 1);
        let seq = self.channel_seq.get();
        self.current_channel.replace(None);
        self.current_vods.borrow_mut().clear();
        self.vods_loaded.set(false);
        self.vods_error.replace(None);
        self.stack.set_visible_child_name("channel");
        self.channel_scroll.vadjustment().set_value(0.0);
        channel::render_loading(self, &slug);
        self.window.set_title(Some("Kick Lite"));

        let a = self.clone();
        spawn_task(
            move || {
                let result = api::channel(&slug);
                let videos = match &result {
                    Ok(ch) if ch.vod_enabled => Some(api::vods(&ch.slug).map_err(|e| format!("{e:#}"))),
                    Ok(_) => Some(Ok(Vec::new())),
                    Err(_) => None,
                };
                (result.map_err(|e| format!("{e:#}")), videos)
            },
            move |(result, videos)| {
                if a.channel_seq.get() != seq {
                    return;
                }
                match result {
                    Ok(ch) => {
                        if follow {
                            let mut cfg = a.cfg.borrow_mut();
                            if cfg.add(&ch.slug, ch.display_name()) {
                                drop(cfg);
                                let snapshot = a.cfg.borrow().clone();
                                let _ = config::save(&snapshot);
                                a.rebuild_sidebar();
                                home::render(&a);
                            }
                        }
                        a.status.borrow_mut().insert(ch.slug.clone(), ch.clone());
                        a.update_sidebar_row(&ch);
                        a.update_sidebar_title();
                        a.current_channel.replace(Some(ch));
                        match videos {
                            Some(Ok(list)) => {
                                a.current_vods.replace(list);
                                a.vods_loaded.set(true);
                            }
                            Some(Err(err)) => {
                                a.vods_error.replace(Some(err));
                                a.vods_loaded.set(true);
                            }
                            None => {
                                a.vods_loaded.set(true);
                            }
                        }
                        channel::render(&a);
                    }
                    Err(err) => {
                        a.toast(&format!("{}: {err}", a.text("Kanal açılamadı", "Could not open channel")));
                        a.stack.set_visible_child_name("home");
                        home::render(&a);
                    }
                }
            },
        );
    }

    pub fn refresh_channel(self: &Rc<Self>) {
        let slug = self
            .current_channel
            .borrow()
            .as_ref()
            .map(|c| c.slug.clone());
        if let Some(slug) = slug {
            self.open_channel(slug, false);
        }
    }

    pub fn toggle_follow(self: &Rc<Self>) {
        let Some((slug, name)) = self
            .current_channel
            .borrow()
            .as_ref()
            .map(|c| (c.slug.clone(), c.display_name().to_string()))
        else {
            return;
        };
        let changed = {
            let mut cfg = self.cfg.borrow_mut();
            if cfg.is_following(&slug) {
                cfg.remove(&slug)
            } else {
                cfg.add(&slug, &name)
            }
        };
        if changed {
            let snapshot = self.cfg.borrow().clone();
            let _ = config::save(&snapshot);
            self.rebuild_sidebar();
            home::render(self);
            channel::render(self);
        }
    }

    // ---- playback ----

    pub fn play_live(self: &Rc<Self>) {
        let Some(ch) = self.current_channel.borrow().clone() else {
            return;
        };
        let slug = ch.slug.clone();
        let a = self.clone();
        spawn_task(
            move || api::channel(&slug),
            move |result| match result {
                Ok(fresh) => {
                    a.status.borrow_mut().insert(fresh.slug.clone(), fresh.clone());
                    a.update_sidebar_row(&fresh);
                    a.current_channel.replace(Some(fresh.clone()));
                    match fresh.live_url().map(|u| u.to_string()) {
                        Some(url) => {
                            let title = if fresh.stream_title().is_empty() {
                                fresh.display_name().to_string()
                            } else {
                                fresh.stream_title().to_string()
                            };
                            let media = Media {
                                kind: MediaKind::Live,
                                title,
                                subtitle: live_subtitle(&fresh),
                                master: url.clone(),
                                url,
                                duration: None,
                                start: 0.0,
                            };
                            a.play_media(media, "channel");
                        }
                        None => {
                            a.toast(a.text("Kanal şu anda canlı değil", "Channel is not live right now"));
                            channel::render(&a);
                        }
                    }
                }
                Err(err) => a.toast(&format!("{err:#}")),
            },
        );
    }

    pub fn play_vod(self: &Rc<Self>, vod: api::Vod) {
        if self.current_channel.borrow().is_none() {
            return;
        }
        let Some(url) = vod.url().map(|u| u.to_string()) else {
            self.toast(self.text("Bu tekrarın oynatma bağlantısı yok", "This replay has no playable URL"));
            return;
        };
        let start = self
            .cfg
            .borrow()
            .resume_positions
            .get(&url)
            .copied()
            .unwrap_or(0.0);
        let media = Media {
            kind: MediaKind::Vod,
            title: vod.title().to_string(),
            subtitle: vod_subtitle(&vod),
            master: url.clone(),
            url,
            duration: vod.duration_secs(),
            start,
        };
        self.play_media(media, "channel");
    }

    pub fn play_resume(self: &Rc<Self>, video: config::ResumeVideo) {
        let media = Media {
            kind: MediaKind::Vod,
            title: video.title,
            subtitle: video.subtitle,
            master: video.url.clone(),
            url: video.url,
            duration: video.duration,
            start: video.position,
        };
        self.play_media(media, "home");
    }

    pub fn play_media(self: &Rc<Self>, media: Media, return_page: &'static str) {
        self.stop_player();
        self.return_page.set(return_page);
        self.media_seq.set(self.media_seq.get() + 1);
        let seq = self.media_seq.get();
        self.phase.set(Phase::Loading);
        self.load_started.set(Some(Instant::now()));
        self.resume_pending.set(
            (media.kind == MediaKind::Vod && media.start > 0.5).then_some(media.start),
        );
        let catch_up = media.kind == MediaKind::Live;
        self.live_catch_up.set(catch_up);
        self.player.set_silence_allowed(media.kind == MediaKind::Vod);
        self.player.set_speed(if catch_up { 2.0 } else { 1.0 });
        self.player_page.speed_btn.set_label(if catch_up { "2x" } else { "1x" });

        self.player_page.set_media(&media);
        self.current_media.replace(Some(media.clone()));
        self.show_full_player();
        self.stack.set_visible_child_name("player");
        self.window
            .set_title(Some(&format!("{} — Kick Lite", fmt::shorten(&media.title, 48))));
        self.player_page.set_controls_visible(true);
        self.poke_controls();

        let start = if media.kind == MediaKind::Vod {
            Some(media.start)
        } else {
            None
        };
        if let Err(err) = self.player.load(&media.url, start) {
            self.phase.set(Phase::Ended);
            self.player_page.show_status(self.text("Yayın açılamadı", "Could not open stream"));
            self.toast(&format!("{err:#}"));
        }

        let master = media.master.clone();
        let a = self.clone();
        spawn_task(
            move || api::hls_variants(&master).unwrap_or_default(),
            move |variants| {
                if a.media_seq.get() != seq {
                    return;
                }
                a.player_page.set_variants(&a, variants);
            },
        );
    }

    pub fn switch_quality(self: &Rc<Self>, url: Option<String>, label: String) {
        let Some(mut media) = self.current_media.borrow().clone() else {
            return;
        };
        let pos = self.player.position().unwrap_or(0.0);
        // Keep the resume target while the replacement HLS stream is loading;
        // otherwise the load callback may start from zero after a quality switch.
        if media.kind == MediaKind::Vod && pos > 0.5 {
            self.resume_pending.set(Some(pos));
        }
        media.url = url.unwrap_or_else(|| media.master.clone());
        self.current_media.replace(Some(media.clone()));
        self.player_page.quality_btn.set_label(&label);
        self.player_page.quality_popover.popdown();
        self.phase.set(Phase::Loading);
        self.load_started.set(Some(Instant::now()));
        self.player_page.spinner.start();
        self.player_page.show_status(self.text("Kalite değiştiriliyor…", "Changing quality…"));
        let start = if media.kind == MediaKind::Vod {
            Some(pos.max(media.start))
        } else {
            None
        };
        if let Err(err) = self.player.load(&media.url, start) {
            self.toast(&format!("{err:#}"));
        }
        self.poke_controls();
    }

    pub fn stop_player(&self) {
        self.save_current_position();
        self.player.set_silence_allowed(false);
        self.player.stop();
        self.resume_pending.set(None);
        self.live_catch_up.set(false);
        self.current_media.replace(None);
        self.player_page.page.set_visible(false);
        self.media_seq.set(self.media_seq.get() + 1);
        self.phase.set(Phase::Idle);
        self.load_started.set(None);
        self.cancel_hide_controls();
        self.player_page.set_controls_visible(true);
        self.player_page.hide_status();
        self.window.set_title(Some("Kick Lite"));
    }

    pub fn go_back(self: &Rc<Self>) {
        let page = self.return_page.get();
        if self.window.is_fullscreen() {
            self.window.unfullscreen();
        }
        self.stack.set_visible_child_name(page);
        self.cancel_hide_controls();
        if self.current_media.borrow().is_some() {
            self.show_mini_player();
        }
        self.home_scroll.vadjustment().set_value(0.0);
        self.channel_scroll.vadjustment().set_value(0.0);
        match page {
            "channel" if self.current_channel.borrow().is_some() => channel::render(self),
            _ => home::render(self),
        }
    }

    /// Returns to the home page without stopping playback; the mini player stays visible if present.
    pub fn go_home(self: &Rc<Self>) {
        if self.window.is_fullscreen() {
            self.window.unfullscreen();
        }
        self.return_page.set("home");
        self.stack.set_visible_child_name("home");
        self.home_scroll.vadjustment().set_value(0.0);
        self.cancel_hide_controls();
        if self.current_media.borrow().is_some() {
            self.show_mini_player();
        }
        home::render(self);
    }

    /// Moves the video back to the main player area when the mini player is clicked.
    pub fn resume_player(self: &Rc<Self>) {
        let Some(media) = self.current_media.borrow().clone() else {
            self.player_page.page.set_visible(false);
            return;
        };
        self.show_full_player();
        self.stack.set_visible_child_name("player");
        self.player_page.set_controls_visible(true);
        self.window
            .set_title(Some(&format!("{} — Kick Lite", fmt::shorten(&media.title, 48))));
        self.poke_controls();
    }

    fn show_full_player(&self) {
        self.player_page.page.remove_css_class("mini-player");
        self.player_page.set_mini_click_enabled(false);
        self.player_page.mini_close.set_visible(false);
        self.player_page.page.set_margin_end(0);
        self.player_page.page.set_margin_bottom(0);
        self.player_page.page.set_visible(true);
        self.player_page.page.set_halign(gtk::Align::Fill);
        self.player_page.page.set_valign(gtk::Align::Fill);
        self.player_page.page.set_hexpand(true);
        self.player_page.page.set_vexpand(true);
        self.player_page.page.set_size_request(-1, -1);
    }

    fn show_mini_player(&self) {
        self.player_page.page.add_css_class("mini-player");
        self.player_page.set_mini_click_enabled(true);
        self.player_page.mini_close.set_visible(true);
        self.player_page.page.set_margin_end(18);
        self.player_page.page.set_margin_bottom(18);
        self.player_page.set_controls_visible(false);
        self.player_page.page.set_visible(true);
        self.player_page.page.set_halign(gtk::Align::End);
        self.player_page.page.set_valign(gtk::Align::End);
        self.player_page.page.set_hexpand(false);
        self.player_page.page.set_vexpand(false);
        self.player_page.page.set_size_request(320, 180);
    }

    pub fn toggle_fullscreen(&self) {
        if self.window.is_fullscreen() {
            self.window.unfullscreen();
        } else {
            self.window.fullscreen();
        }
    }

    pub fn sync_pause_icon(&self) {
        self.player_page.set_paused(self.player.is_paused());
    }

    /// Reduces latency by consuming the live buffer at 2x speed.
    pub fn start_live_catch_up(&self) {
        let is_live = self
            .current_media
            .borrow()
            .as_ref()
            .is_some_and(|media| media.kind == MediaKind::Live);
        if !is_live {
            return;
        }
        self.player.set_speed(2.0);
        self.player_page.speed_btn.set_label("2x");
        self.live_catch_up.set(true);
        self.toast(self.text("Gecikme azaltılıyor; canlı yayın 2x hızda oynatılıyor", "Reducing latency; playing the live stream at 2x"));
    }

    pub fn update_volume_icon(&self) {
        let muted = self.player.is_muted();
        let v = self.player_page.volume.value();
        let icon = if muted || v <= 0.5 {
            "audio-volume-muted-symbolic"
        } else if v < 34.0 {
            "audio-volume-low-symbolic"
        } else if v < 67.0 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        self.player_page.vol_btn.set_icon_name(icon);
    }

    pub fn poke_controls(self: &Rc<Self>) {
        self.player_page.set_controls_visible(true);
        self.schedule_hide_controls();
    }

    pub fn schedule_hide_controls(self: &Rc<Self>) {
        self.cancel_hide_controls();
        if self.stack.visible_child_name().as_deref() != Some("player") {
            return;
        }
        if self.player.is_paused()
            || self.player_page.quality_popover.is_visible()
            || self.current_media.borrow().is_none()
        {
            return;
        }
        let a = self.clone();
        let id = glib::timeout_add_seconds_local(3, move || {
            a.player_page.hide_source.replace(None);
            if a.stack.visible_child_name().as_deref() == Some("player")
                && !a.player.is_paused()
                && !a.player_page.quality_popover.is_visible()
            {
                a.player_page.set_controls_visible(false);
            }
            glib::ControlFlow::Break
        });
        self.player_page.hide_source.replace(Some(id));
    }

    pub fn cancel_hide_controls(&self) {
        if let Some(id) = self.player_page.hide_source.replace(None) {
            id.remove();
        }
    }

    // ---- search ----

    pub fn handle_target(self: &Rc<Self>, text: &str) {
        let Some(target) = api::parse_target(text) else {
            self.toast(self.text("Kanal adı ya da kick.com bağlantısı girin (ör. xqc)", "Enter a channel name or kick.com URL (e.g. xqc)"));
            return;
        };
        match target {
            api::Target::Channel(slug) => {
                self.search.set_text("");
                self.open_channel(slug, true);
            }
            api::Target::Vod { channel, video } => {
                self.search.set_text("");
                let a = self.clone();
                let lookup = channel.clone();
                spawn_task(
                    move || api::find_vod(&lookup, &video),
                    move |vod| match vod {
                        Ok(vod) => match vod.url().map(|u| u.to_string()) {
                            Some(url) => {
                                let start = a
                                    .cfg
                                    .borrow()
                                    .resume_positions
                                    .get(&url)
                                    .copied()
                                    .unwrap_or(0.0);
                                let media = Media {
                                    kind: MediaKind::Vod,
                                    title: vod.title().to_string(),
                                    subtitle: vod_subtitle(&vod),
                                    master: url.clone(),
                                    url,
                                    duration: vod.duration_secs(),
                                    start,
                                };
                                a.play_media(media, "home");
                            }
                            None => a.toast(a.text("Bu tekrarın oynatma bağlantısı yok", "This replay has no playable URL")),
                        },
                        Err(err) => a.toast(&format!("{err:#}")),
                    },
                );
            }
        }
    }

    pub fn toast(&self, message: &str) {
        let toast = adw::Toast::new(message);
        toast.set_timeout(5);
        self.toast_overlay.add_toast(toast);
    }

    // ---- player loop ----

    fn tick(self: &Rc<Self>) {
        // Always drain mpv events (silencedetect reports keep arriving even while
        // the mini player is active). Since silence skipping is implemented as a
        // speed change, it is triggered here.
        self.player.poll_events();

        if self.stack.visible_child_name().as_deref() != Some("player") {
            return;
        }
        let Some(media) = self.current_media.borrow().clone() else {
            return;
        };
        let live = media.kind == MediaKind::Live;

        match self.phase.get() {
            Phase::Loading => {
                if self.player.video_height() > 0
                    || self.player.position().unwrap_or(0.0) > 0.2
                {
                    if let Some(position) = self.resume_pending.take() {
                        self.player.seek(position, true);
                    }
                    self.phase.set(Phase::Playing);
                    self.player_page.hide_status();
                } else if let Some(started) = self.load_started.get() {
                    if started.elapsed().as_secs_f64() > 12.0 && self.player.is_idle() {
                        self.phase.set(Phase::Ended);
                        self.player_page.spinner.stop();
                        self.player_page.show_status(if live {
                            if self.is_english() { "Could not connect to stream or stream ended" } else { "Yayına bağlanılamadı ya da yayın sona erdi" }
                        } else {
                            if self.is_english() { "Could not open stream" } else { "Yayın açılamadı" }
                        });
                        self.poke_controls();
                    }
                }
            }
            Phase::Playing if self.player.is_idle() => {
                self.phase.set(Phase::Ended);
                self.player.clear_silence();
                self.player_page.spinner.stop();
                self.player_page.show_status(if live {
                    if self.is_english() { "Live stream ended" } else { "Canlı yayın sona erdi" }
                } else {
                    if self.is_english() { "Video ended" } else { "Yayın sona erdi" }
                });
                self.cancel_hide_controls();
                self.player_page.set_controls_visible(true);
            }
            _ => {}
        }


        self.sync_pause_icon();

        if live {
            if self.phase.get() == Phase::Playing
                && self.live_catch_up.get()
                && self.player.cache_duration().is_some_and(|seconds| seconds <= 0.5)
            {
                self.player.set_speed(1.0);
                self.player_page.speed_btn.set_label("1x");
                self.live_catch_up.set(false);
                self.toast(self.text("Canlı yayın güncel noktaya ulaştı; hız normale döndü", "Live stream caught up; playback speed restored"));
            }
            return;
        }

        if let Some(duration) = self.player.duration() {
            if duration > 0.0 {
                self.player_page.seek.adjustment().set_upper(duration);
                self.player_page.dur.set_text(&fmt::duration(duration));
            }
        }

        if let Some(target) = self.player_page.pending_seek.get() {
            let waited = self
                .player_page
                .last_user_change
                .get()
                .map(|t| t.elapsed().as_millis())
                .unwrap_or(u128::MAX);
            if waited >= 350 {
                self.player_page.pending_seek.set(None);
                self.player.clear_silence();
                self.player.seek(target, true);
                self.player_page.seek.set_value(target);
                self.player_page.pos.set_text(&fmt::duration(target));
            }
        } else if let Some(pos) = self.player.position() {
            self.player_page.pos.set_text(&fmt::duration(pos));
            self.player_page.seek.set_value(pos);
            if media.kind == MediaKind::Vod {
                self.cfg
                    .borrow_mut()
                    .resume_positions
                    .insert(media.master.clone(), pos);
            }
        }
        self.save_current_position();
    }

    fn save_current_position(&self) {
        let Some(media) = self.current_media.borrow().clone() else {
            return;
        };
        if media.kind != MediaKind::Vod {
            return;
        }
        let Some(pos) = self.player.position() else {
            return;
        };
        let duration = media.duration.or_else(|| self.player.duration());
        let mut cfg = self.cfg.borrow_mut();
        if duration.is_some_and(|d| pos >= d - 5.0) {
            cfg.resume_positions.remove(&media.master);
            cfg.resume_videos.retain(|video| video.url != media.master);
        } else if pos > 1.0 {
            cfg.resume_positions.insert(media.master.clone(), pos);
            if let Some(video) = cfg
                .resume_videos
                .iter_mut()
                .find(|video| video.url == media.master)
            {
                video.position = pos;
                video.duration = duration;
            } else {
                cfg.resume_videos.push(config::ResumeVideo {
                    url: media.master,
                    title: media.title,
                    channel: self
                        .current_channel
                        .borrow()
                        .as_ref()
                        .map(|channel| channel.display_name().to_string())
                        .unwrap_or_default(),
                    subtitle: media.subtitle,
                    position: pos,
                    duration,
                });
            }
        }
    }
}

fn setup_actions(gtk_app: &adw::Application, window: &adw::ApplicationWindow) {
    let about = gio::SimpleAction::new("about", None);
    let win = window.clone();
    about.connect_activate(move |_, _| {
        let dialog = adw::AboutDialog::builder()
            .application_name("Kick Lite")
            .application_icon("io.github.kicklite.KickLite")
            .developer_name("Kick Lite")
            .version(env!("CARGO_PKG_VERSION"))
            .comments(
                "A lightweight Kick.com livestream and replay player.\n\
                 Uses Kick's public, unofficial API.",
            )
            .website("https://kick.com")
            .license_type(gtk::License::MitX11)
            .build();
        dialog.present(Some(&win));
    });
    gtk_app.add_action(&about);

    let quit = gio::SimpleAction::new("quit", None);
    let app = gtk_app.clone();
    quit.connect_activate(move |_, _| app.quit());
    gtk_app.add_action(&quit);
    let _ = gtk_app;
}

fn install_silence_settings_action(gtk_app: &adw::Application, app: &Rc<App>) {
    let action = gio::SimpleAction::new("silence-settings", None);
    let a = app.clone();
    action.connect_activate(move |_, _| a.show_silence_settings());
    gtk_app.add_action(&action);
}

fn install_language_actions(gtk_app: &adw::Application, app: &Rc<App>) {
    for (suffix, choice) in [("auto", "auto"), ("tr", "tr"), ("en", "en")] {
        let action = gio::SimpleAction::new(&format!("language-{suffix}"), None);
        let app_ref = app.clone();
        action.connect_activate(move |_, _| {
            app_ref.change_language(choice);
        });
        gtk_app.add_action(&action);
    }
}

fn system_language() -> String {
    let locale = std::env::var("LANG").unwrap_or_default().to_lowercase();
    if locale.starts_with("en") { "en".into() } else { "tr".into() }
}

pub fn clear_container(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

pub fn spawn_task<T, F, G>(work: F, done: G)
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
    G: FnOnce(T) + 'static,
{
    let (tx, rx) = async_channel::bounded::<T>(1);
    std::thread::spawn(move || {
        let value = work();
        let _ = tx.send_blocking(value);
    });
    glib::spawn_future_local(async move {
        if let Ok(value) = rx.recv().await {
            done(value);
        }
    });
}

pub fn live_subtitle(ch: &api::Channel) -> String {
    let mut parts: Vec<String> = Vec::new();
    let category = ch.category();
    if !category.is_empty() {
        parts.push(category.to_string());
    }
    parts.push(format!("{} viewers", fmt::human_count(ch.viewers())));
    parts.join(" · ")
}

pub fn vod_subtitle(vod: &api::Vod) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(created) = vod.created_at.as_deref() {
        parts.push(fmt::relative_time(created));
    }
    if let Some(duration) = vod.duration_secs() {
        parts.push(fmt::duration(duration));
    }
    if vod.view_count() > 0 {
        parts.push(format!("{} views", fmt::human_count(vod.view_count())));
    }
    parts.join(" · ")
}
