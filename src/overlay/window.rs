use anyhow::Result;
use crossbeam_channel::Sender;

#[derive(Debug, Clone)]
pub enum OverlayCommand {
    Listening { text: String },
    Finalizing { text: String },
    Injecting { text: String },
    Error { message: String },
    Hide,
    Shutdown,
}

pub struct OverlayWindow {
    sender: Sender<OverlayCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl OverlayWindow {
    pub fn spawn() -> Result<Self> {
        let (sender, receiver) = crossbeam_channel::bounded(32);
        let (ready_sender, ready_receiver) = crossbeam_channel::bounded(1);
        let thread = std::thread::Builder::new()
            .name("native-overlay".into())
            .spawn(move || platform::run(receiver, ready_sender))?;
        ready_receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| anyhow::anyhow!("native overlay startup timed out"))?
            .map_err(anyhow::Error::msg)?;
        Ok(Self {
            sender,
            thread: Some(thread),
        })
    }

    pub fn sender(&self) -> Sender<OverlayCommand> {
        self.sender.clone()
    }

    pub fn shutdown(mut self) {
        let _ = self.sender.send(OverlayCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn run(
        receiver: crossbeam_channel::Receiver<super::OverlayCommand>,
        ready: crossbeam_channel::Sender<Result<(), String>>,
    ) {
        let _ = ready.send(Ok(()));
        while let Ok(command) = receiver.recv() {
            if matches!(command, super::OverlayCommand::Shutdown) {
                break;
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::time::Instant;

    use crossbeam_channel::Receiver;
    use windows::core::{PCWSTR, w};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows::Win32::Graphics::Direct2D::Common::{
        D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
    };
    use windows::Win32::Graphics::Direct2D::{
        D2D1CreateFactory, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
        D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
        D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, ID2D1DCRenderTarget, ID2D1Factory,
    };
    use windows::Win32::Graphics::DirectWrite::{
        DWriteCreateFactory, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
        DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
        DWRITE_TEXT_METRICS, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, BI_RGB,
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, DIB_RGB_COLORS,
        GetMonitorInfoW, MonitorFromWindow, MONITOR_DEFAULTTONEAREST, MONITORINFO, RGBQUAD,
        SelectObject, HBITMAP, HDC, HGDIOBJ,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, CS_HREDRAW, CS_VREDRAW, DefWindowProcW, DispatchMessageW,
        GetForegroundWindow, MSG, PeekMessageW, PM_REMOVE, RegisterClassW, SW_HIDE,
        SW_SHOWNOACTIVATE, ShowWindow, SystemParametersInfoW, TranslateMessage, WM_DESTROY,
        WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
        ULW_ALPHA, UpdateLayeredWindow,
    };

    use super::OverlayCommand;

    const PILL_HEIGHT_DIP: f32 = 38.0;
    const PILL_PADDING_DIP: f32 = 24.0;
    const FONT_SIZE_DIP: f32 = 14.0;
    const ANIM_LERP_RATE: f32 = 0.18;
    const BOTTOM_MARGIN: i32 = 48;

    const BG_COLOR: [f32; 4] = [0.082, 0.082, 0.090, 1.0];
    const TEXT_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const GLOW_COLOR: [f32; 4] = [0.20, 0.82, 0.45, 1.0];

    const GLOW_BASE_WIDTH: f32 = 1.5;
    const GLOW_PULSE_AMP: f32 = 1.0;
    const GLOW_PULSE_FREQ: f32 = 2.2;

    struct OverlayState {
        text: String,
        target_alpha: f32,
        anim_alpha: f32,
        target_width: f32,
        anim_width: f32,
        window_visible: bool,
        phase: f32,

        dwrite_factory: IDWriteFactory,
        render_target: ID2D1DCRenderTarget,
        text_format: IDWriteTextFormat,

        cached_dc: HDC,
        cached_dib: HBITMAP,
        cached_w: i32,
        cached_h: i32,
    }

    pub fn run(
        receiver: Receiver<OverlayCommand>,
        ready: crossbeam_channel::Sender<Result<(), String>>,
    ) {
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let result = run_inner(receiver, ready);
        if let Err(e) = result {
            tracing::error!("overlay failed: {e}");
        }
        unsafe { CoUninitialize() };
    }

    fn run_inner(
        receiver: Receiver<OverlayCommand>,
        ready: crossbeam_channel::Sender<Result<(), String>>,
    ) -> Result<(), String> {
        let d2d_factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(|e| format!("d2d factory: {e}"))?;
        let dwrite_factory: IDWriteFactory =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
                .map_err(|e| format!("dwrite factory: {e}"))?;

        let font_name = get_system_font_name();
        let font_wide: Vec<u16> = font_name.encode_utf16().chain(std::iter::once(0)).collect();
        let font_ptr = PCWSTR(font_wide.as_ptr());
        let locale = w!("");
        let text_format = unsafe {
            dwrite_factory.CreateTextFormat(
                font_ptr,
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                FONT_SIZE_DIP,
                locale,
            )
        }
        .map_err(|e| format!("text format: {e}"))?;

        let rt_props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 0.0,
            dpiY: 0.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let render_target: ID2D1DCRenderTarget =
            unsafe { d2d_factory.CreateDCRenderTarget(&rt_props) }
                .map_err(|e| format!("dc render target: {e}"))?;

        let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
            return Err("module handle".into());
        };
        let class_name = w!("OneLastTry.DynamicIsland");
        let class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: class_name,
            lpfnWndProc: Some(window_proc),
            style: CS_HREDRAW | CS_VREDRAW,
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err("register class".into());
        }

        let window = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class_name,
                w!("DictateIn Overlay"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }
        .map_err(|e| format!("create window: {e}"))?;

        let mut state = OverlayState {
            text: String::new(),
            target_alpha: 0.0,
            anim_alpha: 0.0,
            target_width: 0.0,
            anim_width: 0.0,
            window_visible: false,
            phase: 0.0,
            dwrite_factory,
            render_target,
            text_format,
            cached_dc: HDC::default(),
            cached_dib: HBITMAP::default(),
            cached_w: 0,
            cached_h: 0,
        };

        let _ = ready.send(Ok(()));

        let mut last_time = Instant::now();
        loop {
            while let Ok(command) = receiver.try_recv() {
                match command {
                    OverlayCommand::Listening { text }
                    | OverlayCommand::Finalizing { text }
                    | OverlayCommand::Injecting { text }
                    | OverlayCommand::Error { message: text } => {
                        set_content(&mut state, &text);
                        state.target_alpha = 1.0;
                    }
                    OverlayCommand::Hide => {
                        state.target_alpha = 0.0;
                        state.target_width = 0.0;
                    }
                    OverlayCommand::Shutdown => {
                        unsafe {
                            let _ = ShowWindow(window, SW_HIDE);
                        }
                        cleanup_dib(&mut state);
                        return Ok(());
                    }
                }
            }

            let now = Instant::now();
            let dt = now.duration_since(last_time).as_secs_f32();
            last_time = now;

            let animating = update_animation(&mut state, dt);
            let should_render = state.anim_alpha > 0.001 || animating || state.target_alpha > 0.0;
            if should_render {
                render(&mut state, window);
            }

            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    fn set_content(state: &mut OverlayState, text: &str) {
        state.text = text.to_string();
        let (text_w, _text_h) = measure_text(state, text);
        state.target_width = text_w + PILL_PADDING_DIP * 2.0;
    }

    fn update_animation(state: &mut OverlayState, dt: f32) -> bool {
        let rate = (ANIM_LERP_RATE * dt * 120.0).min(1.0);
        state.anim_alpha = lerp(state.anim_alpha, state.target_alpha, rate);
        state.anim_width = lerp(state.anim_width, state.target_width, rate);
        state.phase += dt * GLOW_PULSE_FREQ * std::f32::consts::TAU;

        let alpha_diff = (state.anim_alpha - state.target_alpha).abs();
        let width_diff = (state.anim_width - state.target_width).abs();
        alpha_diff > 0.003 || width_diff > 0.5
    }

    fn lerp(a: f32, b: f32, t: f32) -> f32 {
        a + (b - a) * t
    }

    fn measure_text(state: &OverlayState, text: &str) -> (f32, f32) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout: Result<IDWriteTextLayout, _> = unsafe {
            state
                .dwrite_factory
                .CreateTextLayout(&wide, &state.text_format, 10000.0, PILL_HEIGHT_DIP)
        };
        match layout {
            Ok(layout) => {
                let mut metrics = DWRITE_TEXT_METRICS::default();
                let _ = unsafe { layout.GetMetrics(&mut metrics) };
                (metrics.width, metrics.height)
            }
            Err(_) => (
                text.chars().count() as f32 * 8.0,
                FONT_SIZE_DIP,
            ),
        }
    }

    fn get_system_font_name() -> String {
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        unsafe {
            let _ = SystemParametersInfoW(
                SPI_GETNONCLIENTMETRICS,
                std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
                Some(&mut metrics as *mut NONCLIENTMETRICSW as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
        }
        let name: Vec<u16> = metrics
            .lfMessageFont
            .lfFaceName
            .iter()
            .take_while(|&&c| c != 0)
            .copied()
            .collect();
        let name = String::from_utf16_lossy(&name);
        if name.is_empty() {
            "Segoe UI".to_string()
        } else {
            name
        }
    }

    fn ensure_dib(state: &mut OverlayState, width: i32, height: i32) -> bool {
        if width == state.cached_w
            && height == state.cached_h
            && !state.cached_dib.is_invalid()
        {
            return true;
        }
        cleanup_dib(state);

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD::default()],
        };

        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return false;
        }
        let mut pixels: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = unsafe { CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut pixels, None, 0) };
        let dib = match dib {
            Ok(d) => d,
            Err(_) => {
                unsafe {
                    let _ = DeleteDC(dc);
                }
                return false;
            }
        };
        if dib.is_invalid() {
            unsafe {
                let _ = DeleteDC(dc);
            }
            return false;
        }
        let _old = unsafe { SelectObject(dc, HGDIOBJ(dib.0)) };
        state.cached_dc = dc;
        state.cached_dib = dib;
        state.cached_w = width;
        state.cached_h = height;
        true
    }

    fn cleanup_dib(state: &mut OverlayState) {
        if !state.cached_dib.is_invalid() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(state.cached_dib.0));
            }
            state.cached_dib = HBITMAP::default();
        }
        if !state.cached_dc.is_invalid() {
            unsafe {
                let _ = DeleteDC(state.cached_dc);
            }
            state.cached_dc = HDC::default();
        }
        state.cached_w = 0;
        state.cached_h = 0;
    }

    fn render(state: &mut OverlayState, window: HWND) {
        let dpi = unsafe { GetDpiForWindow(window) };
        let dpi = if dpi == 0 { 96.0 } else { dpi as f32 };
        let scale = dpi / 96.0;

        let dip_w = state.anim_width.max(1.0);
        let dip_h = PILL_HEIGHT_DIP;
        let phys_w = ((dip_w * scale).round() as i32).max(1);
        let phys_h = ((dip_h * scale).round() as i32).max(1);

        if state.anim_alpha < 0.005 && state.target_alpha == 0.0 {
            if state.window_visible {
                unsafe {
                    let _ = ShowWindow(window, SW_HIDE);
                }
                state.window_visible = false;
            }
            return;
        }

        if !state.window_visible {
            unsafe {
                let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
            }
            state.window_visible = true;
        }

        if !ensure_dib(state, phys_w, phys_h) {
            return;
        }

        unsafe {
            state.render_target.SetDpi(dpi, dpi);
        }
        let bind_rect = RECT {
            left: 0,
            top: 0,
            right: phys_w,
            bottom: phys_h,
        };
        let bind_result = unsafe { state.render_target.BindDC(state.cached_dc, &bind_rect) };
        if bind_result.is_err() {
            return;
        }

        let alpha = state.anim_alpha;
        let pulse = (state.phase.sin() * 0.5 + 0.5) as f32;
        let glow_width = GLOW_BASE_WIDTH + GLOW_PULSE_AMP * pulse;
        let glow_alpha = (0.35 + 0.45 * pulse) * alpha;

        unsafe {
            state.render_target.BeginDraw();
            state.render_target
                .Clear(Some(&D2D1_COLOR_F { a: 0.0, r: 0.0, g: 0.0, b: 0.0 }));

            let pill_rect = D2D_RECT_F {
                left: glow_width,
                top: glow_width,
                right: dip_w - glow_width,
                bottom: dip_h - glow_width,
            };
            let corner = (dip_h - glow_width * 2.0) / 2.0;
            let rounded = D2D1_ROUNDED_RECT {
                rect: pill_rect,
                radiusX: corner.max(0.0),
                radiusY: corner.max(0.0),
            };

            let bg_color = D2D1_COLOR_F {
                a: BG_COLOR[3] * alpha,
                r: BG_COLOR[0],
                g: BG_COLOR[1],
                b: BG_COLOR[2],
            };
            if let Ok(brush) = state.render_target.CreateSolidColorBrush(&bg_color, None) {
                state.render_target.FillRoundedRectangle(&rounded, &brush);
            }

            let glow_color = D2D1_COLOR_F {
                a: glow_alpha,
                r: GLOW_COLOR[0],
                g: GLOW_COLOR[1],
                b: GLOW_COLOR[2],
            };
            if let Ok(brush) = state.render_target.CreateSolidColorBrush(&glow_color, None) {
                state.render_target.DrawRoundedRectangle(&rounded, &brush, glow_width, None);
            }

            if !state.text.is_empty() {
                let text_color = D2D1_COLOR_F {
                    a: TEXT_COLOR[3] * alpha,
                    r: TEXT_COLOR[0],
                    g: TEXT_COLOR[1],
                    b: TEXT_COLOR[2],
                };
                if let Ok(brush) = state.render_target.CreateSolidColorBrush(&text_color, None) {
                    let (_, text_h) = measure_text(state, &state.text);
                    let v_offset = ((dip_h - text_h) / 2.0).max(0.0);
                    let text_rect = D2D_RECT_F {
                        left: PILL_PADDING_DIP,
                        top: v_offset,
                        right: dip_w - PILL_PADDING_DIP,
                        bottom: v_offset + text_h,
                    };
                    let wide: Vec<u16> = state.text.encode_utf16().collect();
                    state.render_target.DrawText(
                        &wide,
                        &state.text_format,
                        &text_rect,
                        &brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }

            let _ = state.render_target.EndDraw(None, None);
        }

        position_and_update(state, window, phys_w, phys_h);
    }

    fn position_and_update(state: &OverlayState, window: HWND, width: i32, height: i32) {
        let foreground = unsafe { GetForegroundWindow() };
        let monitor = unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            return;
        }

        let x = info.rcWork.left + (info.rcWork.right - info.rcWork.left - width) / 2;
        let y = info.rcWork.bottom - height - BOTTOM_MARGIN;

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };

        let pt_pos = POINT { x, y };
        let sz = SIZE {
            cx: width,
            cy: height,
        };
        let pt_src = POINT { x: 0, y: 0 };

        unsafe {
            let _ = UpdateLayeredWindow(
                window,
                None,
                Some(&pt_pos),
                Some(&sz),
                Some(state.cached_dc),
                Some(&pt_src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }

    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        word_parameter: WPARAM,
        long_parameter: LPARAM,
    ) -> LRESULT {
        match message {
            WM_DESTROY => LRESULT(0),
            _ => unsafe { DefWindowProcW(window, message, word_parameter, long_parameter) },
        }
    }
}
