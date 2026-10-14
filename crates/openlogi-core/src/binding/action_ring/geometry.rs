//! Where Actions Ring items sit and which one the pointer selects — Logi
//! Options+'s radial-menu geometry, recovered from `logioptionsplus_agent`
//! (`-[RadialMenu createElementsWithMenuItems:…]`,
//! `-[RadialMenu subElementsForMenuElement:…]`,
//! `+[MenuElementPosition positionWithMenuOrigin:level:angle:]` and
//! `-[MenuElementController closestElementWithCursorPosition:…]`).
//!
//! Distances are points at 100 % scale; angles are degrees clockwise from
//! twelve o'clock; offsets are relative to the ring's centre with positive Y
//! pointing **down**, the convention both GPUI frontends draw in.

use super::{ActionRingSlot, ActionRingTarget};

/// Radius of every item's circle.
pub const ITEM_RADIUS: f32 = 24.0;
/// Radius of the centre close button.
pub const CENTER_RADIUS: f32 = 16.0;
/// Distance from the centre to each successive level: root items sit on level
/// one, an expanded folder's items on level two.
pub const LEVEL_SPACING: f32 = 82.0;
/// Within this distance of the centre the pointer selects nothing — the
/// dismiss region around the close button.
pub const DISMISS_RADIUS: f32 = ITEM_RADIUS;
/// Side of the square canvas that holds a ring with an expanded folder.
pub const CANVAS: f32 = 2.0 * (2.0 * LEVEL_SPACING + ITEM_RADIUS) + 2.0 * ITEM_RADIUS;

/// Angle of root item `index` of `count`: evenly spaced, the first at the top.
#[must_use]
pub fn root_angle(index: usize, count: usize) -> f32 {
    step(count) * small(index)
}

/// Angle of child `index` of a folder holding `count` items, whose folder sits
/// at `parent_angle` in a root of `root_count`: half a root step apart and
/// centred on the folder.
#[must_use]
pub fn child_angle(parent_angle: f32, root_count: usize, index: usize, count: usize) -> f32 {
    let step = step(root_count) / 2.0;
    parent_angle - small(count.saturating_sub(1)) * step / 2.0 + small(index) * step
}

fn step(count: usize) -> f32 {
    360.0 / small(count.max(1))
}

/// A position count or index as `f32`. Rings hold at most eight positions per
/// level, so going through `u8` is exact; anything larger saturates.
fn small(n: usize) -> f32 {
    f32::from(u8::try_from(n).unwrap_or(u8::MAX))
}

/// Offset from the centre of an item on `level` at `angle`.
#[must_use]
pub fn offset(level: u8, angle: f32) -> (f32, f32) {
    let radius = f32::from(level) * LEVEL_SPACING;
    let radians = angle.to_radians();
    (radius * radians.sin(), -radius * radians.cos())
}

/// One item the pointer can currently select, at its offset from the centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisibleItem {
    /// What selecting it addresses.
    pub target: ActionRingTarget,
    /// Its centre, relative to the ring's centre.
    pub at: (f32, f32),
}

/// Lay out the populated root `slots` (in [`ActionRingSlot::ALL`] order) and,
/// when `expanded` names one of them, that folder's populated `children`.
#[must_use]
pub fn visible_items(
    slots: &[ActionRingSlot],
    expanded: Option<(ActionRingSlot, &[ActionRingSlot])>,
) -> Vec<VisibleItem> {
    let mut items: Vec<VisibleItem> = slots
        .iter()
        .enumerate()
        .map(|(index, &slot)| VisibleItem {
            target: ActionRingTarget::root(slot),
            at: offset(1, root_angle(index, slots.len())),
        })
        .collect();
    if let Some((folder, children)) = expanded
        && let Some(parent) = slots.iter().position(|&slot| slot == folder)
    {
        let parent_angle = root_angle(parent, slots.len());
        items.extend(
            children
                .iter()
                .enumerate()
                .map(|(index, &child)| VisibleItem {
                    target: ActionRingTarget::child(folder, child),
                    at: offset(
                        2,
                        child_angle(parent_angle, slots.len(), index, children.len()),
                    ),
                }),
        );
    }
    items
}

/// The item the pointer at `cursor` (relative to the centre, in unscaled
/// points) selects: none inside the dismiss region, otherwise the closest
/// visible item, however far out the pointer has travelled.
#[must_use]
pub fn hit(cursor: (f32, f32), items: &[VisibleItem]) -> Option<ActionRingTarget> {
    if cursor.0.hypot(cursor.1) <= DISMISS_RADIUS {
        return None;
    }
    items
        .iter()
        .min_by(|a, b| distance(cursor, a.at).total_cmp(&distance(cursor, b.at)))
        .map(|item| item.target)
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ActionRingSlot as Slot;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        distance(a, b) < 1e-3
    }

    #[test]
    fn root_items_spread_evenly_clockwise_from_the_top() {
        assert!(close(offset(1, root_angle(0, 4)), (0.0, -LEVEL_SPACING)));
        assert!(close(offset(1, root_angle(1, 4)), (LEVEL_SPACING, 0.0)));
        assert!(close(offset(1, root_angle(2, 4)), (0.0, LEVEL_SPACING)));
        assert!(
            (root_angle(1, 3) - 120.0).abs() < 1e-4,
            "three items, 120° apart"
        );
    }

    #[test]
    fn folder_children_fan_out_centred_on_their_folder() {
        // A right-hand folder in a root of four: half-step (45°) spacing,
        // centred on 90°.
        let angles: Vec<f32> = (0..3).map(|i| child_angle(90.0, 4, i, 3)).collect();
        assert_eq!(angles, [45.0, 90.0, 135.0]);
        assert!((child_angle(90.0, 4, 0, 1) - 90.0).abs() < 1e-4);
    }

    #[test]
    fn the_dismiss_region_selects_nothing() {
        let items = visible_items(&[Slot::Top, Slot::Bottom], None);
        assert_eq!(hit((0.0, -DISMISS_RADIUS + 1.0), &items), None);
        assert_eq!(
            hit((0.0, -DISMISS_RADIUS - 1.0), &items),
            Some(ActionRingTarget::root(Slot::Top))
        );
    }

    #[test]
    fn the_closest_item_wins_however_far_out_the_pointer_goes() {
        let items = visible_items(&[Slot::Top, Slot::Right, Slot::Bottom, Slot::Left], None);
        assert_eq!(
            hit((600.0, 40.0), &items),
            Some(ActionRingTarget::root(Slot::Right))
        );
    }

    #[test]
    fn an_expanded_folder_adds_its_children_on_the_next_level() {
        let children = [Slot::Top, Slot::Right];
        let items = visible_items(&[Slot::Top, Slot::Right], Some((Slot::Right, &children)));
        assert_eq!(items.len(), 4);
        let outer = offset(2, child_angle(root_angle(1, 2), 2, 1, 2));
        assert_eq!(
            hit(outer, &items),
            Some(ActionRingTarget::child(Slot::Right, Slot::Right))
        );
        // An unknown folder adds nothing.
        assert_eq!(
            visible_items(&[Slot::Top], Some((Slot::Left, &children))).len(),
            1
        );
    }

    #[test]
    fn a_ring_with_an_expanded_folder_fits_the_canvas() {
        let reach = 2.0 * LEVEL_SPACING + ITEM_RADIUS;
        assert!(CANVAS / 2.0 >= reach);
    }
}
