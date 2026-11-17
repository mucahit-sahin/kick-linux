//! Home page: live statuses of followed channels.

use crate::config;
use crate::fmt;
use crate::ui::{clear_container, ActiveDownload, App};
use gtk::prelude::*;
use std::rc::Rc;

pub fn render(app: &Rc<App>) {
    let container = &app.home_box;
    clear_container(container);
    if app.refreshing.get() {
        let loading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        loading.set_margin_bottom(2);
        let spinner = gtk::Spinner::new();
        spinner.start();
        let label = gtk::Label::new(Some(app.text("Canlı yayınlar yükleniyor…", "Loading live streams…")));
        label.add_css_class("dim-label");
        loading.append(&spinner);
        loading.append(&label);
        container.append(&loading);
    }
    downloads_section(app, container);
    resume_section(app, container);
    let followed = app.cfg.borrow().followed.clone();

    if followed.is_empty() {
        container.set_valign(gtk::Align::Center);
        empty_state(app, container);
        return;
    }

    container.set_valign(gtk::Align::Start);

    let live_count = {
        let status = app.status.borrow();
        status
            .iter()
            .filter(|(slug, ch)| ch.is_live() && app.cfg.borrow().is_following(slug))
            .count()
    };

    let title = gtk::Label::new(Some(app.text("Takip edilenler", "Following")));
    title.add_css_class("title-1");
    title.set_xalign(0.0);
    let subtitle_text = if app.is_english() {
        format!("{} channels · {} live", followed.len(), live_count)
    } else {
        format!("{} kanal · {} canlı", followed.len(), live_count)
    };
    let subtitle = gtk::Label::new(Some(&subtitle_text));
    subtitle.add_css_class("dim-label");
    subtitle.set_xalign(0.0);

    container.append(&title);
    container.append(&subtitle);

    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_max_children_per_line(3);
    flow.set_min_children_per_line(1);
    flow.set_column_spacing(12);
    flow.set_row_spacing(12);
    flow.set_homogeneous(true);
    flow.set_valign(gtk::Align::Start);
    flow.set_margin_top(6);

    let mut visible_channels: Vec<_> = followed.iter().collect();
    visible_channels.sort_by_key(|followed_channel| {
        !app.status
            .borrow()
            .get(&followed_channel.slug)
            .is_some_and(|channel| channel.is_live())
    });

    for followed_channel in visible_channels {
        let status = app.status.borrow().get(&followed_channel.slug).cloned();
        flow.insert(&card(app, followed_channel, status.as_ref()), -1);
    }
    container.append(&flow);
}

fn downloads_section(app: &Rc<App>, container: &gtk::Box) {
    let active: Vec<(String, ActiveDownload)> = app
        .active_downloads
        .borrow()
        .iter()
        .map(|(id, download)| (id.clone(), download.clone()))
        .collect();
    let items: Vec<config::DownloadItem> = app.cfg.borrow().downloads.clone();
    if active.is_empty() && items.is_empty() {
        return;
    }

    let title = gtk::Label::new(Some(app.text("İndirilenler", "Downloads")));
    title.add_css_class("title-2");
    title.set_xalign(0.0);
    container.append(&title);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");

    for (id, download) in active {
        let row = gtk::ListBoxRow::new();
        row.set_selectable(false);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        content.set_margin_top(10);
        content.set_margin_bottom(10);
        content.set_margin_start(14);
        content.set_margin_end(14);

        let details = gtk::Box::new(gtk::Orientation::Vertical, 4);
        details.set_hexpand(true);
        let name = gtk::Label::new(Some(&download.title));
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.add_css_class("heading");
        let status = if download.total > 0 {
            let percent = (download.done as f64 / download.total as f64 * 100.0).round() as u64;
            format!(
                "{}% · {}/{} · {}",
                percent,
                download.done,
                download.total,
                fmt::size(download.bytes)
            )
        } else {
            app.text("Hazırlanıyor…", "Preparing…").to_string()
        };
        let status_label = gtk::Label::new(Some(&status));
        status_label.set_xalign(0.0);
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");
        let progress = gtk::ProgressBar::new();
        progress.set_show_text(false);
        progress.set_fraction(if download.total > 0 {
            (download.done as f64 / download.total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        });
        details.append(&name);
        details.append(&status_label);
        details.append(&progress);

        let cancel = gtk::Button::from_icon_name("process-stop-symbolic");
        cancel.set_tooltip_text(Some(app.text("İndirmeyi iptal et", "Cancel download")));
        cancel.set_has_frame(false);
        cancel.add_css_class("flat");
        cancel.set_valign(gtk::Align::Center);
        let a = app.clone();
        let id = id.clone();
        cancel.connect_clicked(move |_| a.cancel_download(&id));

        content.append(&details);
        content.append(&cancel);
        row.set_child(Some(&content));
        list.append(&row);
    }

    for item in items.into_iter().rev() {
        let row = gtk::ListBoxRow::new();
        row.set_selectable(false);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        content.set_margin_top(10);
        content.set_margin_bottom(10);
        content.set_margin_start(14);
        content.set_margin_end(14);

        let details = gtk::Box::new(gtk::Orientation::Vertical, 4);
        details.set_hexpand(true);
        let name = gtk::Label::new(Some(&item.title));
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.add_css_class("heading");
        let mut meta_parts = vec![item.channel.clone(), item.quality.clone()];
        if let Some(duration) = item.duration {
            meta_parts.push(fmt::duration(duration));
        }
        if item.bytes > 0 {
            meta_parts.push(fmt::size(item.bytes));
        }
        let meta = gtk::Label::new(Some(&meta_parts.join(" · ")));
        meta.set_xalign(0.0);
        meta.add_css_class("dim-label");
        meta.add_css_class("caption");
        details.append(&name);
        details.append(&meta);
        if item.position > 1.0 {
            let progress = gtk::ProgressBar::new();
            progress.set_show_text(false);
            progress.set_fraction(
                item.duration
                    .filter(|duration| *duration > 0.0)
                    .map(|duration| (item.position / duration).clamp(0.0, 1.0))
                    .unwrap_or(0.0),
            );
            details.append(&progress);
        }

        let play = gtk::Button::from_icon_name("media-playback-start-symbolic");
        play.set_tooltip_text(Some(app.text("Çevrimdışı oynat", "Play offline")));
        play.set_has_frame(false);
        play.add_css_class("flat");
        play.set_valign(gtk::Align::Center);
        let a = app.clone();
        let play_item = item.clone();
        play.connect_clicked(move |_| a.play_download(play_item.clone()));

        let delete = gtk::Button::from_icon_name("user-trash-symbolic");
        delete.set_tooltip_text(Some(app.text("İndirmeyi sil", "Delete download")));
        delete.set_has_frame(false);
        delete.add_css_class("flat");
        delete.set_valign(gtk::Align::Center);
        let a = app.clone();
        let id = item.id.clone();
        delete.connect_clicked(move |_| a.delete_download(&id));

        content.append(&details);
        content.append(&play);
        content.append(&delete);
        row.set_child(Some(&content));
        list.append(&row);
    }

    container.append(&list);
}

fn resume_section(app: &Rc<App>, container: &gtk::Box) {
    let videos = app.cfg.borrow().resume_videos.clone();
    if videos.is_empty() {
        return;
    }
    let title = gtk::Label::new(Some(app.text("Kaldığın yerden devam et", "Continue watching")));
    title.add_css_class("title-2");
    title.set_xalign(0.0);
    container.append(&title);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    for video in videos.into_iter().rev().take(6) {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(true);
        row.set_selectable(false);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        content.set_margin_top(10);
        content.set_margin_bottom(10);
        content.set_margin_start(14);
        content.set_margin_end(14);
        let name = gtk::Label::new(Some(&video.title));
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.add_css_class("heading");
        let channel = gtk::Label::new(Some(if video.channel.is_empty() {
            app.text("Kanal bilgisi bulunamadı", "Channel unavailable")
        } else {
            &video.channel
        }));
        channel.set_xalign(0.0);
        channel.add_css_class("dim-label");
        channel.add_css_class("caption");
        let meta = gtk::Label::new(Some(&format!(
            "{} / {}",
            fmt::duration(video.position),
            video.duration.map(fmt::duration).unwrap_or_else(|| "?".into())
        )));
        meta.set_xalign(0.0);
        meta.add_css_class("dim-label");
        let progress = gtk::ProgressBar::new();
        progress.set_show_text(false);
        progress.set_fraction(
            video.duration
                .filter(|duration| *duration > 0.0)
                .map(|duration| (video.position / duration).clamp(0.0, 1.0))
                .unwrap_or(0.0),
        );
        let details = gtk::Box::new(gtk::Orientation::Vertical, 4);
        details.set_hexpand(true);
        details.append(&name);
        details.append(&channel);
        details.append(&meta);
        details.append(&progress);
        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.set_tooltip_text(Some(app.text("Devam listesinden kaldır", "Remove from Continue watching")));
        remove.set_has_frame(false);
        remove.add_css_class("flat");
        content.append(&details);
        content.append(&remove);
        row.set_child(Some(&content));
        let a = app.clone();
        let resume_video = video.clone();
        let open = gtk::GestureClick::new();
        open.connect_released(move |_, _, _, _| a.play_resume(resume_video.clone()));
        row.add_controller(open);
        let a = app.clone();
        let url = video.url.clone();
        remove.connect_clicked(move |_| {
            a.cfg.borrow_mut().resume_videos.retain(|item| item.url != url);
            a.cfg.borrow_mut().resume_positions.remove(&url);
            let cfg = a.cfg.borrow().clone();
            if let Err(err) = config::save(&cfg) {
                a.toast(&format!("{}: {err}", a.text("Kayıt silinemedi", "Could not remove item")));
            } else {
                render(&a);
            }
        });
        list.append(&row);
    }
    container.append(&list);
}

fn card(
    app: &Rc<App>,
    followed: &config::Followed,
    channel: Option<&crate::api::Channel>,
) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("channel-card");
    button.set_hexpand(true);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 5);
    content.set_margin_top(14);
    content.set_margin_bottom(14);
    content.set_margin_start(16);
    content.set_margin_end(16);

    let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some(&followed.name));
    name.add_css_class("heading");
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name_row.append(&name);

    let live = channel.map(|c| c.is_live()).unwrap_or(false);
    if live {
        let badge = gtk::Label::new(Some(app.text("CANLI", "LIVE")));
        badge.add_css_class("live-badge");
        name_row.append(&badge);
    }
    content.append(&name_row);

    let info_text = match channel {
        Some(ch) if ch.is_live() => if app.is_english() { format!("{} viewers", fmt::human_count(ch.viewers())) } else { format!("{} izleyici", fmt::human_count(ch.viewers())) },
        Some(_) => app.text("Çevrimdışı", "Offline").to_string(),
        None => app.text("Durum bilinmiyor", "Status unknown").to_string(),
    };
    let info = gtk::Label::new(Some(&info_text));
    info.set_xalign(0.0);
    info.add_css_class("dim-label");
    info.add_css_class("caption");
    content.append(&info);

    if live {
        let stream_title = channel.map(|c| c.stream_title()).unwrap_or("");
        if !stream_title.is_empty() {
            let title = gtk::Label::new(Some(stream_title));
            title.set_xalign(0.0);
            title.set_wrap(true);
            title.set_lines(2);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title.add_css_class("caption");
            title.set_margin_top(2);
            content.append(&title);
        }
    }

    button.set_child(Some(&content));

    let a = app.clone();
    let slug = followed.slug.clone();
    button.connect_clicked(move |_| a.open_channel(slug.clone(), false));
    button
}

fn empty_state(app: &Rc<App>, container: &gtk::Box) {
    let hero = gtk::Label::new(Some("Kick Lite"));
    hero.add_css_class("hero-title");
    hero.set_halign(gtk::Align::Center);

    let description = gtk::Label::new(Some(app.text("Kick.com canlı yayınlarını ve yayın tekrarlarını hafif bir oynatıcıyla izle.", "Watch Kick.com livestreams and replays with a lightweight player.")));
    description.add_css_class("dim-label");
    description.set_wrap(true);
    description.set_justify(gtk::Justification::Center);
    description.set_halign(gtk::Align::Center);

    let hint_text = if app.is_english() {
        "To get started, enter a channel name above (e.g. xqc)\nor paste a Kick link."
    } else {
        "Başlamak için üstteki arama alanına bir kanal adı yaz (ör. xqc)\n\
         ya da bir Kick bağlantısı yapıştır."
    };
    let hint = gtk::Label::new(Some(hint_text));
    hint.set_wrap(true);
    hint.set_justify(gtk::Justification::Center);
    hint.set_halign(gtk::Align::Center);
    hint.set_margin_top(6);

    let examples = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    examples.set_halign(gtk::Align::Center);
    examples.set_margin_top(8);
    for slug in ["xqc", "kaicenat", "trainwreck"] {
        let button = gtk::Button::with_label(slug);
        button.add_css_class("pill");
        button.set_tooltip_text(Some(&format!("{}", if app.is_english() { format!("Open and follow {slug}") } else { format!("{slug} kanalını aç ve takip et") })));
        let a = app.clone();
        let slug_owned = slug.to_string();
        button.connect_clicked(move |_| a.open_channel(slug_owned.clone(), true));
        examples.append(&button);
    }

    let footer = gtk::Label::new(Some(app.text("Takip ettiğin kanallar kenar çubuğunda listelenir; canlı durumları dakikalar içinde güncellenir.", "Followed channels appear in the sidebar; live status updates every few minutes.")));
    footer.add_css_class("dim-label");
    footer.add_css_class("caption");
    footer.set_wrap(true);
    footer.set_justify(gtk::Justification::Center);
    footer.set_halign(gtk::Align::Center);
    footer.set_margin_top(18);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_halign(gtk::Align::Center);
    content.append(&hero);
    content.append(&description);
    content.append(&hint);
    content.append(&examples);
    content.append(&footer);
    container.append(&content);
}
