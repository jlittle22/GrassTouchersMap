/// Compact, deterministic encoding of selections for share links (the `s` URL parameter).
///
/// Layout, then base64url without padding:
/// - header byte: format version in the low 7 bits, top bit set when the body is deflated
/// - body: selection count, then per selection: name, flags (bit 0 = Or, bit 1 = hidden,
///   bit 2 = selects islands),
///   RGBA (premultiplied, as `Color32` stores it), constraint count, then per constraint:
///   one byte `type_code * 8 + comparator_code` followed by the value.
///
/// Counts and string lengths are LEB128 varints, strings are UTF-8.
/// The type and comparator codes below are part of the link format: never change or reuse one,
/// only add new ones, or links that were already shared will decode to different filters.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

use crate::constraint::{Comparator, ConstraintType};
use crate::emptyconstraint::EmptyConstraint;
use crate::emptyselection::{EmptyTownSelection, HiddenId, SelectionTarget};
use crate::selection::AndOr;

const VERSION: u8 = 1;
const DEFLATED: u8 = 0x80;
const DEFLATE_LEVEL: u8 = 9;
/// Guards against links that inflate to something enormous.
const MAX_INFLATED_LEN: usize = 1 << 20;

const FLAG_OR: u8 = 1;
const FLAG_HIDDEN: u8 = 2;
const FLAG_ISLANDS: u8 = 4;

fn type_code(constraint_type: ConstraintType) -> u8 {
    match constraint_type {
        ConstraintType::PlayerID => 0,
        ConstraintType::PlayerName => 1,
        ConstraintType::PlayerPoints => 2,
        ConstraintType::PlayerRank => 3,
        ConstraintType::PlayerTowns => 4,
        ConstraintType::AllianceName => 5,
        ConstraintType::AlliancePoints => 6,
        ConstraintType::AllianceTowns => 7,
        ConstraintType::AllianceMembers => 8,
        ConstraintType::AllianceRank => 9,
        ConstraintType::TownID => 10,
        ConstraintType::TownName => 11,
        ConstraintType::TownPoints => 12,
        ConstraintType::IslandID => 13,
        ConstraintType::IslandX => 14,
        ConstraintType::IslandY => 15,
        ConstraintType::IslandType => 16,
        ConstraintType::IslandTowns => 17,
        ConstraintType::IslandResMore => 18,
        ConstraintType::IslandResLess => 19,
    }
}

fn type_from_code(code: u8) -> Option<ConstraintType> {
    Some(match code {
        0 => ConstraintType::PlayerID,
        1 => ConstraintType::PlayerName,
        2 => ConstraintType::PlayerPoints,
        3 => ConstraintType::PlayerRank,
        4 => ConstraintType::PlayerTowns,
        5 => ConstraintType::AllianceName,
        6 => ConstraintType::AlliancePoints,
        7 => ConstraintType::AllianceTowns,
        8 => ConstraintType::AllianceMembers,
        9 => ConstraintType::AllianceRank,
        10 => ConstraintType::TownID,
        11 => ConstraintType::TownName,
        12 => ConstraintType::TownPoints,
        13 => ConstraintType::IslandID,
        14 => ConstraintType::IslandX,
        15 => ConstraintType::IslandY,
        16 => ConstraintType::IslandType,
        17 => ConstraintType::IslandTowns,
        18 => ConstraintType::IslandResMore,
        19 => ConstraintType::IslandResLess,
        _ => return None,
    })
}

fn comparator_code(comparator: Comparator) -> u8 {
    match comparator {
        Comparator::LessThan => 0,
        Comparator::Equal => 1,
        Comparator::GreaterThan => 2,
        Comparator::NotEqual => 3,
        Comparator::InSelection => 4,
        Comparator::NotInSelection => 5,
    }
}

fn comparator_from_code(code: u8) -> Option<Comparator> {
    Some(match code {
        0 => Comparator::LessThan,
        1 => Comparator::Equal,
        2 => Comparator::GreaterThan,
        3 => Comparator::NotEqual,
        4 => Comparator::InSelection,
        5 => Comparator::NotInSelection,
        _ => return None,
    })
}

/// Encode selections into the text of the `s` URL parameter.
pub fn encode(selections: &[EmptyTownSelection]) -> String {
    let body = encode_body(selections);
    let deflated = miniz_oxide::deflate::compress_to_vec(&body, DEFLATE_LEVEL);
    // tiny link sets come out larger when deflated, so keep whichever is shorter
    let (header, payload) = if deflated.len() < body.len() {
        (VERSION | DEFLATED, deflated)
    } else {
        (VERSION, body)
    };
    let mut bytes = Vec::with_capacity(payload.len() + 1);
    bytes.push(header);
    bytes.extend_from_slice(&payload);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode the text of the `s` URL parameter. Returns `None` for anything malformed.
pub fn decode(text: &str) -> Option<Vec<EmptyTownSelection>> {
    let bytes = URL_SAFE_NO_PAD.decode(text).ok()?;
    let (&header, payload) = bytes.split_first()?;
    if header & !DEFLATED != VERSION {
        return None;
    }
    if header & DEFLATED == 0 {
        decode_body(payload)
    } else {
        let body =
            miniz_oxide::inflate::decompress_to_vec_with_limit(payload, MAX_INFLATED_LEN).ok()?;
        decode_body(&body)
    }
}

fn encode_body(selections: &[EmptyTownSelection]) -> Vec<u8> {
    let mut out = Vec::new();
    write_varint(&mut out, selections.len());
    for selection in selections {
        write_str(&mut out, &selection.name);
        let mut flags = 0;
        if selection.constraint_join_mode == AndOr::Or {
            flags |= FLAG_OR;
        }
        if selection.hidden {
            flags |= FLAG_HIDDEN;
        }
        if selection.target == SelectionTarget::Islands {
            flags |= FLAG_ISLANDS;
        }
        out.push(flags);
        out.extend_from_slice(&selection.color.to_array());
        write_varint(&mut out, selection.constraints.len());
        for constraint in &selection.constraints {
            out.push(type_code(constraint.constraint_type) * 8 + comparator_code(constraint.comparator));
            write_str(&mut out, &constraint.value);
        }
    }
    out
}

fn decode_body(bytes: &[u8]) -> Option<Vec<EmptyTownSelection>> {
    let mut reader = Reader { bytes, pos: 0 };
    let count = reader.varint()?;
    let mut selections = Vec::new();
    for _ in 0..count {
        let name = reader.string()?;
        let flags = reader.byte()?;
        if flags & !(FLAG_OR | FLAG_HIDDEN | FLAG_ISLANDS) != 0 {
            return None;
        }
        let [r, g, b, a] = [reader.byte()?, reader.byte()?, reader.byte()?, reader.byte()?];
        let constraint_count = reader.varint()?;
        let mut constraints = Vec::new();
        for _ in 0..constraint_count {
            let code = reader.byte()?;
            constraints.push(EmptyConstraint {
                constraint_type: type_from_code(code / 8)?,
                comparator: comparator_from_code(code % 8)?,
                value: reader.string()?,
            });
        }
        selections.push(EmptyTownSelection {
            name,
            hidden_id: HiddenId::default(),
            constraints,
            constraint_join_mode: if flags & FLAG_OR == 0 { AndOr::And } else { AndOr::Or },
            target: if flags & FLAG_ISLANDS == 0 {
                SelectionTarget::Towns
            } else {
                SelectionTarget::Islands
            },
            color: egui::Color32::from_rgba_premultiplied(r, g, b, a),
            hidden: flags & FLAG_HIDDEN != 0,
        });
    }
    // trailing bytes mean the link was damaged or isn't ours
    (reader.pos == bytes.len()).then_some(selections)
}

fn write_varint(out: &mut Vec<u8>, mut n: usize) {
    while n >= 0x80 {
        #[allow(clippy::cast_possible_truncation)]
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    #[allow(clippy::cast_possible_truncation)]
    out.push(n as u8);
}

fn write_str(out: &mut Vec<u8>, text: &str) {
    write_varint(out, text.len());
    out.extend_from_slice(text.as_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.pos)?;
        self.pos += 1;
        Some(byte)
    }

    fn varint(&mut self) -> Option<usize> {
        let mut n: usize = 0;
        for shift in (0..usize::BITS).step_by(7) {
            let byte = self.byte()?;
            n |= usize::from(byte & 0x7f).checked_shl(shift)?;
            if byte & 0x80 == 0 {
                return Some(n);
            }
        }
        None
    }

    fn string(&mut self) -> Option<String> {
        let len = self.varint()?;
        let end = self.pos.checked_add(len)?;
        let text = std::str::from_utf8(self.bytes.get(self.pos..end)?).ok()?;
        self.pos = end;
        Some(text.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    fn selection(
        name: &str,
        constraints: &[(ConstraintType, Comparator, &str)],
        join: AndOr,
        color: egui::Color32,
    ) -> EmptyTownSelection {
        EmptyTownSelection {
            name: name.to_owned(),
            hidden_id: HiddenId::default(),
            constraints: constraints
                .iter()
                .map(|&(constraint_type, comparator, value)| EmptyConstraint {
                    constraint_type,
                    comparator,
                    value: value.to_owned(),
                })
                .collect(),
            constraint_join_mode: join,
            target: SelectionTarget::Towns,
            color,
            hidden: false,
        }
    }

    fn readme_example() -> Vec<EmptyTownSelection> {
        vec![selection(
            "Enemies",
            &[
                (ConstraintType::AllianceName, Comparator::Equal, "The Titans"),
                (ConstraintType::PlayerPoints, Comparator::GreaterThan, "5000"),
            ],
            AndOr::And,
            egui::Color32::RED,
        )]
    }

    fn many_selections() -> Vec<EmptyTownSelection> {
        let alliances = ["The Titans", "Olympus Rising", "Sons of Sparta", "Black Fleet"];
        let mut selections: Vec<_> = alliances
            .iter()
            .map(|alliance| {
                selection(
                    alliance,
                    &[
                        (ConstraintType::AllianceName, Comparator::Equal, alliance),
                        (ConstraintType::TownPoints, Comparator::GreaterThan, "3000"),
                    ],
                    AndOr::And,
                    egui::Color32::from_rgba_unmultiplied(10, 200, 30, 128),
                )
            })
            .collect();
        let mut ghosts = selection(
            "Ghosts",
            &[(ConstraintType::PlayerID, Comparator::Equal, "")],
            AndOr::Or,
            egui::Color32::GRAY,
        );
        ghosts.hidden = true;
        selections.push(ghosts);
        selections.push(selection(
            "Ζεύς 日本",
            &[
                (ConstraintType::AllianceName, Comparator::InSelection, "The Titans"),
                (ConstraintType::AllianceName, Comparator::NotInSelection, "Ghosts"),
            ],
            AndOr::Or,
            egui::Color32::BLUE,
        ));
        let mut free = selection(
            "Free slots",
            &[
                (ConstraintType::IslandTowns, Comparator::GreaterThan, "3"),
                (ConstraintType::AllianceName, Comparator::InSelection, "The Titans"),
            ],
            AndOr::And,
            egui::Color32::YELLOW,
        );
        free.target = SelectionTarget::Islands;
        selections.push(free);
        selections
    }

    /// `EmptyTownSelection`'s `PartialEq` skips color and hidden, so compare every field.
    fn assert_same(a: &[EmptyTownSelection], b: &[EmptyTownSelection]) {
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(b) {
            assert_eq!(a, b);
            assert_eq!(a.color, b.color);
            assert_eq!(a.hidden, b.hidden);
        }
    }

    #[test]
    fn round_trips() {
        for selections in [vec![], readme_example(), many_selections()] {
            let decoded = decode(&encode(&selections)).unwrap();
            assert_same(&selections, &decoded);
        }
    }

    #[test]
    fn round_trips_every_type_and_comparator() {
        let constraints: Vec<_> = ConstraintType::iter()
            .flat_map(|t| Comparator::iter().map(move |c| (t, c, "x")))
            .collect();
        let selections = vec![selection("All", &constraints, AndOr::And, egui::Color32::RED)];
        assert_same(&selections, &decode(&encode(&selections)).unwrap());
    }

    #[test]
    fn small_links_stay_raw_and_big_links_get_deflated() {
        let header = |text: &str| URL_SAFE_NO_PAD.decode(text).unwrap()[0];
        assert_eq!(header(&encode(&readme_example())), VERSION);
        assert_eq!(header(&encode(&many_selections())), VERSION | DEFLATED);
    }

    #[test]
    fn is_much_shorter_than_base64_yaml() {
        let selections = many_selections();
        let yaml = URL_SAFE_NO_PAD.encode(serde_yaml::to_string(&selections).unwrap());
        assert!(encode(&selections).len() * 4 < yaml.len());
    }

    #[test]
    fn decodes_links_made_by_earlier_builds() {
        // if this fails, the format changed and already-shared links would break
        assert_eq!(
            encode(&readme_example()),
            "AQEHRW5lbWllcwD_AAD_AikKVGhlIFRpdGFucxIENTAwMA"
        );
        assert_same(
            &readme_example(),
            &decode("AQEHRW5lbWllcwD_AAD_AikKVGhlIFRpdGFucxIENTAwMA").unwrap(),
        );
    }

    #[test]
    fn rejects_malformed_links() {
        let good = URL_SAFE_NO_PAD.decode(encode(&readme_example())).unwrap();
        let mut unknown_version = good.clone();
        unknown_version[0] = 2;
        let mut trailing = good.clone();
        trailing.push(0);
        for bytes in [vec![], unknown_version, trailing, good[..good.len() - 1].to_vec()] {
            assert!(decode(&URL_SAFE_NO_PAD.encode(bytes)).is_none());
        }
        assert!(decode("not base64!").is_none());
        assert!(decode(&URL_SAFE_NO_PAD.encode([VERSION | DEFLATED, 1, 2, 3])).is_none());
    }
}
