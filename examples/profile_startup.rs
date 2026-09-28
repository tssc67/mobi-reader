//! Time the real native application startup; the test window closes after its first frame.
use anyrender::WindowRenderer;
use dioxus_native::prelude::*;
use mobi_reader::app::{App, Services};
use std::{
    rc::Rc,
    sync::{Arc, OnceLock},
    time::Instant,
};

static START: OnceLock<Instant> = OnceLock::new();

#[path = "../src/splash.rs"]
mod splash;

fn mark(label: &str) {
    println!(
        "{label}: {:.1} ms",
        START.get().unwrap().elapsed().as_secs_f64() * 1000.0
    );
}

fn measured_app() -> Element {
    let handle = dioxus_native::use_raw_window_handle();
    let loading = use_hook(move || Rc::new(splash::LoadingOverlay::attach(handle)));
    let renderer = use_context::<dioxus_native::DioxusNativeWindowRenderer>();
    use_hook(|| mark("Application mounted"));
    use_hook(|| {
        if std::env::args().any(|argument| argument == "--preview-loading") {
            // Simulate slow initialization while the real main-window layer animates.
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    });
    let window = dioxus_native::use_window();
    let mut first = true;
    let mut finished = false;
    let preview = std::env::args().any(|argument| argument == "--preview-loading");
    dioxus_native::use_window_event(move |event, target| {
        if matches!(
            event,
            dioxus_native::winit::event::WindowEvent::SurfaceResized(_)
                | dioxus_native::winit::event::WindowEvent::ScaleFactorChanged { .. }
        ) {
            loading.resize();
        }
        if matches!(
            event,
            dioxus_native::winit::event::WindowEvent::RedrawRequested
        ) && renderer.is_active()
        {
            if first {
                first = false;
                mark("First redraw started");
                window.request_redraw();
            } else if !finished {
                finished = true;
                mark("First frame completed (next redraw)");
                loading.finish();
                if preview {
                    window.request_redraw();
                } else {
                    target.exit();
                }
            } else {
                // Leave the completed main window visible briefly for the preview.
                std::thread::sleep(std::time::Duration::from_secs(1));
                target.exit();
            }
        }
    });
    rsx! { App {} }
}

fn main() {
    START.set(Instant::now()).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let _runtime = runtime.enter();
    let services = Services::load();
    mark("Library loaded");
    let mut fonts = dioxus_native::FontContext::new();
    mark("System font context created");
    for bytes in [
        include_bytes!("../assets/fonts/SourceSerif4-Regular.otf").as_slice(),
        include_bytes!("../assets/fonts/SourceSerif4-It.otf").as_slice(),
        include_bytes!("../assets/fonts/SourceSerif4-Semibold.otf").as_slice(),
        include_bytes!("../assets/fonts/SourceSans3-Regular.otf").as_slice(),
        include_bytes!("../assets/fonts/SourceSans3-Semibold.otf").as_slice(),
    ] {
        fonts
            .collection
            .register_fonts(peniko::Blob::new(Arc::new(bytes.to_vec())), None);
    }
    mark("Bundled fonts registered");
    let window = dioxus_native::WindowAttributes::default()
        .with_title("Mobi Reader startup measurement")
        .with_visible(true)
        .with_surface_size(dioxus_native::LogicalSize::new(1200.0, 820.0));
    #[cfg(windows)]
    let window = window.with_platform_attributes(Box::new(
        dioxus_native::winit::platform::windows::WindowAttributesWindows::default()
            .with_clip_children(true),
    ));
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(30));
        eprintln!("Startup measurement timed out");
        std::process::exit(2);
    });
    dioxus_native::launch_cfg(
        measured_app,
        vec![Box::new(move || Box::new(services.clone()))],
        vec![Box::new(
            dioxus_native::Config::new()
                .with_window_attributes(window)
                .with_font_ctx(fonts),
        )],
    );
    mark("First frame and loading-layer shutdown completed");
}
