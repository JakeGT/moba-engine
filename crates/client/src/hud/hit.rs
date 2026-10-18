//! Reused HUD hit rectangles for mouse routing.

use arrayvec::ArrayVec;

/// Action associated with a HUD rectangle.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum HudAction {
    /// Consume the click (panel background).
    Block,
    /// Buy this catalogue item index.
    ShopItem(u8),
    /// Click on the minimap.
    Minimap,
}

/// One registered HUD rectangle in screen pixels.
#[derive(Copy, Clone, Debug)]
pub(crate) struct HitRect {
    pub(crate) action: HudAction,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl HitRect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

/// Fixed-capacity rectangle list rebuilt every HUD frame.
pub(crate) type HitList = ArrayVec<HitRect, 64>;

pub(crate) fn register(
    hits: &mut HitList,
    action: HudAction,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    if hits.len() < hits.capacity() {
        let _ = hits.try_push(HitRect {
            action,
            x,
            y,
            width,
            height,
        });
    }
}

/// Returns the topmost rectangle under the cursor; later registrations win.
pub(crate) fn rect_at(hits: &HitList, x: f32, y: f32) -> Option<HitRect> {
    hits.iter().rev().find(|rect| rect.contains(x, y)).copied()
}

/// Maps a cursor position inside a minimap rectangle to world coordinates.
#[must_use]
pub(crate) fn minimap_world(rect: HitRect, cursor_x: f32, cursor_y: f32) -> (f32, f32) {
    let fx = ((cursor_x - rect.x) / rect.width).clamp(0.0, 1.0);
    let fy = ((cursor_y - rect.y) / rect.height).clamp(0.0, 1.0);
    (fx * 256.0, fy * 256.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_registrations_win_and_outside_is_none() {
        let mut hits = HitList::new();
        register(&mut hits, HudAction::Block, 0.0, 0.0, 100.0, 100.0);
        register(&mut hits, HudAction::ShopItem(3), 10.0, 10.0, 20.0, 20.0);
        assert_eq!(
            rect_at(&hits, 15.0, 15.0).map(|rect| rect.action),
            Some(HudAction::ShopItem(3))
        );
        assert_eq!(
            rect_at(&hits, 90.0, 90.0).map(|rect| rect.action),
            Some(HudAction::Block)
        );
        assert!(rect_at(&hits, 200.0, 200.0).is_none());
    }

    #[test]
    fn minimap_maps_cursor_to_world() {
        let rect = HitRect {
            action: HudAction::Minimap,
            x: 100.0,
            y: 50.0,
            width: 200.0,
            height: 100.0,
        };
        assert_eq!(minimap_world(rect, 100.0, 50.0), (0.0, 0.0));
        assert_eq!(minimap_world(rect, 200.0, 100.0), (128.0, 128.0));
        assert_eq!(minimap_world(rect, 300.0, 150.0), (256.0, 256.0));
    }
}
