//! gpui-component's markdown `TextView` opens a link on any mouse-up over it,
//! whatever the button (`text/inline.rs`), so a right-click, which only
//! means "menu", opened it. The link handler runs in the bubble phase; a
//! capture-phase listener that stops propagation for those buttons keeps it
//! from ever seeing them. A context menu opens on mouse-down, so it is
//! untouched, and a left click still opens the link.

use gpui::{InteractiveElement, MouseButton};

/// Whether a mouse-up with this button must not reach a link under it.
pub(crate) fn blocks_link_click(button: MouseButton) -> bool {
    matches!(button, MouseButton::Right | MouseButton::Middle)
}

/// `el`, with right and middle mouse-ups over it kept from opening a link in
/// a markdown view inside it.
pub(crate) fn guard_links<E: InteractiveElement>(el: E) -> E {
    el.capture_any_mouse_up(|ev, _, cx| {
        if blocks_link_click(ev.button) {
            cx.stop_propagation();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_left_button_opens_a_link() {
        assert!(!blocks_link_click(MouseButton::Left));
        assert!(blocks_link_click(MouseButton::Right));
        assert!(blocks_link_click(MouseButton::Middle));
    }
}
