//! Downloads page: offline replays and progress of active downloads.

use crate::config;
use crate::fmt;
use crate::ui::{clear_container, ActiveDownload, App};
use gtk::prelude::*;
use std::rc::Rc;

pub fn render(app: &Rc<App>) {
    let container = &app.downloads_box;
    clear_container(container);
    container.set_valign(gtk::Align::Start);

    let title = gtk::Label::new(Some(app.text("İndirilenler", "Downloads")));
    title.add_css_class("title-1");
    title.set_xalign(0.0);
    container.append(&title);

    let active: Vec<(String, ActiveDownload)> = app
        .active_downloads
        .borrow()
        .iter()
        .map(|(id, download)| (id.clone(), download.clone()))
        .collect();
    let items: Vec<config::DownloadItem> = app.cfg.borrow().downloads.clone();

    if active.is_empty() && items.is_empty() {
        container.set_valign(gtk::Align::Fill);
        let empty = gtk::Box::new(gtk::Orientation::Vertical, 10);
        empty.set_valign(gtk::Align::Center);
        empty.set_halign(gtk::Align::Center);
        empty.set_vexpand(true);

        let icon = gtk::Image::from_icon_name("folder-download-symbolic");
        icon.set_icon_size(gtk::IconSize::Large);
        icon.add_css_class("dim-label");
        let heading = gtk::Label::new(Some(app.text("Henüz indirilen yayın yok", "No downloads yet")));
        heading.add_css_class("title-3");
        let hint = gtk::Label::new(Some(app.text(
            "Bir yayın tekrarındaki indirme düğmesiyle kalite seçerek indirebilirsin.\n\
             İndirilenler internet olmadan da oynatılabilir.",
            "Use the download button on a replay to pick a quality and save it\n\
             for offline viewing.",
        )));
        hint.add_css_class("dim-label");
        hint.add_css_class("caption");
        hint.set_justify(gtk::Justification::Center);
        hint.set_wrap(true);

        empty.append(&icon);
        empty.append(&heading);
        empty.append(&hint);
        container.append(&empty);
        return;
    }

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    list.set_margin_top(4);

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
