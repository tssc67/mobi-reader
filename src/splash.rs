//! Loading artwork hosted inside the main native window while its renderer starts.
use std::cell::RefCell;

pub struct LoadingOverlay {
    #[cfg(windows)]
    inner: RefCell<Option<windows::Overlay>>,
    #[cfg(not(windows))]
    _unused: RefCell<()>,
}

impl LoadingOverlay {
    pub fn attach(handle: dioxus_native::winit::raw_window_handle::RawWindowHandle) -> Self {
        #[cfg(windows)]
        {
            Self {
                inner: RefCell::new(windows::Overlay::attach(handle)),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = handle;
            Self {
                _unused: RefCell::new(()),
            }
        }
    }

    pub fn resize(&self) {
        #[cfg(windows)]
        if let Some(overlay) = self.inner.borrow().as_ref() {
            overlay.resize();
        }
    }

    pub fn finish(&self) {
        #[cfg(windows)]
        if let Some(overlay) = self.inner.borrow_mut().take() {
            overlay.finish();
        }
    }
}

impl Drop for LoadingOverlay {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(windows)]
mod windows {
    use std::{
        mem, ptr,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };

    use dioxus_native::winit::raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, BitBlt, CLEARTYPE_QUALITY, CreateCompatibleBitmap, CreateCompatibleDC,
            CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DT_SINGLELINE, DeleteDC, DeleteObject,
            DrawTextW, EndPaint, FillRect, GdiFlush, GetDC, HBRUSH, HDC, HFONT, PAINTSTRUCT,
            ReleaseDC, SRCCOPY, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::GetDpiForWindow,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, MoveWindow,
                RegisterClassW, SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
                UnregisterClassW, WM_ERASEBKGND, WM_PAINT, WNDCLASSW, WS_CHILD, WS_EX_NOACTIVATE,
                WS_EX_NOPARENTNOTIFY, WS_VISIBLE,
            },
        },
    };

    const CLASS_NAME: &str = "MobiReaderLoadingOverlay";
    // COLORREF uses BGR byte order.
    const PAPER: u32 = 0x00f4fafc;
    const BORDER: u32 = 0x00d9e3e9;
    const AMBER: u32 = 0x00336b96;
    const MUTED: u32 = 0x00737d80;
    const INK: u32 = 0x00272b2c;

    pub struct Overlay {
        parent: HWND,
        child: HWND,
        instance: windows_sys::Win32::Foundation::HMODULE,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl Overlay {
        pub fn attach(handle: RawWindowHandle) -> Option<Self> {
            let RawWindowHandle::Win32(handle) = handle else {
                return None;
            };
            let parent = handle.hwnd.get() as HWND;
            if parent.is_null() {
                return None;
            }
            unsafe {
                let instance = GetModuleHandleW(ptr::null());
                if instance.is_null() {
                    return None;
                }
                let class_name = wide(CLASS_NAME);
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance,
                    lpszClassName: class_name.as_ptr(),
                    ..mem::zeroed()
                };
                if RegisterClassW(&class) == 0 {
                    return None;
                }
                let mut rect: RECT = mem::zeroed();
                GetClientRect(parent, &mut rect);
                let child = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_NOPARENTNOTIFY,
                    class_name.as_ptr(),
                    ptr::null(),
                    WS_CHILD | WS_VISIBLE,
                    0,
                    0,
                    rect.right.max(1),
                    rect.bottom.max(1),
                    parent,
                    ptr::null_mut(),
                    instance,
                    ptr::null(),
                );
                if child.is_null() {
                    UnregisterClassW(class_name.as_ptr(), instance);
                    return None;
                }
                // Paint once on the owning thread before renderer initialization blocks it.
                let resources = PaintResources::new(GetDpiForWindow(child).clamp(96, 240) as i32);
                paint_window(child, &resources, 0, false);
                let stop = Arc::new(AtomicBool::new(false));
                let worker_stop = Arc::clone(&stop);
                let child_value = child as isize;
                let worker = thread::Builder::new()
                    .name("mobi-reader-loading".into())
                    .spawn(move || run(child_value as HWND, worker_stop))
                    .ok();
                Some(Self {
                    parent,
                    child,
                    instance,
                    stop,
                    worker,
                })
            }
        }

        pub fn resize(&self) {
            unsafe {
                let mut rect: RECT = mem::zeroed();
                if GetClientRect(self.parent, &mut rect) != 0 {
                    MoveWindow(self.child, 0, 0, rect.right.max(1), rect.bottom.max(1), 0);
                }
            }
        }

        pub fn finish(mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            unsafe {
                DestroyWindow(self.child);
                let class_name = wide(CLASS_NAME);
                UnregisterClassW(class_name.as_ptr(), self.instance);
            }
        }
    }

    fn run(child: HWND, stop: Arc<AtomicBool>) {
        let mut dpi = unsafe { GetDpiForWindow(child).clamp(96, 240) as i32 };
        let mut resources = unsafe { PaintResources::new(dpi) };
        let mut animate = 1u32;
        unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut animate as *mut u32).cast(),
                0,
            );
        }
        let mut phase = 0u32;
        while !stop.load(Ordering::Acquire) {
            unsafe {
                let current_dpi = GetDpiForWindow(child).clamp(96, 240) as i32;
                if current_dpi != dpi {
                    dpi = current_dpi;
                    resources = PaintResources::new(dpi);
                }
                paint_window(child, &resources, phase, animate != 0);
            }
            phase = phase.wrapping_add(1);
            thread::sleep(Duration::from_millis(33));
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_ERASEBKGND => 1,
            WM_PAINT => unsafe {
                let mut paint: PAINTSTRUCT = mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut paint);
                if !hdc.is_null() {
                    EndPaint(hwnd, &paint);
                }
                0
            },
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    struct PaintResources {
        dpi: i32,
        paper: HBRUSH,
        border: HBRUSH,
        amber: HBRUSH,
        heading: HFONT,
        detail: HFONT,
    }

    impl PaintResources {
        unsafe fn new(dpi: i32) -> Self {
            let heading_face = wide("Georgia");
            let detail_face = wide("Segoe UI");
            unsafe {
                Self {
                    dpi,
                    paper: CreateSolidBrush(PAPER),
                    border: CreateSolidBrush(BORDER),
                    amber: CreateSolidBrush(AMBER),
                    heading: CreateFontW(
                        -pixels(26, dpi),
                        0,
                        0,
                        0,
                        500,
                        0,
                        0,
                        0,
                        DEFAULT_CHARSET as u32,
                        0,
                        0,
                        CLEARTYPE_QUALITY as u32,
                        0,
                        heading_face.as_ptr(),
                    ),
                    detail: CreateFontW(
                        -pixels(13, dpi),
                        0,
                        0,
                        0,
                        400,
                        0,
                        0,
                        0,
                        DEFAULT_CHARSET as u32,
                        0,
                        0,
                        CLEARTYPE_QUALITY as u32,
                        0,
                        detail_face.as_ptr(),
                    ),
                }
            }
        }
    }

    impl Drop for PaintResources {
        fn drop(&mut self) {
            unsafe {
                for brush in [self.paper, self.border, self.amber] {
                    if !brush.is_null() {
                        DeleteObject(brush);
                    }
                }
                for font in [self.heading, self.detail] {
                    if !font.is_null() {
                        DeleteObject(font);
                    }
                }
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn pixels(logical: i32, dpi: i32) -> i32 {
        (logical * dpi + 48) / 96
    }

    unsafe fn rectangle(hdc: HDC, brush: HBRUSH, left: i32, top: i32, right: i32, bottom: i32) {
        if !brush.is_null() {
            let rect = RECT {
                left,
                top,
                right,
                bottom,
            };
            unsafe { FillRect(hdc, &rect, brush) };
        }
    }

    unsafe fn paint_window(hwnd: HWND, resources: &PaintResources, phase: u32, animate: bool) {
        unsafe {
            let hdc = GetDC(hwnd);
            if !hdc.is_null() {
                paint_to_dc(hwnd, hdc, resources, phase, animate);
                ReleaseDC(hwnd, hdc);
            }
        }
    }

    unsafe fn paint_to_dc(
        hwnd: HWND,
        target: HDC,
        resources: &PaintResources,
        phase: u32,
        animate: bool,
    ) {
        unsafe {
            let mut rect: RECT = mem::zeroed();
            if GetClientRect(hwnd, &mut rect) == 0 {
                return;
            }
            let width = rect.right.max(1);
            let height = rect.bottom.max(1);
            let memory = CreateCompatibleDC(target);
            if memory.is_null() {
                return;
            }
            let bitmap = CreateCompatibleBitmap(target, width, height);
            if bitmap.is_null() {
                DeleteDC(memory);
                return;
            }
            let old_bitmap = SelectObject(memory, bitmap);
            draw(memory, width, height, resources, phase, animate);
            BitBlt(target, 0, 0, width, height, memory, 0, 0, SRCCOPY);
            GdiFlush();
            SelectObject(memory, old_bitmap);
            DeleteObject(bitmap);
            DeleteDC(memory);
        }
    }

    unsafe fn draw(
        hdc: HDC,
        width: i32,
        height: i32,
        resources: &PaintResources,
        phase: u32,
        animate: bool,
    ) {
        let px = |logical| pixels(logical, resources.dpi);
        let left = (width - px(360)) / 2;
        let top = (height - px(150)) / 2;
        unsafe {
            rectangle(hdc, resources.paper, 0, 0, width, height);
            // Open-book mark; all coordinates are relative to the centered content.
            let book = |x1, y1, x2, y2| {
                rectangle(
                    hdc,
                    resources.amber,
                    left + px(x1),
                    top + px(y1),
                    left + px(x2),
                    top + px(y2),
                );
            };
            book(28, 30, 47, 32);
            book(28, 30, 30, 53);
            book(46, 30, 48, 53);
            book(48, 30, 67, 32);
            book(66, 30, 68, 53);
            book(28, 51, 48, 53);
            book(48, 51, 68, 53);
            SetBkMode(hdc, TRANSPARENT as i32);
            if !resources.heading.is_null() {
                let old = SelectObject(hdc, resources.heading);
                SetTextColor(hdc, INK);
                let heading = wide("mobi reader");
                let mut rect = RECT {
                    left: left + px(84),
                    top: top + px(27),
                    right: left + px(336),
                    bottom: top + px(68),
                };
                DrawTextW(hdc, heading.as_ptr(), -1, &mut rect, DT_SINGLELINE);
                SelectObject(hdc, old);
            }
            if !resources.detail.is_null() {
                let old = SelectObject(hdc, resources.detail);
                SetTextColor(hdc, MUTED);
                let subtitle = wide("Opening your library");
                let mut rect = RECT {
                    left: left + px(29),
                    top: top + px(91),
                    right: left + px(335),
                    bottom: top + px(117),
                };
                DrawTextW(hdc, subtitle.as_ptr(), -1, &mut rect, DT_SINGLELINE);
                SelectObject(hdc, old);
            }
            let line_left = left + px(29);
            let line_right = left + px(331);
            let line_top = top + px(123);
            rectangle(
                hdc,
                resources.border,
                line_left,
                line_top,
                line_right,
                line_top + px(2),
            );
            let segment = px(50);
            let travel = (line_right - line_left - segment).max(0);
            let position = if animate {
                (phase % 48) as i32 * travel / 47
            } else {
                travel / 2
            };
            rectangle(
                hdc,
                resources.amber,
                line_left + position,
                line_top,
                line_left + position + segment,
                line_top + px(2),
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use dioxus_native::winit::raw_window_handle::Win32WindowHandle;
        use std::num::NonZeroIsize;
        use windows_sys::Win32::{
            Graphics::Gdi::{
                BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS,
            },
            UI::WindowsAndMessaging::{
                GetParent, IsWindow, PM_NOREMOVE, PeekMessageW, WM_QUIT, WS_POPUP,
            },
        };

        #[test]
        fn progress_changes_pixels_without_changing_paper() {
            unsafe {
                let width = 360;
                let height = 150;
                let hdc = CreateCompatibleDC(ptr::null_mut());
                assert!(!hdc.is_null());
                let mut info: BITMAPINFO = mem::zeroed();
                info.bmiHeader.biSize = mem::size_of::<BITMAPINFOHEADER>() as u32;
                info.bmiHeader.biWidth = width;
                info.bmiHeader.biHeight = -height;
                info.bmiHeader.biPlanes = 1;
                info.bmiHeader.biBitCount = 32;
                info.bmiHeader.biCompression = BI_RGB;
                let mut bits = ptr::null_mut();
                let bitmap =
                    CreateDIBSection(hdc, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0);
                assert!(!bitmap.is_null());
                assert!(!bits.is_null());
                let old_bitmap = SelectObject(hdc, bitmap);
                let resources = PaintResources::new(96);
                let pixels = |hdc, phase| {
                    draw(hdc, width, height, &resources, phase, true);
                    GdiFlush();
                    std::slice::from_raw_parts(bits.cast::<u32>(), (width * height) as usize)
                        .to_vec()
                };
                let first = pixels(hdc, 0);
                let later = pixels(hdc, 24);
                assert_eq!(first[0], later[0]);
                assert_eq!(first[0] & 0x00ff_ffff, 0x00fc_faf4);
                assert_ne!(first, later);
                SelectObject(hdc, old_bitmap);
                DeleteObject(bitmap);
                DeleteDC(hdc);
            }
        }

        #[test]
        fn finish_removes_only_the_child_window() {
            unsafe {
                let instance = GetModuleHandleW(ptr::null());
                assert!(!instance.is_null());
                let static_class = wide("STATIC");
                let parent = CreateWindowExW(
                    0,
                    static_class.as_ptr(),
                    ptr::null(),
                    WS_POPUP,
                    0,
                    0,
                    1000,
                    700,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    instance,
                    ptr::null(),
                );
                assert!(!parent.is_null());
                let handle = RawWindowHandle::Win32(Win32WindowHandle::new(
                    NonZeroIsize::new(parent as isize).unwrap(),
                ));
                let overlay = Overlay::attach(handle).expect("child overlay should attach");
                let child = overlay.child;
                assert_eq!(GetParent(child), parent);
                assert_ne!(IsWindow(child), 0);
                overlay.finish();
                assert_eq!(IsWindow(child), 0);
                assert_ne!(IsWindow(parent), 0);
                let mut message: windows_sys::Win32::UI::WindowsAndMessaging::MSG = mem::zeroed();
                assert_eq!(
                    PeekMessageW(&mut message, ptr::null_mut(), WM_QUIT, WM_QUIT, PM_NOREMOVE),
                    0
                );
                DestroyWindow(parent);
            }
        }
    }
}
