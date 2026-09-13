//! The history slider and its settings dropdown. Desktop steps through local snapshot files,
//! the web through the reflector's archive.

use egui::{RichText, Ui};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

const HOUR: u64 = 60 * 60;
const DAY: u64 = 24 * HOUR;
const WEEK: u64 = 7 * DAY;
/// 1970-01-01 was a Thursday; shifting by 3 days makes week buckets start on Monday.
#[allow(clippy::cast_possible_wrap)]
const MONDAY_SHIFT: i64 = 3 * DAY as i64;

/// How far back from the newest snapshot the slider reaches.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, EnumIter, Debug)]
pub enum HistoryRange {
    OneDay,
    ThreeDays,
    OneWeek,
    TwoWeeks,
    OneMonth,
    ThreeMonths,
    #[default]
    All,
}

impl HistoryRange {
    fn secs(self) -> Option<u64> {
        match self {
            Self::OneDay => Some(DAY),
            Self::ThreeDays => Some(3 * DAY),
            Self::OneWeek => Some(WEEK),
            Self::TwoWeeks => Some(2 * WEEK),
            Self::OneMonth => Some(30 * DAY),
            Self::ThreeMonths => Some(90 * DAY),
            Self::All => None,
        }
    }

    fn label(self) -> String {
        match self {
            Self::OneDay => t!("sidepanel.history.one_day"),
            Self::ThreeDays => t!("sidepanel.history.three_days"),
            Self::OneWeek => t!("sidepanel.history.one_week"),
            Self::TwoWeeks => t!("sidepanel.history.two_weeks"),
            Self::OneMonth => t!("sidepanel.history.one_month"),
            Self::ThreeMonths => t!("sidepanel.history.three_months"),
            Self::All => t!("sidepanel.history.all"),
        }
    }
}

/// The slider shows at most one snapshot per bucket of this length.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, EnumIter, Debug)]
pub enum HistoryGranularity {
    #[default]
    SixHours,
    TwelveHours,
    OneDay,
    ThreeDays,
    OneWeek,
}

impl HistoryGranularity {
    fn secs(self) -> u64 {
        match self {
            Self::SixHours => 6 * HOUR,
            Self::TwelveHours => 12 * HOUR,
            Self::OneDay => DAY,
            Self::ThreeDays => 3 * DAY,
            Self::OneWeek => WEEK,
        }
    }

    fn label(self) -> String {
        match self {
            Self::SixHours => t!("sidepanel.history.six_hours"),
            Self::TwelveHours => t!("sidepanel.history.twelve_hours"),
            Self::OneDay => t!("sidepanel.history.one_day"),
            Self::ThreeDays => t!("sidepanel.history.three_days"),
            Self::OneWeek => t!("sidepanel.history.one_week"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Debug)]
pub struct HistorySettings {
    #[serde(default)]
    pub range: HistoryRange,
    #[serde(default)]
    pub granularity: HistoryGranularity,
}

/// Indices into `times` (unix seconds, oldest first) of the snapshots the slider shows: those
/// within the range of the newest, and of those only the newest in each granularity bucket.
/// The newest snapshot is always shown. Buckets of a day or longer start at local midnight
/// (weeks on Monday), `utc_offset` seconds east of UTC; shorter ones at UTC 00:00, 06:00, ...,
/// matching the reflector's archive slots.
#[allow(clippy::cast_possible_wrap)]
pub fn visible_points(times: &[u64], settings: HistorySettings, utc_offset: i64) -> Vec<usize> {
    let Some(&newest) = times.last() else {
        return Vec::new();
    };
    let earliest = settings
        .range
        .secs()
        .map_or(0, |range| newest.saturating_sub(range));
    let bucket_secs = settings.granularity.secs();
    let shift = match bucket_secs {
        WEEK => utc_offset + MONDAY_SHIFT,
        secs if secs >= DAY => utc_offset,
        _ => 0,
    };
    let bucket = |t: u64| (t as i64 + shift).div_euclid(bucket_secs as i64);
    (0..times.len())
        .filter(|&i| times[i] >= earliest)
        .filter(|&i| {
            times
                .get(i + 1)
                .is_none_or(|&next| bucket(next) != bucket(times[i]))
        })
        .collect()
}

pub enum Picked {
    /// index into the `times` given to `history_row`
    Snapshot(usize),
    Live,
}

/// The history row: a slider over the snapshots at `times` (unix seconds, oldest first),
/// thinned by `settings`, followed by a "Live" point if `live`, and a dropdown to change
/// `settings`. Shown only when there are at least two points.
///
/// `selected` is the chosen snapshot's time, `None` for the newest point, so the slider keeps
/// following the newest one as more arrive. Returns the point to load: the one the user just
/// moved to, or the nearest shown one when the settings hid the selected snapshot.
pub fn history_row(
    ui: &mut Ui,
    times: &[u64],
    live: bool,
    settings: &mut HistorySettings,
    selected: &mut Option<u64>,
    label: impl Fn(usize) -> String,
) -> Option<Picked> {
    if times.len() + usize::from(live) < 2 {
        return None;
    }
    let visible = visible_points(times, *settings, local_utc_offset());
    // slider positions: the visible snapshots, then the live point
    let max_index = visible.len() + usize::from(live) - 1;
    let mut current = match *selected {
        None => max_index,
        Some(t) => visible.iter().rposition(|&i| times[i] <= t).unwrap_or(0),
    };
    let mut changed = selected.is_some_and(|t| times[visible[current]] != t);
    let point_label = |position: usize| {
        visible
            .get(position)
            .map_or_else(|| t!("sidepanel.history.live"), |&i| label(i))
    };

    let slider_response = ui
        .horizontal(|ui| {
            ui.label(t!("sidepanel.header.history_slider"));
            let slider_response = (max_index > 0).then(|| {
                ui.add(
                    egui::Slider::new(&mut current, 0..=max_index)
                        .custom_formatter(|value, _range| {
                            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                            point_label(value as usize)
                        }),
                )
            });
            if max_index == 0 {
                ui.label(point_label(0));
            }
            settings_menu(ui, settings);
            slider_response
        })
        .inner;

    // egui::Slider already steps via left/right arrow keys once it has keyboard
    // focus. As a convenience also let left/right step through history when the
    // slider is *not* focused, as long as nothing else (e.g. the server id text
    // field) is currently capturing the keyboard.
    let has_focus = slider_response.as_ref().is_some_and(egui::Response::has_focus);
    changed |= slider_response.as_ref().is_some_and(egui::Response::changed);
    if !has_focus && !ui.ctx().wants_keyboard_input() {
        let (pressed_left, pressed_right) = ui.ctx().input(|input| {
            (
                input.key_pressed(egui::Key::ArrowLeft),
                input.key_pressed(egui::Key::ArrowRight),
            )
        });
        if pressed_left && current > 0 {
            current -= 1;
            changed = true;
        } else if pressed_right && current < max_index {
            current += 1;
            changed = true;
        }
    }
    *selected = (current < max_index).then(|| times[visible[current]]);
    changed.then(|| {
        visible
            .get(current)
            .map_or(Picked::Live, |&i| Picked::Snapshot(i))
    })
}

fn settings_menu(ui: &mut Ui, settings: &mut HistorySettings) {
    ui.menu_button("⚙", |ui| {
        ui.label(RichText::new(t!("sidepanel.history.range")).strong());
        for range in HistoryRange::iter() {
            ui.radio_value(&mut settings.range, range, range.label());
        }
        ui.separator();
        ui.label(RichText::new(t!("sidepanel.history.granularity")).strong());
        for granularity in HistoryGranularity::iter() {
            ui.radio_value(&mut settings.granularity, granularity, granularity.label());
        }
    })
    .response
    .on_hover_text(format!(
        "{}: {}\n{}: {}",
        t!("sidepanel.history.range"),
        settings.range.label(),
        t!("sidepanel.history.granularity"),
        settings.granularity.label()
    ));
}

/// Seconds east of UTC, read once: day buckets only need to be roughly aligned to midnight.
fn local_utc_offset() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        #[cfg(target_arch = "wasm32")]
        #[allow(clippy::cast_possible_truncation)]
        let offset = (-js_sys::Date::new_0().get_timezone_offset() * 60.0) as i64;
        #[cfg(not(target_arch = "wasm32"))]
        let offset = time::UtcOffset::current_local_offset()
            .map_or(0, |offset| i64::from(offset.whole_seconds()));
        offset
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-09-07 (a Monday) 00:00:00 UTC
    const MONDAY: u64 = 1_788_739_200;

    fn settings(range: HistoryRange, granularity: HistoryGranularity) -> HistorySettings {
        HistorySettings { range, granularity }
    }

    #[test]
    fn keeps_the_newest_snapshot_per_bucket() {
        let times = [
            MONDAY + 5 * HOUR,
            MONDAY + 5 * HOUR + 1800,
            MONDAY + 7 * HOUR,
            MONDAY + 13 * HOUR,
        ];
        let all = |g| visible_points(&times, settings(HistoryRange::All, g), 0);
        assert_eq!(all(HistoryGranularity::SixHours), vec![1, 2, 3]);
        assert_eq!(all(HistoryGranularity::TwelveHours), vec![2, 3]);
        assert_eq!(all(HistoryGranularity::OneDay), vec![3]);
    }

    #[test]
    fn range_counts_back_from_the_newest_snapshot() {
        let times = [MONDAY, MONDAY + 2 * DAY, MONDAY + 4 * DAY, MONDAY + 5 * DAY];
        let range = |r| visible_points(&times, settings(r, HistoryGranularity::SixHours), 0);
        assert_eq!(range(HistoryRange::OneDay), vec![2, 3]);
        assert_eq!(range(HistoryRange::ThreeDays), vec![1, 2, 3]);
        assert_eq!(range(HistoryRange::All), vec![0, 1, 2, 3]);
    }

    #[test]
    fn day_and_week_buckets_follow_local_time() {
        // Sunday 23:00 and Monday 01:00 in UTC-07:00
        let pdt = -7 * 3600;
        #[allow(clippy::cast_sign_loss)]
        let times = [MONDAY + (7 - 1) * HOUR, MONDAY + (7 + 1) * HOUR];
        let week = settings(HistoryRange::All, HistoryGranularity::OneWeek);
        let day = settings(HistoryRange::All, HistoryGranularity::OneDay);
        assert_eq!(visible_points(&times, week, pdt), vec![0, 1]);
        assert_eq!(visible_points(&times, day, pdt), vec![0, 1]);
        // the same instants are both Monday in UTC
        assert_eq!(visible_points(&times, week, 0), vec![1]);
        assert_eq!(visible_points(&times, day, 0), vec![1]);
    }

    #[test]
    fn no_snapshots_no_points() {
        assert!(visible_points(&[], HistorySettings::default(), 0).is_empty());
    }
}
