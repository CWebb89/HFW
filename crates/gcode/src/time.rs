//! Print-time estimate that accounts for acceleration.
//!
//! The kinematic estimate — distance over feedrate, summed — is exact only for a
//! machine that reaches its commanded speed instantly. A real one accelerates to
//! it, cruises, and decelerates, and the cost of that is what makes a short-move
//! job take three times what the arithmetic says: a 0.4 mm perimeter segment never
//! reaches the plotted speed at all.
//!
//! This is a streaming trapezoidal planner with one junction of lookahead. Each
//! move is timed as accelerate–cruise–decelerate between the speed it inherits
//! from the junction behind it and the speed it may carry into the junction ahead,
//! and the junction speed comes from the machine's own cornering model.
//!
//! Two cornering models exist in the reference slicers, and the profile decides
//! which applies. **Junction deviation** says the nozzle may deviate from the
//! corner by a fixed distance, which bounds the speed through a junction of turn
//! angle θ by
//!
//! ```text
//! v² = a · δ · sin(θ/2) / (1 − sin(θ/2))
//! ```
//!
//! Klipper has no such setting: it exposes `square_corner_velocity`, the speed a
//! 90° corner may be taken at, and derives the deviation from it
//! (`klippy/toolhead.py`: `δ = scv² · (√2 − 1) / a`). Substituting that into the
//! formula above returns exactly `scv` at 90°, so one code path covers both; a
//! Klipper profile that says nothing gets Klipper's own default of 5 mm/s.
//!
//! **Classic jerk** — the `M205 X/Y/E` model — applies when a Marlin profile sets
//! jerk and no junction deviation: there the instantaneous change of speed per axis
//! is what is bounded. A profile with neither is timed kinematically, because
//! inventing a cornering parameter for a machine that was never described would be
//! a number with nothing behind it.
//!
//! What this deliberately does not model: the time to heat and home (which the
//! file's own start G-code causes and no planner can know), the lookahead the
//! firmware really uses through a fan of short segments, and the extruder's own
//! limits. It is an estimate, and it is now an estimate with the right shape.

use hfw_config::{Flavor, PrinterConfig};

/// Cornering parameters of the machine the job is for.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tune {
    /// Junction deviation in millimetres, as `M205 J` sets it; 0 when unset.
    pub junction_deviation: f64,
    /// Speed a 90° corner may be taken at, in millimetres per second; 0 when unset.
    pub square_corner_velocity: f64,
    /// Classic jerk along X, Y and Z in millimetres per second; 0 when unset.
    pub jerk: [f64; 3],
}

/// Klipper's default `square_corner_velocity`, used when a Klipper profile does
/// not carry one, which no setting in the profile format can express today.
const KLIPPER_DEFAULT_SCV: f64 = 5.0;

impl Tune {
    /// Reads the cornering model out of a profile.
    ///
    /// Junction deviation is a Marlin setting and is ignored on Klipper, where the
    /// same quantity is derived from `square_corner_velocity` instead.
    pub fn from_profile(cfg: &PrinterConfig) -> Tune {
        match cfg.flavor {
            Flavor::Klipper => Tune {
                junction_deviation: 0.0,
                square_corner_velocity: if cfg.machine_max_junction_deviation > 0.0 {
                    cfg.machine_max_junction_deviation
                } else {
                    KLIPPER_DEFAULT_SCV
                },
                jerk: [0.0; 3],
            },
            Flavor::Marlin => Tune {
                junction_deviation: cfg.machine_max_junction_deviation,
                square_corner_velocity: 0.0,
                jerk: [
                    cfg.machine_max_jerk_x,
                    cfg.machine_max_jerk_y,
                    cfg.machine_max_jerk_z,
                ],
            },
        }
    }

    /// The deviation to plan with at a given acceleration.
    ///
    /// On Klipper the deviation is a function of the acceleration, because it is
    /// derived from a corner *velocity* rather than a distance.
    fn deviation(&self, accel: f64) -> f64 {
        if self.junction_deviation > 0.0 {
            return self.junction_deviation;
        }
        if self.square_corner_velocity > 0.0 && accel > 0.0 {
            let scv = self.square_corner_velocity;
            return scv * scv * (std::f64::consts::SQRT_2 - 1.0) / accel;
        }
        0.0
    }

    /// True when the profile describes a machine well enough to plan against.
    pub fn is_modelled(&self) -> bool {
        self.junction_deviation > 0.0
            || self.square_corner_velocity > 0.0
            || self.jerk.iter().any(|value| *value > 0.0)
    }
}

/// One straight or interpolated move, in millimetres and millimetres per second.
#[derive(Clone, Copy, Debug)]
pub struct Move {
    /// Length travelled.
    pub len: f64,
    /// Commanded speed.
    pub cruise: f64,
    /// Acceleration and deceleration, in millimetres per second squared; 0 means
    /// the machine's acceleration is unknown and the move is timed kinematically.
    pub accel: f64,
    /// Unit direction of travel.
    pub dir: [f64; 3],
}

#[derive(Clone, Debug)]
struct Pending {
    len: f64,
    cruise: f64,
    accel: f64,
    dir: [f64; 3],
    entry: f64,
}

/// A streaming time planner.
#[derive(Clone, Debug)]
pub struct Planner {
    tune: Tune,
    pending: Option<Pending>,
    total: f64,
    modelled: bool,
}

impl Planner {
    /// Starts a planner for this machine.
    pub fn new(tune: Tune) -> Planner {
        Planner {
            tune,
            pending: None,
            total: 0.0,
            modelled: false,
        }
    }

    /// True when at least one move was timed with an acceleration, so the estimate
    /// is not a pure distance-over-feedrate sum.
    pub fn is_modelled(&self) -> bool {
        self.modelled
    }

    /// Queues one move.
    ///
    /// The move before it is timed out only now, because its exit speed is decided
    /// by the junction it shares with this one.
    pub fn push(&mut self, m: Move) {
        if m.len <= 0.0 || !m.cruise.is_finite() || m.cruise <= 0.0 {
            return;
        }
        if m.accel > 0.0 {
            self.modelled = true;
        }
        let Some(pending) = self.pending.take() else {
            self.pending = Some(Pending {
                len: m.len,
                cruise: m.cruise,
                accel: m.accel,
                dir: m.dir,
                entry: 0.0,
            });
            return;
        };
        let junction = self.junction(&pending, &m);
        self.total += trapezoid(
            pending.len,
            pending.cruise,
            pending.accel,
            pending.entry,
            junction,
        );
        self.pending = Some(Pending {
            len: m.len,
            cruise: m.cruise,
            accel: m.accel,
            dir: m.dir,
            entry: junction,
        });
    }

    /// Records that the machine comes to rest here: a retract, a Z move, a tool
    /// change, the end of the file.
    pub fn stop(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.total += trapezoid(
                pending.len,
                pending.cruise,
                pending.accel,
                pending.entry,
                0.0,
            );
        }
    }

    /// Adds time the planner cannot model, such as a dwell.
    pub fn add(&mut self, seconds: f64) {
        if seconds.is_finite() && seconds > 0.0 {
            self.total += seconds;
        }
    }

    /// Seconds planned so far, including the move still in flight.
    ///
    /// The move in flight is counted at its cruise speed, which is what a progress
    /// reading needs; it is replaced by the planned time when its junction is
    /// known.
    pub fn seconds(&self) -> f64 {
        match &self.pending {
            Some(pending) => self.total + pending.len / pending.cruise,
            None => self.total,
        }
    }

    /// Seconds in total. Consumes the move in flight, timed to a full stop.
    pub fn finish(&mut self) -> f64 {
        self.stop();
        self.total
    }

    /// The highest speed two consecutive moves may share.
    fn junction(&self, prev: &Pending, next: &Move) -> f64 {
        let cap = prev.cruise.min(next.cruise);
        let accel = if prev.accel > 0.0 && next.accel > 0.0 {
            prev.accel.min(next.accel)
        } else {
            0.0
        };
        if accel <= 0.0 {
            return 0.0;
        }
        let dot = prev.dir[0] * next.dir[0] + prev.dir[1] * next.dir[1] + prev.dir[2] * next.dir[2];
        let turn = (-dot).clamp(-1.0, 1.0);
        if turn < -0.999_999 {
            return cap;
        }
        let deviation = self.tune.deviation(accel);
        if deviation <= 0.0 {
            return self.jerk_junction(prev, next, accel, cap);
        }
        let sin_half = (0.5 * (1.0 - turn)).sqrt();
        if sin_half >= 0.999_999 {
            return 0.0;
        }
        let mut squared = accel * deviation * sin_half / (1.0 - sin_half);
        if prev.len < 1.0 && turn > std::f64::consts::FRAC_1_SQRT_2 {
            let theta = std::f64::consts::PI - turn.acos();
            squared = squared.min(prev.len * accel / theta.max(1e-3));
        }
        squared.sqrt().min(cap)
    }

    /// Junction speed under the classic jerk model.
    ///
    /// Each axis may change its speed by at most its jerk within one junction, and
    /// a reversal has to spend both sides of that budget at once. The most
    /// restricting axis sets the factor every axis is scaled by.
    fn jerk_junction(&self, prev: &Pending, next: &Move, accel: f64, cap: f64) -> f64 {
        let _ = accel;
        let mut factor = 1.0f64;
        for axis in 0..3 {
            let jerk = self.tune.jerk[axis];
            if jerk <= 0.0 {
                continue;
            }
            let exit = prev.cruise * prev.dir[axis];
            let entry = next.cruise * next.dir[axis];
            let delta = if exit * entry < 0.0 {
                exit.abs() + entry.abs()
            } else {
                (exit - entry).abs()
            };
            if delta > jerk {
                factor = factor.min(jerk / delta);
            }
        }
        (cap * factor).max(0.0)
    }
}

/// Time for one move between two speeds, accelerating and decelerating at `a`.
///
/// Above the speed a move can reach in its own length the profile becomes a
/// triangle, and the peak is solved for directly.
fn trapezoid(len: f64, cruise: f64, accel: f64, entry: f64, exit: f64) -> f64 {
    if len <= 0.0 || cruise <= 0.0 {
        return 0.0;
    }
    if accel <= 0.0 {
        return len / cruise;
    }
    let entry = entry.clamp(0.0, cruise);
    let exit = exit.clamp(0.0, cruise);
    let stop_distance = |from: f64| (cruise * cruise - from * from) / (2.0 * accel);
    let accel_distance = stop_distance(entry);
    let decel_distance = stop_distance(exit);
    if accel_distance + decel_distance <= len {
        let cruise_distance = len - accel_distance - decel_distance;
        return (cruise - entry) / accel + cruise_distance / cruise + (cruise - exit) / accel;
    }
    let peak = ((2.0 * accel * len + entry * entry + exit * exit) / 2.0)
        .sqrt()
        .min(cruise);
    if peak <= entry && peak <= exit {
        return len / peak.max(1e-9);
    }
    (peak - entry) / accel + (peak - exit) / accel
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tune_marlin(deviation: f64) -> Tune {
        Tune {
            junction_deviation: deviation,
            square_corner_velocity: 0.0,
            jerk: [0.0; 3],
        }
    }

    fn tune_klipper() -> Tune {
        Tune {
            junction_deviation: 0.0,
            square_corner_velocity: KLIPPER_DEFAULT_SCV,
            jerk: [0.0; 3],
        }
    }

    fn straight(len: f64, cruise: f64, accel: f64) -> Move {
        Move {
            len,
            cruise,
            accel,
            dir: [1.0, 0.0, 0.0],
        }
    }

    #[test]
    fn a_move_too_short_to_reach_cruise_costs_a_triangle_not_a_rectangle() {
        let mut planner = Planner::new(tune_marlin(0.013));
        planner.push(straight(20.0, 100.0, 500.0));
        let seconds = planner.finish();
        assert!(
            (seconds - 0.4).abs() < 1e-6,
            "20 mm at 100 mm/s with 500 mm/s² takes 0.4 s, got {seconds}"
        );
        assert!(
            seconds > 20.0 / 100.0,
            "and never less than the kinematic floor"
        );
    }

    #[test]
    fn a_long_move_cruises_and_costs_the_kinematic_time_plus_the_ramps() {
        let mut planner = Planner::new(tune_marlin(0.013));
        planner.push(straight(100.0, 100.0, 1000.0));
        let seconds = planner.finish();
        let ramp = 2.0 * 100.0 / 1000.0;
        let ramp_distance = 2.0 * (100.0 * 100.0) / (2.0 * 1000.0);
        let cruise = (100.0 - ramp_distance) / 100.0;
        assert!(
            (seconds - (ramp + cruise)).abs() < 1e-6,
            "100 mm at 100 mm/s with 1000 mm/s² takes {}, got {seconds}",
            ramp + cruise
        );
    }

    #[test]
    fn collinear_moves_carry_their_speed_through_the_joint() {
        let mut joined = Planner::new(tune_marlin(0.013));
        joined.push(straight(10.0, 100.0, 500.0));
        joined.push(straight(10.0, 100.0, 500.0));
        let joined = joined.finish();

        let mut apart = Planner::new(tune_marlin(0.013));
        apart.push(straight(10.0, 100.0, 500.0));
        apart.stop();
        apart.push(straight(10.0, 100.0, 500.0));
        let apart = apart.finish();

        assert!(
            joined < apart - 0.01,
            "a straight join at {joined} must beat a stop at {apart}"
        );
    }

    #[test]
    fn a_reversal_stops_the_machine() {
        let mut planner = Planner::new(tune_marlin(0.013));
        planner.push(straight(10.0, 100.0, 500.0));
        planner.push(Move {
            len: 10.0,
            cruise: 100.0,
            accel: 500.0,
            dir: [-1.0, 0.0, 0.0],
        });
        let reversal = planner.finish();

        let mut apart = Planner::new(tune_marlin(0.013));
        apart.push(straight(10.0, 100.0, 500.0));
        apart.stop();
        apart.push(straight(10.0, 100.0, 500.0));
        let apart = apart.finish();

        assert!(
            (reversal - apart).abs() < 1e-9,
            "a reversal costs the same as two separate moves: {reversal} vs {apart}"
        );
    }

    #[test]
    fn a_right_angle_is_taken_at_klippers_square_corner_velocity() {
        let tune = tune_klipper();
        let accel = 4000.0;
        let deviation = tune.deviation(accel);
        let planner = Planner {
            tune,
            pending: None,
            total: 0.0,
            modelled: true,
        };
        let prev = Pending {
            len: 5.0,
            cruise: 100.0,
            accel,
            dir: [1.0, 0.0, 0.0],
            entry: 100.0,
        };
        let next = Move {
            len: 5.0,
            cruise: 100.0,
            accel,
            dir: [0.0, 1.0, 0.0],
        };
        let junction = planner.junction(&prev, &next);
        assert!(
            (junction - KLIPPER_DEFAULT_SCV).abs() < 1e-6,
            "derived deviation {deviation} must return scv at a right angle, got {junction}"
        );
    }

    #[test]
    fn a_straight_join_is_not_slowed_by_the_cornering_model() {
        let planner = Planner::new(tune_klipper());
        let prev = Pending {
            len: 5.0,
            cruise: 100.0,
            accel: 4000.0,
            dir: [0.0, 1.0, 0.0],
            entry: 100.0,
        };
        let along = Move {
            len: 5.0,
            cruise: 100.0,
            accel: 4000.0,
            dir: [0.0, 1.0, 0.0],
        };
        let junction = planner.junction(&prev, &along);
        assert!((junction - 100.0).abs() < 1e-9, "got {junction}");
    }

    #[test]
    fn classic_jerk_bounds_how_much_speed_one_axis_may_change() {
        let planner = Planner::new(Tune {
            junction_deviation: 0.0,
            square_corner_velocity: 0.0,
            jerk: [9.0, 9.0, 0.4],
        });
        let prev = Pending {
            len: 5.0,
            cruise: 100.0,
            accel: 3000.0,
            dir: [1.0, 0.0, 0.0],
            entry: 100.0,
        };
        let next = Move {
            len: 5.0,
            cruise: 100.0,
            accel: 3000.0,
            dir: [0.0, 1.0, 0.0],
        };
        let junction = planner.junction(&prev, &next);
        assert!(
            (junction - 9.0).abs() < 1e-9,
            "a 90° turn at 100 mm/s changes X by 100, so jerk 9 allows 9 mm/s, got {junction}"
        );
    }

    #[test]
    fn a_profile_with_no_cornering_model_is_timed_kinematically() {
        let (config, _report) = PrinterConfig::from_preset(
            &hfw_config::Preset::parse("nozzle_diameter = 0.4\nlayer_height = 0.2\n")
                .expect("preset"),
        );
        let mut tune = Tune::from_profile(&config);
        tune.square_corner_velocity = 0.0;
        assert!(!tune.is_modelled(), "a bare profile describes no corner");

        let mut planner = Planner::new(tune);
        planner.push(straight(50.0, 50.0, 0.0));
        assert!((planner.finish() - 1.0).abs() < 1e-9);
        assert!(
            !planner.is_modelled(),
            "a move with no acceleration is timed kinematically, whatever the cornering model"
        );
    }

    #[test]
    fn a_klipper_profile_without_a_corner_setting_still_gets_a_model() {
        let (config, _report) = PrinterConfig::from_preset(
            &hfw_config::Preset::parse("nozzle_diameter = 0.4\ngcode_flavor = klipper\n")
                .expect("preset"),
        );
        let tune = Tune::from_profile(&config);
        assert!(tune.is_modelled());
        assert!((tune.square_corner_velocity - KLIPPER_DEFAULT_SCV).abs() < 1e-9);
    }

    #[test]
    fn a_stop_between_moves_costs_the_second_move_its_own_ramp() {
        let mut planner = Planner::new(tune_marlin(0.013));
        planner.push(straight(5.0, 200.0, 1000.0));
        planner.stop();
        planner.add(0.3);
        planner.push(straight(5.0, 200.0, 1000.0));
        let seconds = planner.finish();
        assert!(seconds > 0.3, "the dwell is counted: {seconds}");
    }
}
