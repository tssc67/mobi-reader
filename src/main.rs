#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyrender::WindowRenderer;
use dioxus_native::prelude::*;
use mobi_reader::app::{App, Services};
use std::{rc::Rc, sync::Arc};

mod splash;

fn startup_app() -> Element {
    let handle = dioxus_native::use_raw_window_handle();
    let loading = use_hook(move || Rc::new(splash::LoadingOverlay::attach(handle)));
    let renderer = use_context::<dioxus_native::DioxusNativeWindowRenderer>();
    let window = dioxus_native::use_window();
    let mut first_redraw = true;
    dioxus_native::use_window_event(move |event, _| {
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
            if first_redraw {
                first_redraw = false;
                // The GPU draws behind the native child layer. Reveal it after
                // an active redraw has completed, never during initialization.
                window.request_redraw();
            } else {
                loading.finish();
            }
        }
    });
    rsx! { App {} }
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Could not start worker runtime");
    let _runtime = runtime.enter();
    let services = Services::load();
    let persistence = services.persistence.clone();
    let cancel = services.cancel.clone();
    let mut fonts = dioxus_native::FontContext::new();
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
    let icon: dioxus_native::winit::icon::Icon = dioxus_native::winit::icon::RgbaIcon::new(
        include_bytes!(concat!(env!("OUT_DIR"), "/window-icon.rgba")).to_vec(),
        64,
        64,
    )
    .expect("Built-in window icon")
    .into();
    let window = dioxus_native::WindowAttributes::default()
        .with_title("Mobi Reader")
        .with_window_icon(Some(icon.clone()))
        .with_surface_size(dioxus_native::LogicalSize::new(1200.0, 820.0))
        .with_min_surface_size(dioxus_native::LogicalSize::new(820.0, 600.0));
    #[cfg(windows)]
    let window = window.with_platform_attributes(Box::new(
        dioxus_native::winit::platform::windows::WindowAttributesWindows::default()
            .with_clip_children(true)
            .with_taskbar_icon(Some(icon)),
    ));
    dioxus_native::launch_cfg(
        startup_app,
        vec![Box::new(move || Box::new(services.clone()))],
        vec![Box::new(
            dioxus_native::Config::new()
                .with_window_attributes(window)
                .with_font_ctx(fonts),
        )],
    );
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(persistence) = persistence
        && let Err(error) = persistence.flush()
    {
        rfd::MessageDialog::new()
            .set_title("Mobi Reader - save failed")
            .set_description(format!("Some changes could not be saved.\n{error:#}"))
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}
