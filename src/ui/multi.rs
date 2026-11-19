//! Extra players for watching several streams at once.
//!
//! Each stream owns its own `PlayerPage`, and therefore its own `GLArea` and
//! mpv render context. Swapping two streams only exchanges the player handles
//! and their metadata between the two slots; the render surfaces stay put, so
//! no stream is reloaded and no render context is invalidated.

use crate::mpv;
use crate::ui::player::PlayerPage;
use crate::ui::{App, Media};
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub struct MiniPlayer {
    pub id: u64,
    /// Repointable handle to the player currently in this mini slot.
    pub player: RefCell<mpv::SharedPlayer>,
    pub media: RefCell<Media>,
    pub page: RefCell<Rc<PlayerPage>>,
}

impl MiniPlayer {
    pub fn new(app: &Rc<App>, id: u64, media: Media) -> Option<Rc<Self>> {
        let cfg = app.cfg.borrow();
        let raw = match mpv::Player::new_mini(cfg.silence_noise_db, cfg.silence_duration) {
            Ok(player) => player,
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
        let page_player = mpv::SharedPlayer::new(raw);
        // Several streams playing sound at once is unusable; minis start muted
        // and inherit the normal volume when promoted.
        page_player.set_mute(true);

        let mini_player = page_player.fork();
        let page = Rc::new(PlayerPage::new(page_player.clone()));
        page.wire(app.clone());
        // wire() applies the global volume/mute to every page; minis must stay
        // silent until they are promoted.
        page_player.set_mute(true);
        page.set_media(&media);
        page.hide_status();
        page.set_controls_visible(false);
        page.set_mini_click_enabled(true);
        page.mini_close.set_visible(true);

        if let Err(err) = page_player.load(&media.url, None) {
            app.toast(&format!("{err:#}"));
            return None;
        }

        Some(Rc::new(Self {
            id,
            player: RefCell::new(mini_player),
            media: RefCell::new(media),
            page: RefCell::new(page),
        }))
    }

    /// Refreshes the title and play icon after a swap.
    pub fn update_media_ui(&self) {
        let page = self.page.borrow();
        page.set_media_info(&self.media.borrow());
        page.set_paused(self.player.borrow().is_paused());
    }
}
