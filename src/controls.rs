use dioxus_native::prelude::*;

/// Native buttons keep normal HTML focus and accessibility semantics. Blitz does
/// not synthesize clicks from keyboard activation, so provide those explicitly.
#[component]
pub fn ActionButton(
    children: Element,
    #[props(default)] class: String,
    #[props(default)] style: String,
    #[props(default)] title: String,
    #[props(default)] aria_label: String,
    #[props(default)] disabled: bool,
    #[props(default)] aria_pressed: Option<bool>,
    onpress: EventHandler<()>,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: "{class}",
            style: "{style}",
            title: (!title.is_empty()).then_some(title),
            aria_label: (!aria_label.is_empty()).then_some(aria_label),
            aria_pressed,
            disabled: disabled.then_some(true),
            onclick: move |_| { if !disabled { onpress.call(()); } },
            onkeydown: move |event| {
                let activation = event.key() == Key::Enter
                    || matches!(event.key(), Key::Character(ref value) if value == " ");
                if activation {
                    event.prevent_default();
                    event.stop_propagation();
                    if !disabled && !event.is_auto_repeating() { onpress.call(()); }
                }
            },
            {children}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buttons() -> Element {
        rsx! {
            ActionButton { title: "enabled", onpress: move |_| {}, "Enabled" }
            ActionButton { title: "disabled", disabled: true, onpress: move |_| {}, "Disabled" }
        }
    }

    #[test]
    fn enabled_buttons_omit_the_native_boolean_attribute() {
        let mut document = dioxus_native::DioxusDocument::new(
            VirtualDom::new(buttons),
            dioxus_native::DocumentConfig::default(),
        );
        document.initial_build();
        document.inner.borrow_mut().resolve(0.0);
        let native = document.inner.borrow();
        let mut pending = vec![native.root_node().id];
        let mut enabled_has_disabled = None;
        let mut disabled_has_disabled = None;
        while let Some(id) = pending.pop() {
            let node = native.get_node(id).unwrap();
            pending.extend(node.children.iter().copied());
            match node.attr("title".into()) {
                Some("enabled") => {
                    enabled_has_disabled = Some(node.attr("disabled".into()).is_some())
                }
                Some("disabled") => {
                    disabled_has_disabled = Some(node.attr("disabled".into()).is_some())
                }
                _ => {}
            }
        }
        assert_eq!(enabled_has_disabled, Some(false));
        assert_eq!(disabled_has_disabled, Some(true));
    }
}
