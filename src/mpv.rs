//! Minimal FFI for libmpv and video rendering through OpenGL (GtkGLArea).
//!
//! mpv's "render API" is used: the video is drawn not in mpv's own window,
//! but into the framebuffer of GTK4's GLArea. This allows playback in a
//! single window with low memory usage on both Wayland and X11.

#![allow(non_snake_case)]

use anyhow::{bail, Result};
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::ptr;

#[repr(C)]
pub struct mpv_handle {
    _private: [u8; 0],
}

#[repr(C)]
pub struct mpv_render_context {
    _private: [u8; 0],
}

#[repr(C)]
pub struct mpv_event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

/// Data structure of the `MPV_EVENT_LOG_MESSAGE` event (client.h).
#[repr(C)]
pub struct mpv_event_log_message {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
    pub log_level: c_int,
}

// mpv_event_id (only the values we use)
const MPV_EVENT_NONE: c_int = 0;
const MPV_EVENT_SHUTDOWN: c_int = 1;
const MPV_EVENT_LOG_MESSAGE: c_int = 2;

/// Playback speed used during silence. At normal speed a 4-second gap
/// shrinks to ~0.4 seconds; this gives the "skipping" effect.
const SKIP_SPEED: f64 = 10.0;
/// Allowed position difference (seconds) for a `silence_start` report to
/// be considered relevant to the current playback.
const SILENCE_POS_TOLERANCE: f64 = 2.5;
/// Maximum number of events processed in one call (to avoid clogging the queue).
const MAX_EVENTS_PER_POLL: usize = 256;
/// Label of the `astats` filter that measures the current audio level. It is
/// exposed by mpv under `af-metadata/<label>/...` and is read to draw the live
/// level meter in the silence settings. `reset=1` keeps the statistics scoped
/// to a single frame, so the reported value follows the audio in real time.
const LEVEL_FILTER: &str = "@level:lavfi=[astats=metadata=1:reset=1]";
const LEVEL_PROPERTY: &str = "af-metadata/level/lavfi.astats.Overall.RMS_level";

#[repr(C)]
#[derive(Clone, Copy)]
pub struct mpv_render_param {
    pub type_: c_int,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_opengl_init_params {
    pub get_proc_address: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void>,
    pub get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
pub struct mpv_opengl_fbo {
    pub fbo: c_int,
    pub w: c_int,
    pub h: c_int,
    pub internal_format: c_int,
}

// mpv_render_param_type
const MPV_RENDER_PARAM_INVALID: c_int = 0;
const MPV_RENDER_PARAM_API_TYPE: c_int = 1;
const MPV_RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const MPV_RENDER_PARAM_OPENGL_FBO: c_int = 3;
const MPV_RENDER_PARAM_FLIP_Y: c_int = 4;

// mpv_format
const MPV_FORMAT_FLAG: c_int = 3;
const MPV_FORMAT_INT64: c_int = 4;
const MPV_FORMAT_DOUBLE: c_int = 5;

const GL_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
const GL_COLOR_BUFFER_BIT: u32 = 0x0000_4000;

#[link(name = "mpv")]
extern "C" {
    fn mpv_create() -> *mut mpv_handle;
    fn mpv_initialize(ctx: *mut mpv_handle) -> c_int;
    fn mpv_terminate_destroy(ctx: *mut mpv_handle);
    fn mpv_set_option_string(
        ctx: *mut mpv_handle,
        name: *const c_char,
        data: *const c_char,
    ) -> c_int;
    fn mpv_set_property(
        ctx: *mut mpv_handle,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    fn mpv_get_property(
        ctx: *mut mpv_handle,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    fn mpv_get_property_string(ctx: *mut mpv_handle, name: *const c_char) -> *mut c_char;
    fn mpv_free(data: *mut c_void);
    fn mpv_command(ctx: *mut mpv_handle, args: *const *const c_char) -> c_int;
    fn mpv_wait_event(ctx: *mut mpv_handle, timeout: c_double) -> *mut mpv_event;
    fn mpv_request_log_messages(ctx: *mut mpv_handle, min_level: *const c_char) -> c_int;

    fn mpv_render_context_create(
        res: *mut *mut mpv_render_context,
        mpv: *mut mpv_handle,
        params: *mut mpv_render_param,
    ) -> c_int;
    fn mpv_render_context_set_update_callback(
        ctx: *mut mpv_render_context,
        callback: Option<unsafe extern "C" fn(*mut c_void)>,
        callback_ctx: *mut c_void,
    );
    fn mpv_render_context_render(
        ctx: *mut mpv_render_context,
        params: *mut mpv_render_param,
    ) -> c_int;
    fn mpv_render_context_report_swap(ctx: *mut mpv_render_context);
    fn mpv_render_context_free(ctx: *mut mpv_render_context);
}

#[link(name = "EGL")]
extern "C" {
    fn eglGetProcAddress(name: *const c_char) -> *mut c_void;
}

#[link(name = "GL")]
extern "C" {
    fn glXGetProcAddressARB(name: *const u8) -> *mut c_void;
}

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// RTLD_DEFAULT on glibc ((void *) 0)
const RTLD_DEFAULT: *mut c_void = std::ptr::null_mut();

pub struct Player {
    handle: *mut mpv_handle,
    render: Cell<*mut mpv_render_context>,
    /// Owner of the WeakRef used by the update callback.
    keep: RefCell<Option<Box<AreaWeak>>>,
    alive: Cell<bool>,
    /// User-controlled preference (on/off).
    skip_silence: Cell<bool>,
    /// Whether skipping is allowed for the current media (VODs only).
    silence_allowed: Cell<bool>,
    /// Whether playback is currently sped up inside a silent section.
    silence_active: Cell<bool>,
    /// Playback speed before the silence (restored afterwards).
    saved_speed: Cell<f64>,
}

impl Player {
    pub fn new(noise_db: i32, duration: f64) -> Result<Self> {
        unsafe {
            let handle = mpv_create();
            if handle.is_null() {
                bail!("Could not create libmpv");
            }
            let player = Self {
                handle,
                render: Cell::new(ptr::null_mut()),
                keep: RefCell::new(None),
                alive: Cell::new(true),
                skip_silence: Cell::new(false),
                silence_allowed: Cell::new(false),
                silence_active: Cell::new(false),
                saved_speed: Cell::new(1.0),
            };
            for (name, value) in options(noise_db, duration) {
                if let Err(err) = player.set_option(name, &value) {
                    eprintln!("mpv: {err}");
                }
            }
            // Since the `silencedetect` log level is "v", we lower the minimum level
            // for messages sent to the client. `msg-level=ffmpeg=v` only produces
            // messages related to this filter; other modules stay silent.
            let level = CString::new("terminal-default").unwrap();
            mpv_request_log_messages(handle, level.as_ptr());
            let rc = mpv_initialize(handle);
            if rc < 0 {
                mpv_terminate_destroy(handle);
                bail!("Could not initialize mpv (error code {rc})");
            }
            Ok(player)
        }
    }

    fn set_option(&self, name: &str, value: &str) -> Result<()> {
        let name = CString::new(name)?;
        let value = CString::new(value)?;
        let rc = unsafe { mpv_set_option_string(self.handle, name.as_ptr(), value.as_ptr()) };
        if rc < 0 {
            bail!("Could not apply mpv option: {}", name.to_string_lossy());
        }
        Ok(())
    }

    /// Sets up the mpv render context when the GLArea is realized.
    pub fn attach(&self, area: &gtk::GLArea) -> bool {
        if !self.render.get().is_null() {
            return true;
        }
        area.make_current();

        let api = CString::new("opengl").unwrap();
        let mut init = mpv_opengl_init_params {
            get_proc_address: Some(gl_proc_address),
            get_proc_address_ctx: ptr::null_mut(),
        };
        let mut params = [
            mpv_render_param {
                type_: MPV_RENDER_PARAM_API_TYPE,
                data: api.as_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: &mut init as *mut _ as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];

        let mut ctx: *mut mpv_render_context = ptr::null_mut();
        let rc = unsafe { mpv_render_context_create(&mut ctx, self.handle, params.as_mut_ptr()) };
        if rc < 0 {
            eprintln!("mpv: could not create render context (error code {rc})");
            return false;
        }

        let weak = Box::new(AreaWeak(area.downgrade()));
        let weak_ptr = &*weak as *const AreaWeak as *mut c_void;
        unsafe {
            mpv_render_context_set_update_callback(ctx, Some(on_update), weak_ptr);
        }
        self.keep.replace(Some(weak));
        self.render.set(ctx);
        true
    }

    /// Releases the render context when the GLArea is unrealized.
    pub fn detach(&self) {
        let ctx = self.render.replace(ptr::null_mut());
        if !ctx.is_null() {
            unsafe { mpv_render_context_free(ctx) };
        }
        self.keep.replace(None);
    }

    pub fn render(&self, area: &gtk::GLArea) {
        let ctx = self.render.get();
        if ctx.is_null() {
            return;
        }
        if area.width() <= 0 || area.height() <= 0 {
            return;
        }
        area.make_current();

        let scale = area.scale_factor();
        let width = area.width() * scale;
        let height = area.height() * scale;

        unsafe {
            let (Some(clear_color), Some(clear), Some(get_integerv)) =
                (gl::clear_color(), gl::clear(), gl::get_integerv())
            else {
                eprintln!("mpv: could not load OpenGL functions");
                return;
            };

            clear_color(0.0, 0.0, 0.0, 1.0);
            clear(GL_COLOR_BUFFER_BIT);

            let mut fbo_id: c_int = 0;
            get_integerv(GL_FRAMEBUFFER_BINDING, &mut fbo_id);

            let mut fbo = mpv_opengl_fbo {
                fbo: fbo_id,
                w: width,
                h: height,
                internal_format: 0,
            };
            let flip_y: c_int = 1;
            let mut params = [
                mpv_render_param {
                    type_: MPV_RENDER_PARAM_OPENGL_FBO,
                    data: &mut fbo as *mut _ as *mut c_void,
                },
                mpv_render_param {
                    type_: MPV_RENDER_PARAM_FLIP_Y,
                    data: &flip_y as *const _ as *mut c_void,
                },
                mpv_render_param {
                    type_: MPV_RENDER_PARAM_INVALID,
                    data: ptr::null_mut(),
                },
            ];
            let rc = mpv_render_context_render(ctx, params.as_mut_ptr());
            if rc >= 0 {
                mpv_render_context_report_swap(ctx);
            }
        }
    }

    pub fn load(&self, url: &str, start: Option<f64>) -> Result<()> {
        self.set_paused(false);
        self.clear_silence();
        // The initial seek is applied after the file is loaded and the duration is
        // available. On some HLS VODs, a `start` parameter passed with the loadfile
        // options can cause the stream to fail.
        let _ = start;
        self.command(&["loadfile", url, "replace"])
    }

    pub fn stop(&self) {
        self.clear_silence();
        let _ = self.command(&["stop"]);
    }

    pub fn command(&self, args: &[&str]) -> Result<()> {
        let cstrings: Vec<CString> = args
            .iter()
            .map(|a| CString::new(*a))
            .collect::<std::result::Result<_, _>>()?;
        let mut ptrs: Vec<*const c_char> = cstrings.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());
        let rc = unsafe { mpv_command(self.handle, ptrs.as_ptr()) };
        if rc < 0 {
            bail!("mpv command failed: {:?} (code {rc})", args);
        }
        Ok(())
    }

    pub fn set_paused(&self, paused: bool) {
        self.set_flag("pause", paused);
    }

    pub fn toggle_pause(&self) {
        let paused = self.is_paused();
        self.set_paused(!paused);
    }

    pub fn set_mute(&self, muted: bool) {
        self.set_flag("mute", muted);
    }

    pub fn set_volume(&self, volume: f64) {
        let name = CString::new("volume").unwrap();
        let mut value = volume.clamp(0.0, 100.0);
        unsafe {
            mpv_set_property(
                self.handle,
                name.as_ptr(),
                MPV_FORMAT_DOUBLE,
                &mut value as *mut _ as *mut c_void,
            );
        }
    }

    pub fn set_speed(&self, speed: f64) {
        let value = speed.clamp(0.25, 8.0);
        if self.silence_active.get() {
            // If the user changes the speed during a silence, we return to
            // this new speed once the silence ends.
            self.saved_speed.set(value);
        } else {
            self.set_speed_raw(value);
        }
    }

    fn set_flag(&self, name: &str, value: bool) {
        if let Ok(name) = CString::new(name) {
            let mut v: c_int = if value { 1 } else { 0 };
            unsafe {
                mpv_set_property(
                    self.handle,
                    name.as_ptr(),
                    MPV_FORMAT_FLAG,
                    &mut v as *mut _ as *mut c_void,
                );
            }
        }
    }

    pub fn position(&self) -> Option<f64> {
        self.get_f64("time-pos")
    }

    pub fn duration(&self) -> Option<f64> {
        self.get_f64("duration")
    }

    /// Duration buffered by mpv ahead of the playback position (seconds).
    pub fn cache_duration(&self) -> Option<f64> {
        self.get_f64("demuxer-cache-duration")
    }

    pub fn is_paused(&self) -> bool {
        self.get_flag("pause").unwrap_or(false)
    }

    pub fn is_muted(&self) -> bool {
        self.get_flag("mute").unwrap_or(false)
    }

    pub fn is_idle(&self) -> bool {
        self.get_flag("idle-active").unwrap_or(true)
    }

    pub fn video_height(&self) -> i64 {
        self.get_i64("height").unwrap_or(0)
    }

    pub fn seek(&self, seconds: f64, exact: bool) {
        let target = format!("{seconds:.3}");
        let mode = if exact { "absolute+exact" } else { "absolute" };
        let _ = self.command(&["seek", &target, mode]);
    }

    pub fn seek_relative(&self, seconds: f64) {
        let delta = format!("{seconds:.3}");
        let _ = self.command(&["seek", &delta, "relative+exact"]);
    }

    // ---- skipping silent parts ----

    /// Current RMS audio level in dBFS, or `None` while no audio is playing.
    /// Digital silence is reported as negative infinity.
    pub fn audio_level_db(&self) -> Option<f64> {
        let name = CString::new(LEVEL_PROPERTY).ok()?;
        let ptr = unsafe { mpv_get_property_string(self.handle, name.as_ptr()) };
        if ptr.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        unsafe { mpv_free(ptr as *mut c_void) };
        let value = text.trim();
        if value == "-inf" {
            Some(f64::NEG_INFINITY)
        } else {
            value.parse().ok()
        }
    }

    /// Turns the feature on/off. When disabled, any sped-up playback returns
    /// to the previous speed.
    pub fn set_skip_silence(&self, enabled: bool) {
        self.skip_silence.set(enabled);
        if !enabled {
            self.end_silence_skip();
        }
    }

    /// Changes the silence-detection thresholds. The `af set` command rebuilds
    /// the audio chain; playback continues.
    pub fn apply_silence_filter(&self, noise_db: i32, duration: f64) -> Result<()> {
        self.end_silence_skip();
        self.command(&["af", "set", &audio_filters(noise_db, duration)])
    }

    /// No skipping in live streams; only allowed in VODs.
    pub fn set_silence_allowed(&self, allowed: bool) {
        self.silence_allowed.set(allowed);
        if !allowed {
            self.end_silence_skip();
        }
    }

    /// Restores the speed when loading a new file or stopping playback.
    pub fn clear_silence(&self) {
        self.end_silence_skip();
    }

    fn end_silence_skip(&self) {
        if self.silence_active.replace(false) {
            self.set_speed_raw(self.saved_speed.get());
        }
    }

    /// Drains the mpv event queue and processes `silencedetect` reports.
    /// Must be called from the main loop (GTK thread); event data is valid only
    /// until the next `mpv_wait_event()` call.
    pub fn poll_events(&self) {
        if !self.alive.get() {
            return;
        }
        for _ in 0..MAX_EVENTS_PER_POLL {
            let event = unsafe { mpv_wait_event(self.handle, 0.0) };
            if event.is_null() {
                return;
            }
            let event = unsafe { &*event };
            match event.event_id {
                MPV_EVENT_NONE | MPV_EVENT_SHUTDOWN => return,
                MPV_EVENT_LOG_MESSAGE if !event.data.is_null() => {
                    let msg = unsafe { &*(event.data as *const mpv_event_log_message) };
                    if msg.text.is_null() {
                        continue;
                    }
                    let text = unsafe { CStr::from_ptr(msg.text) }.to_string_lossy();
                    self.handle_silence_message(&text);
                }
                _ => {}
            }
        }
    }

    /// Temporarily increases playback speed (then restores it) when
    /// `silencedetect` reports that a silence has started/ended.
    ///
    /// Because the `silence_end` report arrives when playback has already
    /// reached the end of the silence, speeding up through the silence is more
    /// reliable than seeking: detection runs in sync with playback.
    fn handle_silence_message(&self, text: &str) {
        if !self.skip_silence.get() || !self.silence_allowed.get() {
            return;
        }
        if text.contains("silence_start:") {
            if self.silence_active.get() {
                return; // duplicate report
            }
            let Some(start) = number_after(text, "silence_start:") else {
                return;
            };
            // Ignore stale reports (e.g. after a seek).
            if let Some(pos) = self.position() {
                if (pos - start).abs() > SILENCE_POS_TOLERANCE {
                    return;
                }
            }
            self.saved_speed.set(self.get_f64("speed").unwrap_or(1.0));
            self.silence_active.set(true);
            self.set_speed_raw(SKIP_SPEED);
        } else if text.contains("silence_end:") && self.silence_active.get() {
            self.end_silence_skip();
        }
    }

    /// Unrestricted speed setting (may exceed the `speed` range for silence skipping).
    fn set_speed_raw(&self, speed: f64) {
        let name = CString::new("speed").unwrap();
        let mut value = speed.clamp(0.1, 60.0);
        unsafe {
            mpv_set_property(
                self.handle,
                name.as_ptr(),
                MPV_FORMAT_DOUBLE,
                &mut value as *mut _ as *mut c_void,
            );
        }
    }

    fn get_f64(&self, name: &str) -> Option<f64> {
        let name = CString::new(name).ok()?;
        let mut value: c_double = 0.0;
        let rc = unsafe {
            mpv_get_property(
                self.handle,
                name.as_ptr(),
                MPV_FORMAT_DOUBLE,
                &mut value as *mut _ as *mut c_void,
            )
        };
        if rc >= 0 {
            Some(value)
        } else {
            None
        }
    }

    fn get_i64(&self, name: &str) -> Option<i64> {
        let name = CString::new(name).ok()?;
        let mut value: i64 = 0;
        let rc = unsafe {
            mpv_get_property(
                self.handle,
                name.as_ptr(),
                MPV_FORMAT_INT64,
                &mut value as *mut _ as *mut c_void,
            )
        };
        if rc >= 0 {
            Some(value)
        } else {
            None
        }
    }

    fn get_flag(&self, name: &str) -> Option<bool> {
        let name = CString::new(name).ok()?;
        let mut value: c_int = 0;
        let rc = unsafe {
            mpv_get_property(
                self.handle,
                name.as_ptr(),
                MPV_FORMAT_FLAG,
                &mut value as *mut _ as *mut c_void,
            )
        };
        if rc >= 0 {
            Some(value != 0)
        } else {
            None
        }
    }

    pub fn shutdown(&self) {
        if !self.alive.replace(false) {
            return;
        }
        self.detach();
        unsafe { mpv_terminate_destroy(self.handle) };
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Reads the first number after `key` in the text (for mpv/ffmpeg log lines).
fn number_after(text: &str, key: &str) -> Option<f64> {
    let rest = &text.get(text.find(key)? + key.len()..)?;
    let rest = rest.trim_start();
    let len = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    rest[..len].parse().ok()
}

/// Builds the audio filter chain: `silencedetect` drives the skipping and
/// `astats` (with a stable label) feeds the live level meter in the settings.
fn audio_filters(noise_db: i32, duration: f64) -> String {
    format!(
        "{LEVEL_FILTER},lavfi=[silencedetect=noise={noise_db}dB:d={duration:.2}]"
    )
}

fn options(noise_db: i32, duration: f64) -> Vec<(&'static str, String)> {
    vec![
        ("terminal", "no".into()),
        ("config", "no".into()),
        ("vo", "libmpv".into()),
        ("osc", "no".into()),
        ("osd-level", "0".into()),
        ("input-default-bindings", "no".into()),
        ("input-vo-keyboard", "no".into()),
        ("idle", "yes".into()),
        ("keep-open", "no".into()),
        ("hwdec", "auto-safe".into()),
        ("cache", "yes".into()),
        ("demuxer-max-bytes", "48MiB".into()),
        ("demuxer-max-back-bytes", "16MiB".into()),
        ("demuxer-readahead-secs", "20".into()),
        ("user-agent", crate::api::UA.into()),
        ("referrer", "https://kick.com/".into()),
        ("ytdl", "no".into()),
        ("sub-auto", "no".into()),
        ("audio-file-auto", "no".into()),
        ("audio-client-name", "Kick Lite".into()),
        ("volume-max", "100".into()),
        // Silence detection is always enabled; the skipping behavior is toggled by
        // the `skip_silence` flag. This way the filter is not restarted at startup,
        // and position/timestamps do not drift.
        ("af", audio_filters(noise_db, duration)),
        ("msg-level", "ffmpeg=v".into()),
    ]
}

unsafe extern "C" fn gl_proc_address(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    // Try EGL first (Wayland and most X11 setups), then GLX, and finally the
    // directly loaded GL library.
    let proc = eglGetProcAddress(name);
    if !proc.is_null() {
        return proc;
    }
    let proc = glXGetProcAddressARB(name as *const u8);
    if !proc.is_null() {
        return proc;
    }
    dlsym(RTLD_DEFAULT, name)
}

/// Loads the few required OpenGL functions at runtime.
///
/// We use EGL/GLX directly instead of libepoxy: this avoids depending on an
/// extra library and its context table (epoxy could crash in GTK's EGL context).
mod gl {
    use super::*;
    use std::sync::OnceLock;

    type ClearColorFn = unsafe extern "C" fn(f32, f32, f32, f32);
    type ClearFn = unsafe extern "C" fn(u32);
    type GetIntegervFn = unsafe extern "C" fn(u32, *mut c_int);

    fn lookup(name: &str) -> usize {
        let Ok(name) = CString::new(name) else {
            return 0;
        };
        unsafe { gl_proc_address(ptr::null_mut(), name.as_ptr()) as usize }
    }

    pub fn clear_color() -> Option<ClearColorFn> {
        static FN: OnceLock<usize> = OnceLock::new();
        let ptr = *FN.get_or_init(|| lookup("glClearColor"));
        (ptr != 0).then(|| unsafe { std::mem::transmute::<usize, ClearColorFn>(ptr) })
    }

    pub fn clear() -> Option<ClearFn> {
        static FN: OnceLock<usize> = OnceLock::new();
        let ptr = *FN.get_or_init(|| lookup("glClear"));
        (ptr != 0).then(|| unsafe { std::mem::transmute::<usize, ClearFn>(ptr) })
    }

    pub fn get_integerv() -> Option<GetIntegervFn> {
        static FN: OnceLock<usize> = OnceLock::new();
        let ptr = *FN.get_or_init(|| lookup("glGetIntegerv"));
        (ptr != 0).then(|| unsafe { std::mem::transmute::<usize, GetIntegervFn>(ptr) })
    }
}

/// GTK objects are not Send; we only use a pointer to carry the weak
/// reference from mpv's thread to the main loop.
/// `upgrade()` and `queue_render()` always run on the main thread.
struct AreaWeak(glib::WeakRef<gtk::GLArea>);
unsafe impl Send for AreaWeak {}

impl AreaWeak {
    /// Returns the `GLArea` if it is still alive (call only on the main thread).
    fn area(&self) -> Option<gtk::GLArea> {
        self.0.upgrade()
    }
}

/// A copy of the weak reference in mpv's callback context.
fn clone_area_weak(ctx: *mut c_void) -> AreaWeak {
    unsafe {
        let weak = &*(ctx as *const AreaWeak);
        AreaWeak(weak.0.clone())
    }
}

/// Called by mpv (from its own thread) when it draws a new frame.
unsafe extern "C" fn on_update(ctx: *mut c_void) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static PENDING: AtomicBool = AtomicBool::new(false);

    // Do not accumulate multiple pending requests; the main loop will draw the frame anyway.
    if PENDING.swap(true, Ordering::SeqCst) {
        return;
    }

    let weak = clone_area_weak(ctx);
    // `invoke` is thread-safe; the closure runs on the main loop.
    glib::MainContext::default().invoke(move || {
        PENDING.store(false, Ordering::SeqCst);
        if let Some(area) = weak.area() {
            if area.is_mapped() {
                area.queue_render();
            }
        }
    });
}

// mpv events are currently unused; can be extended later for chat/status.
#[allow(dead_code)]
fn _unused(event: *mut mpv_event) {
    let _ = event;
}
