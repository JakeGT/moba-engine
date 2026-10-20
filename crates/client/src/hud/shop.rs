//! Shop overlay.

use moba_core::math::Vec4;

use crate::app::NetworkView;
use moba_net::protocol::NetProgress;
use moba_platform::InputState;

use crate::hud::{HitList, HudAction, item_summary, register, theme};
use crate::hud_fmt;

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn draw_shop(
    ui: &mut crate::hud::UiDrawList,
    network: &NetworkView,
    width_f: f32,
    height_f: f32,
    border: Vec4,
    text: Vec4,
    muted: Vec4,
    progress: NetProgress,
    input: &InputState,
    hits: &mut HitList,
) {
    let shop_w = 700.0;
    let shop_h = 390.0;
    let sx = (width_f - shop_w) * 0.5;
    let sy = ((height_f - shop_h) * 0.5 - 80.0).max(40.0);
    ui.panel(
        sx,
        sy,
        shop_w,
        shop_h,
        Vec4::new(0.01, 0.02, 0.035, 0.94),
        border,
    );
    register(hits, HudAction::Block, sx, sy, shop_w, shop_h);
    ui.text_shadowed(sx + 24.0, sy + 18.0, 3.5, text, "SHOP");
    ui.text(
        sx + shop_w - 180.0,
        sy + 26.0,
        2.0,
        theme::GOLD,
        &hud_fmt!("{} GOLD", progress.gold),
    );
    let page = network.shop_page;
    let mut hovered_item = None;
    for (index, item) in network
        .client
        .items()
        .iter()
        .skip(page * 8)
        .take(8)
        .enumerate()
    {
        let row_y = sy + 62.0 + index as f32 * 36.0;
        let item_index = u8::try_from(page * 8 + index).unwrap_or(u8::MAX);
        let row_x = sx + 24.0;
        let row_w = shop_w - 48.0;
        register(
            hits,
            HudAction::ShopItem(item_index),
            row_x,
            row_y - 4.0,
            row_w,
            32.0,
        );
        if input.cursor().is_some_and(|cursor| {
            cursor.x >= row_x
                && cursor.x <= row_x + row_w
                && cursor.y >= row_y - 4.0
                && cursor.y <= row_y + 28.0
        }) {
            hovered_item = input.cursor().map(|cursor| (cursor.x, cursor.y, item));
        }
        let affordable = u32::from(progress.gold) >= item.cost;
        let name_color = if affordable {
            text
        } else {
            Vec4::new(0.45, 0.5, 0.58, 1.0)
        };
        ui.text(
            sx + 24.0,
            row_y,
            2.0,
            muted,
            &hud_fmt!("[{}]", page * 8 + index + 1),
        );
        ui.icon(
            sx + 46.0,
            row_y - 2.0,
            20.0,
            u32::from(item_index) % 16,
            text,
        );
        ui.text(sx + 70.0, row_y, 2.0, name_color, item.name_str());
        ui.text(
            sx + 300.0,
            row_y,
            2.0,
            if affordable {
                theme::GOLD
            } else {
                Vec4::new(0.55, 0.4, 0.3, 1.0)
            },
            &hud_fmt!("{}G", item.cost),
        );
        ui.text(sx + 390.0, row_y, 1.7, muted, &item_summary(item));
    }
    if let Some((cursor_x, cursor_y, item)) = hovered_item {
        let name = item.name_str();
        let summary = item_summary(item);
        let tx = (cursor_x + 14.0).min(width_f - 270.0);
        let ty = (cursor_y + 14.0).min(height_f - 74.0);
        ui.panel(
            tx,
            ty,
            260.0,
            60.0,
            Vec4::new(0.01, 0.02, 0.035, 0.96),
            theme::GOLD,
        );
        ui.text(tx + 10.0, ty + 8.0, 2.0, theme::GOLD, name);
        ui.text(tx + 10.0, ty + 30.0, 1.8, text, &summary);
    }
    ui.text(
        sx + 24.0,
        sy + shop_h - 30.0,
        1.8,
        muted,
        &hud_fmt!(
            "B CLOSE   CLICK OR NUMBER BUY   ARROWS PAGE {}/{}   PURCHASES REQUIRE OWN NEXUS",
            page + 1,
            network.client.items().len().div_ceil(8).max(1)
        ),
    );
}
