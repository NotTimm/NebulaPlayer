//! Video inside the app's window.
//!
//! mpv draws each frame through its render API on a thread of its own, at
//! the size the video takes on screen: into an OpenGL texture on an
//! off-screen EGL context (Linux), or with its software renderer anywhere
//! else or when EGL can't be had. The frame is read back into memory and
//! handed to the UI, where [`Video`] (a wgpu shader widget) uploads it to a
//! texture and draws it. mpv still does the scaling, colour conversion,
//! subtitles and on-screen text; the UI only copies pixels.
//!
//! Reading frames back costs a copy per frame (about 8 MB at 1080p), which
//! modern machines handle easily; handing the GPU texture over directly
//! (dmabuf) would avoid it, and is the next step if it ever matters.

use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};

use cosmic::iced::widget::shader;
use cosmic::iced::{mouse, wgpu, Rectangle};

use crate::mpv::{MpvHandle, RenderApi};

/// The largest frame rendered, in pixels across: beyond it the GPU scales
/// up the rest of the way, which keeps the per-frame copy in check on 4K
/// screens.
const MAX_WIDTH: u32 = 2560;

/// A frame rendered and read back: RGBA (or RGB0), rows top to bottom.
#[derive(Default)]
struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

#[derive(Default)]
struct Wake {
    /// mpv has something new to draw.
    update: bool,
    /// The video's size on screen changed.
    resized: bool,
    quit: bool,
}

/// What the render thread and the UI share.
pub struct Output {
    /// The newest frame.
    frame: Mutex<Frame>,
    /// Counts frames, so the UI uploads each one once.
    serial: AtomicU64,
    /// The size to render at, in physical pixels.
    target: Mutex<(u32, u32)>,
    wake: Mutex<Wake>,
    cv: Condvar,
}

/// Rings when a new frame is ready; the app's subscription redraws.
pub static FRAME_READY: LazyLock<tokio::sync::Notify> = LazyLock::new(tokio::sync::Notify::new);

impl Output {
    fn signal(&self, f: impl FnOnce(&mut Wake)) {
        f(&mut self.wake.lock().unwrap());
        self.cv.notify_one();
    }

    /// mpv is shutting down: the render thread lets go of it.
    pub fn quit(&self) {
        self.signal(|w| w.quit = true);
    }

    fn set_target(&self, w: u32, h: u32) {
        let (w, h) = if w > MAX_WIDTH { (MAX_WIDTH, (h as u64 * MAX_WIDTH as u64 / w as u64) as u32) } else { (w, h) };
        let mut t = self.target.lock().unwrap();
        if *t != (w, h) && w > 0 && h > 0 {
            *t = (w, h);
            drop(t);
            self.signal(|wk| wk.resized = true);
        }
    }
}

/// Called by mpv (on one of its threads) when there's a frame to draw.
extern "C" fn on_update(ctx: *mut c_void) {
    // SAFETY: `ctx` is the Output the render thread keeps alive until it has
    // removed this callback.
    let out = unsafe { &*(ctx as *const Output) };
    out.signal(|w| w.update = true);
}

/// Starts rendering `handle`'s video. `keep` holds the mpv handle alive
/// until the render context is freed, which mpv requires.
pub fn start(api: &'static RenderApi, handle: *mut MpvHandle, keep: Arc<dyn Send + Sync>) -> Result<Arc<Output>, String> {
    let out = Arc::new(Output {
        frame: Mutex::new(Frame::default()),
        serial: AtomicU64::new(0),
        target: Mutex::new((1280, 720)),
        wake: Mutex::new(Wake::default()),
        cv: Condvar::new(),
    });
    let thread_out = out.clone();
    let handle = handle as usize;
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("mpv-render".into())
        .spawn(move || {
            let _keep = keep;
            render_thread(api, handle as *mut MpvHandle, thread_out, ready_tx);
        })
        .map_err(|e| e.to_string())?;
    ready_rx.recv().map_err(|_| "mpv's renderer stopped".to_string())??;
    Ok(out)
}

const UPDATE_FRAME: u64 = 1;

fn render_thread(
    api: &'static RenderApi,
    handle: *mut MpvHandle,
    out: Arc<Output>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    // OpenGL when an EGL context can be had, mpv's software renderer if not.
    #[cfg(target_os = "linux")]
    let mut gl = match gl::Context::new() {
        Ok(gl) => Some(gl),
        Err(e) => {
            log::info!("no OpenGL for video ({e}); using mpv's software renderer");
            None
        }
    };
    #[cfg(not(target_os = "linux"))]
    let mut gl: Option<NoGl> = None;

    let ctx = match create_context(api, handle, gl.as_ref()) {
        Ok(ctx) => ctx,
        Err(e) if gl.is_some() => {
            log::warn!("{e}; trying mpv's software renderer");
            gl = None;
            match create_context(api, handle, None) {
                Ok(ctx) => ctx,
                Err(e) => {
                    let _ = ready.send(Err(e));
                    return;
                }
            }
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    log::info!("video renders with {}", if gl.is_some() { "OpenGL" } else { "mpv's software renderer" });
    // SAFETY: valid context; `out` outlives the callback, which is removed
    // before the context is freed below.
    unsafe { (api.set_update_callback)(ctx, Some(on_update), Arc::as_ptr(&out) as *mut c_void) };
    let _ = ready.send(Ok(()));

    let mut back = Frame::default();
    loop {
        let (update, resized) = {
            let mut w = out.wake.lock().unwrap();
            while !(w.update || w.resized || w.quit) {
                w = out.cv.wait(w).unwrap();
            }
            if w.quit {
                break;
            }
            (std::mem::take(&mut w.update), std::mem::take(&mut w.resized))
        };
        let flags = if update {
            // SAFETY: valid context, on its render thread.
            unsafe { (api.update)(ctx) }
        } else {
            0
        };
        if flags & UPDATE_FRAME == 0 && !resized {
            continue;
        }
        let (w, h) = *out.target.lock().unwrap();
        let rendered = match gl.as_mut() {
            #[cfg(target_os = "linux")]
            Some(gl) => gl.render(api, ctx, w, h, &mut back),
            #[cfg(not(target_os = "linux"))]
            Some(_) => false,
            None => render_sw(api, ctx, w, h, &mut back),
        };
        if rendered {
            std::mem::swap(&mut *out.frame.lock().unwrap(), &mut back);
            out.serial.fetch_add(1, Ordering::Release);
            // SAFETY: valid context.
            unsafe { (api.report_swap)(ctx) };
            FRAME_READY.notify_one();
        }
    }
    // SAFETY: the callback goes first, then the context, as mpv asks; the
    // GL context (if any) is still current.
    unsafe {
        (api.set_update_callback)(ctx, None, std::ptr::null_mut());
        (api.free)(ctx);
    }
    drop(gl);
}

#[cfg(not(target_os = "linux"))]
struct NoGl;

// ── mpv's render API ────────────────────────────────────────────────────────

#[repr(C)]
pub struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}

const PARAM_INVALID: c_int = 0;
const PARAM_API_TYPE: c_int = 1;
const PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const PARAM_OPENGL_FBO: c_int = 3;
const PARAM_FLIP_Y: c_int = 4;
const PARAM_SW_SIZE: c_int = 17;
const PARAM_SW_FORMAT: c_int = 18;
const PARAM_SW_STRIDE: c_int = 19;
const PARAM_SW_POINTER: c_int = 20;

/// An opaque mpv_render_context.
#[repr(C)]
pub struct RenderContext {
    _private: [u8; 0],
}

#[repr(C)]
struct OpenGlInitParams {
    get_proc_address: extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    get_proc_address_ctx: *mut c_void,
    /// Only libmpv 1.x reads this; later versions ignore it.
    extra_exts: *const c_char,
}

fn create_context(
    api: &RenderApi,
    handle: *mut MpvHandle,
    #[cfg(target_os = "linux")] gl: Option<&gl::Context>,
    #[cfg(not(target_os = "linux"))] gl: Option<&NoGl>,
) -> Result<*mut RenderContext, String> {
    let mut ctx: *mut RenderContext = std::ptr::null_mut();
    let r = match gl {
        #[cfg(target_os = "linux")]
        Some(_) => {
            let mut init = OpenGlInitParams {
                get_proc_address: gl::get_proc_address,
                get_proc_address_ctx: std::ptr::null_mut(),
                extra_exts: std::ptr::null(),
            };
            let mut params = [
                RenderParam { kind: PARAM_API_TYPE, data: c"opengl".as_ptr() as *mut c_void },
                RenderParam { kind: PARAM_OPENGL_INIT_PARAMS, data: &mut init as *mut _ as *mut c_void },
                RenderParam { kind: PARAM_INVALID, data: std::ptr::null_mut() },
            ];
            // SAFETY: params end with INVALID and outlive the call.
            unsafe { (api.create)(&mut ctx, handle, params.as_mut_ptr()) }
        }
        #[cfg(not(target_os = "linux"))]
        Some(_) => unreachable!(),
        None => {
            let mut params = [
                RenderParam { kind: PARAM_API_TYPE, data: c"sw".as_ptr() as *mut c_void },
                RenderParam { kind: PARAM_INVALID, data: std::ptr::null_mut() },
            ];
            // SAFETY: as above.
            unsafe { (api.create)(&mut ctx, handle, params.as_mut_ptr()) }
        }
    };
    if r < 0 || ctx.is_null() {
        return Err(format!("mpv's renderer didn't start ({})", crate::mpv::error_text(r)));
    }
    Ok(ctx)
}

/// Draws the current frame with mpv's software renderer.
fn render_sw(api: &RenderApi, ctx: *mut RenderContext, w: u32, h: u32, frame: &mut Frame) -> bool {
    // Rows a multiple of 64 bytes, as mpv's renderer prefers.
    let w = (w / 16 * 16).max(16);
    let stride = w as usize * 4;
    frame.width = w;
    frame.height = h;
    frame.pixels.resize(stride * h as usize, 0);
    let mut size = [w as c_int, h as c_int];
    let mut stride = stride;
    let mut params = [
        RenderParam { kind: PARAM_SW_SIZE, data: size.as_mut_ptr() as *mut c_void },
        RenderParam { kind: PARAM_SW_FORMAT, data: c"rgb0".as_ptr() as *mut c_void },
        RenderParam { kind: PARAM_SW_STRIDE, data: &mut stride as *mut usize as *mut c_void },
        RenderParam { kind: PARAM_SW_POINTER, data: frame.pixels.as_mut_ptr() as *mut c_void },
        RenderParam { kind: PARAM_INVALID, data: std::ptr::null_mut() },
    ];
    // SAFETY: the buffer is stride × h bytes and outlives the call.
    let r = unsafe { (api.render)(ctx, params.as_mut_ptr()) };
    if r < 0 {
        log::warn!("mpv couldn't draw a frame: {}", crate::mpv::error_text(r));
    }
    r >= 0
}

/// An off-screen OpenGL context (EGL, no window) and the texture mpv draws
/// into.
#[cfg(target_os = "linux")]
mod gl {
    use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
    use std::sync::OnceLock;

    use super::{Frame, RenderContext, RenderParam, PARAM_FLIP_Y, PARAM_INVALID, PARAM_OPENGL_FBO};
    use crate::mpv::RenderApi;

    type Display = *mut c_void;
    type Config = *mut c_void;
    type EglContext = *mut c_void;

    const EGL_NONE: c_int = 0x3038;
    const EGL_EXTENSIONS: c_int = 0x3055;
    const EGL_RENDERABLE_TYPE: c_int = 0x3040;
    const EGL_OPENGL_BIT: c_int = 0x0008;
    const EGL_SURFACE_TYPE: c_int = 0x3033;
    const EGL_RED_SIZE: c_int = 0x3024;
    const EGL_GREEN_SIZE: c_int = 0x3023;
    const EGL_BLUE_SIZE: c_int = 0x3022;
    const EGL_OPENGL_API: c_uint = 0x30A2;
    const EGL_PLATFORM_SURFACELESS_MESA: c_uint = 0x31DD;
    const EGL_PLATFORM_DEVICE_EXT: c_uint = 0x313F;

    const GL_TEXTURE_2D: c_uint = 0x0DE1;
    const GL_RGBA: c_uint = 0x1908;
    const GL_RGBA8: c_int = 0x8058;
    const GL_UNSIGNED_BYTE: c_uint = 0x1401;
    const GL_FRAMEBUFFER: c_uint = 0x8D40;
    const GL_COLOR_ATTACHMENT0: c_uint = 0x8CE0;
    const GL_TEXTURE_MIN_FILTER: c_uint = 0x2801;
    const GL_TEXTURE_MAG_FILTER: c_uint = 0x2800;
    const GL_LINEAR: c_int = 0x2601;
    const GL_PACK_ALIGNMENT: c_uint = 0x0D05;

    struct Egl {
        get_proc_address: unsafe extern "C" fn(*const c_char) -> *mut c_void,
        get_display: unsafe extern "C" fn(*mut c_void) -> Display,
        initialize: unsafe extern "C" fn(Display, *mut c_int, *mut c_int) -> c_uint,
        terminate: unsafe extern "C" fn(Display) -> c_uint,
        query_string: unsafe extern "C" fn(Display, c_int) -> *const c_char,
        bind_api: unsafe extern "C" fn(c_uint) -> c_uint,
        choose_config: unsafe extern "C" fn(Display, *const c_int, *mut Config, c_int, *mut c_int) -> c_uint,
        create_context: unsafe extern "C" fn(Display, Config, EglContext, *const c_int) -> EglContext,
        destroy_context: unsafe extern "C" fn(Display, EglContext) -> c_uint,
        make_current: unsafe extern "C" fn(Display, *mut c_void, *mut c_void, EglContext) -> c_uint,
        _lib: libloading::Library,
    }

    fn egl() -> Result<&'static Egl, String> {
        static EGL: OnceLock<Result<Egl, String>> = OnceLock::new();
        EGL.get_or_init(|| {
            // SAFETY: loading the system's EGL runs only its constructors.
            let lib = unsafe { libloading::Library::new("libEGL.so.1") }.map_err(|e| e.to_string())?;
            // SAFETY: each symbol is looked up with EGL's signature.
            unsafe {
                macro_rules! sym {
                    ($n:literal) => {
                        *lib.get($n).map_err(|e| e.to_string())?
                    };
                }
                Ok(Egl {
                    get_proc_address: sym!(b"eglGetProcAddress\0"),
                    get_display: sym!(b"eglGetDisplay\0"),
                    initialize: sym!(b"eglInitialize\0"),
                    terminate: sym!(b"eglTerminate\0"),
                    query_string: sym!(b"eglQueryString\0"),
                    bind_api: sym!(b"eglBindAPI\0"),
                    choose_config: sym!(b"eglChooseConfig\0"),
                    create_context: sym!(b"eglCreateContext\0"),
                    destroy_context: sym!(b"eglDestroyContext\0"),
                    make_current: sym!(b"eglMakeCurrent\0"),
                    _lib: lib,
                })
            }
        })
        .as_ref()
        .map_err(|e| e.clone())
    }

    /// mpv's way of finding GL functions.
    pub extern "C" fn get_proc_address(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
        match egl() {
            // SAFETY: a NUL-terminated name from mpv.
            Ok(egl) => unsafe { (egl.get_proc_address)(name) },
            Err(_) => std::ptr::null_mut(),
        }
    }

    struct Gl {
        gen_framebuffers: unsafe extern "C" fn(c_int, *mut c_uint),
        delete_framebuffers: unsafe extern "C" fn(c_int, *const c_uint),
        bind_framebuffer: unsafe extern "C" fn(c_uint, c_uint),
        framebuffer_texture_2d: unsafe extern "C" fn(c_uint, c_uint, c_uint, c_uint, c_int),
        gen_textures: unsafe extern "C" fn(c_int, *mut c_uint),
        delete_textures: unsafe extern "C" fn(c_int, *const c_uint),
        bind_texture: unsafe extern "C" fn(c_uint, c_uint),
        tex_image_2d: unsafe extern "C" fn(c_uint, c_int, c_int, c_int, c_int, c_int, c_uint, c_uint, *const c_void),
        tex_parameteri: unsafe extern "C" fn(c_uint, c_uint, c_int),
        pixel_storei: unsafe extern "C" fn(c_uint, c_int),
        read_pixels: unsafe extern "C" fn(c_int, c_int, c_int, c_int, c_uint, c_uint, *mut c_void),
    }

    pub struct Context {
        display: Display,
        context: EglContext,
        gl: Gl,
        fbo: c_uint,
        texture: c_uint,
        size: (u32, u32),
    }

    /// An EGL display that needs no window system: Mesa's surfaceless
    /// platform, else the first GPU device (NVIDIA), else the default.
    unsafe fn display(egl: &Egl) -> Option<Display> {
        let client = unsafe { (egl.query_string)(std::ptr::null_mut(), EGL_EXTENSIONS) };
        let exts = if client.is_null() { String::new() } else { unsafe { CStr::from_ptr(client) }.to_string_lossy().into_owned() };
        type GetPlatformDisplay = unsafe extern "C" fn(c_uint, *mut c_void, *const c_int) -> Display;
        let get_platform = unsafe { (egl.get_proc_address)(c"eglGetPlatformDisplayEXT".as_ptr()) };
        if !get_platform.is_null() {
            let get_platform: GetPlatformDisplay = unsafe { std::mem::transmute(get_platform) };
            if exts.contains("EGL_MESA_platform_surfaceless") {
                let d = unsafe { get_platform(EGL_PLATFORM_SURFACELESS_MESA, std::ptr::null_mut(), std::ptr::null()) };
                if !d.is_null() && unsafe { (egl.initialize)(d, std::ptr::null_mut(), std::ptr::null_mut()) } != 0 {
                    return Some(d);
                }
            }
            if exts.contains("EGL_EXT_platform_device") {
                type QueryDevices = unsafe extern "C" fn(c_int, *mut *mut c_void, *mut c_int) -> c_uint;
                let query = unsafe { (egl.get_proc_address)(c"eglQueryDevicesEXT".as_ptr()) };
                if !query.is_null() {
                    let query: QueryDevices = unsafe { std::mem::transmute(query) };
                    let mut device = std::ptr::null_mut();
                    let mut n = 0;
                    if unsafe { query(1, &mut device, &mut n) } != 0 && n > 0 {
                        let d = unsafe { get_platform(EGL_PLATFORM_DEVICE_EXT, device, std::ptr::null()) };
                        if !d.is_null() && unsafe { (egl.initialize)(d, std::ptr::null_mut(), std::ptr::null_mut()) } != 0 {
                            return Some(d);
                        }
                    }
                }
            }
        }
        let d = unsafe { (egl.get_display)(std::ptr::null_mut()) };
        (!d.is_null() && unsafe { (egl.initialize)(d, std::ptr::null_mut(), std::ptr::null_mut()) } != 0).then_some(d)
    }

    impl Context {
        /// Makes a context current on this thread.
        pub fn new() -> Result<Self, String> {
            let egl = egl()?;
            // SAFETY: plain EGL calls with valid arguments; failures are
            // checked and nothing is used after one.
            unsafe {
                let display = display(egl).ok_or("no EGL display")?;
                if (egl.bind_api)(EGL_OPENGL_API) == 0 {
                    (egl.terminate)(display);
                    return Err("EGL has no desktop OpenGL".into());
                }
                let attrs = [
                    EGL_RENDERABLE_TYPE, EGL_OPENGL_BIT,
                    EGL_SURFACE_TYPE, 0,
                    EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8,
                    EGL_NONE,
                ];
                let mut config = std::ptr::null_mut();
                let mut n = 0;
                if (egl.choose_config)(display, attrs.as_ptr(), &mut config, 1, &mut n) == 0 || n < 1 {
                    (egl.terminate)(display);
                    return Err("no EGL config for OpenGL".into());
                }
                let context = (egl.create_context)(display, config, std::ptr::null_mut(), [EGL_NONE].as_ptr());
                if context.is_null() {
                    (egl.terminate)(display);
                    return Err("couldn't create an OpenGL context".into());
                }
                if (egl.make_current)(display, std::ptr::null_mut(), std::ptr::null_mut(), context) == 0 {
                    (egl.destroy_context)(display, context);
                    (egl.terminate)(display);
                    return Err("couldn't use an OpenGL context without a window".into());
                }
                macro_rules! f {
                    ($n:literal) => {{
                        let p = (egl.get_proc_address)(concat!($n, "\0").as_ptr() as *const c_char);
                        if p.is_null() {
                            return Err(format!("OpenGL has no {}", $n));
                        }
                        std::mem::transmute::<*mut c_void, _>(p)
                    }};
                }
                let gl = Gl {
                    gen_framebuffers: f!("glGenFramebuffers"),
                    delete_framebuffers: f!("glDeleteFramebuffers"),
                    bind_framebuffer: f!("glBindFramebuffer"),
                    framebuffer_texture_2d: f!("glFramebufferTexture2D"),
                    gen_textures: f!("glGenTextures"),
                    delete_textures: f!("glDeleteTextures"),
                    bind_texture: f!("glBindTexture"),
                    tex_image_2d: f!("glTexImage2D"),
                    tex_parameteri: f!("glTexParameteri"),
                    pixel_storei: f!("glPixelStorei"),
                    read_pixels: f!("glReadPixels"),
                };
                let mut fbo = 0;
                let mut texture = 0;
                (gl.gen_framebuffers)(1, &mut fbo);
                (gl.gen_textures)(1, &mut texture);
                Ok(Context { display, context, gl, fbo, texture, size: (0, 0) })
            }
        }

        /// Draws the current frame at w × h and reads it into `frame`.
        pub fn render(&mut self, api: &RenderApi, ctx: *mut RenderContext, w: u32, h: u32, frame: &mut Frame) -> bool {
            let gl = &self.gl;
            // SAFETY: the context is current on this thread and the objects
            // are its own; the read fits the buffer sized here.
            unsafe {
                if self.size != (w, h) {
                    (gl.bind_texture)(GL_TEXTURE_2D, self.texture);
                    (gl.tex_image_2d)(GL_TEXTURE_2D, 0, GL_RGBA8, w as c_int, h as c_int, 0, GL_RGBA, GL_UNSIGNED_BYTE, std::ptr::null());
                    (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
                    (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
                    (gl.bind_framebuffer)(GL_FRAMEBUFFER, self.fbo);
                    (gl.framebuffer_texture_2d)(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, self.texture, 0);
                    self.size = (w, h);
                }
                #[repr(C)]
                struct Fbo {
                    fbo: c_int,
                    w: c_int,
                    h: c_int,
                    internal_format: c_int,
                }
                let mut fbo = Fbo { fbo: self.fbo as c_int, w: w as c_int, h: h as c_int, internal_format: GL_RGBA8 };
                // Unflipped, mpv draws into an FBO top row first, which is
                // the order the pixels read back in.
                let mut flip: c_int = 0;
                let mut params = [
                    RenderParam { kind: PARAM_OPENGL_FBO, data: &mut fbo as *mut _ as *mut c_void },
                    RenderParam { kind: PARAM_FLIP_Y, data: &mut flip as *mut _ as *mut c_void },
                    RenderParam { kind: PARAM_INVALID, data: std::ptr::null_mut() },
                ];
                let r = (api.render)(ctx, params.as_mut_ptr());
                if r < 0 {
                    log::warn!("mpv couldn't draw a frame: {}", crate::mpv::error_text(r));
                    return false;
                }
                frame.width = w;
                frame.height = h;
                frame.pixels.resize(w as usize * h as usize * 4, 0);
                (gl.bind_framebuffer)(GL_FRAMEBUFFER, self.fbo);
                (gl.pixel_storei)(GL_PACK_ALIGNMENT, 4);
                (gl.read_pixels)(0, 0, w as c_int, h as c_int, GL_RGBA, GL_UNSIGNED_BYTE, frame.pixels.as_mut_ptr() as *mut c_void);
            }
            true
        }
    }

    impl Drop for Context {
        fn drop(&mut self) {
            if let Ok(egl) = egl() {
                // SAFETY: objects of this (still current) context, freed once.
                unsafe {
                    (self.gl.delete_framebuffers)(1, &self.fbo);
                    (self.gl.delete_textures)(1, &self.texture);
                    (egl.make_current)(self.display, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
                    (egl.destroy_context)(self.display, self.context);
                }
            }
        }
    }
}

// ── The widget ──────────────────────────────────────────────────────────────

/// The video, filling its space; mpv letterboxes it.
pub struct Video {
    pub output: Arc<Output>,
}

impl<Message> shader::Program<Message> for Video {
    type State = ();
    type Primitive = Primitive;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, _bounds: Rectangle) -> Primitive {
        Primitive { output: self.output.clone() }
    }
}

pub struct Primitive {
    output: Arc<Output>,
}

impl std::fmt::Debug for Primitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("video::Primitive")
    }
}

pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
    texture: Option<(wgpu::Texture, wgpu::BindGroup, (u32, u32))>,
    uploaded: u64,
}

const SHADER: &str = r#"
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    var out: Out;
    out.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@group(0) @binding(0) var frame: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    // mpv's software renderer leaves alpha at 0 (RGB0).
    return vec4<f32>(textureSample(frame, smp, in.uv).rgb, 1.0);
}
"#;

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("video shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("video pipeline layout"),
            bind_group_layouts: &[&layout],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("video pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("video sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Pipeline { pipeline, layout, sampler, format, texture: None, uploaded: 0 }
    }
}

impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let scale = viewport.scale_factor() as f32;
        self.output.set_target((bounds.width * scale).round() as u32, (bounds.height * scale).round() as u32);

        let serial = self.output.serial.load(Ordering::Acquire);
        if serial == pipeline.uploaded {
            return;
        }
        let frame = self.output.frame.lock().unwrap();
        if frame.width == 0 || frame.height == 0 {
            return;
        }
        let size = (frame.width, frame.height);
        if pipeline.texture.as_ref().map(|t| t.2) != Some(size) {
            // The frame's values are sRGB already: sampled through an sRGB
            // view into an sRGB target (or raw into a linear one), they
            // arrive unchanged.
            let format = if pipeline.format.is_srgb() {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("video frame"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("video bind group"),
                layout: &pipeline.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&pipeline.sampler) },
                ],
            });
            pipeline.texture = Some((texture, bind_group, size));
        }
        let (texture, _, _) = pipeline.texture.as_ref().unwrap();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.0 * 4), rows_per_image: Some(size.1) },
            wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        );
        pipeline.uploaded = serial;
    }

    fn render(
        &self,
        pipeline: &Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let Some((_, bind_group, _)) = &pipeline.texture else { return };
        if clip_bounds.width == 0 || clip_bounds.height == 0 {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("video pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_viewport(
            clip_bounds.x as f32,
            clip_bounds.y as f32,
            clip_bounds.width as f32,
            clip_bounds.height as f32,
            0.0,
            1.0,
        );
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..4, 0..1);
    }
}
