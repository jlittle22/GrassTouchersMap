use std::collections::HashSet;
use std::fmt::Write as _;

use egui::Shape;

use crate::constraint::{Comparator, ConstraintType};
use crate::emptyconstraint::EmptyConstraint;
use crate::emptyselection::{EmptyTownSelection, SelectionTarget};
use crate::island::Island;
use crate::selection::{AndOr, TownSelection};
use crate::town::Town;

use super::{
    data::{CanvasData, ViewPortFilter},
    screenshot::Scene,
    Progress, State, View,
};

/// brand's lightish green (sampled from assets/grass_touchers_logo.png), used to
/// highlight the ctrl+drag rectangle selection and the towns it picks out.
const SELECTION_GREEN: egui::Color32 = egui::Color32::from_rgb(143, 199, 62);

/// zoom (screen pixels per world unit) from which islands are drawn as outlines, and island
/// selections as filled outlines instead of dots
const ISLAND_OUTLINE_ZOOM: f32 = 4.0;
/// zoom from which the free town slots on islands are drawn
const FREE_SLOT_ZOOM: f32 = 12.0;
/// island selections drawn as dots when zoomed out
const ISLAND_DOT_RADIUS: f32 = 2.5;

/// an island's outline on screen
fn island_outline(canvas: &CanvasData, island: &Island) -> Vec<egui::Pos2> {
    island
        .outline()
        .map(|p| canvas.world_to_screen(p.to_vec2()).to_pos2())
        .collect()
}

/// fill and stroke of an island drawn as an outline: the layer gets a faint fill, island
/// selections a stronger one
fn island_paint(color: egui::Color32, selected: bool) -> (egui::Color32, egui::Stroke) {
    if selected {
        (color.gamma_multiply(0.35), egui::Stroke::new(1.5, color))
    } else {
        (color.gamma_multiply(0.2), egui::Stroke::new(1.0, color))
    }
}

/// The island under `click` on screen: inside its outline when outlines are drawn, otherwise
/// near its dot.
fn island_at<'a>(
    canvas: &CanvasData,
    islands: &[&'a Island],
    outlines: bool,
    click: egui::Pos2,
) -> Option<&'a Island> {
    if outlines {
        let world = canvas.screen_to_world(click.to_vec2()).to_pos2();
        islands
            .iter()
            .copied()
            .find(|island| island.contains(world))
    } else {
        let screen = |island: &Island| {
            canvas
                .world_to_screen(island.center().to_vec2())
                .to_pos2()
                .distance(click)
        };
        islands
            .iter()
            .copied()
            .map(|island| (island, screen(island)))
            .filter(|&(_, distance)| distance <= ISLAND_DOT_RADIUS + 4.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(island, _)| island)
    }
}

/// the next of a few distinct colors that no selection uses yet, for new selections
fn unused_color(selections: &[TownSelection]) -> egui::Color32 {
    const PALETTE: [egui::Color32; 8] = [
        egui::Color32::from_rgb(255, 140, 0),
        egui::Color32::from_rgb(0, 200, 255),
        egui::Color32::from_rgb(255, 64, 160),
        egui::Color32::from_rgb(255, 220, 0),
        egui::Color32::from_rgb(160, 100, 255),
        egui::Color32::from_rgb(255, 64, 64),
        egui::Color32::from_rgb(0, 220, 140),
        egui::Color32::from_rgb(255, 255, 255),
    ];
    PALETTE
        .into_iter()
        .find(|color| selections.iter().all(|s| s.color != *color))
        .unwrap_or(PALETTE[selections.len() % PALETTE.len()])
}

/// where the logo overlay goes, fixed to the map's top left corner regardless of pan/zoom
fn logo_rect(map: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        map.left_top() + egui::vec2(8.0, 8.0),
        egui::vec2(48.0, 48.0),
    )
}

/// `ctx.copy_text` does nothing on the web with our eframe build, see `wasm_utils::copy_to_clipboard`.
fn copy_text(ctx: &egui::Context, text: String) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        crate::wasm_utils::copy_to_clipboard(&text);
    }
    #[cfg(not(target_arch = "wasm32"))]
    ctx.copy_text(text);
}

/// One `[quote]` table of every selected town, in DIO-Tools' town list layout with an
/// added player column, signed with our own link:
///
/// ```text
/// [quote][size=9]2 Cities[/size]
/// [table]
/// [*]1.[|][town]3135[/town][|][player]Frayte[/player][|]14937 points[|]Ocean 44[/*]
/// [*]2.[|][town]3503[/town][|][player]Frayte[/player][|]12965 points[|]Ocean 44[/*]
/// [/table][url=https://map.grasstouchers.gg]Grass Touchers Map[/url][/quote]
/// ```
///
/// Towns are sorted by id. Ghost towns get an empty player cell.
fn town_bbcode(towns: &[&Town]) -> String {
    let mut towns = towns.to_vec();
    towns.sort_unstable_by_key(|town| town.id);

    let cities = if towns.len() == 1 { "City" } else { "Cities" };
    let mut bbcode = format!("[quote][size=9]{} {cities}[/size]\n[table]\n", towns.len());
    for (i, town) in towns.iter().enumerate() {
        let player = town
            .player_name
            .as_deref()
            .map(|name| format!("[player]{name}[/player]"))
            .unwrap_or_default();
        // oceans are 100x100 squares, numbered x then y: (447, 462) is ocean 44.
        // Coordinates are always within 0..1000, so the casts are safe.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ocean = (town.x as u32 / 100) * 10 + town.y as u32 / 100;
        let _ = writeln!(
            bbcode,
            "[*]{}.[|][town]{}[/town][|]{player}[|]{} points[|]Ocean {ocean}[/*]",
            i + 1,
            town.id,
            town.points
        );
    }
    bbcode.push_str("[/table][url=https://map.grasstouchers.gg]Grass Touchers Map[/url][/quote]");
    bbcode
}

impl View {
    #[allow(clippy::too_many_lines)] // UI Code, am I right, hahah
    pub fn ui_map(&mut self, ctx: &egui::Context) {
        // set by the context menu, done once the map no longer borrows the selections
        let mut add_island_selection = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            // the canvas frame's fill, for screenshots
            let background = ui.visuals().extreme_bg_color;
            egui::Frame::canvas(ui.style()).show(ui, |ui| {
                let (mut response, painter) = ui.allocate_painter(
                    ui.available_size_before_wrap(),
                    egui::Sense::click_and_drag(),
                );

                if self.ui_data.canvas.is_none() {
                    self.ui_data.canvas =
                        Some(CanvasData::new(-response.rect.left_top().to_vec2()));
                }
                // we need to have this as an option so we are reminded when we have to
                // reset it. The .unwrap here is fine, because if it is none we make it
                // Some() just a line above this comment.
                let canvas_data = self.ui_data.canvas.as_mut().unwrap();
                // the toggle and color of the group an island belongs to, resource islands or rocks
                let (settings_resource_islands, settings_rocks) = (
                    &self.ui_data.settings_resource_islands,
                    &self.ui_data.settings_rocks,
                );
                let island_settings = |island: &Island| {
                    if island.is_resource_island() {
                        settings_resource_islands
                    } else {
                        settings_rocks
                    }
                };

                let ctrl_held = ctx.input(|input| input.modifiers.ctrl);

                //DRAG (panning). Ctrl+drag is reserved for the rectangle town selection below.
                if !ctrl_held {
                    canvas_data.world_offset_px -=
                        canvas_data.scale_screen_to_world(response.drag_delta());
                }

                // ZOOM
                // as per https://www.youtube.com/watch?v=ZQ8qtAizis4
                if response.hovered() {
                    let mouse_position_in_world_space_before_zoom_change = {
                        if let Some(mouse_position) = response.hover_pos() {
                            canvas_data.screen_to_world(mouse_position.to_vec2())
                        } else {
                            egui::vec2(0.0, 0.0)
                        }
                    };

                    // since egui 0.26 there is the option of smooth scrolling. I don't think it'll be an improvement though
                    let scroll_delta = ctx.input(|input| input.raw_scroll_delta.y);
                    if scroll_delta > 0.0 {
                        canvas_data.zoom *= 1.2;
                    } else if scroll_delta < 0.0 {
                        canvas_data.zoom /= 1.2;
                    }

                    let mouse_position_in_world_space_after_zoom_change = {
                        if let Some(mouse_position) = response.hover_pos() {
                            canvas_data.screen_to_world(mouse_position.to_vec2())
                        } else {
                            egui::vec2(0.0, 0.0)
                        }
                    };

                    canvas_data.world_offset_px += mouse_position_in_world_space_before_zoom_change
                        - mouse_position_in_world_space_after_zoom_change;
                }

                // filter everything that is not visible
                let filter = ViewPortFilter::new(canvas_data, response.rect);
                let visible_towns_all: Vec<&Town> = self
                    .ui_data
                    .all_towns
                    .iter()
                    .filter(|town| filter.town_in_viewport(town))
                    .collect();
                let visible_islands: Vec<&Island> = self
                    .ui_data
                    .all_islands
                    .iter()
                    .filter(|island| {
                        filter.island_in_viewport(island) && island_settings(island).enabled
                    })
                    .collect();
                // hiding a group drops its picked islands, so they can't end up in a selection
                // unseen
                if !self.selected_island_ids.is_empty() {
                    for island in self.ui_data.all_islands.iter() {
                        if !island_settings(island).enabled {
                            self.selected_island_ids.remove(&island.id);
                        }
                    }
                }
                let island_outlines = canvas_data.zoom >= ISLAND_OUTLINE_ZOOM;
                let visible_ghost_towns: Vec<&Town> = self
                    .ui_data
                    .ghost_towns
                    .iter()
                    .filter(|town| filter.town_in_viewport(town))
                    .collect();

                // every town that is *actually drawn as a dot* right now: respects the "all
                // towns"/"ghost towns" toggles, the viewport, and includes towns that are only
                // shown because they match a (non-transparent) named/colored selection. This is
                // the set the ctrl+drag rectangle below is allowed to pick from, so the existing
                // filters can be used to narrow down what's selectable before marquee-selecting.
                let rendered_towns: Vec<&Town> = {
                    let mut re: Vec<&Town> = Vec::new();
                    if self.ui_data.settings_all.enabled {
                        re.extend(visible_towns_all.iter().copied());
                    }
                    if self.ui_data.settings_ghosts.enabled {
                        re.extend(visible_ghost_towns.iter().copied());
                    }
                    for selection in &self.ui_data.selections {
                        if selection.is_hidden() {
                            continue;
                        }
                        re.extend(
                            selection
                                .towns
                                .iter()
                                .filter(|t| filter.town_in_viewport(t)),
                        );
                    }
                    re
                };

                // CTRL+DRAG RECTANGLE TOWN SELECTION
                if ctrl_held {
                    if response.drag_started() {
                        self.marquee_drag_start = response.interact_pointer_pos();
                    }
                } else {
                    self.marquee_drag_start = None;
                }
                let marquee_rect = ctrl_held
                    .then(|| self.marquee_drag_start)
                    .flatten()
                    .and_then(|start| {
                        response
                            .interact_pointer_pos()
                            .or_else(|| response.hover_pos())
                            .map(|current| egui::Rect::from_two_pos(start, current))
                    });
                if let Some(rect) = marquee_rect {
                    if response.drag_released() {
                        self.selected_town_ids = rendered_towns
                            .iter()
                            .filter(|town| {
                                rect.contains(
                                    canvas_data
                                        .world_to_screen(egui::vec2(town.x, town.y))
                                        .to_pos2(),
                                )
                            })
                            .map(|town| town.id)
                            .collect();
                        self.marquee_drag_start = None;
                    }
                }

                // CLICK HANDLING. `clicked()` never fires for the drag that creates a new
                // marquee rectangle above, so neither of these can undo it.
                if response.clicked() {
                    if ctrl_held {
                        // ctrl+click (no drag) toggles just the nearest rendered town under the
                        // cursor in/out of the selection, leaving the rest of it untouched.
                        // Off the towns it toggles the island under the cursor instead.
                        if let Some(click_pos) = response.interact_pointer_pos() {
                            let mut town_hit = false;
                            let closest = rendered_towns.iter().min_by(|a, b| {
                                let da = canvas_data
                                    .world_to_screen(egui::vec2(a.x, a.y))
                                    .to_pos2()
                                    .distance(click_pos);
                                let db = canvas_data
                                    .world_to_screen(egui::vec2(b.x, b.y))
                                    .to_pos2()
                                    .distance(click_pos);
                                da.total_cmp(&db)
                            });
                            if let Some(town) = closest {
                                let screen_pos = canvas_data
                                    .world_to_screen(egui::vec2(town.x, town.y))
                                    .to_pos2();
                                // match the actual drawn dot radius (see the "DRAW ALL/GHOST
                                // TOWNS" sections below) plus a small constant margin, so the
                                // clickable area always covers the visible dot instead of a
                                // fixed pixel radius that falls inside it once zoomed in.
                                let hit_radius_px = 4.0 + canvas_data.scale_world_to_screen(0.15);
                                if screen_pos.distance(click_pos) <= hit_radius_px {
                                    town_hit = true;
                                    if !self.selected_town_ids.remove(&town.id) {
                                        self.selected_town_ids.insert(town.id);
                                    }
                                }
                            }
                            if !town_hit {
                                if let Some(island) = island_at(
                                    canvas_data,
                                    &visible_islands,
                                    island_outlines,
                                    click_pos,
                                ) {
                                    if !self.selected_island_ids.remove(&island.id) {
                                        self.selected_island_ids.insert(island.id);
                                    }
                                }
                            }
                        }
                    } else {
                        // a plain click (no ctrl) clears the whole selection
                        self.selected_town_ids.clear();
                        self.selected_island_ids.clear();
                    }
                }
                if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                    self.selected_town_ids.clear();
                    self.selected_island_ids.clear();
                }

                // DRAW GRID
                for i in (0u16..=10).map(|i| f32::from(i) * 100.0) {
                    // vertical
                    let one = canvas_data.world_to_screen(egui::vec2(0.0, i)).to_pos2();
                    let two = canvas_data.world_to_screen(egui::vec2(1000.0, i)).to_pos2();
                    painter
                        .line_segment([one, two], egui::Stroke::new(2.0, egui::Color32::DARK_GRAY));
                    // horizontal
                    let one = canvas_data.world_to_screen(egui::vec2(i, 0.0)).to_pos2();
                    let two = canvas_data.world_to_screen(egui::vec2(i, 1000.0)).to_pos2();
                    painter
                        .line_segment([one, two], egui::Stroke::new(2.0, egui::Color32::DARK_GRAY));
                }
                if canvas_data.zoom > 5.0 {
                    for i in (0u16..=100)
                        .map(|i| f32::from(i) * 10.0)
                        .filter(|&i| filter.x_in_viewport(i) || filter.y_in_viewport(i))
                    {
                        // vertical
                        let one = canvas_data.world_to_screen(egui::vec2(0.0, i)).to_pos2();
                        let two = canvas_data.world_to_screen(egui::vec2(1000.0, i)).to_pos2();
                        painter.add(Shape::dashed_line(
                            &[one, two],
                            egui::Stroke::new(1.0, egui::Color32::DARK_GRAY),
                            7.0,
                            7.0,
                        ));
                        // horizontal
                        let one = canvas_data.world_to_screen(egui::vec2(i, 0.0)).to_pos2();
                        let two = canvas_data.world_to_screen(egui::vec2(i, 1000.0)).to_pos2();
                        painter.add(Shape::dashed_line(
                            &[one, two],
                            egui::Stroke::new(1.0, egui::Color32::DARK_GRAY),
                            7.0,
                            7.0,
                        ));
                    }
                }

                // DRAW ISLANDS under the towns: outlines once they're big enough to tell apart,
                // and the free town slots as hollow circles closer still
                if island_outlines {
                    for island in &visible_islands {
                        let (fill, stroke) = island_paint(island_settings(island).color, false);
                        painter.add(egui::Shape::convex_polygon(
                            island_outline(canvas_data, island),
                            fill,
                            stroke,
                        ));
                    }
                    if canvas_data.zoom >= FREE_SLOT_ZOOM {
                        for island in &visible_islands {
                            let stroke = egui::Stroke::new(1.0, island_settings(island).color);
                            for slot in island.free_slot_positions() {
                                painter.circle_stroke(
                                    canvas_data.world_to_screen(slot.to_vec2()).to_pos2(),
                                    1.0 + canvas_data.scale_world_to_screen(0.15),
                                    stroke,
                                );
                            }
                        }
                    }
                }

                // DRAW ISLAND SELECTIONS, also under the towns so the towns stay readable
                for selection in &self.ui_data.selections {
                    if selection.is_hidden() {
                        continue;
                    }
                    let (fill, stroke) = island_paint(selection.color, true);
                    for island in selection
                        .islands
                        .iter()
                        .filter(|island| filter.island_in_viewport(island))
                    {
                        if island_outlines {
                            painter.add(egui::Shape::convex_polygon(
                                island_outline(canvas_data, island),
                                fill,
                                stroke,
                            ));
                        } else {
                            painter.circle_filled(
                                canvas_data
                                    .world_to_screen(island.center().to_vec2())
                                    .to_pos2(),
                                ISLAND_DOT_RADIUS,
                                selection.color,
                            );
                        }
                    }
                }

                // DRAW CTRL+CLICKED ISLANDS on top of the island selections
                let picked_islands: Vec<&Island> = visible_islands
                    .iter()
                    .copied()
                    .filter(|island| self.selected_island_ids.contains(&island.id))
                    .collect();
                for island in &picked_islands {
                    if island_outlines {
                        painter.add(egui::Shape::convex_polygon(
                            island_outline(canvas_data, island),
                            SELECTION_GREEN.gamma_multiply(0.25),
                            egui::Stroke::new(2.5, SELECTION_GREEN),
                        ));
                    } else {
                        painter.circle_stroke(
                            canvas_data
                                .world_to_screen(island.center().to_vec2())
                                .to_pos2(),
                            ISLAND_DOT_RADIUS + 1.5,
                            egui::Stroke::new(2.0, SELECTION_GREEN),
                        );
                    }
                }

                // DRAW ALL TOWNS
                // towns have a diameter of .25 units, approximately
                if self.ui_data.settings_all.enabled {
                    for town in &visible_towns_all {
                        painter.circle_filled(
                            canvas_data
                                .world_to_screen(egui::vec2(town.x, town.y))
                                .to_pos2(),
                            1.0 + canvas_data.scale_world_to_screen(0.15),
                            self.ui_data.settings_all.color,
                        );
                    }
                }

                // DRAW GHOST TOWNS
                if self.ui_data.settings_ghosts.enabled {
                    for town in &visible_ghost_towns {
                        painter.circle_filled(
                            canvas_data
                                .world_to_screen(egui::vec2(town.x, town.y))
                                .to_pos2(),
                            2.0 + canvas_data.scale_world_to_screen(0.15),
                            self.ui_data.settings_ghosts.color,
                        );
                    }
                }

                // DRAW SELECTED TOWS
                for selection in &self.ui_data.selections {
                    // skip selections toggled off via the eye button
                    if selection.is_hidden() {
                        continue;
                    }

                    for town in selection
                        .towns
                        .iter()
                        .filter(|t| filter.town_in_viewport(t))
                    {
                        painter.circle_filled(
                            canvas_data
                                .world_to_screen(egui::vec2(town.x, town.y))
                                .to_pos2(),
                            1.0 + canvas_data.scale_world_to_screen(0.15),
                            selection.color,
                        );
                    }
                }

                // DRAW MARQUEE (CTRL+DRAG) SELECTED TOWNS as a highlighted ring
                if !self.selected_town_ids.is_empty() {
                    for town in rendered_towns
                        .iter()
                        .filter(|t| self.selected_town_ids.contains(&t.id))
                    {
                        painter.circle_stroke(
                            canvas_data
                                .world_to_screen(egui::vec2(town.x, town.y))
                                .to_pos2(),
                            3.0 + canvas_data.scale_world_to_screen(0.15),
                            egui::Stroke::new(2.0, SELECTION_GREEN),
                        );
                    }
                }

                // DRAW THE LIVE CTRL+DRAG SELECTION RECTANGLE
                if let Some(rect) = marquee_rect {
                    painter.rect_filled(
                        rect,
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(143, 199, 62, 30),
                    );
                    painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, SELECTION_GREEN));
                }

                // RIGHT CLICK MENU FOR ACTIONS ON THE (MARQUEE) SELECTION
                response.context_menu(|ui| {
                    if !self.selected_island_ids.is_empty()
                        && ui
                            .button(t!(
                                "map.context_menu.add_island_selection",
                                count = self.selected_island_ids.len()
                            ))
                            .clicked()
                    {
                        add_island_selection = true;
                        ui.close_menu();
                    }

                    if ui
                        .button(t!("map.context_menu.copy_mailing_list"))
                        .clicked()
                    {
                        // unique, alphabetically sorted owners of the selected towns, joined as
                        // "adamb; bobbydonut; lawnjittle; timbrick". Ghost/unowned towns have no
                        // player and are skipped.
                        let owners: std::collections::BTreeSet<&str> = self
                            .ui_data
                            .all_towns
                            .iter()
                            .chain(self.ui_data.ghost_towns.iter())
                            .filter(|town| self.selected_town_ids.contains(&town.id))
                            .filter_map(|town| town.player_name.as_deref())
                            .collect();
                        let mailing_list = owners.into_iter().collect::<Vec<_>>().join("; ");
                        copy_text(ctx, mailing_list);
                        ui.close_menu();
                    }

                    if ui.button(t!("map.context_menu.copy_town_bbcode")).clicked() {
                        // one DIO-Tools style table of all the towns, see `town_bbcode`.
                        // all_towns already includes the ghost towns.
                        let towns: Vec<&Town> = self
                            .ui_data
                            .all_towns
                            .iter()
                            .filter(|town| self.selected_town_ids.contains(&town.id))
                            .collect();
                        let bbcode = town_bbcode(&towns);
                        copy_text(ctx, bbcode);
                        ui.close_menu();
                    }

                    if ui
                        .button(t!("map.context_menu.copy_screenshot_link"))
                        .clicked()
                    {
                        // the same layers as the drawing above, in the same order
                        let to_screen = |town: &Town| {
                            canvas_data
                                .world_to_screen(egui::vec2(town.x, town.y))
                                .to_pos2()
                        };
                        let town_radius = canvas_data.scale_world_to_screen(0.15);
                        let mut scene =
                            Scene::new(response.rect, ctx.pixels_per_point(), background);
                        for i in (0u16..=10).map(|i| f32::from(i) * 100.0) {
                            for (from, to) in [
                                (egui::vec2(0.0, i), egui::vec2(1000.0, i)),
                                (egui::vec2(i, 0.0), egui::vec2(i, 1000.0)),
                            ] {
                                scene.line(
                                    canvas_data.world_to_screen(from).to_pos2(),
                                    canvas_data.world_to_screen(to).to_pos2(),
                                    2.0,
                                    egui::Color32::DARK_GRAY,
                                    None,
                                );
                            }
                        }
                        if canvas_data.zoom > 5.0 {
                            for i in (0u16..=100)
                                .map(|i| f32::from(i) * 10.0)
                                .filter(|&i| filter.x_in_viewport(i) || filter.y_in_viewport(i))
                            {
                                for (from, to) in [
                                    (egui::vec2(0.0, i), egui::vec2(1000.0, i)),
                                    (egui::vec2(i, 0.0), egui::vec2(i, 1000.0)),
                                ] {
                                    scene.line(
                                        canvas_data.world_to_screen(from).to_pos2(),
                                        canvas_data.world_to_screen(to).to_pos2(),
                                        1.0,
                                        egui::Color32::DARK_GRAY,
                                        Some((7.0, 7.0)),
                                    );
                                }
                            }
                        }
                        if island_outlines {
                            for (group, resource_islands) in
                                [(settings_resource_islands, true), (settings_rocks, false)]
                            {
                                let islands = || {
                                    visible_islands.iter().filter(move |island| {
                                        island.is_resource_island() == resource_islands
                                    })
                                };
                                let (fill, stroke) = island_paint(group.color, false);
                                scene.polygons(
                                    fill,
                                    stroke,
                                    islands().map(|island| island_outline(canvas_data, island)),
                                );
                                if canvas_data.zoom >= FREE_SLOT_ZOOM {
                                    scene.rings(
                                        group.color,
                                        1.0 + town_radius,
                                        1.0,
                                        islands().flat_map(|i| i.free_slot_positions()).map(
                                            |slot| {
                                                canvas_data
                                                    .world_to_screen(slot.to_vec2())
                                                    .to_pos2()
                                            },
                                        ),
                                    );
                                }
                            }
                        }
                        for selection in &self.ui_data.selections {
                            if selection.is_hidden() || selection.islands.is_empty() {
                                continue;
                            }
                            let islands = selection
                                .islands
                                .iter()
                                .filter(|island| filter.island_in_viewport(island));
                            if island_outlines {
                                let (fill, stroke) = island_paint(selection.color, true);
                                scene.polygons(
                                    fill,
                                    stroke,
                                    islands.map(|island| island_outline(canvas_data, island)),
                                );
                            } else {
                                scene.dots(
                                    selection.color,
                                    ISLAND_DOT_RADIUS,
                                    islands.map(|island| {
                                        canvas_data
                                            .world_to_screen(island.center().to_vec2())
                                            .to_pos2()
                                    }),
                                );
                            }
                        }
                        if !picked_islands.is_empty() && island_outlines {
                            scene.polygons(
                                SELECTION_GREEN.gamma_multiply(0.25),
                                egui::Stroke::new(2.5, SELECTION_GREEN),
                                picked_islands
                                    .iter()
                                    .map(|island| island_outline(canvas_data, island)),
                            );
                        } else if !picked_islands.is_empty() {
                            scene.rings(
                                SELECTION_GREEN,
                                ISLAND_DOT_RADIUS + 1.5,
                                2.0,
                                picked_islands.iter().map(|island| {
                                    canvas_data
                                        .world_to_screen(island.center().to_vec2())
                                        .to_pos2()
                                }),
                            );
                        }
                        if self.ui_data.settings_all.enabled {
                            scene.dots(
                                self.ui_data.settings_all.color,
                                1.0 + town_radius,
                                visible_towns_all.iter().map(|t| to_screen(t)),
                            );
                        }
                        if self.ui_data.settings_ghosts.enabled {
                            scene.dots(
                                self.ui_data.settings_ghosts.color,
                                2.0 + town_radius,
                                visible_ghost_towns.iter().map(|t| to_screen(t)),
                            );
                        }
                        for selection in &self.ui_data.selections {
                            if !selection.is_hidden() {
                                scene.dots(
                                    selection.color,
                                    1.0 + town_radius,
                                    selection
                                        .towns
                                        .iter()
                                        .filter(|t| filter.town_in_viewport(t))
                                        .map(to_screen),
                                );
                            }
                        }
                        if !self.selected_town_ids.is_empty() {
                            scene.rings(
                                SELECTION_GREEN,
                                3.0 + town_radius,
                                2.0,
                                rendered_towns
                                    .iter()
                                    .filter(|t| self.selected_town_ids.contains(&t.id))
                                    .map(|t| to_screen(t)),
                            );
                        }
                        scene.logo(logo_rect(response.rect));
                        copy_text(ctx, scene.upload());
                        ui.close_menu();
                    }

                    if ui.button(t!("map.context_menu.copy_stats")).clicked() {
                        // summary stats for the selected towns, joined as
                        // "12 towns | avg 342 pts | min 120 | max 980".
                        let points: Vec<u16> = self
                            .ui_data
                            .all_towns
                            .iter()
                            .chain(self.ui_data.ghost_towns.iter())
                            .filter(|town| self.selected_town_ids.contains(&town.id))
                            .map(|town| town.points)
                            .collect();
                        if !points.is_empty() {
                            let count = points.len();
                            let sum: u64 = points.iter().map(|&p| p as u64).sum();
                            let avg = sum as f64 / count as f64;
                            let min = points.iter().min().unwrap();
                            let max = points.iter().max().unwrap();
                            let summary =
                                format!("{count} towns | avg {avg:.0} pts | min {min} | max {max}");
                            copy_text(ctx, summary);
                        }
                        ui.close_menu();
                    }
                });

                // POPUP WITH TOWN INFORMATION
                if canvas_data.zoom > 10.0 {
                    let optional_mouse_position = response.hover_pos();
                    // NOTE: This is broken with the jump from egui 0.26 to egui 0.28. The hover ui now apparently
                    // remembers the smallest width it ever had and will never go above that width. Since we have short
                    // and long hover texts this causes unnecessary and ugly wrapping.
                    // Solved it by downgrading. But it if I'm felling up to it I might make an issue later.
                    response = response.on_hover_ui_at_pointer(|ui| {
                        let position = if let Some(mouse_position) = optional_mouse_position {
                            canvas_data
                                .screen_to_world(mouse_position.to_vec2())
                                .to_pos2()
                        } else {
                            return;
                        };
                        ui.label(format!("{position:?}"));

                        let closest_town = visible_towns_all
                            .iter()
                            .map(|town| (town, position.distance(egui::pos2(town.x, town.y))))
                            .min_by(|a, b| a.1.total_cmp(&b.1))
                            .filter(|&(_, distance)| distance < 1.5)
                            .map(|(town, _)| *town);

                        if let Some(town) = closest_town {
                            let mut text = t!(
                                "map.hover",
                                name = town.name,
                                points = town.points,
                                player = town.player_name.as_deref().unwrap_or(""),
                                alliance = town.alliance_name.as_deref().unwrap_or(""),
                            );
                            let all_islands = &self.ui_data.all_islands;
                            if let Some(island) =
                                all_islands.iter().find(|i| i.id == town.island_id)
                            {
                                text.push('\n');
                                text.push_str(&t!(
                                    "map.hover_island",
                                    id = island.id,
                                    free = island.free_slots,
                                    slots = island.slots,
                                ));
                            }
                            ui.label(text);
                        } else if let Some(island) = visible_islands
                            .iter()
                            .find(|island| island.contains(position))
                        {
                            let resource = |r: Option<crate::island::Resource>| {
                                r.map_or("?", crate::island::Resource::name)
                            };
                            ui.label(t!(
                                "map.island_hover",
                                id = island.id,
                                ocean = island.ocean(),
                                typ = island.typ,
                                free = island.free_slots,
                                slots = island.slots,
                                plus = resource(island.resource_plus),
                                minus = resource(island.resource_minus),
                            ));
                        }
                    });
                }

                // LOGO OVERLAY, fixed to the top left corner of the map regardless of pan/zoom
                let logo_rect = logo_rect(response.rect);
                ui.put(
                    logo_rect,
                    egui::Image::new(egui::include_image!("../../assets/grass_touchers_logo.png"))
                        .max_size(logo_rect.size())
                        .maintain_aspect_ratio(true),
                );

                response
            })
        });
        if add_island_selection {
            self.add_island_selection();
        }
    }

    /// Turn the ctrl+clicked islands into a new island selection, one `IslandID = id` per
    /// island joined with `or`, in a color no other selection uses so it stands out. Its
    /// color can then be changed in the side panel like any other selection's.
    fn add_island_selection(&mut self) {
        let mut ids: Vec<u32> = self.selected_island_ids.drain().collect();
        ids.sort_unstable();
        let base = t!("map.picked_islands");
        // with n selections one of the first n + 1 names is always free
        let name = (1..=self.ui_data.selections.len() + 1)
            .map(|n| {
                if n == 1 {
                    base.clone()
                } else {
                    format!("{base} {n}")
                }
            })
            .find(|name| self.ui_data.selections.iter().all(|s| &s.name != name))
            .unwrap_or(base);
        let mut selection = EmptyTownSelection {
            name,
            constraints: ids
                .iter()
                .map(|id| EmptyConstraint {
                    constraint_type: ConstraintType::IslandID,
                    comparator: Comparator::Equal,
                    value: id.to_string(),
                })
                .collect(),
            constraint_join_mode: AndOr::Or,
            target: SelectionTarget::Islands,
            color: unused_color(&self.ui_data.selections),
            ..EmptyTownSelection::default()
        }
        .fill();

        let mut all_selections: Vec<EmptyTownSelection> = self
            .ui_data
            .selections
            .iter()
            .map(TownSelection::partial_clone)
            .collect();
        all_selections.push(selection.partial_clone());
        if let Err(err) =
            selection.refresh_self(&mut self.presenter, &HashSet::new(), &all_selections)
        {
            self.ui_state = State::Uninitialized(Progress::BackendCrashed(format!("{err:?}")));
        }
        self.ui_data.selections.push(selection);
    }
}
