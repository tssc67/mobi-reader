use crate::controls::ActionButton;
use dioxus_native::prelude::*;

#[component]
pub fn Settings(data_folder: String, on_close: EventHandler<()>) -> Element {
    rsx! {
        div { class: "modal-backdrop", role: "presentation",
            div { class: "modal settings-panel", role: "dialog", aria_modal: "true", aria_label: "Reader settings",
                div { style: "display:flex;justify-content:space-between;align-items:center;",
                    h2 { "Make yourself at home" }
                    ActionButton { class: "icon-button", aria_label: "Close settings", onpress: move |_| on_close.call(()), "x" }
                }
                p { class: "muted", "Your library and preferences stay on this computer." }
                div { class: "settings-row", style: "display:block;",
                    h3 { "Bringing your books" }
                    p { style: "margin:10px 0;font-size:14px;", "Open EPUB and MOBI books directly. MOBI conversion is built into the reader, so no additional software is needed." }
                    p { class: "muted", style: "font-size:13px;", "Common DRM-free MOBI books are supported. Protected books and Kindle-only KF8 files are unsupported; choose an EPUB edition instead." }
                }
                div { class: "settings-row", style: "display:block;",
                    h3 { "Your library" }
                    p { class: "muted", style: "font-size:13px;margin:8px 0;", "Imported files are copied here. Your original files are never changed." }
                    p { style: "font-size:12px;overflow-wrap:anywhere;", "{data_folder}" }
                }
                p { class: "muted", style: "font-size:13px;margin-top:18px;", "Open a book and choose Aa to adjust type, spacing, and reading colors. Ctrl+O adds books from anywhere." }
                div { class: "modal-actions", ActionButton { class: "button button-primary", onpress: move |_| on_close.call(()), "Done" } }
            }
        }
    }
}
