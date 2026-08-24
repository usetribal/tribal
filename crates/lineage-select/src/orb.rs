//! The orb the share confirmation draws, and its fill-and-burst animation.
//!
//! Everything here samples a continuous field and maps it through a character
//! ramp, rather than placing glyphs at computed positions. That is what lets a
//! character grid show a curve: hard-edged geometry drawn cell by cell reads as
//! the grid's own squareness, while a sampled falloff reads as a soft shape.
//!
//! Every frame is a pure function of elapsed time, so the animation plays at the
//! same speed whatever the terminal manages and a dropped frame shifts nothing.

use std::time::Duration;

/// Density ramp, dimmest first. A cell's sampled value picks one of these, so
/// brightness and glyph move together.
const RAMP: &[char] = &[
    ' ', '.', ',', ':', ';', '!', '+', '*', 'c', 'o', '0', 'O', '@', '#',
];

/// Below this a cell is empty rather than faintly marked, so the field has a
/// clean edge instead of a haze of full stops.
const INK_FLOOR: f32 = 0.02;

/// A terminal cell is about twice as tall as it is wide. Vertical distances are
/// scaled by this so a circle draws as a circle rather than a wide ellipse.
const CELL_ASPECT: f32 = 2.0;

/// How bright a cell is, on the same 0..=1 scale the ramp indexes.
pub type Density = f32;

/// One frame of the animation, as a grid of sampled densities.
pub struct Field {
    pub width: usize,
    pub height: usize,
    cells: Vec<Density>,
    /// Glyphs placed outright rather than sampled — the orb itself, which is one
    /// character and not a shape the ramp could express at this size.
    marks: Vec<Option<char>>,
}

impl Field {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![0.0; width * height],
            marks: vec![None; width * height],
        }
    }

    fn set_mark(&mut self, column: usize, row: usize, glyph: char) {
        if column < self.width && row < self.height {
            self.marks[row * self.width + column] = Some(glyph);
        }
    }

    fn set(&mut self, column: usize, row: usize, value: Density) {
        self.cells[row * self.width + column] = value;
    }

    pub fn at(&self, column: usize, row: usize) -> Density {
        self.cells[row * self.width + column]
    }

    /// The glyph for a cell, or none where the field is empty.
    pub fn glyph(&self, column: usize, row: usize) -> Option<char> {
        if let Some(mark) = self.marks[row * self.width + column] {
            return Some(mark);
        }
        let value = self.at(column, row);
        if value <= INK_FLOOR {
            return None;
        }
        let step = ((value.clamp(0.0, 1.0) * (RAMP.len() - 1) as f32).round() as usize)
            .min(RAMP.len() - 1);
        Some(RAMP[step])
    }

    /// Fill from a function of each cell's position, in units where 1.0 is the
    /// distance to the nearest edge of the panel.
    ///
    /// Scaled by the shorter axis rather than the width, so a wave that has
    /// travelled 1.0 has reached the top and bottom of a wide panel. The corners
    /// sit further out again and are simply clipped, which is what
    /// [`RING_REACH`] allows for: a field normalised on the width alone dies on
    /// a circle inscribed in it, leaving empty bands above and below.
    fn sample(&mut self, mut f: impl FnMut(f32, f32) -> Density) {
        let centre_x = (self.width - 1) as f32 / 2.0;
        let centre_y = (self.height - 1) as f32 / 2.0;
        // Both axes in the same units, then normalised by whichever is nearer.
        let reach_y = centre_y * CELL_ASPECT;
        let nearest = centre_x.min(reach_y).max(f32::EPSILON);
        for row in 0..self.height {
            for column in 0..self.width {
                let dx = (column as f32 - centre_x) / nearest;
                let dy = (row as f32 - centre_y) * CELL_ASPECT / nearest;
                self.set(column, row, f(dx, dy).clamp(0.0, 1.0));
            }
        }
    }
}

// --- Beats -----------------------------------------------------------------

/// The orb lighting up. Short: it is a switch being thrown, and the rings
/// leaving at the same moment are what carries the motion.
const FILL: Duration = Duration::from_millis(220);
/// The whole animation. Long enough that the rings read as travelling rather
/// than flashing.
const BURST: Duration = Duration::from_millis(4000);
/// How long the first ring takes to reach the panel's nearest edge.
/// Rings are emitted for the rest of the run, so later ones are still in flight
/// when it ends.
const CROSSING: Duration = Duration::from_millis(1500);

/// The orb is a single glyph at the centre, so the waves start just outside it.
const ORB_SCALE: f32 = 0.08;

/// How long the whole animation runs.
pub fn duration() -> Duration {
    BURST
}

// --- Frames ----------------------------------------------------------------

/// The orb's two states, the same mark the empty state uses.
pub const ORB_EMPTY: char = '○';
pub const ORB_FULL: char = '●';

/// The orb at rest: the unfilled mark, still.
///
/// A static shape is what a waiting control should be — motion before anything
/// has happened reads as something already in progress.
pub fn idle(width: usize, height: usize) -> Field {
    let mut field = Field::new(width, height);
    let (column, row) = centre(width, height);
    field.set_mark(column, row, ORB_EMPTY);
    field
}

/// The cell the sampled field treats as its origin.
///
/// `(n - 1) / 2`, matching `sample`: placing the mark at `n / 2` puts it half a
/// cell off on even sizes, which reads as the orb sitting beside its own rings
/// rather than at their centre.
fn centre(width: usize, height: usize) -> (usize, usize) {
    (
        (width.saturating_sub(1)) / 2,
        (height.saturating_sub(1)) / 2,
    )
}

/// The orb `elapsed` into the animation: filled, with rings leaving it.
pub fn frame(width: usize, height: usize, elapsed: Duration) -> Field {
    let mut field = Field::new(width, height);
    add_rings(&mut field, elapsed);
    // Drawn last so the orb is never overwritten by a wave passing through it.
    let (column, row) = centre(width, height);
    let filled = elapsed >= FILL;
    field.set_mark(column, row, if filled { ORB_FULL } else { ORB_EMPTY });
    field
}

/// Add concentric wavefronts leaving the orb, sampled the way it is so the rings
/// stay circular however far they travel.
fn add_rings(field: &mut Field, elapsed: Duration) {
    field.sample(|dx, dy| ring_at((dx * dx + dy * dy).sqrt(), elapsed));
}

/// How bright a travelling ring is at distance `d`, `elapsed` into the burst.
///
/// Rings leave the orb continuously and each travels outward at the same speed,
/// so a crest's age is how long ago it left: `elapsed` minus the time it took to
/// reach `d`. Expressing it that way is what keeps every ring's speed identical
/// however many are in flight.
fn ring_at(d: f32, elapsed: Duration) -> Density {
    if !(ORB_SCALE..=RING_REACH).contains(&d) {
        return 0.0;
    }
    let seconds = elapsed.as_secs_f32();
    let crossing = CROSSING.as_secs_f32();
    // How long a wavefront needs to get here, in the same units.
    let travel = (d - ORB_SCALE) / (1.0 - ORB_SCALE) * crossing;
    if travel > seconds {
        // Nothing has reached this far yet.
        return 0.0;
    }

    let phase = (seconds - travel) / crossing * RINGS_PER_CROSSING * std::f32::consts::TAU;
    let crest = phase.cos().max(0.0).powf(1.0 / RING_THICKNESS);
    let spread = (-d * RING_DECAY).exp();
    // Fades toward the reach so a ring dies out rather than being cut off at
    // the panel's edge. Emission continues for the whole run — the modal closes
    // on waves still travelling rather than on an empty box.
    let edge = (1.0 - (d / RING_REACH).powi(2)).max(0.0);
    crest * spread * edge
}

/// Rings per rim radius, how fast they travel, how sharp each crest is, and how
/// quickly they lose energy with distance.
/// How far a ring travels before it is gone, in units of the distance to the
/// nearest edge.
///
/// Well past 1.0 so the circle is cropped by the modal's border rather than
/// fading inside it. A wide panel's corners are further out still; the waves are
/// simply clipped there, which is what makes them look like they fill the box.
const RING_REACH: f32 = 2.6;
/// How many crests leave the orb in the time one takes to cross the panel.
const RINGS_PER_CROSSING: f32 = 2.0;
/// How broad each crest is. Higher is thicker: the exponent below flattens the
/// cosine's peak into a band rather than a line.
const RING_THICKNESS: f32 = 1.6;
const RING_DECAY: f32 = 0.7;

#[cfg(test)]
mod tests {
    use super::*;

    // Odd, so there is a true centre cell for the orb to sit in and mirror
    // about. An even width has no such column.
    const W: usize = 41;
    const H: usize = 17;

    fn drawn(field: &Field) -> String {
        (0..field.height)
            .map(|row| {
                (0..field.width)
                    .map(|column| field.glyph(column, row).unwrap_or(' '))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn inked(field: &Field) -> usize {
        (0..field.height)
            .flat_map(|row| (0..field.width).map(move |column| (column, row)))
            .filter(|&(column, row)| field.glyph(column, row).is_some())
            .count()
    }

    #[test]
    fn the_orb_rests_as_an_unfilled_mark_at_the_centre() {
        let field = idle(W, H);
        assert_eq!(field.glyph(W / 2, H / 2), Some(ORB_EMPTY));
        assert_eq!(inked(&field), 1, "nothing else is drawn at rest");
    }

    #[test]
    fn the_orb_fills_once_the_share_lands() {
        assert_eq!(
            frame(W, H, FILL / 2).glyph(W / 2, H / 2),
            Some(ORB_EMPTY),
            "still filling"
        );
        assert_eq!(frame(W, H, FILL).glyph(W / 2, H / 2), Some(ORB_FULL));
    }

    #[test]
    fn the_first_ring_reaches_the_edge_after_one_crossing() {
        let edge = W - 2;
        assert_eq!(
            frame(W, H, CROSSING / 3).glyph(edge, H / 2),
            None,
            "nothing has travelled that far yet"
        );
        // Sampled just inside the reach, where the first wavefront lands.
        let arrived = (0..W).any(|column| {
            frame(W, H, CROSSING).glyph(column, H / 2).is_some() && column > W * 3 / 4
        });
        assert!(arrived, "the first ring should have crossed by now");
    }

    #[test]
    fn rings_keep_leaving_for_the_whole_run() {
        // Several crests are in flight at once, so the burst reads as continuous
        // emission rather than one expanding shell.
        let mid = frame(W, H, BURST / 2);
        let lit: Vec<usize> = (W / 2..W)
            .filter(|&column| mid.glyph(column, H / 2).is_some())
            .collect();
        assert!(lit.len() > 2, "expected several rings, found {lit:?}");
    }

    #[test]
    fn the_orb_is_symmetric_about_both_axes() {
        let field = frame(W, H, CROSSING / 2);
        for row in 0..H {
            for column in 0..W {
                assert_eq!(
                    field.glyph(column, row),
                    field.glyph(W - 1 - column, row),
                    "asymmetric horizontally at {row},{column}"
                );
                assert_eq!(
                    field.glyph(column, row),
                    field.glyph(column, H - 1 - row),
                    "asymmetric vertically at {row},{column}"
                );
            }
        }
    }

    #[test]
    fn rings_are_still_travelling_when_the_animation_ends() {
        // The modal closes on motion rather than on an empty panel, so the last
        // frame still has waves in it.
        let last = frame(W, H, duration());
        assert!(inked(&last) > 1, "expected waves at the final frame");
    }

    #[test]
    fn every_frame_fills_its_grid() {
        for ms in (0..1000).step_by(25) {
            let field = frame(W, H, Duration::from_millis(ms));
            assert_eq!(field.width, W);
            assert_eq!(field.height, H);
        }
    }

    /// Not an assertion — run with `--nocapture` to look at the animation.
    #[test]
    fn show_frames() {
        println!("--- idle ---\n{}", drawn(&idle(W, H)));
        for ms in [0u64, 220, 750, 1500, 2400, 3600] {
            println!(
                "--- {ms}ms ---\n{}",
                drawn(&frame(W, H, Duration::from_millis(ms)))
            );
        }
    }
}
