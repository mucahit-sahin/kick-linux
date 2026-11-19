//! Channel page: live status, follow button, and replays.

use crate::fmt;
use crate::ui::{clear_container, App};
use gtk::gio;
use gtk::prelude::*;
use std::rc::Rc;

pub fn render_loading(app: &Rc<App>, slug: &str) {
    let container = &app.channel_box;
    clear_container(container);
    container.set_valign(gtk::Align::Fill);

    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 10);
    wrapper.set_valign(gtk::Align::Center);
    wrapper.set_halign(gtk::Align::Center);
    wrapper.set_vexpand(true);

    let spinner = gtk::Spinner::new();
    spinner.start();
    let label = gtk::Label::new(Some(&format!("@{slug} {}", app.text("yükleniyor…", "loading…"))));
    label.add_css_class("dim-label");
    wrapper.append(&spinner);
    wrapper.append(&label);
    container.append(&wrapper);
}

pub fn render(app: &Rc<App>) {
    let container = &app.channel_box;
    clear_container(container);
    container.set_valign(gtk::Align::Fill);

    let Some(channel) = app.current_channel.borrow().clone() else {
        return;
    };
    let following = app.cfg.borrow().is_following(&channel.slug);
    let live = channel.is_live();

    // --- header ---
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
    titles.set_hexpand(true);

    let name = gtk::Label::new(Some(channel.display_name()));
    name.add_css_class("title-1");
    name.set_xalign(0.0);
    let mut meta_text = if app.is_english() {
        format!("@{} · {} followers", channel.slug, fmt::human_count(channel.followers()))
    } else {
        format!("@{} · {} takipçi", channel.slug, fmt::human_count(channel.followers()))
    };
    if channel.verified {
        meta_text.push_str(app.text(" · doğrulanmış", " · verified"));
    }
    let meta = gtk::Label::new(Some(&meta_text));
    meta.add_css_class("dim-label");
    meta.set_xalign(0.0);
    titles.append(&name);
    titles.append(&meta);

    let open_btn = gtk::Button::with_label(app.text("Kick'te aç", "Open on Kick"));
    open_btn.set_tooltip_text(Some(app.text("Kanalı tarayıcıda aç", "Open channel in browser")));
    {
        let uri = format!("https://kick.com/{}", channel.slug);
        open_btn.connect_clicked(move |_| {
            let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
        });
    }

    let follow_btn = gtk::Button::with_label(if following {
        app.text("Takipten çık", "Unfollow")
    } else {
        app.text("Takip et", "Follow")
    });
    if !following {
        follow_btn.add_css_class("suggested-action");
    }
    {
        let a = app.clone();
        follow_btn.connect_clicked(move |_| a.toggle_follow());
    }

    header.append(&titles);
    header.append(&open_btn);
    header.append(&follow_btn);
    container.append(&header);

    // --- live / offline card ---
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("card");
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
    inner.set_margin_top(16);
    inner.set_margin_bottom(16);
    inner.set_margin_start(18);
    inner.set_margin_end(18);
    card.append(&inner);

    if live {
        let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let badge = gtk::Label::new(Some(app.text("CANLI", "LIVE")));
        badge.add_css_class("live-badge");
        let viewers_text = if app.is_english() {
            format!("{} viewers", fmt::human_count(channel.viewers()))
        } else {
            format!("{} izleyici", fmt::human_count(channel.viewers()))
        };
        let viewers = gtk::Label::new(Some(&viewers_text));
        viewers.add_css_class("dim-label");
        status_row.append(&badge);
        status_row.append(&viewers);
        let category = channel.category();
        if !category.is_empty() {
            let cat = gtk::Label::new(Some(category));
            cat.add_css_class("dim-label");
            status_row.append(&cat);
        }
        inner.append(&status_row);

        let stream_title = channel.stream_title();
        if !stream_title.is_empty() {
            let title = gtk::Label::new(Some(stream_title));
            title.add_css_class("title-3");
            title.set_xalign(0.0);
            title.set_wrap(true);
            title.set_lines(3);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            inner.append(&title);
        }

        let watch_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        watch_row.set_margin_top(4);

        let watch = gtk::Button::with_label(app.text("Canlı yayını izle", "Watch live"));
        watch.add_css_class("suggested-action");
        watch.set_halign(gtk::Align::Start);
        {
            let a = app.clone();
            watch.connect_clicked(move |_| a.play_live());
        }
        watch_row.append(&watch);

        // Multi-view: keep the current stream playing and open this one in a
        // mini player so several broadcasts can be watched at once.
        let watch_mini = gtk::Button::with_label(app.text(
            "Küçük oynatıcıda aç",
            "Open in mini player",
        ));
        watch_mini.set_halign(gtk::Align::Start);
        // Only useful when there is already a stream to watch alongside.
        watch_mini.set_visible(app.has_active_stream());
        {
            let a = app.clone();
            watch_mini.connect_clicked(move |_| a.play_live_multi());
        }
        watch_row.append(&watch_mini);
        inner.append(&watch_row);
    } else {
        let title = gtk::Label::new(Some(app.text("Şu anda çevrimdışı", "Currently offline")));
        title.add_css_class("title-3");
        title.set_xalign(0.0);
        inner.append(&title);

        let hint = gtk::Label::new(Some(
            app.text("Kanal yayına başladığında bu sayfayı yenileyerek izleyebilirsin.", "Refresh this page when the channel goes live to watch."),
        ));
        hint.add_css_class("dim-label");
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        inner.append(&hint);

        let last_vod = app.current_vods.borrow().first().cloned();
        if let Some(vod) = last_vod {
            if vod.url().is_some() {
                let replay = gtk::Button::with_label(app.text("Son tekrarı izle", "Watch latest replay"));
                replay.set_halign(gtk::Align::Start);
                replay.set_margin_top(4);
                {
                    let a = app.clone();
                    replay.connect_clicked(move |_| a.play_vod(vod.clone()));
                }
                inner.append(&replay);
            }
        }
    }
    container.append(&card);

    // --- replays ---
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.set_margin_top(4);
    let head_label = gtk::Label::new(Some(app.text("Yayın tekrarları", "Replays")));
    head_label.add_css_class("title-3");
    head_label.set_xalign(0.0);
    head_label.set_hexpand(true);
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some(app.text("Yayın tekrarlarını yenile", "Refresh replays")));
    refresh.set_has_frame(false);
    {
        let a = app.clone();
        refresh.connect_clicked(move |_| a.refresh_channel());
    }
    head.append(&head_label);
    head.append(&refresh);
    container.append(&head);

    if !channel.vod_enabled {
        let label = gtk::Label::new(Some(app.text("Kanal yayın tekrarlarını kapatmış.", "This channel has disabled replays.")));
        label.add_css_class("dim-label");
        label.set_xalign(0.0);
        container.append(&label);
        return;
    }

    if !app.vods_loaded.get() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_top(6);
        let spinner = gtk::Spinner::new();
        spinner.start();
        let label = gtk::Label::new(Some(app.text("Yayın tekrarları yükleniyor…", "Loading replays…")));
        label.add_css_class("dim-label");
        row.append(&spinner);
        row.append(&label);
        container.append(&row);
        return;
    }

    if let Some(err) = app.vods_error.borrow().as_deref() {
        let label = gtk::Label::new(Some(&format!("{}: {err}", app.text("Yayın tekrarları alınamadı", "Could not load replays"))));
        label.add_css_class("dim-label");
        label.set_xalign(0.0);
        label.set_wrap(true);
        container.append(&label);
        return;
    }

    let vods = app.current_vods.borrow().clone();
    if vods.is_empty() {
        let label = gtk::Label::new(Some(app.text("Bu kanalda kayıtlı yayın tekrarı yok.", "No replays are available for this channel.")));
        label.add_css_class("dim-label");
        label.set_xalign(0.0);
        container.append(&label);
        return;
    }

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    list.set_margin_top(2);

    for (index, vod) in vods.iter().enumerate() {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&index.to_string());

        let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        content.set_margin_top(10);
        content.set_margin_bottom(10);
        content.set_margin_start(14);
        content.set_margin_end(14);

        let play = gtk::Button::from_icon_name("media-playback-start-symbolic");
        play.set_has_frame(false);
        play.add_css_class("flat");
        play.set_valign(gtk::Align::Center);
        play.set_tooltip_text(Some(app.text("Tekrarı oynat", "Play replay")));
        {
            let a = app.clone();
            let playable = vod.clone();
            play.connect_clicked(move |_| a.play_vod(playable.clone()));
        }
        if vod.url().is_none() {
            play.set_sensitive(false);
        }

        let texts = gtk::Box::new(gtk::Orientation::Vertical, 3);
        texts.set_hexpand(true);
        let title = gtk::Label::new(Some(vod.title()));
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.add_css_class("heading");
        texts.append(&title);

        let mut parts: Vec<String> = Vec::new();
        if let Some(created) = vod.created_at.as_deref() {
            parts.push(fmt::relative_time_lang(created, app.is_english()));
        }
        if let Some(duration) = vod.duration_secs() {
            parts.push(fmt::duration(duration));
        }
        if vod.view_count() > 0 {
            parts.push(if app.is_english() { format!("{} views", fmt::human_count(vod.view_count())) } else { format!("{} görüntüleme", fmt::human_count(vod.view_count())) });
        }
        if vod.is_live {
            parts.push(app.text("canlı kayıt", "live recording").to_string());
        }
        let meta = gtk::Label::new(Some(&parts.join(" · ")));
        meta.set_xalign(0.0);
        meta.add_css_class("dim-label");
        meta.add_css_class("caption");
        texts.append(&meta);

        if let Some(url) = vod.url() {
            if let Some(position) = a_resume_position(app, url) {
                let progress = gtk::ProgressBar::new();
                progress.set_show_text(false);
                progress.set_fraction(
                    vod.duration_secs()
                        .filter(|duration| *duration > 0.0)
                        .map(|duration| (position / duration).clamp(0.0, 1.0))
                        .unwrap_or(0.0),
                );
                texts.append(&progress);
                let resume_text = if app.is_english() {
                    format!("Resume at: {}", fmt::duration(position))
                } else {
                    format!("Kaldığın yer: {}", fmt::duration(position))
                };
                let resume = gtk::Label::new(Some(&resume_text));
                resume.set_xalign(0.0);
                resume.add_css_class("dim-label");
                resume.add_css_class("caption");
                texts.append(&resume);
            }
        }

        let download = gtk::Button::from_icon_name("document-save-symbolic");
        download.set_has_frame(false);
        download.add_css_class("flat");
        download.set_valign(gtk::Align::Center);
        download.set_tooltip_text(Some(app.text("Kalite seçerek indir", "Download with quality selection")));
        {
            let a = app.clone();
            let downloadable = vod.clone();
            download.connect_clicked(move |_| a.download_vod(downloadable.clone()));
        }
        if vod.url().is_none() {
            download.set_sensitive(false);
        }

        content.append(&play);
        content.append(&texts);
        content.append(&download);
        row.set_child(Some(&content));
        list.append(&row);
    }

    container.append(&list);
}

fn a_resume_position(app: &Rc<App>, url: &str) -> Option<f64> {
    app.cfg.borrow().resume_videos.iter()
        .find(|video| video.url == url)
        .map(|video| video.position)
        .or_else(|| app.cfg.borrow().resume_positions.get(url).copied())
}
