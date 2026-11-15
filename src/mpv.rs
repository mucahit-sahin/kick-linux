//! libmpv için minimal FFI ve OpenGL (GtkGLArea) üzerinden video çizimi.
//!
//! mpv'nin "render API"si kullanılır: video, mpv'nin kendi penceresinde değil,
//! GTK4'ün GLArea'sının framebuffer'ına çizilir. Böylece Wayland ve X11'de
//! tek pencere içinde, düşük bellek kullanımıyla oynatma yapılabilir.

#![allow(non_snake_case)]

use anyhow::{bail, Result};
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_double, c_int, c_void, CString};
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
    fn mpv_command(ctx: *mut mpv_handle, args: *const *const c_char) -> c_int;

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

/// glibc'te RTLD_DEFAULT ((void *) 0)
const RTLD_DEFAULT: *mut c_void = std::ptr::null_mut();

pub struct Player {
    handle: *mut mpv_handle,
    render: Cell<*mut mpv_render_context>,
    /// Güncelleme geri çağrısının kullandığı WeakRef'in sahibi.
    keep: RefCell<Option<Box<AreaWeak>>>,
    alive: Cell<bool>,
}

impl Player {
    pub fn new() -> Result<Self> {
        unsafe {
            let handle = mpv_create();
            if handle.is_null() {
                bail!("libmpv oluşturulamadı");
            }
            let player = Self {
                handle,
                render: Cell::new(ptr::null_mut()),
                keep: RefCell::new(None),
                alive: Cell::new(true),
            };
            for (name, value) in options() {
                if let Err(err) = player.set_option(name, &value) {
                    eprintln!("mpv: {err}");
                }
            }
            let rc = mpv_initialize(handle);
            if rc < 0 {
                mpv_terminate_destroy(handle);
                bail!("mpv başlatılamadı (hata kodu {rc})");
            }
            Ok(player)
        }
    }

    fn set_option(&self, name: &str, value: &str) -> Result<()> {
        let name = CString::new(name)?;
        let value = CString::new(value)?;
        let rc = unsafe { mpv_set_option_string(self.handle, name.as_ptr(), value.as_ptr()) };
        if rc < 0 {
            bail!("mpv ayarı uygulanamadı: {}", name.to_string_lossy());
        }
        Ok(())
    }

    /// GLArea gerçekleştiğinde (realize) mpv render bağlamını kurar.
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
            eprintln!("mpv: render bağlamı kurulamadı (hata kodu {rc})");
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

    /// GLArea kaybolduğunda render bağlamını serbest bırakır.
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
                eprintln!("mpv: OpenGL işlevleri yüklenemedi");
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
        // Başlangıç seek'i dosya yüklenip süre bilgisi hazır olduktan sonra
        // uygulanır. Bazı HLS VOD'larda loadfile seçenekleriyle verilen start
        // parametresi yayının hata vermesine neden olabiliyor.
        let _ = start;
        self.command(&["loadfile", url, "replace"])
    }

    pub fn stop(&self) {
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
            bail!("mpv komutu başarısız: {:?} (kod {rc})", args);
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
        let name = CString::new("speed").unwrap();
        let mut value = speed.clamp(0.25, 8.0);
        unsafe {
            mpv_set_property(
                self.handle,
                name.as_ptr(),
                MPV_FORMAT_DOUBLE,
                &mut value as *mut _ as *mut c_void,
            );
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

    /// Oynatma konumunun ilerisinde mpv'nin tamponladığı süre (saniye).
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

fn options() -> Vec<(&'static str, String)> {
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
    ]
}

unsafe extern "C" fn gl_proc_address(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    // Önce EGL (Wayland ve çoğu X11 kurulumu), sonra GLX, en son doğrudan
    // yüklenmiş GL kütüphanesinden dene.
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

/// Gereken birkaç OpenGL işlevini çalışma anında yükler.
///
/// libepoxy yerine doğrudan EGL/GLX kullanıyoruz: böylece ek bir kütüphaneye
/// ve onun bağlam tablosuna bağımlı değiliz (epoxy, GTK'nın EGL bağlamında
/// çökebiliyordu).
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

/// Gtk nesneleri Send değildir; ama zayıf başvuruyu mpv'nin iş parçacığından
/// ana döngüye taşımak için yalnızca bir işaretçi olarak kullanıyoruz.
/// `upgrade()` ve `queue_render()` her zaman ana iş parçacığında çalışır.
struct AreaWeak(glib::WeakRef<gtk::GLArea>);
unsafe impl Send for AreaWeak {}

impl AreaWeak {
    /// `GLArea` hâlâ yaşıyorsa döndürür (yalnızca ana iş parçacığında çağırın).
    fn area(&self) -> Option<gtk::GLArea> {
        self.0.upgrade()
    }
}

/// mpv'nin geri çağrı bağlamındaki zayıf başvurunun bir kopyası.
fn clone_area_weak(ctx: *mut c_void) -> AreaWeak {
    unsafe {
        let weak = &*(ctx as *const AreaWeak);
        AreaWeak(weak.0.clone())
    }
}

/// mpv yeni bir kare çizdiğinde (kendi iş parçacığından) çağrılır.
unsafe extern "C" fn on_update(ctx: *mut c_void) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static PENDING: AtomicBool = AtomicBool::new(false);

    // Aynı anda birden çok istek birikmesin; ana döngü zaten kareyi çizecek.
    if PENDING.swap(true, Ordering::SeqCst) {
        return;
    }

    let weak = clone_area_weak(ctx);
    // `invoke` iş parçacığı güvenlidir; kapanış ana döngüde çalışır.
    glib::MainContext::default().invoke(move || {
        PENDING.store(false, Ordering::SeqCst);
        if let Some(area) = weak.area() {
            if area.is_mapped() {
                area.queue_render();
            }
        }
    });
}

// mpv olayları şu an kullanılmıyor; ileride sohbet/durum için genişletilebilir.
#[allow(dead_code)]
fn _unused(event: *mut mpv_event) {
    let _ = event;
}
