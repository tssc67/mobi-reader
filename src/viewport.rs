//! Native layout measurements. Positions are semantic block anchors, never window pixels.
use dioxus_html::geometry::PixelsVector2D;
use dioxus_native::prelude::*;
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

use crate::model::ReadingLocation;

#[derive(Clone)]
pub struct Viewport(Rc<RefCell<State>>);

struct State {
    scroll: Option<Rc<MountedData>>,
    blocks: HashMap<String, Rc<MountedData>>,
    order: Vec<String>,
    location: ReadingLocation,
    ready: bool,
    generation: u64,
    anchor_hint: usize,
}

impl Viewport {
    pub fn new(location: ReadingLocation, order: Vec<String>) -> Self {
        let anchor_hint = order
            .iter()
            .position(|id| id == &location.block_id)
            .unwrap_or(0);
        Self(Rc::new(RefCell::new(State {
            scroll: None,
            blocks: HashMap::new(),
            order,
            location,
            ready: false,
            generation: 0,
            anchor_hint,
        })))
    }

    pub fn mount_scroll(&self, element: Rc<MountedData>) {
        self.0.borrow_mut().scroll = Some(element);
    }
    pub fn mount_block(&self, id: String, element: Rc<MountedData>) {
        self.0.borrow_mut().blocks.insert(id, element);
    }
    pub fn location(&self) -> ReadingLocation {
        self.0.borrow().location.clone()
    }

    /// An explicit scroll wins over a queued restoration. Programmatic scroll events
    /// do not call this, so restoring itself cannot cancel the pending generation.
    pub fn user_scroll(&self) {
        let mut state = self.0.borrow_mut();
        if !state.ready {
            state.generation = state.generation.wrapping_add(1);
            state.ready = true;
        }
    }

    /// The native backend supports wheel/scrollbar input but does not supply HTML
    /// keyboard scrolling. Keep these keys within the chapter's own scroll container.
    pub fn keyboard_scroll(&self, key: &Key, shift: bool) -> bool {
        let direction = match key {
            Key::PageDown => Some((1.0, true)),
            Key::PageUp => Some((-1.0, true)),
            Key::ArrowDown => Some((40.0, false)),
            Key::ArrowUp => Some((-40.0, false)),
            Key::Character(value) if value == " " => Some((if shift { -1.0 } else { 1.0 }, true)),
            Key::Home | Key::End => None,
            _ => return false,
        };
        let measured = (|| {
            let state = self.0.borrow();
            let mounted = state.scroll.as_ref()?.clone();
            let handle = mounted.downcast::<dioxus_native::NodeHandle>()?;
            let doc = handle.try_doc()?;
            let rect = doc.get_client_bounding_rect(handle.node_id())?;
            let node = doc.get_node(handle.node_id())?;
            let maximum = (node.scroll_height() as f64 - node.client_height() as f64).max(0.0);
            let offset = match direction {
                Some((amount, pages)) => {
                    node.scroll_offset().y + amount * if pages { rect.height * 0.85 } else { 1.0 }
                }
                None if *key == Key::Home => 0.0,
                None => maximum,
            }
            .clamp(0.0, maximum);
            drop(doc);
            Some((mounted, offset))
        })();
        if let Some((mounted, offset)) = measured {
            self.user_scroll();
            spawn(async move {
                let _ = mounted
                    .scroll(PixelsVector2D::new(0.0, offset), ScrollBehavior::Instant)
                    .await;
            });
        }
        true
    }

    /// Disable recording until the new layout has been restored. Otherwise the opening
    /// scroll position would replace the book's saved anchor with the top of the chapter.
    pub fn prepare(&self, location: ReadingLocation, order: Vec<String>) {
        let mut state = self.0.borrow_mut();
        if state.location.spine_index != location.spine_index {
            state.blocks.clear();
        }
        state.anchor_hint = order
            .iter()
            .position(|id| id == &location.block_id)
            .unwrap_or(0);
        state.location = location;
        state.order = order;
        state.ready = false;
        state.generation = state.generation.wrapping_add(1);
    }

    pub fn capture(&self) -> ReadingLocation {
        let state = self.0.borrow();
        if !state.ready {
            return state.location.clone();
        }
        let Some(scroll) = state
            .scroll
            .as_ref()
            .and_then(|mounted| mounted.downcast::<dioxus_native::NodeHandle>())
        else {
            return state.location.clone();
        };
        let Some(doc) = scroll.try_doc() else {
            return state.location.clone();
        };
        let Some(rect) = doc.get_client_bounding_rect(scroll.node_id()) else {
            return state.location.clone();
        };
        let Some(node) = doc.get_node(scroll.node_id()) else {
            return state.location.clone();
        };
        let offset = node.scroll_offset().y;
        // Blitz client rectangles include the node's own scroll offset, unlike a
        // browser's border box. Add it back to obtain the stationary viewport edge.
        let viewport_top = rect.y + offset;
        let maximum = (node.scroll_height() as f64 - node.client_height() as f64).max(0.0);
        let bounds = |index: usize| {
            let id = state.order.get(index)?;
            let handle = state
                .blocks
                .get(id)?
                .downcast::<dioxus_native::NodeHandle>()?;
            let block = doc.get_client_bounding_rect(handle.node_id())?;
            if block.height <= 0.0 {
                return None;
            }
            Some((block.y, block.height))
        };
        // Keep the block immediately above the viewport edge, including its fractional
        // offset. Start at the previous anchor so ordinary scrolling only measures
        // neighboring paragraphs, rather than traversing an entire long chapter.
        let mut index = state.anchor_hint.min(state.order.len().saturating_sub(1));
        while index > 0 {
            if bounds(index).is_some_and(|(top, _)| top <= viewport_top + 1.0) {
                break;
            }
            index -= 1;
        }
        let mut anchor = bounds(index).map(|(top, height)| (index, top, height));
        while index + 1 < state.order.len() {
            index += 1;
            let Some((top, height)) = bounds(index) else {
                continue;
            };
            if anchor.is_none() || top <= viewport_top + 1.0 {
                anchor = Some((index, top, height));
            }
            if top > viewport_top + 1.0 {
                break;
            }
        }
        let Some((index, block_top, block_height)) = anchor else {
            return state.location.clone();
        };
        let block_id = state.order[index].clone();
        let block_fraction = ((viewport_top - block_top) / block_height).clamp(0.0, 1.0);
        let location = ReadingLocation {
            spine_index: state.location.spine_index,
            block_id,
            block_fraction,
            chapter_fraction: if maximum <= 1.0 {
                1.0
            } else {
                (offset / maximum).clamp(0.0, 1.0)
            },
        };
        drop(doc);
        drop(state);
        let mut state = self.0.borrow_mut();
        state.location = location.clone();
        state.anchor_hint = index;
        location
    }

    pub fn link_at(&self, x: f64, y: f64) -> Option<String> {
        let state = self.0.borrow();
        let handle = state
            .scroll
            .as_ref()?
            .downcast::<dioxus_native::NodeHandle>()?;
        let doc = handle.try_doc()?;
        let mut id = doc.hit(x as f32, y as f32)?.node_id;
        loop {
            let node = doc.get_node(id)?;
            if let Some(href) = node.attr("href".into()) {
                return Some(href.to_string());
            }
            if id == handle.node_id() {
                return None;
            }
            id = node.parent?;
        }
    }

    /// Schedule against the native layout, without borrowing its document across await.
    pub fn restore(
        &self,
        on_location: EventHandler<ReadingLocation>,
        on_error: EventHandler<String>,
    ) {
        let viewport = self.clone();
        let generation = self.0.borrow().generation;
        spawn(async move {
            let mut previous_layout = None;
            for delay in [100, 80, 160, 320, 640] {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                if viewport.0.borrow().generation != generation {
                    return;
                }
                if let Some((mounted, offset, maximum)) = viewport.restore_offset() {
                    // Embedded images and native text layout can finish after mounting.
                    // Require two stable measurements before consuming the saved anchor.
                    let stable = previous_layout.is_some_and(
                        |(previous_offset, previous_maximum): (f64, f64)| {
                            (previous_offset - offset).abs() < 0.5
                                && (previous_maximum - maximum).abs() < 0.5
                        },
                    );
                    previous_layout = Some((offset, maximum));
                    if !stable {
                        continue;
                    }
                    match mounted
                        .scroll(PixelsVector2D::new(0.0, offset), ScrollBehavior::Instant)
                        .await
                    {
                        Ok(()) => {
                            if viewport.0.borrow().generation != generation {
                                return;
                            }
                            viewport.0.borrow_mut().ready = true;
                            on_location.call(viewport.capture());
                            return;
                        }
                        Err(_) => continue,
                    }
                }
            }
            // Preserve the saved location when native measurements are unavailable.
            // A subsequent resize or navigation can retry without losing that anchor.
            on_error.call(
                "The reading position could not be restored. Your saved place has been kept."
                    .into(),
            );
        });
    }

    fn restore_offset(&self) -> Option<(Rc<MountedData>, f64, f64)> {
        let state = self.0.borrow();
        let mounted = state.scroll.as_ref()?.clone();
        let handle = mounted.downcast::<dioxus_native::NodeHandle>()?;
        let doc = handle.try_doc()?;
        let rect = doc.get_client_bounding_rect(handle.node_id())?;
        if rect.height <= 0.0 {
            return None;
        }
        let node = doc.get_node(handle.node_id())?;
        let maximum = (node.scroll_height() as f64 - node.client_height() as f64).max(0.0);
        let block = state.blocks.get(&state.location.block_id).or_else(|| {
            if state.location.block_id.is_empty() {
                state.order.first().and_then(|id| state.blocks.get(id))
            } else {
                None
            }
        });
        let starts_at_top = state.location.block_fraction <= 0.0
            && state.location.chapter_fraction <= 0.0
            && (state.location.block_id.is_empty()
                || state.order.first() == Some(&state.location.block_id));
        let offset = if starts_at_top && (block.is_some() || state.order.is_empty()) {
            0.0
        } else if let Some(block) = block {
            let block = block.downcast::<dioxus_native::NodeHandle>()?;
            let block = doc.get_client_bounding_rect(block.node_id())?;
            if block.height <= 0.0 {
                return None;
            }
            block.y - rect.y + block.height * state.location.block_fraction.clamp(0.0, 1.0)
        } else if state.blocks.len() >= state.order.len() {
            maximum * state.location.chapter_fraction.clamp(0.0, 1.0)
        } else {
            return None;
        };
        drop(doc);
        Some((mounted, offset.clamp(0.0, maximum), maximum))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dioxus_native::{DioxusDocument, DocumentConfig};
    use std::task::{Context, Poll, Waker};

    fn fixture() -> Element {
        let viewport = use_context::<Viewport>();
        let scroll_viewport = viewport.clone();
        rsx! {
            div { style: "width: 500px; height: 300px; overflow: auto;", onmounted: move |event| scroll_viewport.mount_scroll(event.data()),
                for index in 0..5 {
                    {
                        let block_viewport = viewport.clone();
                        let id = format!("block-{index}");
                        rsx! { div { style: "display:block;height:200px;", id: "{id}", onmounted: move |event| block_viewport.mount_block(id.clone(), event.data()), "A quiet paragraph." } }
                    }
                }
            }
        }
    }

    fn document() -> (Viewport, DioxusDocument) {
        let viewport = Viewport::new(
            ReadingLocation::default(),
            (0..5).map(|index| format!("block-{index}")).collect(),
        );
        let virtual_dom = VirtualDom::new(fixture);
        virtual_dom.provide_root_context(viewport.clone());
        let mut config = DocumentConfig {
            viewport: Some(Default::default()),
            ..DocumentConfig::default()
        };
        config.viewport.as_mut().unwrap().window_size = (800, 600);
        let mut doc = DioxusDocument::new(virtual_dom, config);
        doc.initial_build();
        doc.inner.borrow_mut().resolve(0.0);
        viewport.0.borrow_mut().ready = true;
        (viewport, doc)
    }

    fn scroll(viewport: &Viewport, offset: f64) {
        let mounted = viewport.0.borrow().scroll.clone().unwrap();
        let mut future = mounted.scroll(PixelsVector2D::new(0.0, offset), ScrollBehavior::Instant);
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            future.as_mut().poll(&mut context),
            Poll::Ready(Ok(()))
        ));
    }

    #[test]
    fn native_scroll_captures_block_fraction_in_both_directions() {
        let (viewport, _doc) = document();
        scroll(&viewport, 250.0);
        let location = viewport.capture();
        assert_eq!(location.block_id, "block-1");
        assert!((location.block_fraction - 0.25).abs() < 0.01);
        assert!((location.chapter_fraction - 250.0 / 700.0).abs() < 0.01);
        // Verify the cached anchor also works when the user scrolls backwards.
        scroll(&viewport, 100.0);
        let location = viewport.capture();
        assert_eq!(location.block_id, "block-0");
        assert!((location.block_fraction - 0.5).abs() < 0.01);
    }

    #[test]
    fn native_reflow_restores_the_same_paragraph_fraction() {
        let (viewport, doc) = document();
        scroll(&viewport, 250.0);
        let original = viewport.capture();
        let order = viewport.0.borrow().order.clone();
        viewport.prepare(original.clone(), order);
        // A larger font or narrower window doubles the paragraphs' rendered height.
        let ids = viewport
            .0
            .borrow()
            .blocks
            .values()
            .map(|mounted| {
                mounted
                    .downcast::<dioxus_native::NodeHandle>()
                    .unwrap()
                    .node_id()
            })
            .collect::<Vec<_>>();
        {
            let mut native = doc.inner.borrow_mut();
            for id in ids {
                native.set_style_property(id, "height", "400px");
            }
            native.resolve(0.0);
        }
        // While waiting for layout, a capture must keep the saved anchor intact.
        assert_eq!(viewport.capture(), original);
        let (_, offset, _) = viewport.restore_offset().unwrap();
        assert!((offset - 500.0).abs() < 0.01);
        scroll(&viewport, offset);
        viewport.0.borrow_mut().ready = true;
        let restored = viewport.capture();
        assert_eq!(restored.block_id, original.block_id);
        assert!((restored.block_fraction - original.block_fraction).abs() < 0.01);
    }

    #[test]
    fn unknown_anchor_falls_back_to_chapter_progress() {
        let (viewport, _doc) = document();
        let location = ReadingLocation {
            block_id: "removed-by-new-edition".into(),
            chapter_fraction: 0.6,
            ..ReadingLocation::default()
        };
        let order = viewport.0.borrow().order.clone();
        viewport.prepare(location.clone(), order);
        assert_eq!(viewport.capture(), location);
        let (_, offset, _) = viewport.restore_offset().unwrap();
        assert!((offset - 420.0).abs() < 0.01);
    }

    #[test]
    fn a_user_scroll_supersedes_a_pending_restore() {
        let (viewport, _doc) = document();
        let saved = ReadingLocation {
            block_id: "block-3".into(),
            block_fraction: 0.25,
            chapter_fraction: 0.8,
            ..ReadingLocation::default()
        };
        let order = viewport.0.borrow().order.clone();
        viewport.prepare(saved, order);
        let generation = viewport.0.borrow().generation;
        scroll(&viewport, 100.0);
        viewport.user_scroll();
        assert_ne!(viewport.0.borrow().generation, generation);
        assert_eq!(viewport.capture().block_id, "block-0");
    }

    #[test]
    fn native_reading_text_exposes_selection_for_copy() {
        let (viewport, doc) = document();
        let id = viewport.0.borrow().blocks["block-0"]
            .downcast::<dioxus_native::NodeHandle>()
            .unwrap()
            .node_id();
        let mut native = doc.inner.borrow_mut();
        native.set_text_selection(id, 2, id, 7);
        assert_eq!(native.get_selected_text().as_deref(), Some("quiet"));
    }
}
