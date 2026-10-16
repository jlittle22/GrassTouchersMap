use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use crate::model::offset_data::OFFSET_DATA;

/// world units per offset unit in `OFFSET_DATA`
const OFFSET_SCALE: f32 = 125.0;
/// how far an island's outline reaches past its outermost town slots, in world units
const OUTLINE_MARGIN: f32 = 0.45;
/// town slots on every resource island, rocks have fewer
const RESOURCE_ISLAND_SLOTS: u8 = 20;

/// an island's more or less plentiful resource
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Resource {
    Wood,
    Stone,
    Iron,
}

impl Resource {
    /// parses the names Grepolis uses (`wood`, `stone`, `iron`), ignoring case
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_lowercase().as_str() {
            "wood" => Some(Self::Wood),
            "stone" => Some(Self::Stone),
            "iron" => Some(Self::Iron),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Wood => "wood",
            Self::Stone => "stone",
            Self::Iron => "iron",
        }
    }
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// An island as the view uses it. Only islands with town slots make it to the view, the
/// uninhabitable types 17-36 don't.
#[derive(Debug, Clone)]
pub struct Island {
    pub id: u32,
    /// island coordinates, the top left of its shape
    pub x: u16,
    pub y: u16,
    pub typ: u8,
    /// town slots on this island type
    pub slots: u8,
    /// slots without a town, as reported by Grepolis
    pub free_slots: u8,
    /// bit `n` is set when slot `n` has a town
    pub occupied: u32,
    pub resource_plus: Option<Resource>,
    pub resource_minus: Option<Resource>,
}

impl Island {
    pub fn shape(&self) -> Option<&'static IslandShape> {
        IslandShape::of(self.typ)
    }

    /// the middle of the island in world coordinates
    pub fn center(&self) -> egui::Pos2 {
        let origin = egui::pos2(f32::from(self.x), f32::from(self.y));
        self.shape()
            .map_or(origin + egui::vec2(0.5, 0.5), |shape| origin + shape.center)
    }

    /// the outline in world coordinates, empty for unknown island types
    pub fn outline(&self) -> impl Iterator<Item = egui::Pos2> + '_ {
        let origin = egui::pos2(f32::from(self.x), f32::from(self.y));
        self.shape()
            .into_iter()
            .flat_map(|shape| shape.outline.iter())
            .map(move |p| origin + *p)
    }

    /// world positions of the slots without a town
    pub fn free_slot_positions(&self) -> impl Iterator<Item = egui::Pos2> + '_ {
        let origin = egui::pos2(f32::from(self.x), f32::from(self.y));
        self.shape()
            .into_iter()
            .flat_map(|shape| shape.slots.iter())
            .filter(|(slot, _)| self.occupied & (1 << slot) == 0)
            .map(move |(_, p)| origin + *p)
    }

    /// whether the world position `p` is inside the island's outline
    pub fn contains(&self, p: egui::Pos2) -> bool {
        let outline: Vec<egui::Pos2> = self.outline().collect();
        outline.len() >= 3
            && (0..outline.len()).all(|i| {
                let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
                (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x) >= 0.0
            })
    }

    /// whether this is a full sized island with resources and 20 town slots, as opposed to a
    /// rock with fewer slots
    pub fn is_resource_island(&self) -> bool {
        self.slots >= RESOURCE_ISLAND_SLOTS
    }

    /// Grepolis' ocean number, e.g. 45 for x 400..500, y 500..600
    pub fn ocean(&self) -> u16 {
        self.x / 100 * 10 + self.y / 100
    }
}

/// The layout of one island type: where its town slots are and an outline around them,
/// relative to the island's coordinates.
#[derive(Debug)]
pub struct IslandShape {
    /// slot number and position
    pub slots: Vec<(u8, egui::Vec2)>,
    /// convex, clockwise on screen
    pub outline: Vec<egui::Vec2>,
    pub center: egui::Vec2,
}

impl IslandShape {
    /// the shape of island type `typ`, `None` for types without town slots
    pub fn of(typ: u8) -> Option<&'static Self> {
        static SHAPES: OnceLock<HashMap<u8, IslandShape>> = OnceLock::new();
        SHAPES.get_or_init(Self::from_offset_data).get(&typ)
    }

    /// number of town slots on island type `typ`
    pub fn slot_count(typ: u8) -> u8 {
        Self::of(typ).map_or(0, |shape| {
            u8::try_from(shape.slots.len()).unwrap_or(u8::MAX)
        })
    }

    fn from_offset_data() -> HashMap<u8, Self> {
        let mut slots: HashMap<u8, Vec<(u8, egui::Vec2)>> = HashMap::new();
        for line in OFFSET_DATA.lines() {
            let mut values = line.split(',').map(str::parse::<u16>);
            if let (Some(Ok(typ)), Some(Ok(x)), Some(Ok(y)), Some(Ok(slot))) =
                (values.next(), values.next(), values.next(), values.next())
            {
                let (Ok(typ), Ok(slot)) = (u8::try_from(typ), u8::try_from(slot)) else {
                    continue;
                };
                let position = egui::vec2(f32::from(x) / OFFSET_SCALE, f32::from(y) / OFFSET_SCALE);
                slots.entry(typ).or_default().push((slot, position));
            }
        }
        slots
            .into_iter()
            .map(|(typ, mut slots)| {
                slots.sort_unstable_by_key(|(slot, _)| *slot);
                let points: Vec<egui::Vec2> = slots.iter().map(|(_, p)| *p).collect();
                #[allow(clippy::cast_precision_loss)]
                let center =
                    points.iter().fold(egui::Vec2::ZERO, |sum, p| sum + *p) / points.len() as f32;
                let outline = expand(&convex_hull(points), center, OUTLINE_MARGIN);
                (
                    typ,
                    Self {
                        slots,
                        outline,
                        center,
                    },
                )
            })
            .collect()
    }
}

/// Andrew's monotone chain, clockwise on screen (y pointing down)
fn convex_hull(mut points: Vec<egui::Vec2>) -> Vec<egui::Vec2> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |o: egui::Vec2, a: egui::Vec2, b: egui::Vec2| {
        (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
    };
    let mut hull: Vec<egui::Vec2> = Vec::with_capacity(points.len() * 2);
    for pass in [
        &points[..],
        &points.iter().rev().copied().collect::<Vec<_>>()[..],
    ] {
        let start = hull.len();
        for &p in pass {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0
            {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// push every point of a convex outline `margin` further away from `center`
fn expand(outline: &[egui::Vec2], center: egui::Vec2, margin: f32) -> Vec<egui::Vec2> {
    outline
        .iter()
        .map(|&p| {
            let away = p - center;
            let length = away.length();
            if length > f32::EPSILON {
                p + away / length * margin
            } else {
                p
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_inhabitable_type_has_a_shape() {
        // types 1-16 and 37-60 have town slots, 17-36 have none
        for typ in (1..=16).chain(37..=60) {
            let shape = IslandShape::of(typ).unwrap_or_else(|| panic!("type {typ}"));
            assert!(shape.outline.len() >= 3, "type {typ}");
            assert!(!shape.slots.is_empty(), "type {typ}");
        }
        assert!(IslandShape::of(20).is_none());
        assert_eq!(IslandShape::slot_count(1), 20);
        assert_eq!(IslandShape::slot_count(20), 0);
    }

    #[test]
    fn outline_surrounds_every_slot() {
        for typ in (1..=16).chain(37..=60) {
            let shape = IslandShape::of(typ).unwrap();
            let outline = &shape.outline;
            for (slot, p) in &shape.slots {
                // inside a convex polygon: on the same side of every edge
                let inside = (0..outline.len()).all(|i| {
                    let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
                    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x) >= 0.0
                });
                assert!(inside, "type {typ} slot {slot}");
            }
        }
    }

    #[test]
    fn free_slots_skip_occupied_ones() {
        let island = Island {
            id: 1,
            x: 447,
            y: 462,
            typ: 1,
            slots: 20,
            free_slots: 18,
            occupied: 0b101,
            resource_plus: Resource::parse("Iron"),
            resource_minus: Resource::parse("wood"),
        };
        assert_eq!(island.free_slot_positions().count(), 18);
        assert!(island.contains(island.center()));
        assert!(!island.contains(island.center() + egui::vec2(10.0, 0.0)));
        assert_eq!(island.ocean(), 44);
        assert_eq!(island.resource_plus, Some(Resource::Iron));
        assert!(island.is_resource_island());
    }

    #[test]
    fn resource_islands_have_20_slots_and_rocks_fewer() {
        for typ in (1..=16).chain(37..=60) {
            let slots = IslandShape::slot_count(typ);
            let resource_island = (1..=10).contains(&typ) || (37..=46).contains(&typ);
            assert_eq!(slots == 20, resource_island, "type {typ}");
            assert!(slots <= 20, "type {typ}");
        }
    }
}
