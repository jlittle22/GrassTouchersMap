use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::ops::Deref;
use std::rc::Rc;

use crate::constraint::Comparator;
use crate::emptyconstraint::EmptyConstraint;
use crate::emptyselection::{EmptyTownSelection, SelectionTarget};
use crate::island::{Island, Resource};
use crate::model::ConstraintType;
use crate::selection::AndOr;
use crate::town::Town;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Offset {
    pub typ: u8,
    pub x: u16,
    pub y: u16,
    pub slot_number: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendIsland {
    pub id: u32,
    pub x: u16,
    pub y: u16,
    pub typ: u8,
    /// town slots on this island type, 0 for the uninhabitable rocks
    pub slots: u8,
    /// slots without a town, as reported by Grepolis
    pub free_slots: u8,
    pub resource_plus: Option<Resource>,
    pub resource_minus: Option<Resource>,
}

impl BackendIsland {
    /// whether towns can be founded here, i.e. it isn't a rock
    pub fn is_inhabitable(&self) -> bool {
        self.slots > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Alliance {
    pub id: u32,
    pub name: String,
    pub points: u32,
    pub towns: u32,
    pub members: u16,
    pub rank: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Player {
    pub id: u32,
    pub name: String,
    pub alliance: Option<(u32, Rc<Alliance>)>, // link player.alliance_id == alliance.id
    pub points: u32,
    pub rank: u16,
    pub towns: u16,
}

// TODO: Merge BackendTown and Town as it is used for the frontend into one struct

#[derive(Debug, Clone)]
pub struct BackendTown {
    pub id: u32,
    pub name: String,
    pub points: u16, // had a bug where a city actually had negative points in the game
    pub player: Option<(u32, Rc<Player>)>, // link town.player_id == player.id
    pub island: Rc<BackendIsland>, // link town.x = island.x && town.y == island.y
    pub offset: (u8, Rc<Offset>), // link town.slot_number = offset.slot_number && offset.type == island.type
    pub actual_x: f32,
    pub actual_y: f32, // computed from the linked island and offset
}
impl Eq for BackendTown {}
impl PartialEq for BackendTown {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl std::hash::Hash for BackendTown {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl From<&BackendTown> for Town {
    fn from(value: &BackendTown) -> Self {
        Self {
            id: value.id,
            player_id: value.player.as_ref().map(|(id, _)| *id),
            player_name: value.player.as_ref().map(|(_, p)| p.name.clone()),
            alliance_name: value
                .player
                .as_ref()
                .and_then(|(_, p)| p.alliance.as_ref())
                .map(|(_, a)| a.name.clone()),
            name: value.name.clone(),
            x: value.actual_x,
            y: value.actual_y,
            slot_number: value.offset.1.slot_number,
            points: value.points,
            island_id: value.island.id,
        }
    }
}

pub struct DataTable {
    pub towns: Vec<Rc<BackendTown>>,
    /// every island, the uninhabitable rocks included
    pub islands: Vec<Rc<BackendIsland>>,
    /// island id -> the towns on it
    pub towns_by_island: HashMap<u32, Vec<Rc<BackendTown>>>,
}

impl DataTable {
    pub fn new(towns: Vec<Rc<BackendTown>>, islands: Vec<Rc<BackendIsland>>) -> Self {
        let mut towns_by_island: HashMap<u32, Vec<Rc<BackendTown>>> = HashMap::new();
        for town in &towns {
            towns_by_island
                .entry(town.island.id)
                .or_default()
                .push(Rc::clone(town));
        }
        Self {
            towns,
            islands,
            towns_by_island,
        }
    }

    pub fn get_all_towns(&self) -> Vec<Town> {
        self.towns.iter().map(|t| t.deref().into()).collect()
    }

    /// every island that has town slots
    pub fn get_all_islands(&self) -> Vec<Island> {
        self.islands
            .iter()
            .filter(|island| island.is_inhabitable())
            .map(|island| self.view_island(island))
            .collect()
    }

    fn view_island(&self, island: &BackendIsland) -> Island {
        let occupied = self
            .towns_by_island
            .get(&island.id)
            .into_iter()
            .flatten()
            .filter(|town| town.offset.0 < 32)
            .fold(0, |mask, town| mask | 1 << town.offset.0);
        Island {
            id: island.id,
            x: island.x,
            y: island.y,
            typ: island.typ,
            slots: island.slots,
            free_slots: island.free_slots,
            occupied,
            resource_plus: island.resource_plus,
            resource_minus: island.resource_minus,
        }
    }

    /// the islands matching an island selection, see `matching_islands_for_selection`
    pub fn get_islands_for_constraints(
        &self,
        selection: &EmptyTownSelection,
        all_selections: &[EmptyTownSelection],
    ) -> Vec<Island> {
        if selection.constraints.is_empty() {
            return Vec::new();
        }
        let ids = matching_islands_for_selection(self, selection, all_selections);
        self.islands
            .iter()
            .filter(|island| ids.contains(&island.id))
            .map(|island| self.view_island(island))
            .collect()
    }

    pub fn get_ghost_towns(&self) -> Vec<Town> {
        self.towns
            .iter()
            .map(std::ops::Deref::deref)
            .filter(|t| t.player.is_none())
            .map(std::convert::Into::into)
            .collect()
    }

    pub fn get_names_for_constraint_type(&self, constraint_type: ConstraintType) -> Vec<String> {
        let inhabitable = self.islands.iter().filter(|i| i.is_inhabitable());
        names_for_island_constraint(inhabitable, constraint_type).unwrap_or_else(|| {
            get_names_for_constraint_type_in_town_list(&self.towns, constraint_type)
        })
    }

    pub fn get_names_for_constraint_type_in_constraints(
        &self,
        constraint_type: ConstraintType,
        selection: &EmptyTownSelection,
        all_selections: &[EmptyTownSelection],
    ) -> Vec<String> {
        if selection.constraints.is_empty() {
            return self.get_names_for_constraint_type(constraint_type);
        }

        if selection.target == SelectionTarget::Islands {
            let ids = matching_islands_for_selection(self, selection, all_selections);
            let islands = self.islands.iter().filter(|i| ids.contains(&i.id));
            if let Some(names) = names_for_island_constraint(islands, constraint_type) {
                return names;
            }
            // town attributes of an island selection: the towns on the matching islands
            let towns: Vec<Rc<BackendTown>> = ids
                .iter()
                .filter_map(|id| self.towns_by_island.get(id))
                .flatten()
                .cloned()
                .collect();
            return get_names_for_constraint_type_in_town_list(&towns, constraint_type);
        }

        let towns = self.get_backendtowns_for_constraints(selection, all_selections);
        return get_names_for_constraint_type_in_town_list(&towns, constraint_type);
    }

    pub fn get_towns_for_constraints(
        &self,
        selection: &EmptyTownSelection,
        all_selections: &[EmptyTownSelection],
    ) -> Vec<Town> {
        if selection.constraints.is_empty() {
            return Vec::new();
        }

        return self
            .get_backendtowns_for_constraints(selection, all_selections)
            .iter()
            .map(|bt| &**bt)
            .map(std::convert::Into::into)
            .collect();
    }

    pub fn get_backendtowns_for_constraints(
        &self,
        selection: &EmptyTownSelection,
        all_selections: &[EmptyTownSelection],
    ) -> Vec<Rc<BackendTown>> {
        if selection.constraints.is_empty() {
            return Vec::new();
        }

        return matching_towns_for_selection(
            self,
            &HashSet::from_iter(self.towns.clone()),
            selection,
            all_selections,
        )
        .into_iter()
        .collect();
    }
}

pub fn matching_towns_for_selection(
    db: &DataTable,
    towns: &HashSet<Rc<BackendTown>>,
    selection: &EmptyTownSelection,
    all_selections: &[EmptyTownSelection],
) -> HashSet<Rc<BackendTown>> {
    // short circuit useless selections.
    // useless selection in this case means that for all constraints where a value is provided by the user the input must be valid
    if !selection
        .constraints
        .iter()
        .filter(|c| !c.value.is_empty())
        .all(|c| c.has_valid_input(all_selections))
    {
        return HashSet::new();
    }

    let mut local_towns = match selection.constraint_join_mode {
        AndOr::And => towns.clone(), // for and we need to start with the full set and widdle it down
        AndOr::Or => HashSet::new(), // for or we need to start with nothing and build it up gradually
    };

    // for all valid constraints
    for constraint in selection
        .constraints
        .iter()
        .filter(|c| c.has_valid_input(all_selections))
    {
        // add/remove towns to/from the inital list based on if they match the constraint or not
        match selection.constraint_join_mode {
            AndOr::And => {
                // shortcut dataprocessing. AND join means that we can never reintroduce towns that were already excluded by another constraint
                constraint.matching_towns(
                    db,
                    &mut local_towns,
                    all_selections,
                    selection.constraint_join_mode,
                );
            }
            AndOr::Or => {
                // for OR joining we need to do the full list with every constraint
                let mut these_towns = towns.clone();

                constraint.matching_towns(
                    db,
                    &mut these_towns,
                    all_selections,
                    selection.constraint_join_mode,
                );

                local_towns.extend(these_towns.into_iter());
            }
        }
    }

    return local_towns;
}

/// Ids of the inhabitable islands matching an island selection. Island attributes are checked
/// on the island itself. Town attributes match islands with at least one such town, each
/// constraint on its own, so "player A and alliance B" finds islands where both have a town.
/// `in` a town selection matches islands with any of its towns, `not in` islands with none.
pub fn matching_islands_for_selection(
    db: &DataTable,
    selection: &EmptyTownSelection,
    all_selections: &[EmptyTownSelection],
) -> HashSet<u32> {
    // the same short circuit as for towns: any filled in but invalid constraint matches nothing
    if !selection
        .constraints
        .iter()
        .filter(|c| !c.value.is_empty())
        .all(|c| c.has_valid_input(all_selections))
    {
        return HashSet::new();
    }

    let candidates: HashSet<u32> = db
        .islands
        .iter()
        .filter(|island| island.is_inhabitable())
        .map(|island| island.id)
        .collect();
    let mut result = match selection.constraint_join_mode {
        AndOr::And => candidates.clone(),
        AndOr::Or => HashSet::new(),
    };
    for constraint in selection
        .constraints
        .iter()
        .filter(|c| c.has_valid_input(all_selections))
    {
        let matched = islands_matching_constraint(db, constraint, &candidates, all_selections);
        match selection.constraint_join_mode {
            AndOr::And => result.retain(|id| matched.contains(id)),
            AndOr::Or => result.extend(matched),
        }
    }
    result
}

fn islands_matching_constraint(
    db: &DataTable,
    constraint: &EmptyConstraint,
    candidates: &HashSet<u32>,
    all_selections: &[EmptyTownSelection],
) -> HashSet<u32> {
    let islands_of = |towns: HashSet<Rc<BackendTown>>| -> HashSet<u32> {
        towns.iter().map(|town| town.island.id).collect()
    };
    match constraint.comparator {
        Comparator::InSelection | Comparator::NotInSelection => {
            let Some(referenced) = all_selections.iter().find(|s| s.name == constraint.value)
            else {
                return HashSet::new();
            };
            let ids = match referenced.target {
                SelectionTarget::Islands => {
                    matching_islands_for_selection(db, referenced, all_selections)
                }
                SelectionTarget::Towns => islands_of(matching_towns_for_selection(
                    db,
                    &db.towns.iter().cloned().collect(),
                    referenced,
                    all_selections,
                )),
            };
            if constraint.comparator == Comparator::InSelection {
                ids
            } else {
                candidates.difference(&ids).copied().collect()
            }
        }
        Comparator::LessThan
        | Comparator::Equal
        | Comparator::GreaterThan
        | Comparator::NotEqual => {
            let value = constraint.value.parse().unwrap_or(0.0);
            if constraint.is_island_attribute() {
                db.islands
                    .iter()
                    .filter(|island| candidates.contains(&island.id))
                    .filter(|island| constraint.island_matches(island, value))
                    .map(|island| island.id)
                    .collect()
            } else {
                let mut towns = db.towns.iter().cloned().collect();
                constraint.matching_towns(db, &mut towns, all_selections, AndOr::And);
                islands_of(towns)
            }
        }
    }
}

/// Drop down values of an island attribute, `None` for other constraint types.
fn names_for_island_constraint<'a>(
    islands: impl Iterator<Item = &'a Rc<BackendIsland>>,
    constraint_type: ConstraintType,
) -> Option<Vec<String>> {
    fn numbers<T: Ord + ToString>(mut values: Vec<T>) -> Vec<String> {
        values.sort_unstable();
        values.dedup();
        values.iter().map(ToString::to_string).collect()
    }
    let islands: Vec<&Rc<BackendIsland>> = islands.collect();
    Some(match constraint_type {
        ConstraintType::IslandID => numbers(islands.iter().map(|i| i.id).collect()),
        ConstraintType::IslandX => numbers(islands.iter().map(|i| i.x).collect()),
        ConstraintType::IslandY => numbers(islands.iter().map(|i| i.y).collect()),
        ConstraintType::IslandType => numbers(islands.iter().map(|i| i.typ).collect()),
        ConstraintType::IslandTowns => numbers(islands.iter().map(|i| i.free_slots).collect()),
        ConstraintType::IslandResMore => {
            numbers(islands.iter().filter_map(|i| i.resource_plus).collect())
        }
        ConstraintType::IslandResLess => {
            numbers(islands.iter().filter_map(|i| i.resource_minus).collect())
        }
        _ => return None,
    })
}

#[allow(clippy::too_many_lines)]
pub fn get_names_for_constraint_type_in_town_list(
    towns: &[Rc<BackendTown>],
    constraint_type: ConstraintType,
) -> Vec<String> {
    // This is a big chunk of the actual work the program is doing. If we want to speed it up, we could
    // - cache the format! calls
    //
    // It does not help to
    // - turn the collect::<Vec> -> sort -> dedup chain into a BTreeSet. I tested it

    return match constraint_type {
        ConstraintType::PlayerID => {
            let mut values = towns
                .iter()
                .filter_map(|t| t.player.as_ref())
                .map(|(id, _player)| id)
                .collect::<Vec<_>>();
            values.sort_unstable();
            values.dedup();
            values.iter().map(|x| format!("{x}")).collect::<Vec<_>>()
        }
        ConstraintType::PlayerName => {
            let mut values = towns
                .iter()
                .filter_map(|t| t.player.as_ref())
                .map(|(_id, player)| player.name.clone())
                .collect::<Vec<_>>();
            values.sort_unstable_by_key(|k| k.to_lowercase());
            values.dedup();
            values
        }
        ConstraintType::PlayerPoints => {
            // todo
            let mut values = towns
                .iter()
                .filter_map(|t| t.player.as_ref())
                .map(|(_id, player)| player.points)
                .collect::<Vec<_>>();
            values.sort_unstable();
            values.dedup();
            values.iter().map(|x| format!("{x}")).collect::<Vec<_>>()
        }
        ConstraintType::PlayerRank => {
            let mut values = towns
                .iter()
                .filter_map(|t| t.player.as_ref())
                .map(|(_id, player)| player.rank)
                .collect::<Vec<_>>();
            values.sort_unstable();
            values.dedup();
            values.iter().map(|x| format!("{x}")).collect::<Vec<_>>()
        }
        ConstraintType::PlayerTowns => {
            let mut values = towns
                .iter()
                .filter_map(|t| t.player.as_ref())
                .map(|(_id, player)| player.towns)
                .collect::<Vec<_>>();
            values.sort_unstable();
            values.dedup();
            values.iter().map(|x| format!("{x}")).collect::<Vec<_>>()
        }
        ConstraintType::AllianceName => {
            let mut values = towns
                .iter()
                .filter_map(|t| t.player.as_ref())
                .filter_map(|(_id, player)| player.alliance.as_ref())
                .map(|(_id, ally)| ally.name.clone())
                .collect::<Vec<_>>();
            values.sort_unstable_by_key(|k| k.to_lowercase());
            values.dedup();
            values
        }
        ConstraintType::AlliancePoints => {
            let mut values = towns
                .iter()
                .filter_map(|t| t.player.as_ref())
                .filter_map(|(_id, player)| player.alliance.as_ref())
                .map(|(_id, ally)| ally.points)
                .collect::<Vec<_>>();
            values.sort_unstable();
            values.dedup();
            values.iter().map(|x| format!("{x}")).collect::<Vec<_>>()
        }
        ConstraintType::AllianceTowns => {
            let mut values = towns
                .iter()
                .filter_map(|t| t.player.as_ref())
                .filter_map(|(_id, player)| player.alliance.as_ref())
                .map(|(_id, ally)| ally.towns)
                .collect::<Vec<_>>();
            values.sort_unstable();
            values.dedup();
            values.iter().map(|x| format!("{x}")).collect::<Vec<_>>()
        }
        ConstraintType::AllianceMembers => {
            let mut values = towns
                .iter()
                .filter_map(|t| t.player.as_ref())
                .filter_map(|(_id, player)| player.alliance.as_ref())
                .map(|(_id, ally)| ally.members)
                .collect::<Vec<_>>();
            values.sort_unstable();
            values.dedup();
            values.iter().map(|x| format!("{x}")).collect::<Vec<_>>()
        }
        ConstraintType::AllianceRank => {
            let mut values = towns
                .iter()
                .filter_map(|t| t.player.as_ref())
                .filter_map(|(_id, player)| player.alliance.as_ref())
                .map(|(_id, ally)| ally.rank)
                .collect::<Vec<_>>();
            values.sort_unstable();
            values.dedup();
            values.iter().map(|x| format!("{x}")).collect::<Vec<_>>()
        }
        ConstraintType::TownID => {
            let mut values = towns.iter().map(|t| t.id).collect::<Vec<_>>();
            values.sort_unstable();
            values.dedup();
            values.iter().map(|x| format!("{x}")).collect::<Vec<_>>()
        }
        ConstraintType::TownName => {
            let mut values = towns.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
            values.sort_unstable_by_key(|s| s.to_lowercase());
            values.dedup();
            values
        }
        ConstraintType::TownPoints => {
            let mut values = towns.iter().map(|t| t.points).collect::<Vec<_>>();
            values.sort_unstable();
            values.dedup();
            values.iter().map(|x| format!("{x}")).collect::<Vec<_>>()
        }
        ConstraintType::IslandID
        | ConstraintType::IslandX
        | ConstraintType::IslandY
        | ConstraintType::IslandType
        | ConstraintType::IslandTowns
        | ConstraintType::IslandResMore
        | ConstraintType::IslandResLess => {
            names_for_island_constraint(towns.iter().map(|t| &t.island), constraint_type)
                .unwrap_or_default()
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::APIResponse;

    /// The world in `GTMAP_TEST_DATA` (a folder with players, alliances, towns and islands
    /// .txt), or `None` to skip.
    fn load() -> Option<DataTable> {
        let dir = std::path::PathBuf::from(std::env::var_os("GTMAP_TEST_DATA")?);
        let read = |file: &str| Some(std::fs::read_to_string(dir.join(file)).unwrap());
        let mut response = APIResponse::new(String::from("test"));
        response.players = read("players.txt");
        response.alliances = read("alliances.txt");
        response.towns = read("towns.txt");
        response.islands = read("islands.txt");
        Some(DataTable::create_for_world(response))
    }

    fn selection(
        name: &str,
        target: SelectionTarget,
        constraints: &[(ConstraintType, Comparator, &str)],
    ) -> EmptyTownSelection {
        EmptyTownSelection {
            name: name.to_owned(),
            target,
            constraints: constraints
                .iter()
                .map(|&(constraint_type, comparator, value)| EmptyConstraint {
                    constraint_type,
                    comparator,
                    value: value.to_owned(),
                })
                .collect(),
            ..EmptyTownSelection::default()
        }
    }

    #[test]
    fn islands_on_real_data() {
        let Some(db) = load() else {
            eprintln!("GTMAP_TEST_DATA not set, skipping");
            return;
        };

        // free slots from islands.txt agree with the towns on each island
        let inhabitable: Vec<_> = db.islands.iter().filter(|i| i.is_inhabitable()).collect();
        for island in &inhabitable {
            let towns = db.towns_by_island.get(&island.id).map_or(0, Vec::len);
            assert_eq!(
                usize::from(island.free_slots) + towns,
                usize::from(island.slots)
            );
        }
        let views = db.get_all_islands();
        assert_eq!(views.len(), inhabitable.len());
        for island in &views {
            assert_eq!(
                island.free_slot_positions().count(),
                usize::from(island.free_slots)
            );
        }

        // the alliance with the most towns
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for town in &db.towns {
            if let Some((_, ally)) = town.player.as_ref().and_then(|(_, p)| p.alliance.as_ref()) {
                *counts.entry(&ally.name).or_default() += 1;
            }
        }
        let (alliance, _) = counts.into_iter().max_by_key(|&(_, n)| n).unwrap();
        let alliance_islands: HashSet<u32> = db
            .towns
            .iter()
            .filter(|t| {
                t.player
                    .as_ref()
                    .and_then(|(_, p)| p.alliance.as_ref())
                    .is_some_and(|(_, a)| a.name == alliance)
            })
            .map(|t| t.island.id)
            .collect();

        let towns_of = selection(
            "towns",
            SelectionTarget::Towns,
            &[(ConstraintType::AllianceName, Comparator::Equal, alliance)],
        );
        let islands_of = selection(
            "islands",
            SelectionTarget::Islands,
            &[(ConstraintType::AllianceName, Comparator::Equal, alliance)],
        );
        let free = selection(
            "free",
            SelectionTarget::Islands,
            &[(ConstraintType::IslandTowns, Comparator::GreaterThan, "3")],
        );
        let free_in_towns = selection(
            "free in towns",
            SelectionTarget::Islands,
            &[
                (ConstraintType::IslandTowns, Comparator::GreaterThan, "3"),
                (
                    ConstraintType::AllianceName,
                    Comparator::InSelection,
                    "towns",
                ),
            ],
        );
        let towns_on_free = selection(
            "towns on free",
            SelectionTarget::Towns,
            &[(ConstraintType::PlayerName, Comparator::InSelection, "free")],
        );
        let all = [
            towns_of.clone(),
            islands_of.clone(),
            free.clone(),
            free_in_towns.clone(),
            towns_on_free.clone(),
        ];

        let found = matching_islands_for_selection(&db, &islands_of, &all);
        assert_eq!(found, alliance_islands);

        let free_ids = matching_islands_for_selection(&db, &free, &all);
        let expected_free: HashSet<u32> = inhabitable
            .iter()
            .filter(|i| i.free_slots >= 3)
            .map(|i| i.id)
            .collect();
        assert_eq!(free_ids, expected_free);

        let both = matching_islands_for_selection(&db, &free_in_towns, &all);
        assert_eq!(both, &free_ids & &alliance_islands);

        let towns = db.get_backendtowns_for_constraints(&towns_on_free, &all);
        assert!(towns.iter().all(|t| free_ids.contains(&t.island.id)));
        let expected_towns = db
            .towns
            .iter()
            .filter(|t| free_ids.contains(&t.island.id))
            .count();
        assert_eq!(towns.len(), expected_towns);

        // drop down values of an island selection come from its islands
        let names = db.get_names_for_constraint_type_in_constraints(
            ConstraintType::IslandTowns,
            &free,
            &all,
        );
        assert!(names.iter().all(|n| n.parse::<u8>().unwrap() >= 3));

        eprintln!(
            "{alliance}: {} islands, inhabitable {}, free>=3 {}, both {}, towns on free>=3 {}",
            found.len(),
            inhabitable.len(),
            free_ids.len(),
            both.len(),
            towns.len()
        );
    }
}
