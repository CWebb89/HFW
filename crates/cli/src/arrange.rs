//! Packing several objects onto one bed.
//!
//! Laying objects out in a row is what a person does by hand, and it wastes the
//! bed as soon as the parts are not all the same height: a tall part and two
//! short ones fit in the space one row of talls leaves empty. This is a shelf-free
//! packer — MaxRects with a best-short-side-fit score — which keeps the free space
//! as a list of rectangles, places each object in the gap it wastes least, and
//! splits the rectangles it lands in.
//!
//! Everything is packed at a footprint of the object's own bounding box plus half
//! the spacing on each side, so a spacing of 5 mm leaves 5 mm of air between any
//! two objects and 2.5 mm at the bed edge. Each object may be turned a quarter
//! turn, because a part that is 200 mm long and 30 mm wide across is a different
//! problem on a 250 mm bed than on a 350 mm one.

/// Footprint of one object, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    /// Extent along X before any rotation.
    pub width: f64,
    /// Extent along Y before any rotation.
    pub height: f64,
}

/// Where one object goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Lowest X of the object's box on the bed, in millimetres.
    pub x: f64,
    /// Lowest Y of the object's box on the bed, in millimetres.
    pub y: f64,
    /// True when the object was turned a quarter turn, so its footprint is
    /// `height` by `width`.
    pub rotated: bool,
}

/// An object that does not fit, and the bed it does not fit on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TooLarge {
    /// Index of the object in the input, upright or turned.
    pub item: usize,
    /// Footprint the packer could offer it.
    pub footprint: (f64, f64),
    /// Bed the packer worked in.
    pub bed: (f64, f64),
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn right(&self) -> f64 {
        self.x + self.width
    }

    fn top(&self) -> f64 {
        self.y + self.height
    }

    fn contains(&self, other: &Rect) -> bool {
        other.x >= self.x - 1e-9
            && other.y >= self.y - 1e-9
            && other.right() <= self.right() + 1e-9
            && other.top() <= self.top() + 1e-9
    }

    fn intersects(&self, other: &Rect) -> bool {
        other.x < self.right() - 1e-9
            && other.right() > self.x + 1e-9
            && other.y < self.top() - 1e-9
            && other.top() > self.y + 1e-9
    }
}

/// Packs every item onto a bed, or reports the first one that cannot fit.
///
/// The result is centred on the bed: the packer works from the origin, and a plate
/// that hugs one corner looks like a mistake even when every part fits.
pub fn pack(items: &[Item], bed: (f64, f64), spacing: f64) -> Result<Vec<Placement>, TooLarge> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|left, right| {
        let area = |index: usize| items[index].width * items[index].height;
        let longest = |index: usize| items[index].width.max(items[index].height);
        longest(*right)
            .partial_cmp(&longest(*left))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                area(*right)
                    .partial_cmp(&area(*left))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(left.cmp(right))
    });

    let mut free = vec![Rect {
        x: 0.0,
        y: 0.0,
        width: bed.0,
        height: bed.1,
    }];
    let mut placed: Vec<Option<Placement>> = vec![None; items.len()];
    for index in order {
        let item = items[index];
        let padded = (item.width + spacing, item.height + spacing);
        let footprint = vec![(padded.0, padded.1, false), (padded.1, padded.0, true)];
        let mut best: Option<(f64, f64, Placement)> = None;
        for (width, height, rotated) in footprint {
            for rect in &free {
                if width > rect.width + 1e-9 || height > rect.height + 1e-9 {
                    continue;
                }
                let leftover_width = rect.width - width;
                let leftover_height = rect.height - height;
                let short = leftover_width.min(leftover_height);
                let long = leftover_width.max(leftover_height);
                let candidate = Placement {
                    x: rect.x + 0.5 * spacing,
                    y: rect.y + 0.5 * spacing,
                    rotated,
                };
                let better = match best {
                    None => true,
                    Some((best_short, best_long, _)) => {
                        short < best_short - 1e-9
                            || ((short - best_short).abs() <= 1e-9 && long < best_long - 1e-9)
                    }
                };
                if better {
                    best = Some((short, long, candidate));
                }
            }
        }
        let Some((_, _, placement)) = best else {
            return Err(TooLarge {
                item: index,
                footprint: (item.width + spacing, item.height + spacing),
                bed,
            });
        };
        let used = Rect {
            x: placement.x - 0.5 * spacing,
            y: placement.y - 0.5 * spacing,
            width: if placement.rotated {
                padded.1
            } else {
                padded.0
            },
            height: if placement.rotated {
                padded.0
            } else {
                padded.1
            },
        };
        free = split(free, &used);
        placed[index] = Some(placement);
    }

    let mut placements: Vec<Placement> =
        placed.into_iter().map(|p| p.expect("all placed")).collect();
    centre(&mut placements, items, bed);
    Ok(placements)
}

/// Shifts the arrangement so its own centre sits at the centre of the bed.
fn centre(placements: &mut [Placement], items: &[Item], bed: (f64, f64)) {
    let mut low = (f64::INFINITY, f64::INFINITY);
    let mut high = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (index, placement) in placements.iter().enumerate() {
        let (width, height) = footprint(items[index], *placement);
        low = (low.0.min(placement.x), low.1.min(placement.y));
        high = (
            high.0.max(placement.x + width),
            high.1.max(placement.y + height),
        );
    }
    if !low.0.is_finite() {
        return;
    }
    let shift_x = 0.5 * (bed.0 - (high.0 - low.0)) - low.0;
    let shift_y = 0.5 * (bed.1 - (high.1 - low.1)) - low.1;
    for placement in placements.iter_mut() {
        placement.x = (placement.x + shift_x).max(0.0);
        placement.y = (placement.y + shift_y).max(0.0);
    }
}

/// The footprint an object occupies once placed.
pub fn footprint(item: Item, placement: Placement) -> (f64, f64) {
    match placement.rotated {
        true => (item.height, item.width),
        false => (item.width, item.height),
    }
}

/// Splits free rectangles around a rect that has just been used, and drops the
/// pieces wholly covered by another.
fn split(free: Vec<Rect>, used: &Rect) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::with_capacity(free.len() + 4);
    for rect in free {
        if !rect.intersects(used) {
            out.push(rect);
            continue;
        }
        if used.x > rect.x + 1e-9 {
            out.push(Rect {
                x: rect.x,
                y: rect.y,
                width: used.x - rect.x,
                height: rect.height,
            });
        }
        if used.right() < rect.right() - 1e-9 {
            out.push(Rect {
                x: used.right(),
                y: rect.y,
                width: rect.right() - used.right(),
                height: rect.height,
            });
        }
        if used.y > rect.y + 1e-9 {
            out.push(Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: used.y - rect.y,
            });
        }
        if used.top() < rect.top() - 1e-9 {
            out.push(Rect {
                x: rect.x,
                y: used.top(),
                width: rect.width,
                height: rect.top() - used.top(),
            });
        }
    }
    let mut pruned: Vec<Rect> = Vec::with_capacity(out.len());
    for (index, rect) in out.iter().enumerate() {
        let covered = out
            .iter()
            .enumerate()
            .any(|(other, candidate)| other != index && candidate.contains(rect));
        if !covered {
            pruned.push(*rect);
        }
    }
    pruned
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE: Item = Item {
        width: 20.0,
        height: 20.0,
    };

    fn inside(placements: &[Placement], items: &[Item], bed: (f64, f64)) -> bool {
        placements.iter().enumerate().all(|(index, placement)| {
            let (width, height) = footprint(items[index], *placement);
            placement.x >= -1e-9
                && placement.y >= -1e-9
                && placement.x + width <= bed.0 + 1e-9
                && placement.y + height <= bed.1 + 1e-9
        })
    }

    fn separated(placements: &[Placement], items: &[Item], spacing: f64) -> bool {
        for (index, left) in placements.iter().enumerate() {
            let (left_w, left_h) = footprint(items[index], *left);
            for (other, right) in placements.iter().enumerate().skip(index + 1) {
                let (right_w, right_h) = footprint(items[other], *right);
                let gap_x = (right.x - (left.x + left_w)).max(left.x - (right.x + right_w));
                let gap_y = (right.y - (left.y + left_h)).max(left.y - (right.y + right_h));
                if gap_x < spacing - 1e-6 && gap_y < spacing - 1e-6 {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn one_object_is_centred_on_the_bed() {
        let placements = pack(&[CUBE], (220.0, 220.0), 5.0).expect("fits");
        assert!((placements[0].x - 100.0).abs() < 1e-9, "{placements:?}");
        assert!((placements[0].y - 100.0).abs() < 1e-9, "{placements:?}");
    }

    #[test]
    fn four_objects_fit_without_touching_and_stay_inside_the_bed() {
        let items = [CUBE; 4];
        let placements = pack(&items, (220.0, 220.0), 5.0).expect("fits");
        assert!(
            inside(&placements, &items, (220.0, 220.0)),
            "{placements:?}"
        );
        assert!(separated(&placements, &items, 5.0), "{placements:?}");
    }

    #[test]
    fn an_object_longer_than_the_bed_is_turned_a_quarter_turn() {
        let item = Item {
            width: 50.0,
            height: 25.0,
        };
        let placements = pack(&[item], (30.0, 60.0), 0.0).expect("fits turned");
        assert!(placements[0].rotated, "{placements:?}");
        let (width, height) = footprint(item, placements[0]);
        assert!((width - 25.0).abs() < 1e-9 && (height - 50.0).abs() < 1e-9);
    }

    #[test]
    fn an_object_that_cannot_fit_at_all_is_reported_with_its_size() {
        let item = Item {
            width: 300.0,
            height: 10.0,
        };
        let error = pack(&[item], (220.0, 220.0), 5.0).expect_err("too wide");
        assert_eq!(error.item, 0);
        assert!((error.bed.0 - 220.0).abs() < 1e-9);
    }

    #[test]
    fn the_packer_uses_the_second_row_rather_than_one_long_row() {
        let items = [CUBE; 6];
        let placements = pack(&items, (100.0, 220.0), 5.0).expect("fits");
        assert!(
            inside(&placements, &items, (100.0, 220.0)),
            "{placements:?}"
        );
        assert!(separated(&placements, &items, 5.0), "{placements:?}");
        let rows = placements
            .iter()
            .map(|placement| (placement.y * 100.0).round() as i64)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            rows.len() >= 2,
            "six 20 mm cubes do not fit on one 100 mm row"
        );
    }

    #[test]
    fn packing_is_deterministic() {
        let items = [CUBE; 5];
        let first = pack(&items, (220.0, 220.0), 5.0).expect("fits");
        let second = pack(&items, (220.0, 220.0), 5.0).expect("fits");
        assert_eq!(first, second);
    }
}
