//! Reused HUD hit rectangles for mouse routing.

use arrayvec::ArrayVec;

/// Action associated with a HUD rectangle.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum HudAction {
    /// Consume the click (panel background).
    Block,
    /// Buy this catalogue item index.
    ShopItem(u8),
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

/// Returns the topmost action under the cursor; later registrations win.
pub(crate) fn action_at(hits: &HitList, x: f32, y: f32) -> Option<HudAction> {
    hits.iter()
        .rev()
        .find(|rect| rect.contains(x, y))
        .map(|rect| rect.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_registrations_win_and_outside_is_none() {
        let mut hits = HitList::new();
        register(&mut hits, HudAction::Block, 0.0, 0.0, 100.0, 100.0);
        register(&mut hits, HudAction::ShopItem(3), 10.0, 10.0, 20.0, 20.0);
        assert_eq!(action_at(&hits, 15.0, 15.0), Some(HudAction::ShopItem(3)));
        assert_eq!(action_at(&hits, 90.0, 90.0), Some(HudAction::Block));
        assert_eq!(action_at(&hits, 200.0, 200.0), None);
    }
}
