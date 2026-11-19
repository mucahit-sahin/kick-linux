//! Application state, main window, and navigation between pages.

pub mod channel;
pub mod downloads;
pub mod home;
pub mod multi;
pub mod player;

use crate::{api, config, download, fmt, mpv};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MediaKind {
    Live,
    Vod,
}

#[derive(Clone, Debug)]
pub struct Media {
    pub kind: MediaKind,
    /// Channel slug, used to avoid opening the same stream twice in multi-view.
    pub slug: Option<String>,
    pub title: String,
    pub subtitle: String,
    /// Master URL, regardless of the selected quality.
    pub master: String,
    /// Currently played URL (changes when the quality changes).
    pub url: String,
    pub duration: Option<f64>,
    pub start: f64,
    /// Set when playing an offline download; the position is stored on the
    /// DownloadItem instead of the online resume list.
    pub download_id: Option<String>,
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

#[derive(Clone)]
pub struct ActiveDownload {
    pub title: String,
    pub done: usize,
    pub total: usize,
    pub bytes: u64,
    pub cancel: Arc<AtomicBool>,
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
    pub downloads_btn: gtk::Button,
    pub add_btn: gtk::Button,
    menu_btn: gtk::MenuButton,
    pub stack: gtk::Stack,
    stack_overlay: gtk::Overlay,
    pub home_box: gtk::Box,
    pub home_scroll: gtk::ScrolledWindow,
    pub channel_box: gtk::Box,
    pub channel_scroll: gtk::ScrolledWindow,
    pub downloads_box: gtk::Box,
    pub downloads_scroll: gtk::ScrolledWindow,
    /// The page currently shown full-screen. Swapping a mini player replaces
    /// this reference, so all player UI helpers follow the focused stream.
    pub player_page: RefCell<Rc<player::PlayerPage>>,
    /// Swappable handle for the focused stream.
    pub player: mpv::SharedPlayer,
    /// Extra players for simultaneous streams (multi-view).
    pub mini_players: RefCell<Vec<Rc<multi::MiniPlayer>>>,
    mini_seq: Cell<u64>,
    pub language: RefCell<String>,

    pub cfg: RefCell<config::Config>,
    /// Rolling RMS history used to derive the automatic silence threshold.
    auto_levels: RefCell<Vec<f64>>,
    /// Last threshold derived from the audio; `None` until enough samples exist.
    auto_db: Cell<Option<i32>>,
    last_auto_apply: Cell<Option<Instant>>,
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
    pub active_downloads: RefCell<HashMap<String, ActiveDownload>>,
    download_indicator: gtk::Button,
    download_indicator_label: gtk::Label,
    download_indicator_spinner: gtk::Spinner,
}

/// Builds the three-dot menu with labels in the selected UI language.
fn build_menu(english: bool) -> gio::Menu {
    let model = gio::Menu::new();
    model.append(
        Some(if english { "About" } else { "Hakkında" }),
        Some("app.about"),
    );
    model.append(
        Some(if english {
            "Silence settings"
        } else {
            "Sessizlik ayarları"
        }),
        Some("app.silence-settings"),
    );
    model.append(
        Some(if english {
            "Close mini players"
        } else {
            "Küçük oynatıcıları kapat"
        }),
        Some("app.close-mini-players"),
    );

    let language_menu = gio::Menu::new();
    language_menu.append(
        Some(if english {
            "System default"
        } else {
            "Sistem dili"
        }),
        Some("app.language-auto"),
    );
    language_menu.append(Some("Türkçe"), Some("app.language-tr"));
    language_menu.append(Some("English"), Some("app.language-en"));
    model.append_submenu(
        Some(if english { "Language" } else { "Dil" }),
        &language_menu,
    );

    model.append(
        Some(if english { "Quit" } else { "Çıkış" }),
        Some("app.quit"),
    );
    model
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

        let downloads_btn = gtk::Button::from_icon_name("folder-download-symbolic");
        downloads_btn.set_tooltip_text(Some("Downloads"));
        downloads_btn.set_has_frame(false);

        let home_btn = gtk::Button::from_icon_name("go-home-symbolic");
        home_btn.set_tooltip_text(Some("Go to home"));
        home_btn.set_has_frame(false);

        let language = if cfg.language == "auto" || cfg.language.is_empty() {
            system_language()
        } else {
            cfg.language.clone()
        };
        let en = language == "en";

        let menu_btn = gtk::MenuButton::new();
        menu_btn.set_icon_name("open-menu-symbolic");
        menu_btn.set_tooltip_text(Some(if en { "Menu" } else { "Menü" }));
        menu_btn.set_menu_model(Some(&build_menu(en)));
        menu_btn.set_has_frame(false);

        // Shown only while a replay is being downloaded. Clicking it returns
        // to the home page, where the Downloads section shows the progress.
        let download_indicator = gtk::Button::new();
        download_indicator.add_css_class("flat");
        download_indicator.set_visible(false);
        let indicator_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        indicator_content.set_margin_start(8);
        indicator_content.set_margin_end(8);
        let download_indicator_spinner = gtk::Spinner::new();
        let download_indicator_label = gtk::Label::new(None);
        download_indicator_label.add_css_class("dim-label");
        indicator_content.append(&download_indicator_spinner);
        indicator_content.append(&download_indicator_label);
        download_indicator.set_child(Some(&indicator_content));

        header.pack_end(&menu_btn);
        header.pack_end(&refresh_btn);
        header.pack_end(&download_indicator);
        header.pack_end(&downloads_btn);
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

        let downloads_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
        downloads_box.set_margin_top(28);
        downloads_box.set_margin_bottom(28);
        downloads_box.set_margin_start(30);
        downloads_box.set_margin_end(30);
        let downloads_scroll = gtk::ScrolledWindow::new();
        downloads_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        downloads_scroll.set_child(Some(&downloads_box));
        downloads_scroll.set_vexpand(true);

        let page_player = mpv::SharedPlayer::new(player);
        // The app's player handle can be repointed when another page takes over
        // the full-screen slot; the page keeps its own stable handle.
        let app_player = page_player.fork();
        let player_page = Rc::new(player::PlayerPage::new(page_player));

        stack.add_named(&home_scroll, Some("home"));
        stack.add_named(&channel_scroll, Some("channel"));
        stack.add_named(&downloads_scroll, Some("downloads"));
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
        // Every stream's page is a sibling overlay. Swapping streams only
        // changes which page has the full layout and which are mini tiles.
        stack_overlay.add_overlay(&player_page.page);
        content.append(&stack_overlay);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&content));

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&toolbar));
        window.set_content(Some(&toast_overlay));

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
            downloads_btn: downloads_btn.clone(),
            add_btn,
            menu_btn,
            stack,
            stack_overlay,
            home_box,
            home_scroll,
            channel_box,
            channel_scroll,
            downloads_box,
            downloads_scroll,
            player_page: RefCell::new(player_page),
            player: app_player,
            mini_players: RefCell::new(Vec::new()),
            mini_seq: Cell::new(0),
            language: RefCell::new(language),
            cfg: RefCell::new(cfg),
            auto_levels: RefCell::new(Vec::new()),
            auto_db: Cell::new(None),
            last_auto_apply: Cell::new(None),
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
            active_downloads: RefCell::new(HashMap::new()),
            download_indicator: download_indicator.clone(),
            download_indicator_label,
            download_indicator_spinner,
        });
        download::cleanup(&app.cfg.borrow().downloads);
        app.apply_language();

        {
            let a = app.clone();
            home_btn.connect_clicked(move |_| a.go_home());
        }
        {
            let a = app.clone();
            download_indicator.connect_clicked(move |_| a.go_downloads());
        }
        {
            let a = app.clone();
            downloads_btn.connect_clicked(move |_| a.go_downloads());
        }

        app.clone().wire();
        setup_actions(gtk_app, &app);
        install_language_actions(gtk_app, &app);
        install_silence_settings_action(gtk_app, &app);
        install_multi_actions(gtk_app, &app);
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

    /// The page currently shown in the full player slot.
    fn page(&self) -> Rc<player::PlayerPage> {
        self.player_page.borrow().clone()
    }

    /// Whether at least one stream (full or mini) is currently playing.
    pub fn has_active_stream(&self) -> bool {
        self.current_media.borrow().is_some() || !self.mini_players.borrow().is_empty()
    }

    /// Rebuilds the visible page so controls that depend on an active stream
    /// (e.g. "open in mini player") appear or disappear immediately.
    fn refresh_stream_actions(self: &Rc<Self>) {
        match self.stack.visible_child_name().as_deref() {
            Some("player") | None => {}
            Some("channel") => channel::render(self),
            Some("downloads") => downloads::render(self),
            _ => home::render(self),
        }
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

        let auto = adw::SwitchRow::builder()
            .title(self.text("Otomatik eşik", "Automatic threshold"))
            .subtitle(self.text(
                "Ses düzeyini analiz ederek eşiği kendiliğinden belirler",
                "Derives the threshold by analyzing the audio levels",
            ))
            .active(cfg.auto_silence)
            .build();

        // AdwSpinRow varsayılan adjustment'ında step-increment 0 olduğundan
        // +/− düğmeleri çalışmaz. Adım ve sayfa artışını açıkça tanımlıyoruz.
        let noise_adjustment = gtk::Adjustment::new(
            cfg.silence_noise_db as f64,
            -80.0,
            0.0,
            1.0,   // step increment
            10.0,  // page increment
            0.0,
        );
        let noise = adw::SpinRow::builder()
            .adjustment(&noise_adjustment)
            .title(self.text("Ses eşiği (dB)", "Noise threshold (dB)"))
            .subtitle(self.text(
                "Altındaki ses düzeyleri sessiz sayılır",
                "Audio levels below this are treated as silence",
            ))
            .build();
        noise.set_digits(0);
        noise.set_climb_rate(1.0);
        noise.set_numeric(true);
        noise.set_sensitive(!cfg.auto_silence);
        {
            let a = self.clone();
            let noise = noise.clone();
            auto.connect_active_notify(move |row| {
                noise.set_sensitive(!row.is_active());
                a.set_auto_silence(row.is_active());
            });
        }

        let duration_adjustment = gtk::Adjustment::new(
            cfg.silence_duration,
            0.1,
            10.0,
            0.1,  // step increment
            1.0,  // page increment
            0.0,
        );
        let duration = adw::SpinRow::builder()
            .adjustment(&duration_adjustment)
            .title(self.text("En kısa sessizlik (sn)", "Minimum silence (s)"))
            .subtitle(self.text(
                "Bu süreden kısa boşluklar atlanmaz",
                "Gaps shorter than this are ignored",
            ))
            .build();
        duration.set_digits(1);
        duration.set_climb_rate(0.1);
        duration.set_numeric(true);

        // Save the setting and reapply the filter when the value changes. Use a
        // short delay to avoid changing the filter too often while the spin button
        // is held down. A generation counter keeps only the newest change; calling
        // SourceId::remove() on an already-fired timeout panics, so the old
        // timeout is simply left to expire and skips its work.
        let generation = Rc::new(Cell::new(0u64));
        for row in [noise.clone(), duration.clone()] {
            let a = self.clone();
            let generation = generation.clone();
            let noise = noise.clone();
            let duration = duration.clone();
            row.connect_value_notify(move |_| {
                {
                    let mut cfg = a.cfg.borrow_mut();
                    cfg.silence_noise_db = noise.value().round() as i32;
                    cfg.silence_duration = duration.value();
                }
                generation.set(generation.get() + 1);
                let my_generation = generation.get();
                let a = a.clone();
                let generation = generation.clone();
                glib::timeout_add_local(std::time::Duration::from_millis(350), move || {
                    if generation.get() == my_generation {
                        let cfg = a.cfg.borrow().clone();
                        if let Err(err) = a.player.apply_silence_filter(
                            a.effective_silence_db(),
                            cfg.silence_duration,
                        ) {
                            eprintln!("Could not apply silence filter: {err}");
                        }
                        let _ = config::save(&cfg);
                    }
                    glib::ControlFlow::Break
                });
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

        group.add(&auto);
        group.add(&noise);
        group.add(&duration);
        group.add(&reset);
        page.add(&group);

        // Live level meter: a scrolling dB history with the silence threshold
        // drawn on top, so the user can see at which level silence is detected.
        const METER_MIN_DB: f64 = -80.0;
        const METER_MAX_DB: f64 = 0.0;
        const METER_SAMPLES: usize = 120;

        let meter_group = adw::PreferencesGroup::builder()
            .title(self.text("Anlık ses seviyesi", "Live audio level"))
            .description(self.text(
                "Turuncu çizgi sessizlik eşiğini gösterir; eşiği bu grafiğe göre ayarla.",
                "The orange line marks the silence threshold; adjust it using this graph.",
            ))
            .build();

        let meter_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        meter_box.set_margin_top(6);
        meter_box.set_margin_bottom(6);

        let now_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let now_caption = gtk::Label::new(Some(self.text("Şu an:", "Now:")));
        now_caption.add_css_class("dim-label");
        let level_label = gtk::Label::new(Some("—"));
        level_label.add_css_class("title-2");
        level_label.set_xalign(0.0);
        let state_label = gtk::Label::new(None);
        state_label.add_css_class("dim-label");
        let auto_label = gtk::Label::new(None);
        auto_label.add_css_class("dim-label");
        now_row.append(&now_caption);
        now_row.append(&level_label);
        now_row.append(&state_label);
        now_row.append(&auto_label);
        meter_box.append(&now_row);

        let samples = Rc::new(RefCell::new(vec![METER_MIN_DB; METER_SAMPLES]));
        let meter = gtk::DrawingArea::new();
        meter.set_height_request(110);
        meter.set_hexpand(true);
        {
            let samples = samples.clone();
            let a = self.clone();
            meter.set_draw_func(move |widget, cr, w, h| {
                let fg = widget.color();
                let (fr, fgc, fb) = (fg.red() as f64, fg.green() as f64, fg.blue() as f64);
                let to_y = |db: f64| {
                    let db = if db.is_finite() {
                        db.clamp(METER_MIN_DB, METER_MAX_DB)
                    } else {
                        METER_MIN_DB
                    };
                    h as f64 - (db - METER_MIN_DB) / (METER_MAX_DB - METER_MIN_DB) * h as f64
                };

                cr.set_source_rgba(fr, fgc, fb, 0.06);
                cr.rectangle(0.0, 0.0, w as f64, h as f64);
                let _ = cr.fill();

                // dB grid
                cr.set_font_size(9.0);
                for db in [0.0_f64, -20.0, -40.0, -60.0, -80.0] {
                    let y = to_y(db);
                    cr.set_source_rgba(fr, fgc, fb, 0.14);
                    cr.set_line_width(1.0);
                    cr.move_to(0.0, y);
                    cr.line_to(w as f64, y);
                    let _ = cr.stroke();
                    cr.set_source_rgba(fr, fgc, fb, 0.55);
                    cr.move_to(4.0, (y - 3.0).max(10.0));
                    let _ = cr.show_text(&format!("{db:.0}"));
                }

                let threshold = a.effective_silence_db() as f64;
                let ty = to_y(threshold);

                // Region below the threshold is treated as silence.
                cr.set_source_rgba(0.90, 0.25, 0.30, 0.13);
                cr.rectangle(0.0, ty, w as f64, h as f64 - ty);
                let _ = cr.fill();

                // Filled waveform of recent levels.
                let history = samples.borrow();
                let step = w as f64 / (history.len() - 1) as f64;
                cr.move_to(0.0, h as f64);
                for (i, db) in history.iter().enumerate() {
                    cr.line_to(i as f64 * step, to_y(*db));
                }
                cr.line_to(w as f64, h as f64);
                cr.close_path();
                cr.set_source_rgba(0.15, 0.72, 0.45, 0.28);
                let _ = cr.fill();

                cr.move_to(0.0, to_y(history[0]));
                for (i, db) in history.iter().enumerate() {
                    cr.line_to(i as f64 * step, to_y(*db));
                }
                cr.set_source_rgba(0.15, 0.72, 0.45, 0.95);
                cr.set_line_width(1.5);
                let _ = cr.stroke();

                let _ = cr.set_dash(&[4.0, 3.0], 0.0);
                cr.set_source_rgba(0.95, 0.60, 0.15, 0.95);
                cr.set_line_width(1.5);
                cr.move_to(0.0, ty);
                cr.line_to(w as f64, ty);
                let _ = cr.stroke();
                let _ = cr.set_dash(&[], 0.0);
            });
        }
        meter_box.append(&meter);
        meter_group.add(&meter_box);
        page.add(&meter_group);

        // Poll the level while the dialog is open; the timeout is removed on close.
        let meter_timer = Rc::new(Cell::new(None::<glib::SourceId>));
        dialog.connect_closed({
            let timer = meter_timer.clone();
            move |_| {
                if let Some(id) = timer.take() {
                    id.remove();
                }
            }
        });
        {
            let a = self.clone();
            let history = samples.clone();
            let level_label = level_label.clone();
            let state_label = state_label.clone();
            let auto_label = auto_label.clone();
            let meter = meter.clone();
            let id = glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                let db = a.player.audio_level_db();
                {
                    let mut history = history.borrow_mut();
                    history.pop();
                    history.insert(0, db.unwrap_or(f64::NEG_INFINITY));
                }
                let effective = a.effective_silence_db();
                let silent = db.is_some_and(|v| v < effective as f64);
                level_label.set_text(&match db {
                    Some(v) if v.is_finite() => format!("{v:.1} dB"),
                    Some(_) => "-∞ dB".to_string(),
                    None => "—".to_string(),
                });
                state_label.set_text(if db.is_some() && silent {
                    a.text("· sessiz", "· silent")
                } else {
                    ""
                });
                let auto_text = if a.cfg.borrow().auto_silence {
                    match a.auto_db.get() {
                        Some(db) => format!("· otomatik: {db} dB"),
                        None => a
                            .text("· otomatik: analiz ediliyor…", "· auto: analyzing…")
                            .to_string(),
                    }
                } else {
                    String::new()
                };
                auto_label.set_text(&auto_text);
                meter.queue_draw();
                glib::ControlFlow::Continue
            });
            meter_timer.set(Some(id));
        }

        dialog.add(&page);
        dialog.present(Some(&self.window));
    }

    fn apply_language(self: &Rc<Self>) {
        let en = self.is_english();
        crate::set_messages_locale(&self.cfg.borrow().language);
        self.window.set_title(Some(if en { "Kick Lite" } else { "Kick Lite" }));
        self.search.set_placeholder_text(Some(if en { "Channel name or Kick link…" } else { "Kanal adı veya Kick bağlantısı…" }));
        self.sidebar_title.set_text(if en { "Following" } else { "Takip edilenler" });
        self.sidebar_empty.set_text(if en { "You are not following any channels yet.\nSearch for a channel above." } else { "Henüz kanal takip etmiyorsun.\nÜstteki arama alanına bir kanal adı yaz." });
        self.refresh_btn.set_tooltip_text(Some(if en { "Refresh followed channels" } else { "Takip edilenleri yenile" }));
        self.add_btn.set_tooltip_text(Some(if en { "Add channel" } else { "Kanal ekle" }));
        self.search.set_tooltip_text(Some(if en { "Enter a channel name or Kick link" } else { "Kanal adı veya Kick bağlantısı gir" }));
        self.menu_btn.set_tooltip_text(Some(if en { "Menu" } else { "Menü" }));
        // Rebuild the model after the current event finishes: the language
        // action is invoked from inside the open popover, where GTK may ignore
        // a model swap. Dropping and resetting the model forces a fresh popover.
        let menu_btn = self.menu_btn.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(0), move || {
            menu_btn.set_menu_model(None::<&gio::Menu>);
            menu_btn.set_menu_model(Some(&build_menu(en)));
            glib::ControlFlow::Break
        });
        self.download_indicator.set_tooltip_text(Some(if en {
            "Download in progress — click to view"
        } else {
            "İndirme devam ediyor — görüntülemek için tıkla"
        }));
        self.update_download_indicator();
        if let Some(header) = self.sidebar_title.parent().and_then(|p| p.parent()) {
            let _ = header;
        }
        self.sidebar_empty.set_text(if en {
            "You are not following any channels yet.\nSearch for a channel above."
        } else {
            "Henüz kanal takip etmiyorsun.\nÜstteki arama alanına bir kanal adı yaz."
        });
        self.page().set_language(en);
        for mini in self.mini_players.borrow().iter() {
            mini.page.borrow().set_language(en);
        }
        self.downloads_btn.set_tooltip_text(Some(if en { "Downloads" } else { "İndirilenler" }));
        self.rebuild_sidebar();
        home::render(self);
        downloads::render(self);
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
        self.page().wire(self.clone());

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
                    let v = (a.page().volume.value() + step).clamp(0.0, 100.0);
                    a.page().volume.set_value(v);
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
                    a.page().fs.set_icon_name(if fs {
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
                a.drain_minis();
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

    /// Followed channels ordered for display: live channels first (highest viewer
    /// count first), then offline channels alphabetically by name.
    fn sorted_followed(&self) -> Vec<config::Followed> {
        let cfg = self.cfg.borrow();
        let status = self.status.borrow();
        let mut followed = cfg.followed.clone();
        followed.sort_by(|a, b| {
            let ca = status.get(&a.slug);
            let cb = status.get(&b.slug);
            let la = ca.is_some_and(|c| c.is_live());
            let lb = cb.is_some_and(|c| c.is_live());
            match (la, lb) {
                (true, true) => {
                    let va = ca.map(|c| c.viewers()).unwrap_or(0);
                    let vb = cb.map(|c| c.viewers()).unwrap_or(0);
                    vb.cmp(&va).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                }
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                (false, false) => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });
        followed
    }

    /// Reorder the existing sidebar rows to match [`Self::sorted_followed`]
    /// without recreating their widgets (keeps focus and selection intact).
    fn reorder_sidebar(self: &Rc<Self>) {
        let slugs: Vec<String> = self.sorted_followed().into_iter().map(|f| f.slug).collect();
        let views = self.follow_views.borrow();
        for v in views.values() {
            self.sidebar_list.remove(&v._row);
        }
        for (index, slug) in slugs.iter().enumerate() {
            if let Some(v) = views.get(slug) {
                self.sidebar_list.insert(&v._row, index as i32);
            }
        }
    }

    fn rebuild_sidebar(self: &Rc<Self>) {
        while let Some(row) = self.sidebar_list.row_at_index(0) {
            self.sidebar_list.remove(&row);
        }
        let followed = self.sorted_followed();
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
                a.reorder_sidebar();
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
                        a.reorder_sidebar();
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

    // ---- downloads ----

    /// Fetches the available qualities of a replay and lets the user pick one.
    pub fn download_vod(self: &Rc<Self>, vod: api::Vod) {
        let Some(master) = vod.url().map(str::to_string) else {
            self.toast(self.text("Bu tekrarın oynatma bağlantısı yok", "This replay has no playable URL"));
            return;
        };
        let (slug, name) = self
            .current_channel
            .borrow()
            .as_ref()
            .map(|ch| (ch.slug.clone(), ch.display_name().to_string()))
            .unwrap_or_default();
        let a = self.clone();
        spawn_task(
            move || api::hls_variants(&master).unwrap_or_default(),
            move |variants| a.show_quality_picker(vod, variants, slug, name),
        );
    }

    fn show_quality_picker(
        self: &Rc<Self>,
        vod: api::Vod,
        variants: Vec<api::Variant>,
        slug: String,
        name: String,
    ) {
        let dialog = adw::Dialog::builder()
            .title(self.text("Kalite seç", "Choose quality"))
            .content_width(400)
            .content_height(460)
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);

        let heading = gtk::Label::new(Some(self.text("İndirilecek kaliteyi seç", "Select the quality to download")));
        heading.add_css_class("title-3");
        heading.set_xalign(0.0);
        let title = gtk::Label::new(Some(vod.title()));
        title.add_css_class("dim-label");
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.set_max_width_chars(46);
        content.append(&heading);
        content.append(&title);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);

        let mut choices: Vec<(String, Option<String>)> = Vec::new();
        choices.push((self.text("Otomatik (en iyi)", "Auto (best)").to_string(), None));
        for variant in &variants {
            choices.push((variant.label(), Some(variant.url.clone())));
        }

        for (label, url) in choices {
            let row = adw::ActionRow::builder()
                .title(&label)
                .activatable(true)
                .build();
            let weak = dialog.downgrade();
            let a = self.clone();
            let vod = vod.clone();
            let slug = slug.clone();
            let name = name.clone();
            let label = label.clone();
            row.connect_activated(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.close();
                }
                a.start_download(&vod, url.clone(), label.clone(), &slug, &name);
            });
            list.append(&row);
        }

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_min_content_height(240);
        scroll.set_max_content_height(320);
        scroll.set_child(Some(&list));
        content.append(&scroll);

        let cancel = gtk::Button::with_label(self.text("İptal", "Cancel"));
        cancel.set_halign(gtk::Align::End);
        let weak = dialog.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(dialog) = weak.upgrade() {
                dialog.close();
            }
        });
        content.append(&cancel);

        dialog.set_child(Some(&content));
        dialog.present(Some(&self.window));
    }

    pub fn start_download(
        self: &Rc<Self>,
        vod: &api::Vod,
        variant: Option<String>,
        quality: String,
        slug: &str,
        name: &str,
    ) {
        let Some(master) = vod.url().map(str::to_string) else {
            return;
        };
        let (id, dir) = download::new_ids(slug);
        let params = download::Params {
            master,
            variant,
            quality,
            title: vod.title().to_string(),
            channel: name.to_string(),
            channel_slug: slug.to_string(),
            subtitle: vod_subtitle(vod),
            duration: vod.duration_secs(),
            id: id.clone(),
            dir,
        };
        let handle = download::begin(params);
        self.active_downloads.borrow_mut().insert(
            id.clone(),
            ActiveDownload {
                title: vod.title().to_string(),
                done: 0,
                total: 0,
                bytes: 0,
                cancel: handle.cancel.clone(),
            },
        );
        self.update_download_indicator();
        if self.stack.visible_child_name().as_deref() == Some("downloads") {
            downloads::render(self);
        }

        let a = self.clone();
        let progress_id = id.clone();
        let finished_id = id.clone();
        glib::spawn_future_local(async move {
            while let Ok(event) = handle.rx.recv().await {
                match event {
                    download::Event::Progress { done, total, bytes } => {
                        if let Some(active) = a.active_downloads.borrow_mut().get_mut(&progress_id) {
                            active.done = done;
                            active.total = total;
                            active.bytes = bytes;
                        }
                        a.update_download_indicator();
                        if a.stack.visible_child_name().as_deref() == Some("downloads") {
                            downloads::render(&a);
                        }
                    }
                    download::Event::Finished(result) => {
                        a.active_downloads.borrow_mut().remove(&finished_id);
                        a.update_download_indicator();
                        match result {
                            Ok(outcome) => {
                                let mut cfg = a.cfg.borrow_mut();
                                cfg.downloads.push(outcome.item);
                                let snapshot = cfg.clone();
                                drop(cfg);
                                if let Err(err) = config::save(&snapshot) {
                                    eprintln!("Could not save the download list: {err}");
                                }
                                a.toast(a.text("İndirme tamamlandı", "Download complete"));
                            }
                            Err(err) => {
                                a.toast(&format!(
                                    "{}: {err}",
                                    a.text("İndirme başarısız", "Download failed")
                                ));
                            }
                        }
                        if a.stack.visible_child_name().as_deref() == Some("downloads") {
                            downloads::render(&a);
                        }
                        break;
                    }
                }
            }
        });
    }

    pub fn cancel_download(&self, id: &str) {
        if let Some(active) = self.active_downloads.borrow().get(id) {
            active
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Keeps the header-bar badge in sync with the running downloads.
    fn update_download_indicator(&self) {
        let active = self.active_downloads.borrow();
        if active.is_empty() {
            if self.download_indicator.is_visible() {
                self.download_indicator_spinner.stop();
                self.download_indicator.set_visible(false);
            }
            return;
        }

        let count = active.len();
        let measured: Vec<&ActiveDownload> = active.values().filter(|item| item.total > 0).collect();
        let percent = if measured.is_empty() {
            None
        } else {
            let done: usize = measured.iter().map(|item| item.done).sum();
            let total: usize = measured.iter().map(|item| item.total).sum();
            Some((done as f64 / total as f64 * 100.0).round() as u64)
        };
        let text = match (count, percent) {
            (1, Some(percent)) => format!("{} %{percent}", self.text("İndiriliyor", "Downloading")),
            (1, None) => self.text("İndiriliyor…", "Downloading…").to_string(),
            (count, Some(percent)) => format!(
                "{count} {} · %{percent}",
                self.text("indirme", "downloads")
            ),
            (count, None) => format!("{count} {}…", self.text("indirme", "downloads")),
        };
        self.download_indicator_label.set_text(&text);
        if !self.download_indicator.is_visible() {
            self.download_indicator_spinner.start();
            self.download_indicator.set_visible(true);
        }
    }

    pub fn play_download(self: &Rc<Self>, item: config::DownloadItem) {
        let path = download::playlist_path(&item);
        if !path.exists() {
            self.toast(self.text("İndirilen dosya bulunamadı", "Downloaded file not found"));
            return;
        }
        let url = path.to_string_lossy().to_string();
        let media = Media {
            kind: MediaKind::Vod,
            slug: Some(item.channel_slug.clone()),
            title: item.title.clone(),
            subtitle: item.subtitle.clone(),
            master: url.clone(),
            url,
            duration: item.duration,
            start: item.position,
            download_id: Some(item.id.clone()),
        };
        self.play_media(media, "downloads");
    }

    pub fn delete_download(self: &Rc<Self>, id: &str) {
        let Some(item) = self
            .cfg
            .borrow()
            .downloads
            .iter()
            .find(|item| item.id == id)
            .cloned()
        else {
            return;
        };
        download::remove(&item);
        {
            let mut cfg = self.cfg.borrow_mut();
            cfg.downloads.retain(|item| item.id != id);
            let snapshot = cfg.clone();
            drop(cfg);
            if let Err(err) = config::save(&snapshot) {
                self.toast(&format!("{}: {err}", self.text("Kayıt silinemedi", "Could not remove item")));
                return;
            }
        }
        downloads::render(&self);
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
                                slug: Some(fresh.slug.clone()),
                                title,
                                subtitle: live_subtitle(&fresh),
                                master: url.clone(),
                                url,
                                duration: None,
                                start: 0.0,
                                download_id: None,
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
            slug: self
                .current_channel
                .borrow()
                .as_ref()
                .map(|ch| ch.slug.clone()),
            title: vod.title().to_string(),
            subtitle: vod_subtitle(&vod),
            master: url.clone(),
            url,
            duration: vod.duration_secs(),
            start,
            download_id: None,
        };
        self.play_media(media, "channel");
    }

    pub fn play_resume(self: &Rc<Self>, video: config::ResumeVideo) {
        let media = Media {
            kind: MediaKind::Vod,
            slug: None,
            title: video.title,
            subtitle: video.subtitle,
            master: video.url.clone(),
            url: video.url,
            duration: video.duration,
            start: video.position,
            download_id: None,
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
        self.page().speed_btn.set_label(if catch_up { "2x" } else { "1x" });

        self.page().set_media(&media);
        self.current_media.replace(Some(media.clone()));
        self.show_full_player();
        self.stack.set_visible_child_name("player");
        self.window
            .set_title(Some(&format!("{} — Kick Lite", fmt::shorten(&media.title, 48))));
        self.page().set_controls_visible(true);
        self.poke_controls();

        let start = if media.kind == MediaKind::Vod {
            Some(media.start)
        } else {
            None
        };
        if let Err(err) = self.player.load(&media.url, start) {
            self.phase.set(Phase::Ended);
            self.page().show_status(self.text("Yayın açılamadı", "Could not open stream"));
            self.toast(&format!("{err:#}"));
        }

        // Offline downloads have a fixed quality baked into the local playlist.
        if media.download_id.is_none() {
            self.page().quality_btn.set_sensitive(true);
            let master = media.master.clone();
            let a = self.clone();
            spawn_task(
                move || api::hls_variants(&master).unwrap_or_default(),
                move |variants| {
                    if a.media_seq.get() != seq {
                        return;
                    }
                    a.page().set_variants(&a, variants);
                },
            );
        } else {
            self.page().quality_btn.set_sensitive(false);
            self.page().set_variants(self, Vec::new());
        }
        self.refresh_stream_actions();
    }

    // ---- multi-view ----

    /// Adds a live channel to the mini players without interrupting playback.
    /// The player screen is opened immediately; the new stream appears there
    /// as a mini tile while the current one keeps the full view.
    pub fn open_live_multi(self: &Rc<Self>, slug: String) {
        if self.current_media.borrow().is_some() {
            self.resume_player();
        }
        let a = self.clone();
        spawn_task(
            move || api::channel(&slug),
            move |result| match result {
                Ok(ch) => match ch.live_url().map(|u| u.to_string()) {
                    Some(url) => {
                        let title = if ch.stream_title().is_empty() {
                            ch.display_name().to_string()
                        } else {
                            ch.stream_title().to_string()
                        };
                        a.add_multi_media(Media {
                            kind: MediaKind::Live,
                            slug: Some(ch.slug.clone()),
                            title,
                            subtitle: live_subtitle(&ch),
                            master: url.clone(),
                            url,
                            duration: None,
                            start: 0.0,
                            download_id: None,
                        });
                    }
                    None => a.toast(a.text("Kanal şu anda canlı değil", "Channel is not live right now")),
                },
                Err(err) => a.toast(&format!("{err:#}")),
            },
        );
    }

    /// Entry point from the "open in mini player" button on the channel page.
    pub fn play_live_multi(self: &Rc<Self>) {
        let Some(slug) = self
            .current_channel
            .borrow()
            .as_ref()
            .map(|ch| ch.slug.clone())
        else {
            return;
        };
        self.open_live_multi(slug);
    }

    /// Each tile owns a libmpv instance, so keep a small cap to protect memory.
    const MAX_MINI_PLAYERS: usize = 4;

    fn add_multi_media(self: &Rc<Self>, media: Media) {
        if media.slug.is_some() {
            let already_mini = self
                .mini_players
                .borrow()
                .iter()
                .any(|mini| mini.media.borrow().slug == media.slug);
            let already_main = self
                .current_media
                .borrow()
                .as_ref()
                .is_some_and(|m| m.slug == media.slug);
            if already_mini || already_main {
                self.toast(self.text("Bu yayın zaten açık", "This stream is already open"));
                return;
            }
        }
        if self.mini_players.borrow().len() >= Self::MAX_MINI_PLAYERS {
            self.toast(self.text(
                "En fazla 4 küçük oynatıcı açabilirsin",
                "You can open at most 4 mini players",
            ));
            return;
        }
        self.insert_mini(media);
    }

    fn insert_mini(self: &Rc<Self>, media: Media) {
        let id = self.mini_seq.get() + 1;
        self.mini_seq.set(id);
        let Some(mini) = multi::MiniPlayer::new(&self, id, media) else {
            return;
        };
        let index = self.mini_players.borrow().len();
        let page = mini.page.borrow().clone();
        self.stack_overlay.add_overlay(&page.page);
        Self::set_mini_layout(&page.page, index);
        page.page
            .insert_before(&self.stack_overlay, None::<&gtk::Widget>);
        self.mini_players.borrow_mut().push(mini);
        self.refresh_stream_actions();
    }

    fn close_mini(&self, mini: &multi::MiniPlayer) {
        let page = mini.page.borrow().clone();
        // Unparent before shutdown so the GLArea detaches its render context.
        page.page.unparent();
        mini.player.borrow().shutdown();
    }

    pub fn close_multi(self: &Rc<Self>, id: u64) {
        let removed = {
            let mut minis = self.mini_players.borrow_mut();
            minis
                .iter()
                .position(|mini| mini.id == id)
                .map(|pos| minis.remove(pos))
        };
        if let Some(mini) = removed {
            self.close_mini(&mini);
            self.reindex_minis();
            self.refresh_stream_actions();
        }
    }

    /// Closes either the focused player or one of the mini tiles by its page.
    pub fn close_page(self: &Rc<Self>, page: &Rc<player::PlayerPage>) {
        if Rc::ptr_eq(&self.page(), page) {
            self.stop_player();
            return;
        }
        let id = self
            .mini_players
            .borrow()
            .iter()
            .find(|mini| Rc::ptr_eq(&mini.page.borrow(), page))
            .map(|mini| mini.id);
        if let Some(id) = id {
            self.close_multi(id);
        }
    }

    /// Shuts down every extra player (used by the menu action and on exit).
    fn drain_minis(&self) {
        for mini in self.mini_players.borrow_mut().drain(..) {
            self.close_mini(&mini);
        }
    }

    pub fn close_all_multi(self: &Rc<Self>) {
        self.drain_minis();
        self.refresh_stream_actions();
    }

    // Layout is only a matter of size/alignment: the GLArea and its mpv render
    // context never move, so switching between these two layouts is instant.
    fn set_full_layout(page: &gtk::Overlay) {
        page.remove_css_class("mini-player");
        page.set_margin_top(0);
        page.set_margin_bottom(0);
        page.set_margin_start(0);
        page.set_margin_end(0);
        page.set_halign(gtk::Align::Fill);
        page.set_valign(gtk::Align::Fill);
        page.set_hexpand(true);
        page.set_vexpand(true);
        page.set_size_request(-1, -1);
        page.set_visible(true);
    }

    fn set_mini_layout(page: &gtk::Overlay, index: usize) {
        page.add_css_class("mini-player");
        page.set_margin_top(0);
        page.set_margin_start(18);
        page.set_margin_end(0);
        page.set_margin_bottom(18 + (index as i32) * 168);
        page.set_halign(gtk::Align::Start);
        page.set_valign(gtk::Align::End);
        page.set_hexpand(false);
        page.set_vexpand(false);
        page.set_size_request(260, 156);
        page.set_visible(true);
    }

    fn reindex_minis(&self) {
        for (index, mini) in self.mini_players.borrow().iter().enumerate() {
            let page = mini.page.borrow().clone();
            Self::set_mini_layout(&page.page, index);
        }
    }

    /// Swaps the focused page with a mini page. Only the player handles and
    /// metadata are exchanged: both mpv instances keep playing in their own
    /// GLArea, so neither stream is reloaded.
    pub fn promote_page(self: &Rc<Self>, target: &Rc<player::PlayerPage>) {
        let focused = self.page();
        if Rc::ptr_eq(&focused, target) {
            self.resume_player();
            return;
        }
        let mini = self
            .mini_players
            .borrow()
            .iter()
            .find(|mini| Rc::ptr_eq(&mini.page.borrow(), target))
            .cloned();
        let Some(mini) = mini else {
            return;
        };
        let incoming = mini.media.borrow().clone();

        // Nothing full-screen to trade with: just open this stream normally.
        if self.current_media.borrow().is_none() {
            self.close_multi(mini.id);
            self.play_media(incoming, "home");
            return;
        }

        self.save_current_position();

        // Repoint only the app's mini/full handles. Each page keeps its own
        // handle, so no mpv player ever leaves its GLArea or render context.
        self.player.repoint(&mini.player.borrow());
        mini.player.borrow_mut().repoint(&focused.player);
        {
            let mut current = self.current_media.borrow_mut();
            let mut tile = mini.media.borrow_mut();
            if let Some(main) = current.take() {
                let old_tile = std::mem::replace(&mut *tile, main);
                *current = Some(old_tile);
            }
        }
        *self.player_page.borrow_mut() = target.clone();
        *mini.page.borrow_mut() = focused.clone();

        // Exchange the layouts. Each page keeps its GLArea/render context, so
        // the demoted stream simply becomes small and the promoted one full.
        Self::set_mini_layout(&focused.page, 0);
        Self::set_full_layout(&target.page);
        self.reindex_minis();
        // The focused page goes to the bottom of the overlay stack; minis stay
        // on top of it so they remain visible and clickable. This only changes
        // sibling order, so the GLArea stays realized with its GL context.
        target
            .page
            .insert_after(&self.stack_overlay, Some(&self.stack));

        focused.set_controls_visible(false);
        focused.hide_generation.set(focused.hide_generation.get() + 1);
        focused.set_mini_click_enabled(true);
        focused.mini_close.set_visible(true);
        target.set_controls_visible(true);
        target.set_mini_click_enabled(false);
        target.mini_close.set_visible(false);

        // Sound follows the slot; the tile goes back to being muted.
        let cfg = self.cfg.borrow().clone();
        self.player.set_volume(cfg.volume);
        self.player.set_mute(cfg.muted);
        mini.player.borrow().set_mute(true);
        mini.update_media_ui();
        focused.set_paused(mini.player.borrow().is_paused());
        target.set_paused(self.player.is_paused());

        // Adopt the incoming stream as already playing (no loading status).
        self.media_seq.set(self.media_seq.get() + 1);
        let seq = self.media_seq.get();
        self.phase.set(Phase::Playing);
        self.load_started.set(None);
        self.resume_pending.set(None);
        self.live_catch_up.set(false);
        self.reset_auto_silence();
        target.set_media_info(&incoming);
        target.hide_status();
        target.speed_btn.set_label("1x");
        target.quality_btn.set_sensitive(incoming.download_id.is_none());

        self.stack.set_visible_child_name("player");
        self.window.set_title(Some(&format!(
            "{} — Kick Lite",
            fmt::shorten(&incoming.title, 48)
        )));
        self.poke_controls();

        // Quality options belong to the stream, so refresh them for the new one.
        if incoming.download_id.is_none() {
            let master = incoming.master.clone();
            let a = self.clone();
            spawn_task(
                move || api::hls_variants(&master).unwrap_or_default(),
                move |variants| {
                    if a.media_seq.get() != seq {
                        return;
                    }
                    a.page().set_variants(&a, variants);
                },
            );
        } else {
            self.page().set_variants(self, Vec::new());
        }
    }

    /// A click on a tile promotes it; a click on the already focused page
    /// restores it from mini size to the full player.
    pub fn activate_page(self: &Rc<Self>, page: &Rc<player::PlayerPage>) {
        if Rc::ptr_eq(&self.page(), page) {
            self.resume_player();
        } else {
            self.promote_page(page);
        }
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
        self.page().quality_btn.set_label(&label);
        self.page().quality_popover.popdown();
        self.phase.set(Phase::Loading);
        self.load_started.set(Some(Instant::now()));
        self.page().spinner.start();
        self.page().show_status(self.text("Kalite değiştiriliyor…", "Changing quality…"));
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

    pub fn stop_player(self: &Rc<Self>) {
        self.save_current_position();
        self.reset_auto_silence();
        self.player.set_silence_allowed(false);
        self.player.stop();
        self.resume_pending.set(None);
        self.live_catch_up.set(false);
        self.current_media.replace(None);
        self.page().page.set_visible(false);
        self.media_seq.set(self.media_seq.get() + 1);
        self.phase.set(Phase::Idle);
        self.load_started.set(None);
        self.cancel_hide_controls();
        self.page().set_controls_visible(true);
        self.page().hide_status();
        self.window.set_title(Some("Kick Lite"));
        self.refresh_stream_actions();
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
        self.downloads_scroll.vadjustment().set_value(0.0);
        match page {
            "channel" if self.current_channel.borrow().is_some() => channel::render(self),
            "downloads" => downloads::render(self),
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

    /// Opens the dedicated downloads page without stopping playback.
    pub fn go_downloads(self: &Rc<Self>) {
        if self.window.is_fullscreen() {
            self.window.unfullscreen();
        }
        self.return_page.set("downloads");
        self.stack.set_visible_child_name("downloads");
        self.downloads_scroll.vadjustment().set_value(0.0);
        self.cancel_hide_controls();
        if self.current_media.borrow().is_some() {
            self.show_mini_player();
        }
        downloads::render(self);
    }

    /// Moves the video back to the main player area when the mini player is clicked.
    pub fn resume_player(self: &Rc<Self>) {
        let Some(media) = self.current_media.borrow().clone() else {
            self.page().page.set_visible(false);
            return;
        };
        self.show_full_player();
        self.stack.set_visible_child_name("player");
        self.page().set_controls_visible(true);
        self.window
            .set_title(Some(&format!("{} — Kick Lite", fmt::shorten(&media.title, 48))));
        self.poke_controls();
    }

    fn show_full_player(&self) {
        let page = self.page();
        Self::set_full_layout(&page.page);
        page.set_mini_click_enabled(false);
        page.mini_close.set_visible(false);
        page.set_controls_visible(true);
        // Keep other streams' tiles above the full player.
        page.page
            .insert_after(&self.stack_overlay, Some(&self.stack));
    }

    fn show_mini_player(&self) {
        let page = self.page();
        let index = self.mini_players.borrow().len();
        Self::set_mini_layout(&page.page, index);
        page.set_mini_click_enabled(true);
        page.mini_close.set_visible(true);
        page.set_controls_visible(false);
        page.page
            .insert_before(&self.stack_overlay, None::<&gtk::Widget>);
    }

    pub fn toggle_fullscreen(&self) {
        if self.window.is_fullscreen() {
            self.window.unfullscreen();
        } else {
            self.window.fullscreen();
        }
    }

    pub fn sync_pause_icon(&self) {
        self.page().set_paused(self.player.is_paused());
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
        self.page().speed_btn.set_label("2x");
        self.live_catch_up.set(true);
        self.toast(self.text("Gecikme azaltılıyor; canlı yayın 2x hızda oynatılıyor", "Reducing latency; playing the live stream at 2x"));
    }

    pub fn update_volume_icon(&self) {
        let muted = self.player.is_muted();
        let v = self.page().volume.value();
        let icon = if muted || v <= 0.5 {
            "audio-volume-muted-symbolic"
        } else if v < 34.0 {
            "audio-volume-low-symbolic"
        } else if v < 67.0 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        self.page().vol_btn.set_icon_name(icon);
    }

    pub fn poke_controls(self: &Rc<Self>) {
        self.page().set_controls_visible(true);
        self.schedule_hide_controls();
    }

    pub fn schedule_hide_controls(self: &Rc<Self>) {
        self.cancel_hide_controls();
        if self.stack.visible_child_name().as_deref() != Some("player") {
            return;
        }
        if self.player.is_paused()
            || self.page().quality_popover.is_visible()
            || self.current_media.borrow().is_none()
        {
            return;
        }
        let page = self.page();
        let generation = page.hide_generation.get() + 1;
        page.hide_generation.set(generation);
        let a = self.clone();
        glib::timeout_add_seconds_local(3, move || {
            // The timed-out request may belong to a page that is no longer
            // focused after a swap, so check its own generation first.
            if page.hide_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            if a.stack.visible_child_name().as_deref() == Some("player")
                && !a.player.is_paused()
                && !page.quality_popover.is_visible()
            {
                page.set_controls_visible(false);
            }
            glib::ControlFlow::Break
        });
    }

    pub fn cancel_hide_controls(&self) {
        // Invalidating the generation is enough; the timeout expires on its
        // own. Removing an already-fired SourceId would panic in glib.
        let page = self.page();
        page.hide_generation.set(page.hide_generation.get() + 1);
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
                                    slug: Some(channel.clone()),
                                    title: vod.title().to_string(),
                                    subtitle: vod_subtitle(&vod),
                                    master: url.clone(),
                                    url,
                                    duration: vod.duration_secs(),
                                    start,
                                    download_id: None,
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

    // ---- automatic silence threshold ----

    /// Threshold actually in use: either the value derived from the audio or
    /// the manually configured one.
    pub fn effective_silence_db(&self) -> i32 {
        let cfg = self.cfg.borrow();
        if cfg.auto_silence {
            self.auto_db.get().unwrap_or(cfg.silence_noise_db)
        } else {
            cfg.silence_noise_db
        }
    }

    /// Drops the analysis window; called when the media or the mode changes.
    fn reset_auto_silence(&self) {
        self.auto_levels.borrow_mut().clear();
        self.auto_db.set(None);
        self.last_auto_apply.set(None);
    }

    /// Turns automatic detection on/off and applies the resulting threshold
    /// to the mpv filter chain immediately.
    fn set_auto_silence(self: &Rc<Self>, enabled: bool) {
        {
            let mut cfg = self.cfg.borrow_mut();
            if cfg.auto_silence == enabled {
                return;
            }
            cfg.auto_silence = enabled;
        }
        self.reset_auto_silence();
        let cfg = self.cfg.borrow().clone();
        if let Err(err) = self.player.apply_silence_filter(
            self.effective_silence_db(),
            cfg.silence_duration,
        ) {
            eprintln!("Could not apply silence threshold: {err}");
        }
        let _ = config::save(&cfg);
    }

    /// Collects one RMS sample per tick and periodically re-derives the noise
    /// threshold from the level distribution: the quiet floor (10th percentile)
    /// and the loud parts (85th percentile). The gate is placed between them so
    /// it adapts both to quiet recordings and to noisy ones. A fairly uniform
    /// level means there is no silence to separate, so nothing is changed.
    fn sample_auto_silence(&self) {
        if !self.cfg.borrow().auto_silence {
            return;
        }
        if self.stack.visible_child_name().as_deref() != Some("player") {
            return;
        }
        if self
            .current_media
            .borrow()
            .as_ref()
            .is_some_and(|m| m.kind != MediaKind::Vod)
        {
            return;
        }
        let Some(level) = self.player.audio_level_db() else {
            return;
        };
        {
            let mut levels = self.auto_levels.borrow_mut();
            // Digital silence is a valid sample; pin it to the meter floor.
            levels.push(if level.is_finite() { level } else { -80.0 });
            if levels.len() > 400 {
                levels.remove(0);
            }
        }
        if self
            .last_auto_apply
            .get()
            .is_some_and(|t| t.elapsed().as_secs_f64() < 4.0)
        {
            return;
        }
        let levels = self.auto_levels.borrow();
        let Some(detected) = auto_threshold(&levels) else {
            return;
        };
        if self.auto_db.get() == Some(detected) {
            return;
        }
        drop(levels);
        self.auto_db.set(Some(detected));
        self.last_auto_apply.set(Some(Instant::now()));
        let duration = self.cfg.borrow().silence_duration;
        if let Err(err) = self.player.apply_silence_filter(detected, duration) {
            eprintln!("Could not apply automatic silence threshold: {err}");
        }
    }

    // ---- player loop ----

    fn tick(self: &Rc<Self>) {
        // Always drain mpv events (silencedetect reports keep arriving even while
        // the mini player is active). Since silence skipping is implemented as a
        // speed change, it is triggered here.
        self.player.poll_events();
        // Extra players are always live streams; drain their queues too so mpv
        // does not accumulate events while a tile sits in the corner.
        for mini in self.mini_players.borrow().iter() {
            mini.player.borrow().poll_events();
        }
        self.sample_auto_silence();

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
                    self.page().hide_status();
                } else if let Some(started) = self.load_started.get() {
                    if started.elapsed().as_secs_f64() > 12.0 && self.player.is_idle() {
                        self.phase.set(Phase::Ended);
                        self.page().spinner.stop();
                        self.page().show_status(if live {
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
                self.page().spinner.stop();
                self.page().show_status(if live {
                    if self.is_english() { "Live stream ended" } else { "Canlı yayın sona erdi" }
                } else {
                    if self.is_english() { "Video ended" } else { "Yayın sona erdi" }
                });
                self.cancel_hide_controls();
                self.page().set_controls_visible(true);
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
                self.page().speed_btn.set_label("1x");
                self.live_catch_up.set(false);
                self.toast(self.text("Canlı yayın güncel noktaya ulaştı; hız normale döndü", "Live stream caught up; playback speed restored"));
            }
            return;
        }

        if let Some(duration) = self.player.duration() {
            if duration > 0.0 {
                self.page().seek.adjustment().set_upper(duration);
                self.page().dur.set_text(&fmt::duration(duration));
            }
        }

        if let Some(target) = self.page().pending_seek.get() {
            let waited = self
                .page()
                .last_user_change
                .get()
                .map(|t| t.elapsed().as_millis())
                .unwrap_or(u128::MAX);
            if waited >= 350 {
                self.page().pending_seek.set(None);
                self.player.clear_silence();
                self.player.seek(target, true);
                self.page().seek.set_value(target);
                self.page().pos.set_text(&fmt::duration(target));
            }
        } else if let Some(pos) = self.player.position() {
            self.page().pos.set_text(&fmt::duration(pos));
            self.page().seek.set_value(pos);
            if media.kind == MediaKind::Vod && media.download_id.is_none() {
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
        if let Some(id) = &media.download_id {
            let mut cfg = self.cfg.borrow_mut();
            if let Some(item) = cfg.downloads.iter_mut().find(|item| &item.id == id) {
                if media.duration.is_some_and(|d| pos >= d - 5.0) {
                    item.position = 0.0;
                } else if pos > 1.0 {
                    item.position = pos;
                }
            }
            return;
        }
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

/// Places the noise gate between the quiet floor and the loud parts of the
/// observed RMS levels. Returns `None` until there are enough samples or when
/// the level is too uniform to tell silence from speech.
fn auto_threshold(levels: &[f64]) -> Option<i32> {
    if levels.len() < 20 {
        return None;
    }
    let mut sorted = levels.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let percentile = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    let quiet = percentile(0.10);
    let loud = percentile(0.85);
    let spread = loud - quiet;
    if spread < 8.0 {
        return None;
    }
    Some(
        ((loud - 18.0).min(quiet + 0.5 * spread))
            .clamp(-70.0, -20.0)
            .round() as i32,
    )
}

fn setup_actions(gtk_app: &adw::Application, app: &Rc<App>) {
    const PROJECT_URL: &str = "https://github.com/mucahit-sahin/kick-linux";
    const AUTHOR_URL: &str = "https://github.com/mucahit-sahin";

    let about = gio::SimpleAction::new("about", None);
    let a = app.clone();
    about.connect_activate(move |_, _| {
        let dialog = adw::AboutDialog::builder()
            .application_name("Kick Lite")
            .application_icon("io.github.kicklite.KickLite")
            .developer_name("Mucahit Sahin")
            .version(env!("CARGO_PKG_VERSION"))
            .comments(a.text(
                concat!(
                    "Kick Lite, Kick.com canlı yayınlarını ve yayın tekrarlarını hafif bir ",
                    "masaüstü oynatıcıyla izlemenizi sağlar. GTK4, libadwaita ve libmpv ",
                    "kullanır; web tarayıcısı gömmez.",
                    "\n\nProje: https://github.com/mucahit-sahin/kick-linux",
                    "\nYazar: Mucahit Sahin — https://github.com/mucahit-sahin",
                    "\n\nKick'in herkese açık, resmî olmayan API'sini kullanır.",
                ),
                concat!(
                    "Kick Lite is a lightweight desktop player for watching Kick.com ",
                    "livestreams and replays. It uses GTK4, libadwaita and libmpv, ",
                    "and does not embed a web browser.",
                    "\n\nProject: https://github.com/mucahit-sahin/kick-linux",
                    "\nAuthor: Mucahit Sahin — https://github.com/mucahit-sahin",
                    "\n\nUses Kick's public, unofficial API.",
                ),
            ))
            .website(PROJECT_URL)
            .license_type(gtk::License::MitX11)
            .build();
        dialog.add_link("Kick.com", "https://kick.com");
        dialog.add_link(
            &a.text("Sorun bildir", "Report an issue"),
            "https://github.com/mucahit-sahin/kick-linux/issues",
        );
        dialog.add_link(
            &a.text("Yazarın GitHub profili", "Author's GitHub profile"),
            AUTHOR_URL,
        );
        dialog.present(Some(&a.window));
    });
    gtk_app.add_action(&about);

    let quit = gio::SimpleAction::new("quit", None);
    let gtk_app_for_quit = gtk_app.clone();
    quit.connect_activate(move |_, _| gtk_app_for_quit.quit());
    gtk_app.add_action(&quit);
}

fn install_silence_settings_action(gtk_app: &adw::Application, app: &Rc<App>) {
    let action = gio::SimpleAction::new("silence-settings", None);
    let a = app.clone();
    action.connect_activate(move |_, _| a.show_silence_settings());
    gtk_app.add_action(&action);
}

fn install_multi_actions(gtk_app: &adw::Application, app: &Rc<App>) {
    let action = gio::SimpleAction::new("close-mini-players", None);
    let a = app.clone();
    action.connect_activate(move |_, _| a.close_all_multi());
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

#[cfg(test)]
mod tests {
    use super::auto_threshold;

    #[test]
    fn needs_enough_audio_samples() {
        assert_eq!(auto_threshold(&vec![-80.0; 19]), None);
    }

    #[test]
    fn ignores_uniform_audio() {
        let levels = vec![-30.0; 100];
        assert_eq!(auto_threshold(&levels), None);
    }

    #[test]
    fn places_gate_between_digital_silence_and_speech() {
        // Half the frames are silent, the rest peak near -43 dB (measured from
        // a real tone through mpv's astats filter).
        let mut levels = vec![-80.0; 50];
        levels.extend(vec![-43.0; 50]);
        assert_eq!(auto_threshold(&levels), Some(-62));
    }

    #[test]
    fn adapts_to_a_noisy_room() {
        let mut levels = vec![-60.0; 50];
        levels.extend(vec![-25.0; 50]);
        assert_eq!(auto_threshold(&levels), Some(-43));
    }

    #[test]
    fn stays_within_the_allowed_range() {
        let mut levels = vec![-90.0; 50];
        levels.extend(vec![0.0; 50]);
        let db = auto_threshold(&levels).unwrap();
        assert!((-70..=-20).contains(&db));
    }
}
