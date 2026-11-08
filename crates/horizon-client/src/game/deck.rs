//! iPhone Duo held upright: the game above the fold, a page view below it
//! (`gfx/deck.rs` draws the deck's surface and the game's reflection on it).
//!
//! The pages are a native SwiftUI pager (`crates/horizon-ios/swift/DeckPages.swift`, through
//! `window::PagerHost`): it owns the paging, each page's title and the page dots, and reports
//! where it is; the game draws each page's content under it at that offset: the inventory, a
//! map, the player as they are now, and their status. The deck's sides are the controls' pads
//! (game/touch.rs); the game's view above the fold looks and acts.
//!
//! The layout follows UIKit's fold (`window::display_shape`): asked again on every resize.
use super::Game;
use super::touch::Side;
use horizon_client::gfx::deck::DeckLayout;
use horizon_client::hud::{HudBatch, MINIMAP_SPRITE, rgba};
use horizon_client::lang;
use std::time::Instant;
use winit::dpi::PhysicalSize;
use winit::event::Touch;

/// The pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Inventory,
    Map,
    Character,
    Status,
}
const PAGES: [Page; 4] = [Page::Inventory, Page::Map, Page::Character, Page::Status];

/// A slot on the inventory page: 18 GUI pixels, as in the inventory.
const SLOT: f32 = 18.0;
/// The space around a page's content.
const MARGIN: f32 = 6.0;
/// The reflection: strength, clarity (0 blurs most), the glow at the fold.
const LOOK: [f32; 3] = [0.55, 0.0, 0.10];
/// Each side of the deck (a fraction of its width) is a pad for the controls; the card between.
const CARD_SIDE: f64 = 0.24;
/// The space around the card, and its corners' radius (points).
const CARD_PADDING: f64 = 12.0;
const CARD_CORNER: f64 = 26.0;

#[derive(Default)]
pub(super) struct Deck {
    layout: Option<DeckLayout>,
    /// The layout was asked for once the renderer existed.
    checked: bool,
    /// When the display was last asked about, while it showed no fold: UIKit may report the
    /// fold only some time after the view is laid out in its scene.
    asked: Option<Instant>,
    first_asked: Option<Instant>,
    /// The orientations last allowed: upright too, or landscape only.
    portrait: Option<bool>,
    /// The pager's position in pages, as of this frame.
    offset: f32,
}

impl Game<'_> {
    /// The window's new size: the game's part of it (above the fold) when the deck is laid out.
    pub(super) fn deck_resized(&mut self, size: PhysicalSize<u32>) -> PhysicalSize<u32> {
        let Some(window) = &self.window else {
            return size;
        };
        let shape = horizon_client::window::display_shape(window);
        log::debug!("Horizon: display {size:?}: {shape:?}");
        if self.deck.portrait != Some(shape.portrait) {
            self.deck.portrait = Some(shape.portrait);
            horizon_client::window::fit_orientations(window, shape);
        }
        let layout = shape
            .division
            .filter(|d| d.horizontal && size.height > 1)
            .map(|d| {
                let game_height = (d.start.floor() as u32).clamp(1, size.height - 1);
                DeckLayout {
                    full: [size.width, size.height],
                    game_height,
                    deck_top: (d.end.ceil() as u32).clamp(game_height, size.height - 1),
                }
            });
        let layout = match self.renderer.as_mut().map(|r| r.set_deck(layout)) {
            Some(Err(error)) => {
                log::warn!("Horizon: the deck is off: {error}");
                None
            }
            _ => layout,
        };
        if layout.is_none()
            && let (Some(pager), Some(window)) = (horizon_client::window::pager(), &self.window)
        {
            (pager.update)(window, None, &[]);
        }
        self.deck.layout = layout;
        self.deck.checked = self.renderer.is_some();
        match layout {
            Some(l) => PhysicalSize::new(l.full[0], l.game_height),
            None => size,
        }
    }

    /// Once the renderer exists: lay the deck out if the display has a fold (the window's
    /// first resize can come before it).
    pub(super) fn deck_frame(&mut self) {
        // UIKit may report the fold only some time after the view is laid out: ask once a
        // second for the first ten seconds while there is none, then only on resizes.
        let again = self.deck.layout.is_none()
            && self.deck.asked.is_none_or(|at| at.elapsed().as_secs() >= 1)
            && self
                .deck
                .first_asked
                .is_none_or(|at| at.elapsed().as_secs() < 10);
        if (self.deck.checked && !again) || self.renderer.is_none() {
            return;
        }
        self.deck.asked = Some(Instant::now());
        self.deck.first_asked.get_or_insert_with(Instant::now);
        let Some(size) = self
            .window
            .as_ref()
            .map(|w| horizon_client::window::surface_size(w))
        else {
            return;
        };
        let game = self.deck_resized(size);
        if let Some(r) = &mut self.renderer
            && [r.config.width, r.config.height] != [game.width, game.height]
        {
            r.resize(game.width, game.height);
        }
    }

    /// The deck's GUI: the game's scale, over the deck's pixels: (layout, scale, width, height).
    fn deck_gui(&self) -> Option<(DeckLayout, f32, f32, f32)> {
        let layout = self.deck.layout?;
        let scale = self.touch_scale();
        let [w, h] = layout.deck_size();
        Some((
            layout,
            scale,
            (w as f32 / scale).ceil(),
            (h as f32 / scale).ceil(),
        ))
    }

    /// GUI pixels per point.
    fn deck_gui_per_point(&self) -> f32 {
        let points = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        points / self.touch_scale()
    }

    /// The middle of the deck, recessed: the card the pager shows its pages in, in window
    /// pixels (x, y, width, height). The deck's sides are the controls' pads.
    pub(super) fn deck_card(&self) -> Option<[f64; 4]> {
        let layout = self.deck.layout?;
        let points = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
        let (w, h) = (f64::from(layout.full[0]), f64::from(layout.full[1]));
        let top = f64::from(layout.deck_top);
        let side = (w * CARD_SIDE).round();
        // Padding around it; more below, clear of the home indicator.
        let (above, below) = (CARD_PADDING * points, (CARD_PADDING + 18.0) * points);
        Some([
            side,
            top + above,
            w - 2.0 * side,
            (h - top - above - below).max(1.0),
        ])
    }

    /// Where a finger landing on the deck goes: `None` when it lands above the fold (the game's
    /// view), else the pad it lands on (`Some(None)` on the fold or the card).
    pub(super) fn deck_zone(&self, touch: &Touch) -> Option<Option<Side>> {
        let layout = self.deck.layout?;
        let (x, y) = (touch.location.x, touch.location.y);
        if y < f64::from(layout.game_height) {
            return None;
        }
        if y < f64::from(layout.deck_top) {
            return Some(None);
        }
        let [cx, _, cw, _] = self.deck_card()?;
        Some(if x < cx {
            Some(Side::Walk)
        } else if x >= cx + cw {
            Some(Side::Act)
        } else {
            None
        })
    }

    /// Whether the deck is up (iPhone Duo upright): then the game's whole view looks.
    pub(super) fn deck_up(&self) -> bool {
        self.deck.layout.is_some()
    }

    /// The card in the deck's GUI pixels.
    fn deck_card_gui(&self) -> Option<[f32; 4]> {
        let layout = self.deck.layout?;
        let scale = self.touch_scale();
        let [x, y, w, h] = self.deck_card()?;
        Some([
            x as f32 / scale,
            (y - f64::from(layout.deck_top)) as f32 / scale,
            w as f32 / scale,
            h as f32 / scale,
        ])
    }

    /// Where page content goes in the card, between the pager's title and dots (deck GUI
    /// pixels, for the page at offset 0).
    fn deck_content(&self) -> [f32; 4] {
        let Some([x, y, w, h]) = self.deck_card_gui() else {
            return [0.0; 4];
        };
        let per_point = self.deck_gui_per_point();
        let (top, bottom) = horizon_client::window::pager().map_or((0.0, 0.0), |p| (p.bands)());
        let top = top as f32 * per_point;
        let bottom = bottom as f32 * per_point;
        [
            x + MARGIN,
            y + top,
            w - 2.0 * MARGIN,
            (h - top - bottom).max(1.0),
        ]
    }

    /// The page shown (the pager's nearest).
    fn deck_page(&self) -> usize {
        (self.deck.offset.round().max(0.0) as usize).min(PAGES.len() - 1)
    }

    /// Whether the deck's map page shows: the minimap keeps its tiles for it.
    pub(super) fn deck_shows_map(&self) -> bool {
        self.deck.layout.is_some() && PAGES[self.deck_page()] == Page::Map
    }

    /// After the game's frame: the pager placed and read, then the deck's frame presented with
    /// the game's above the fold.
    pub(super) fn present_deck(&mut self) -> horizon_client::Result<()> {
        if self.renderer.as_ref().and_then(|r| r.deck()).is_none() {
            return Ok(());
        }
        let Some((layout, scale, w, h)) = self.deck_gui() else {
            return Ok(());
        };
        let card = self.deck_card().unwrap_or_default();
        if let (Some(pager), Some(window)) = (horizon_client::window::pager(), &self.window) {
            let titles: Vec<String> = PAGES.iter().map(|p| page_title(*p)).collect();
            (pager.update)(window, Some(card), &titles);
            self.deck.offset = (pager.offset)() as f32;
            while let Some((page, x, y)) = (pager.next_tap)() {
                let per_point = self.deck_gui_per_point();
                let [cx, cy, ..] = self.deck_card_gui().unwrap_or_default();
                self.deck_tapped(page, [cx + x as f32 * per_point, cy + y as f32 * per_point]);
            }
        }
        let mut batch = HudBatch::new(layout.deck_size());
        batch.scale = scale;
        batch.width = w;
        batch.height = h;
        if self.player.is_some() && self.title.is_none() {
            let [x, y, cw, ch] = self.deck_content();
            let page_w = self.deck_card_gui().map_or(w, |c| c[2]);
            for (i, page) in PAGES.iter().enumerate() {
                let shift = (i as f32 - self.deck.offset) * page_w;
                if shift.abs() >= page_w {
                    continue;
                }
                let rect = [x + shift, y, cw, ch];
                match page {
                    Page::Inventory => self.deck_inventory(&mut batch, rect),
                    Page::Map => self.deck_map(&mut batch, rect),
                    Page::Character => self.deck_character(&mut batch, rect),
                    Page::Status => self.deck_status(&mut batch, rect),
                }
            }
        }
        let corner = (CARD_CORNER * self.window.as_ref().map_or(1.0, |w| w.scale_factor())) as f32;
        if let Some(r) = &mut self.renderer {
            r.present_deck(&batch, LOOK, card.map(|v| v as f32), corner)?;
        }
        Ok(())
    }

    /// A tap on page `page` at `at` (deck GUI pixels, the page at offset 0).
    fn deck_tapped(&mut self, page: usize, at: [f32; 2]) {
        if PAGES.get(page) != Some(&Page::Inventory) {
            return;
        }
        let (ox, oy) = deck_inventory_origin(self.deck_content());
        let hotbar_y = oy + 3.0 * SLOT + 4.0;
        if at[1] >= hotbar_y && at[1] < hotbar_y + SLOT && at[0] >= ox && at[0] < ox + 9.0 * SLOT {
            self.hotbar_presses.push(((at[0] - ox) / SLOT) as usize);
        }
    }

    fn deck_inventory(&self, batch: &mut HudBatch, content: [f32; 4]) {
        let atlas = &self.gui.atlas;
        let (ox, oy) = deck_inventory_origin(content);
        let slot = |b: &mut HudBatch, x: f32, y: f32, selected: bool| {
            b.fill(atlas, x, y, SLOT - 1.0, SLOT - 1.0, rgba(255, 255, 255, 22));
            if selected {
                let c = rgba(255, 255, 255, 210);
                b.fill(atlas, x - 1.0, y - 1.0, SLOT + 1.0, 1.0, c);
                b.fill(atlas, x - 1.0, y + SLOT - 1.0, SLOT + 1.0, 1.0, c);
                b.fill(atlas, x - 1.0, y - 1.0, 1.0, SLOT + 1.0, c);
                b.fill(atlas, x + SLOT - 1.0, y - 1.0, 1.0, SLOT + 1.0, c);
            }
        };
        for row in 0..3 {
            for col in 0..9 {
                let (x, y) = (ox + col as f32 * SLOT, oy + row as f32 * SLOT);
                slot(batch, x, y, false);
                if let Some(stack) = self.stack_in(9 + row * 9 + col) {
                    self.draw_item(batch, stack, x, y);
                }
            }
        }
        let hy = oy + 3.0 * SLOT + 4.0;
        for col in 0..9 {
            let x = ox + col as f32 * SLOT;
            slot(batch, x, hy, col == self.selected);
            if let Some(stack) = self.stack_in(col) {
                self.draw_item(batch, stack, x, hy);
            }
        }
    }

    fn deck_map(&self, batch: &mut HudBatch, [x, y, w, h]: [f32; 4]) {
        let atlas = &self.gui.atlas;
        let side = w.min(h).floor();
        let (mx, my) = (
            (x + (w - side) / 2.0).round(),
            (y + (h - side) / 2.0).round(),
        );
        if let Some(rect) = atlas.sprites.get(MINIMAP_SPRITE).copied() {
            batch.rect_uv(mx, my, side, side, rect, u32::MAX);
        }
        // The player at the centre, facing up (the map turns with them).
        let (c0, c1) = (mx + side / 2.0, my + side / 2.0);
        let white = rgba(255, 255, 255, 255);
        batch.fill(atlas, c0 - 1.0, c1 - 4.0, 2.0, 2.0, white);
        batch.fill(atlas, c0 - 2.0, c1 - 2.0, 4.0, 2.0, white);
        batch.fill(atlas, c0 - 3.0, c1, 6.0, 2.0, white);
    }

    fn deck_character(&self, batch: &mut HudBatch, [x, y, w, h]: [f32; 4]) {
        // The inventory's player box, as big as the page allows: 30 per 70 pixels of height.
        let box_h = h - 12.0;
        let box_w = (box_h * 0.7).min(w);
        let x0 = x + (w - box_w) / 2.0;
        self.draw_player_preview(
            batch,
            x0,
            y,
            x0 + box_w,
            y + box_h,
            box_h * 30.0 / 70.0,
            0.0625,
        );
        // What they hold.
        if let Some(stack) = self.stack_in(self.selected) {
            let kind = if self
                .items()
                .get(&stack.item)
                .is_some_and(|i| i.block.is_some())
            {
                "block"
            } else {
                "item"
            };
            let name = lang::get(&format!("{kind}.{}", stack.item.replace(':', ".")));
            let tw = HudBatch::text_width(&self.gui.atlas, &name);
            batch.text(
                &self.gui.atlas,
                &name,
                (x + (w - tw) / 2.0).round(),
                (y + h - 9.0).round(),
                rgba(255, 255, 255, 220),
                true,
            );
        }
    }

    fn deck_status(&self, batch: &mut HudBatch, [x, y, w, _]: [f32; 4]) {
        let atlas = &self.gui.atlas;
        let Some(p) = self.player.as_ref() else {
            return;
        };
        // A column centred on the page.
        let x = x + (w - 120.0).max(0.0) / 2.0;
        let line = |b: &mut HudBatch, row: f32, icon: Option<&str>, text: String, color: u32| {
            let ty = y + 4.0 + row * 13.0;
            let tx = if let Some(icon) = icon {
                b.sprite(atlas, icon, x, ty - 1.0);
                x + 12.0
            } else {
                x
            };
            b.text(atlas, &text, tx, ty, color, true);
        };
        let white = rgba(255, 255, 255, 230);
        line(
            batch,
            0.0,
            Some("gui/sprites/hud/heart/full"),
            format!("{:.0}", p.health.health()),
            white,
        );
        line(
            batch,
            1.0,
            Some("gui/sprites/hud/food_full"),
            format!("{}", p.health.food.food),
            white,
        );
        // The experience level, in the experience bar's green.
        line(
            batch,
            2.0,
            None,
            format!("{}", self.experience.level),
            rgba(128, 255, 32, 255),
        );
        line(
            batch,
            3.0,
            None,
            format!(
                "{:.0} / {:.0} / {:.0}",
                p.position.x.floor(),
                p.position.y.floor(),
                p.position.z.floor()
            ),
            white,
        );
        let biome = self.biome_at([p.position.x, p.position.y, p.position.z]);
        line(
            batch,
            4.0,
            None,
            lang::get(&format!("biome.{}", biome.replace(':', "."))),
            white,
        );
        let day = self.world.day_time.div_euclid(24000) + 1;
        // Minecraft's clock: tick 0 is 06:00.
        let minutes = ((self.world.day_time.rem_euclid(24000) + 6000) % 24000) * 1440 / 24000;
        line(
            batch,
            5.0,
            None,
            format!("#{day}  {:02}:{:02}", minutes / 60, minutes % 60),
            white,
        );
    }
}

/// The inventory page's grid, centred in the content: its top left corner.
fn deck_inventory_origin([x, y, w, h]: [f32; 4]) -> (f32, f32) {
    let grid_w = 9.0 * SLOT;
    let grid_h = 4.0 * SLOT + 4.0;
    (
        (x + (w - grid_w) / 2.0).round(),
        (y + (h - grid_h) / 2.0).round(),
    )
}

fn page_title(page: Page) -> String {
    lang::get(match page {
        Page::Inventory => "container.inventory",
        Page::Map => "item.minecraft.filled_map",
        Page::Character => "entity.minecraft.player",
        Page::Status => "gui.stats",
    })
}
