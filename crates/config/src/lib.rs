//! Printer, filament and process profiles.
//!
//! Two dialects are read, and the difference between them is deliberate.
//!
//! A **native** profile uses this crate's own key names. It is parsed by
//! `PrinterConfig::parse` with every key validated, and an unknown key is an
//! error rather than a silently ignored setting: a profile that quietly drops
//! `retract_len` produces G-code that looks fine and prints badly.
//!
//! A **preset** is what PrusaSlicer or OrcaSlicer writes — the shared config
//! dialect, with engine setting names and an `inherits` chain. It is parsed by
//! `Preset` and applied by `PrinterConfig::from_preset`, which never rejects a
//! preset for carrying a setting this engine does not implement: it applies what
//! it understands and reports how every remaining key was handled. An engine
//! that refuses a user's preset teaches nothing; one that reports what it did is
//! verifiable.
//!
//! A **bundle** is a whole collection of presets — a slicer's shipped profiles or
//! a user's own — from which one machine, one process and one filament are picked
//! and layered. `Bundle` indexes a directory or a sectioned vendor file and
//! resolves the inheritance chains across it, which is the step between "the
//! presets exist on disk" and "the engine can be handed this job".
//!
//! `crates/config/src/dialect_table.rs` is generated from next-layer's proven
//! rename table and the engine's own definition table by
//! `tools/gen-dialect-table.py`.

#![forbid(unsafe_code)]

mod alias_table;
mod bundle;
mod dialect;
mod dialect_table;
mod error;
mod expression;
mod parse;

pub use bundle::{Bundle, Kind, Loaded, Resolution};
pub use dialect::{
    object_overrides, Intake, IntakeReport, ObjectOverride, Preset, ProjectSettings,
};
pub use error::ConfigError;

/// G-code dialect the profile targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    /// Klipper, including `SET_PRINT_STATS_INFO` and `EXCLUDE_OBJECT_DEFINE`.
    Klipper,
    /// Marlin, without Klipper-specific commands.
    Marlin,
}

/// Where the seam of a closed loop is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seam {
    /// Rotate every loop to the same absolute direction from the centroid.
    Aligned,
    /// Start each loop at the point nearest the previous end position.
    Nearest,
    /// Start each loop at a different point each layer, so seams scatter.
    Random,
    /// Put every seam at the rear of the part, on the bed's +Y side.
    Rear,
}

/// Order a plate of objects is printed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintSequence {
    /// Finish every object's layer before starting the next layer.
    ByLayer,
    /// Finish one whole object before starting the next.
    ByObject,
}

/// One embedded preview image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    /// Image width in pixels.
    pub width: usize,
    /// Image height in pixels.
    pub height: usize,
    /// True for PNG, false for QOI.
    pub png: bool,
}

/// A number without trailing zeros, for the placeholder text.
fn trim(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    text.to_owned()
}

/// A complete, validated printer and process profile.
#[derive(Clone, Debug)]
pub struct PrinterConfig {
    /// Profile name, echoed into the G-code banner.
    pub name: String,
    /// Travel limit along X in millimetres.
    pub bed_x: f64,
    /// Travel limit along Y in millimetres.
    pub bed_y: f64,
    /// Bed outline in millimetres, front left first and wound either way; empty
    /// means the rectangle `bed_x` by `bed_y` stands in for it. A profile that
    /// sets one also sets `bed_x` and `bed_y` to its span, because those two are
    /// what the placement centre and the fit check are written against.
    pub bed_shape: Vec<(f64, f64)>,
    /// Travel limit along Z in millimetres.
    pub max_z: f64,
    /// True when the bed origin is at its centre rather than its front left.
    pub origin_center: bool,
    /// Target G-code dialect.
    pub flavor: Flavor,
    /// True to emit relative extruder moves (`M83`).
    pub relative_e: bool,
    /// Nozzle diameter in millimetres.
    pub nozzle_diameter: f64,
    /// Filament diameter in millimetres.
    pub filament_diameter: f64,
    /// Filament density in grams per cubic centimetre.
    pub filament_density: f64,
    /// Nominal extrusion width in millimetres.
    pub line_width: f64,
    /// Narrowest bead the process is willing to print.
    pub min_bead_width: f64,
    /// Layer height after the first layer, in millimetres.
    pub layer_height: f64,
    /// True to honour [`PrinterConfig::layer_height_profile`]; the uniform
    /// `layer_height` is used everywhere until a profile says otherwise.
    pub variable_layer_height: bool,
    /// Per-height layer steps, as `(z, height)` pairs in millimetres.
    ///
    /// The layer whose bottom sits at or above a `z` uses that `height` until
    /// the next pair takes over. Written in a profile as
    /// `layer_height_profile = 0:0.2, 8:0.12, 16:0.28`.
    pub layer_height_profile: Vec<(f64, f64)>,
    /// True prints one continuously rising wall with no infill or top skin.
    pub spiral_vase: bool,
    /// True picks loop entry points that avoid crossing printed perimeters.
    pub avoid_crossing_perimeters: bool,
    /// True to route travels around this layer's overhanging beads, whose edges
    /// curl up as they cool.
    pub avoid_crossing_curled_overhangs: bool,
    /// True to lift the nozzle before a travel that crosses an overhanging bead.
    pub travel_lift_before_obstacle: bool,
    /// Longest extra travel worth taking to avoid a crossing, in millimetres.
    ///
    /// Zero means no limit, which is the dialect default.
    pub avoid_crossing_perimeters_max_detour: f64,
    /// True collapses features too narrow for two beads into one trace.
    pub thin_walls: bool,
    /// Features thinner than this are not printed, in millimetres.
    ///
    /// Negative means unset, which keeps the dialect default of 25 percent of
    /// the nozzle diameter. Zero is a real value there: it prints everything.
    pub min_feature_size: f64,
    /// Longest segment that ties an infill line to the perimeter, in millimetres.
    ///
    /// Negative means unset, which keeps the dialect default of 600 percent of
    /// the infill extrusion width. Zero disables the anchor outright.
    pub infill_anchor: f64,
    /// Ceiling on that segment, in millimetres.
    pub infill_anchor_max: f64,
    /// Thinnest layer this machine prints, in millimetres.
    pub min_layer_height: f64,
    /// Thickest layer this machine prints, in millimetres; `0` follows the
    /// nozzle.
    pub max_layer_height: f64,
    /// First layer height in millimetres.
    pub first_layer_height: f64,
    /// Perimeter loop count.
    pub walls: usize,
    /// Sparse infill density in `0..=1`.
    pub infill_density: f64,
    /// Sparse infill pattern name.
    pub infill_pattern: String,
    /// Solid skin layers over the top surface.
    pub top_layers: usize,
    /// Solid skin layers under the bottom surface.
    pub bottom_layers: usize,
    /// Steady-state nozzle temperature in degrees Celsius.
    pub nozzle_temp: f64,
    /// First layer nozzle temperature in degrees Celsius.
    pub first_nozzle_temp: f64,
    /// Bed temperature in degrees Celsius.
    pub bed_temp: f64,
    /// Bed temperature on the first layer in degrees Celsius; 0 uses
    /// [`PrinterConfig::bed_temp`].
    pub first_layer_bed_temp: f64,
    /// Chamber temperature in degrees Celsius; 0 emits nothing.
    pub chamber_temp: f64,
    /// Part cooling fan duty in percent, `0..=100`.
    pub fan_speed: f64,
    /// Run the part cooling fan at all; false for materials that hate it.
    pub cooling: bool,
    /// Keep the fan running instead of switching it off between layers.
    pub fan_always_on: bool,
    /// First-layer fan duty in percent; negative leaves the layer ramp in charge.
    pub initial_layer_fan_speed: f64,
    /// Fan duty while ironing, in percent; negative leaves the layer value.
    pub ironing_fan_speed: f64,
    /// Fan duty on the support interface, in percent; negative leaves the layer
    /// value.
    pub support_interface_fan_speed: f64,
    /// Pick the fan duty from how much of a path hangs over air.
    pub dynamic_fan_speeds: bool,
    /// Fan duty for each overhang quarter, shallowest first, in percent.
    pub overhang_fan_speeds: [f64; 4],
    /// Layer index at which the fan is switched on.
    pub fan_start_layer: usize,
    /// Retraction distance in millimetres; zero disables retraction.
    pub retract_len: f64,
    /// Retraction speed in millimetres per second.
    pub retract_speed: f64,
    /// Shortest travel that triggers a retraction away from infill.
    pub retract_min_travel: f64,
    /// Shortest travel that triggers a retraction between infill moves.
    pub infill_retract_min_travel: f64,
    /// Z hop height in millimetres; zero disables hopping.
    pub z_hop: f64,
    /// Travel speed in millimetres per second.
    pub travel_speed: f64,
    /// Pick the speed from how much of a path hangs over air.
    pub dynamic_overhang_speeds: bool,
    /// Speed for each overhang quarter, shallowest first, in millimetres per
    /// second; 0 leaves the feature speed alone.
    pub overhang_speeds: [f64; 4],
    /// Vertical travel speed in millimetres per second; 0 falls back to
    /// [`PrinterConfig::travel_speed`].
    pub travel_speed_z: f64,
    /// First layer speed in millimetres per second.
    pub first_layer_speed: f64,
    /// First layer perimeter speed, both walls; 0 leaves each wall to its own
    /// first-layer key or to [`PrinterConfig::first_layer_speed`].
    pub first_layer_perimeter_speed: f64,
    /// First layer outer wall speed; 0 caps the wall at
    /// [`PrinterConfig::first_layer_speed`] instead.
    pub first_layer_outer_speed: f64,
    /// First layer inner wall speed; 0 caps walls at
    /// [`PrinterConfig::first_layer_speed`].
    pub first_layer_inner_speed: f64,
    /// First layer gap-fill speed; 0 caps it at
    /// [`PrinterConfig::first_layer_speed`].
    pub first_layer_thin_speed: f64,
    /// First layer sparse infill speed; 0 caps it at
    /// [`PrinterConfig::first_layer_speed`].
    pub first_layer_infill_speed: f64,
    /// First layer internal solid speed; 0 caps it at
    /// [`PrinterConfig::first_layer_speed`].
    pub first_layer_solid_speed: f64,
    /// First layer top surface speed; 0 caps it at
    /// [`PrinterConfig::first_layer_speed`].
    pub first_layer_top_solid_speed: f64,
    /// First layer support speed; 0 caps it at
    /// [`PrinterConfig::first_layer_speed`].
    pub first_layer_support_speed: f64,
    /// Outer wall speed in millimetres per second.
    pub outer_wall_speed: f64,
    /// Inner wall speed in millimetres per second.
    pub inner_wall_speed: f64,
    /// Speed for beads from the medial axis, in millimetres per second.
    pub thin_speed: f64,
    /// Sparse infill speed in millimetres per second.
    pub infill_speed: f64,
    /// Solid skin speed in millimetres per second.
    pub solid_speed: f64,
    /// Speed for the visible top surface, in millimetres per second; 0 falls
    /// back to [`PrinterConfig::solid_speed`].
    pub top_solid_speed: f64,
    /// Support body speed in millimetres per second.
    pub support_speed: f64,
    /// Support interface speed in millimetres per second.
    pub support_interface_speed: f64,
    /// Generate supports under overhangs.
    pub support: bool,
    /// Steepest overhang printed without support, in degrees from vertical.
    pub support_threshold_angle: f64,
    /// Horizontal clearance between support and part, in millimetres.
    pub support_xy_gap: f64,
    /// Vertical clearance between support top and the part, in millimetres.
    pub support_z_gap: f64,
    /// Dense layers immediately under the contact surface.
    pub support_interface_layers: usize,
    /// Spacing between support body lines in millimetres; `0` prints the body
    /// solid, and `-1` leaves [`PrinterConfig::support_density`] in charge.
    pub support_spacing: f64,
    /// Spacing between support interface lines in millimetres; `0` prints the
    /// interface solid, which is what the preset dialects default to, and `-1`
    /// leaves [`PrinterConfig::support_interface_density`] in charge.
    pub support_interface_spacing: f64,
    /// Line density of the support body.
    pub support_density: f64,
    /// Line density of the interface layers.
    pub support_interface_density: f64,
    /// Drop support columns that would have to rest on the part.
    pub support_on_build_plate_only: bool,
    /// Smallest overhang patch worth supporting, in square millimetres.
    pub support_min_area: f64,
    /// Support body shape: `normal` or `tree`.
    pub support_style: String,
    /// Steepest branch lean from vertical, in degrees.
    pub support_branch_angle: f64,
    /// Distance under which two branches start converging, in millimetres.
    pub support_branch_distance: f64,
    /// Diameter of the contact tip under an overhang, in millimetres.
    pub support_tip_diameter: f64,
    /// How fast a branch widens as it descends, in degrees of cone.
    pub support_branch_diameter_angle: f64,
    /// Largest branch diameter a trunk may reach, in millimetres.
    pub support_max_branch_diameter: f64,
    /// Volumetric flow ceiling in cubic millimetres per second; 0 is no ceiling.
    ///
    /// The reference slicers default this to 0 and mean "unlimited", so a
    /// configuration written by one of them carries a zero here and expecting a
    /// positive number would refuse every such file.
    pub max_volumetric_speed: f64,
    /// Fastest the volumetric rate may rise, in cubic millimetres per second
    /// squared; 0 leaves the rate unconstrained.
    pub max_volumetric_rate_slope_positive: f64,
    /// Fastest the volumetric rate may fall, in cubic millimetres per second
    /// squared; 0 leaves the rate unconstrained.
    pub max_volumetric_rate_slope_negative: f64,
    /// Machine speed ceiling in millimetres per second.
    pub max_speed: f64,
    /// Minimum time per layer used to slow small layers down.
    pub min_layer_time: f64,
    /// Lower bound on the layer-time speed scale.
    pub min_speed_factor: f64,
    /// Speed a slowed-down layer will not go below, in millimetres per second;
    /// 0 falls back to [`PrinterConfig::min_speed_factor`].
    pub min_print_speed: f64,
    /// How the extra time a short layer needs is distributed: `proportional`
    /// scales every move, `uniform` slows the fastest moves first, and
    /// `consistent_surface` is `uniform` with the band nearest the wall held
    /// back, [`PrinterConfig::cooling_perimeter_transition_distance`]
    /// millimetres wide.
    pub cooling_slowdown_logic: String,
    /// Width of the band around this layer's walls that `consistent_surface`
    /// slows last, in millimetres.
    pub cooling_perimeter_transition_distance: f64,
    /// Extrusion multiplier applied to every bead.
    pub flow: f64,
    /// Seam placement rule for closed loops.
    pub seam: Seam,
    /// True to print inner walls before the outer wall.
    pub inner_walls_first: bool,
    /// How far the solid shell reaches between the skin and the sparse fill:
    /// `disabled`, `partial` or `enabled`.
    pub ensure_vertical_shell_thickness: String,
    /// Number of skirt loops on the first layer.
    pub skirt_loops: usize,
    /// Gap between the skirt and the part, in millimetres.
    pub skirt_distance: f64,
    /// Number of adhesion brim loops on the first layer.
    pub brim_loops: usize,
    /// Brim width in millimetres; 0 uses [`PrinterConfig::brim_loops`].
    pub brim_width: f64,
    /// Clearance between the part and the innermost brim loop, in millimetres.
    pub brim_gap: f64,
    /// `outer_only`, `outer_and_inner`, `inner_only` or `no_brim`.
    pub brim_type: String,
    /// Gap between two brim loops, in millimetres; 0 spaces them one bead apart.
    pub brim_separation: f64,
    /// Shortest skirt this profile accepts, in millimetres; 0 disables the floor.
    pub min_skirt_length: f64,
    /// Shrink applied to the first layer in the XY plane, in millimetres.
    ///
    /// Squash on the first layer makes a part wider at the bed than above it;
    /// this pulls the first cross-section in by the measured overshoot.
    pub elefant_foot_compensation: f64,
    /// Growth applied to every layer in the XY plane, in millimetres.
    ///
    /// Positive grows the object and closes holes, negative shrinks it and opens
    /// holes: the setting used to tune a press fit or a hole diameter.
    pub xy_size_compensation: f64,
    /// Cracks narrower than twice this are filled while slicing, in millimetres.
    pub slice_closing_radius: f64,
    /// Maximum deviation allowed when simplifying a sliced cross-section.
    ///
    /// Zero disables simplification and keeps every vertex the slice produced.
    pub resolution: f64,
    /// Raft layers printed under the part before the first object layer.
    pub raft_layers: usize,
    /// Line density of the first raft layer.
    pub raft_first_layer_density: f64,
    /// How far the first raft layer reaches past the part, in millimetres.
    pub raft_first_layer_expansion: f64,
    /// Vertical gap between the top raft layer and the part, in millimetres.
    pub raft_contact_distance: f64,
    /// `disabled`, `enabled` or `limited`.
    pub draft_shield: String,
    /// Height a limited draft shield stops at, in millimetres.
    pub draft_shield_height: f64,
    /// Slowest hop a travel may start with, in millimetres per second of climb.
    pub travel_slope: f64,
    /// True to climb over a whole travel instead of dropping straight down.
    pub travel_ramping_lift: bool,
    /// Highest a hop may rise above the current layer, in millimetres.
    pub travel_max_lift: f64,
    /// Above this Z no hop is made, in millimetres; 0 disables the ceiling.
    pub retract_lift_above: f64,
    /// Below this Z no hop is made, in millimetres; 0 disables the floor.
    pub retract_lift_below: f64,
    /// Seam gap left open at the start of a closed wall, in millimetres.
    pub seam_gap: f64,
    /// True to move each inner wall's seam away from the outer wall's seam.
    pub staggered_inner_seams: bool,
    /// Length over which the seam is graduated instead of butted, in millimetres.
    pub scarf_seam_length: f64,
    /// Longest segment the scarf ramp may use, in millimetres.
    pub scarf_seam_max_segment: f64,
    /// True to scarf inner walls as well as the outer one.
    pub scarf_seam_inner: bool,
    /// Where a scarf joint may go: `nowhere`, `contours` or `everywhere`.
    pub scarf_seam_placement: String,
    /// True to graduate the flow across the whole loop instead of one seam
    /// length either side of it.
    pub scarf_seam_entire_loop: bool,
    /// True to scarf only where the seam vertex is smooth rather than a corner.
    pub scarf_seam_only_on_smooth: bool,
    /// Z below which the seam is butted rather than scarfed, in millimetres.
    pub scarf_seam_start_height: f64,
    /// `rectilinear` or `grid`.
    pub support_pattern: String,
    /// `rectilinear` or `grid`, for the interface layers only; empty follows the
    /// body pattern.
    pub support_interface_pattern: String,
    /// Closed support-interface outline loops.
    pub support_interface_loops: usize,
    /// True to give the interface two shells instead of one.
    pub support_interface_shells: bool,
    /// Closed outline loop around the support body.
    pub support_sheath: bool,
    /// How far the support footprint grows on the first layer, in millimetres.
    pub support_first_layer_expansion: f64,
    /// Layers from the bed that are supported whether they overhang or not.
    pub support_enforce_layers: usize,
    /// Dense layers where a column meets what it stands on.
    pub support_bottom_interface_layers: usize,
    /// Steepest lean of a branch at its tip, in degrees.
    pub support_tree_angle_slow: f64,
    /// Tip sampling density at the contact surface, in percent.
    pub support_tree_top_rate: f64,
    /// True to let a two-wall trunk reach twice the single-wall diameter.
    pub support_tree_double_wall: bool,
    /// Closing radius applied to each layer's support area, in millimetres.
    pub support_closing_radius: f64,
    /// Gap under a support column that lands on the part, in millimetres.
    pub support_bottom_contact_distance: f64,
    /// True to leave bridges unsupported.
    pub support_skip_bridges: bool,
    /// Line density of the first support layer; 0 uses the body density.
    pub support_first_layer_density: f64,
    /// `none`, `external` or `all`.
    pub fuzzy_skin: String,
    /// Peak displacement of a fuzzy wall, in millimetres.
    pub fuzzy_skin_thickness: f64,
    /// Distance between fuzzy wall samples, in millimetres.
    pub fuzzy_skin_point_dist: f64,
    /// Speed for skin printed over air, in millimetres per second.
    pub bridge_speed: f64,
    /// Extrusion multiplier applied to bridge beads only.
    pub bridge_flow: f64,
    /// Bridge line density; 1.0 means adjacent beads touch, above 1.0 overlaps.
    pub bridge_density: f64,
    /// Bridge bead width in millimetres; 0 uses `line_width`.
    pub bridge_line_width: f64,
    /// Bead width of the inner perimeter loops; 0 uses `line_width`.
    ///
    /// The dialect names this `perimeter_extrusion_width`, and a percent value
    /// there means percent of the nozzle diameter, not of `line_width`.
    pub wall_width: f64,
    /// Bead width of the outermost perimeter; 0 uses the inner wall width.
    ///
    /// This is the loop a part is judged by, so a profile often prints it
    /// narrower or wider than the loops behind it. The wall ladder is
    /// cumulative: a wider outer bead moves every inner loop inwards by the
    /// same amount, so the band stays gap-free and overlap-free.
    pub outer_wall_width: f64,
    /// Bead width of sparse infill; 0 uses `line_width`.
    pub infill_width: f64,
    /// Bead width of solid skin infill; 0 uses the sparse infill width.
    pub solid_infill_width: f64,
    /// Bead width of the first layer; 0 keeps the nominal width.
    ///
    /// A first layer is squashed by the nozzle against a flat bed, so the bead
    /// comes out wider than the nozzle can lay on a previous layer. That is why
    /// the dialect default is 150 percent of the nozzle rather than its usual
    /// 112.5: the extra width is what the squash leaves behind, and the slicer
    /// writes the volume for the bead it expects to see.
    pub first_layer_width: f64,
    /// Bead width of the top solid surface; 0 uses `solid_infill_width`.
    ///
    /// The dialect calls this `top_infill_extrusion_width` and gives it to the
    /// top skin only, so the face a print is judged on can be drawn finer than
    /// the layers of shell underneath it.
    pub top_infill_width: f64,
    /// Derive an unset extrusion width from the nozzle diameter.
    ///
    /// Off by default because a width of zero already means "the nominal width",
    /// which is what an HFW profile expects. On, a zero width is derived per
    /// feature from the nozzle (112.5 percent, the dialect's rounded-rectangle
    /// rule), so a preset that leaves every width at zero slices the way its
    /// author's slicer does instead of this engine's defaults.
    pub automatic_extrusion_widths: bool,
    /// Bead width of support body and interface; 0 uses `line_width`.
    pub support_width: f64,
    /// Bead width of the dense support interface; 0 uses `support_width`.
    pub support_interface_width: f64,
    /// Fill pattern of the top solid surface.
    ///
    /// The dialect's list is `rectilinear`, `monotonic`, `monotoniclines`,
    /// `alignedrectilinear` and `concentric`; the three curve patterns are
    /// refused rather than silently replaced, because a skin drawn in the wrong
    /// pattern is a wrong-looking part, not a missing one.
    pub top_fill_pattern: String,
    /// Fill pattern of the bottom solid surface.
    pub bottom_fill_pattern: String,
    /// Force a solid layer every N layers; 0 leaves sparse infill sparse.
    ///
    /// Upstream turns the internal fill of that layer solid without touching
    /// the top and bottom shells, which is what makes a part with no top or
    /// bottom surface still hold together.
    pub solid_infill_every_layers: usize,
    /// Regions smaller than this area are filled solid, in square millimetres.
    ///
    /// A small island in the middle of a part gets one or two sparse lines and
    /// nothing to land on. The dialect's default of 70 fills anything a thumb
    /// could cover.
    pub solid_infill_below_area: f64,
    /// Hard ceiling on the volumetric flow of this filament, in mm³/s; 0 off.
    ///
    /// The printer's own limit is `max_volumetric_speed`; this one belongs to the
    /// material, and the two are both enforced: whichever is lower wins.
    pub filament_max_volumetric_speed: f64,
    /// Ceiling on the speed of sparse infill, in mm/s; 0 off.
    pub filament_infill_max_speed: f64,
    /// Ceiling on sparse infill that reaches into the shell, in mm/s; 0 off.
    ///
    /// This engine anchors every infill line into the perimeter by default, so
    /// this cap is the one an infill line usually meets. Turn the anchor off and
    /// the plain infill cap applies instead.
    pub filament_infill_max_crossing_speed: f64,
    /// Speed for solid infill printed over a bridge of the layer below, in mm/s.
    pub over_bridge_speed: f64,
    /// Fan duty over bridges that nothing above covers, as a percentage.
    ///
    /// Negative leaves the bridge on the bridge fan. A part's visible bridges are
    /// what it is judged on, so they get full cooling; a bridge buried inside the
    /// part only has to hold the next layer, and too much fan curls its edges.
    pub internal_bridge_fan_speed: f64,
    /// Allow the overhang fan quarters to shape the fan over bridges.
    ///
    /// Off, every bridge gets the bridge fan speed instead of the overhang
    /// ladder, which is what a machine with a strong part fan usually wants.
    pub enable_overhang_bridge_fan: bool,
    /// Percentage of wipes that retract first. Zero wipes before retracting.
    ///
    /// The amount is read as a switch: any non-zero value retracts and then
    /// wipes, which is what a direct drive extruder needs; zero drags the still
    /// pressurised nozzle back along the bead first, which is what a bowden tube
    /// needs.
    pub retract_before_wipe: f64,
    /// Retract only when the travel actually crosses a printed perimeter.
    pub only_retract_when_crossing_perimeters: bool,
    /// Expansion of each raft layer above the first, in millimetres.
    pub raft_expansion: f64,
    /// Speed for the first layer printed on a raft, in mm/s.
    pub first_layer_speed_over_raft: f64,
    /// Acceleration for the first layer printed on a raft, in mm/s².
    pub first_layer_acceleration_over_raft: f64,
    /// Print one perimeter on the first layer and no inner loops.
    ///
    /// A first layer with only its outer loop is a wider flat bead that squashes
    /// into the bed, which is how some machines get adhesion without a brim.
    pub only_one_perimeter_first_layer: bool,
    /// One perimeter on flat top surfaces: `none`, `top` or `topmost`.
    ///
    /// A top face bounded by a single wall leaves the infill one bead more room
    /// to land in, which shows on a large flat surface.
    pub top_one_perimeter_type: String,
    /// Acceleration for travel moves, in mm/s²; 0 leaves it to the machine.
    pub travel_acceleration: f64,
    /// Acceleration for travel moves shorter than a millimetre or two.
    pub travel_short_distance_acceleration: f64,
    /// Which machine limits to emit: `emit_to_gcode`, `time_estimate_only` or
    /// `ignore`.
    ///
    /// `time_estimate_only` is the dialect's default and this engine's is
    /// `emit_to_gcode`: HFW writes the envelope into the file so a machine with
    /// no printer.cfg limits still gets them. `ignore` drops both the M-codes
    /// and the speed clamp.
    pub machine_limits_usage: String,
    /// Let the firmware watch for a stuck filament over `M591`.
    pub stuck_filament_detection: bool,
    /// Shrinkage of the filament in X and Y, as a percentage, compensated by
    /// scaling the model up.
    pub shrinkage_xy: f64,
    /// Shrinkage of the filament in Z, as a percentage.
    pub shrinkage_z: f64,
    /// Perimeter generator: `arachne` for this engine's medial-axis beading,
    /// `classic` for fixed-width offsets.
    pub perimeter_generator: String,
    /// Length over which a bead count change is blended, in millimetres.
    pub wall_transition_length: f64,
    /// Angle at which a wall transition happens, in degrees.
    pub wall_transition_angle: f64,
    /// How far a transition may deviate before it is filtered, as a percentage.
    pub wall_transition_filter_deviation: f64,
    /// Detect overhangs and print them at the overhang speeds and fan.
    pub overhangs: bool,
    /// Chamber temperature the print waits for, in °C; 0 does not wait.
    pub chamber_minimal_temperature: f64,
    /// Print the sparse fill of N layers in one thicker layer; 1 is off.
    ///
    /// A combinfill layer is printed at N times the spacing with N times the
    /// flow, so the plastic of N layers lands in one pass. It is the oldest trick
    /// there is for making a print faster without weakening it.
    pub infill_every_layers: usize,
    /// Combine infill automatically, up to the nozzle diameter.
    pub automatic_infill_combination: bool,
    /// Ceiling on a combined layer's height, in millimetres; 0 uses the nozzle.
    pub infill_combination_max_layer_height: f64,
    /// Number of loops over which an extrusion width variation is spread.
    ///
    /// With the outer loop wider or narrower than the inner ones, this decides
    /// how many loops share the difference. One puts all of it on the loop
    /// nearest the middle of the band, which is upstream's default; a larger
    /// number tapers it instead of stepping it.
    pub wall_distribution_count: usize,
    /// Emit the thin bead that fills a gap between the shell and the infill.
    pub gap_fill_enabled: bool,
    /// Print internal fill before the perimeters of the same layer.
    ///
    /// The fill then anchors to the layer below instead of to the wall beside
    /// it. It is worse for surface finish and better for parts that shear
    /// between shells, which is why it is off by default.
    pub infill_first: bool,
    /// Add a loop that follows the unsupported part of each layer.
    ///
    /// Layer 1 is exempt for the same reason the step-out rule exempts it: an
    /// elephant-foot correction makes the first layer smaller on purpose.
    pub extra_perimeters_on_overhangs: bool,
    /// Add a perimeter on layers that step out over the one below.
    ///
    /// A sloping wall widens by less than a bead per layer, so the outline of a
    /// layer can sit further out than its walls reach, leaving the sparse fill
    /// hanging over air. One more loop closes that gap. The first two layers are
    /// exempt: layer 1 may step out over a first layer that the elephant-foot
    /// correction deliberately shrank, and a loop there would put the lip back
    /// that the correction just removed.
    pub extra_perimeters: bool,
    /// Emit `G2`/`G3` arcs where a run of moves lies on a circle.
    ///
    /// Marlin needs `ARC_SUPPORT` and Klipper needs a `[gcode_arcs]` section; a
    /// machine without either rejects the file, so this stays off by default.
    pub arc_fitting: bool,
    /// Largest deviation, in millimetres, allowed between a fitted arc and the
    /// moves it replaces.
    pub arc_fitting_tolerance: f64,
    /// Iron the top surface: a slow, near-zero-flow pass over the skin.
    pub ironing: bool,
    /// Ironing passes over the top skin only, or over every solid surface.
    pub ironing_all_solid: bool,
    /// Which surfaces an ironing pass touches: `top_surfaces` for every visible
    /// top skin, `topmost_only` for the highest one, or `all_solid`.
    pub ironing_type: String,
    /// Extrusion flow for an ironing pass, as a fraction of normal flow.
    pub ironing_flow: f64,
    /// Distance between ironing passes in millimetres.
    pub ironing_spacing: f64,
    /// Ironing speed in millimetres per second.
    pub ironing_speed: f64,
    /// Forced bridge direction in degrees; negative lets the engine choose.
    pub bridge_angle: f64,
    /// Print bridge beads one nozzle diameter thick instead of one layer.
    pub thick_bridges: bool,
    /// Print two crossing strands across each gap before the bridge fill.
    pub bridge_cross_hatch: bool,
    /// Fan duty cycle while bridge beads print, in percent.
    pub bridge_fan_speed: f64,
    /// Acceleration applied while bridges print, in mm/s^2; 0 leaves it alone.
    ///
    /// Only Klipper profiles get this: the value is wrapped in
    /// `SAVE_VELOCITY_LIMIT`/`RESTORE_VELOCITY_LIMIT` so the print's own limit is
    /// restored afterwards.
    pub bridge_acceleration: f64,
    /// Longest span bridged without help, in millimetres; 0 means no limit.
    pub max_bridge_length: f64,
    /// Extra bridge flow per millimetre of span past `bridge_span_ref`.
    ///
    /// Negative values thin long spans, which is the direction that helps a
    /// sagging external bridge: a lighter strand hangs taut between its anchors,
    /// while extra material makes a heavier rope that droops.
    pub bridge_span_flow: f64,
    /// Fraction of bridge speed lost per millimetre of span above
    /// `bridge_span_ref`.
    pub bridge_span_speed: f64,
    /// Span, in millimetres, at which the span adjustments start.
    pub bridge_span_ref: f64,
    /// True to emit `;` comments in the body of the file.
    pub gcode_comments: bool,
    /// True to label the printed object so the machine can exclude it.
    pub label_objects: bool,
    /// Coordinate rounding in millimetres; `0` writes the raw value.
    pub gcode_resolution: f64,
    /// True to write `E` in cubic millimetres and declare the diameter.
    pub volumetric_e: bool,
    /// True to retract with `G10`/`G11` instead of an axis move.
    pub firmware_retraction: bool,
    /// True to print closed loops clockwise.
    pub clockwise: bool,
    /// True to emit `M73` progress and remaining time.
    pub remaining_times: bool,
    /// False to leave every temperature command to the start gcode.
    pub autoemit_temperatures: bool,
    /// G-code inserted before each layer change.
    pub before_layer_gcode: String,
    /// G-code inserted after each layer.
    pub layer_gcode: String,
    /// G-code appended after the start gcode for the loaded filament.
    pub start_filament_gcode: String,
    /// G-code inserted before the end gcode for the loaded filament.
    pub end_filament_gcode: String,
    /// Preview thumbnails embedded in the header.
    pub thumbnails: Vec<Thumbnail>,
    /// True to emit `SET_PRINT_STATS_INFO` for Klipper layer tracking.
    pub emit_print_stats: bool,
    /// True to emit `EXCLUDE_OBJECT_DEFINE` so the part can be cancelled.
    pub exclude_object: bool,
    /// Pressure advance applied at the start of the print, if non-zero.
    pub pressure_advance: f64,
    /// Deceleration and acceleration defaults, in mm/s^2.
    ///
    /// Zero everywhere in this block means "emit nothing": the machine keeps
    /// whatever acceleration its own configuration set, which is what a profile
    /// that never mentions acceleration should do.
    pub default_accel: f64,
    /// Acceleration for the outer wall, in mm/s^2; 0 falls back to
    /// [`PrinterConfig::perimeter_accel`], then to
    /// [`PrinterConfig::default_accel`].
    pub outer_wall_accel: f64,
    /// Acceleration for inner walls, in mm/s^2; 0 falls back to
    /// [`PrinterConfig::perimeter_accel`], then to
    /// [`PrinterConfig::default_accel`].
    pub inner_wall_accel: f64,
    /// Acceleration shared by both walls, in mm/s^2; 0 falls back to
    /// [`PrinterConfig::default_accel`].
    pub perimeter_accel: f64,
    /// Acceleration for the visible top surface, in mm/s^2; 0 falls back to
    /// [`PrinterConfig::solid_accel`].
    pub top_solid_accel: f64,
    /// Acceleration for the first layer, in mm/s^2; 0 leaves the feature value.
    pub initial_layer_accel: f64,
    /// Acceleration for solid skin, in mm/s^2; 0 falls back to
    /// [`PrinterConfig::default_accel`].
    pub solid_accel: f64,
    /// Acceleration for sparse infill, in mm/s^2; 0 falls back to
    /// [`PrinterConfig::default_accel`].
    pub infill_accel: f64,
    /// Acceleration for the support body, in mm/s^2; 0 falls back to
    /// [`PrinterConfig::default_accel`].
    pub support_accel: f64,
    /// Acceleration for the support interface, in mm/s^2; 0 falls back to
    /// [`PrinterConfig::default_accel`].
    pub support_interface_accel: f64,
    /// X motor acceleration ceiling, in mm/s^2; 0 leaves the machine alone.
    pub machine_max_accel_x: f64,
    /// Y motor acceleration ceiling, in mm/s^2; 0 leaves the machine alone.
    pub machine_max_accel_y: f64,
    /// Z motor acceleration ceiling, in mm/s^2; 0 leaves the machine alone.
    pub machine_max_accel_z: f64,
    /// Extruder acceleration ceiling, in mm/s^2; 0 leaves the machine alone.
    pub machine_max_accel_e: f64,
    /// X feedrate ceiling, in millimetres per second; 0 leaves the machine alone.
    pub machine_max_feedrate_x: f64,
    /// Y feedrate ceiling, in millimetres per second; 0 leaves the machine alone.
    pub machine_max_feedrate_y: f64,
    /// Z feedrate ceiling, in millimetres per second; 0 leaves the machine alone.
    pub machine_max_feedrate_z: f64,
    /// Extruder feedrate ceiling, in millimetres per second; 0 leaves it alone.
    pub machine_max_feedrate_e: f64,
    /// X jerk, in millimetres per second; 0 leaves the machine alone.
    ///
    /// Jerk is a Marlin concept: it reaches the output as `M205`. Klipper has no
    /// jerk limit, so a Klipper profile ignores this and the intake says so.
    pub machine_max_jerk_x: f64,
    /// Y jerk, in millimetres per second; 0 leaves the machine alone.
    pub machine_max_jerk_y: f64,
    /// Z jerk, in millimetres per second; 0 leaves the machine alone.
    pub machine_max_jerk_z: f64,
    /// Extruder jerk, in millimetres per second; 0 leaves the machine alone.
    pub machine_max_jerk_e: f64,
    /// Printing acceleration ceiling, in mm/s^2; 0 leaves the machine alone.
    ///
    /// Reaches Marlin as the `P` (or `S`) argument of `M204`. Klipper has no
    /// separate printing ceiling — `SET_VELOCITY_LIMIT ACCEL=` is the whole limit
    /// — so a Klipper profile ignores this and the intake says so.
    pub machine_max_accel_extruding: f64,
    /// Retraction acceleration ceiling, in mm/s^2; 0 leaves the machine alone.
    ///
    /// Marlin's `M204 R` (`T` on legacy firmware). Klipper ignores it.
    pub machine_max_accel_retracting: f64,
    /// Travel acceleration ceiling, in mm/s^2; 0 leaves the machine alone.
    ///
    /// Marlin's `M204 T`. Klipper ignores it. Distinct from
    /// [`PrinterConfig::travel_accel`], which the writer applies per move.
    pub machine_max_accel_travel: f64,
    /// Junction deviation, in millimetres; 0 leaves the machine alone.
    ///
    /// Marlin's `M205 J`. Klipper has no junction deviation — its nearest
    /// equivalent is `SQUARE_CORNER_VELOCITY`, a different quantity — so a
    /// Klipper profile ignores this and the intake says so.
    pub machine_max_junction_deviation: f64,
    /// Minimum feedrate while extruding, in millimetres per second; 0 leaves the
    /// machine alone. Marlin's `M205 S`.
    pub machine_min_extruding_rate: f64,
    /// Minimum feedrate while travelling, in millimetres per second; 0 leaves the
    /// machine alone. Marlin's `M205 T`.
    pub machine_min_travel_rate: f64,
    /// Added to every Z coordinate in the output, in millimetres; negative moves
    /// the print towards the bed.
    ///
    /// Compensates for an endstop that triggers off the bed: a machine whose
    /// zero sits 0.3 mm above the bed is given `-0.3`. The shift lands on the
    /// emitted coordinates only — the sliced layers keep their own Z, so the
    /// layer schedule and the Z-hop heights are unaffected.
    pub z_offset: f64,
    /// Fan duty the first cooled layer runs at, in percent; 0 means start at
    /// [`PrinterConfig::fan_speed`].
    pub min_fan_speed: f64,
    /// Layer at which the fan reaches [`PrinterConfig::fan_speed`]; 0 or a value
    /// at or below the start layer means full fan immediately.
    pub full_fan_speed_layer: usize,
    /// Direction sparse infill prints on the first layer, in degrees.
    ///
    /// Patterns that alternate by layer add 90 degrees on the odd ones, so the
    /// dialect's 45 gives the usual 45/135 cross-hatch.
    pub fill_angle: f64,
    /// A layer printed faster than this runs the fan at full speed, in seconds;
    /// 0 disables the rule.
    pub fan_below_layer_time: f64,
    /// A layer shorter than this is slowed down, in seconds; 0 disables the rule.
    pub slow_down_layer_time: f64,
    /// Speed a slowed-down layer will not go below, in millimetres per second;
    /// 0 uses [`PrinterConfig::min_speed_factor`] instead.
    pub slow_down_min_speed: f64,
    /// Layers the skirt is printed on, counting from the first; 1 is the default
    /// and means the first layer only.
    pub skirt_height: usize,
    /// Order a plate of several objects is printed in.
    pub print_sequence: PrintSequence,
    /// Height the toolhead can pass over a finished object, in millimetres.
    ///
    /// Zero means unmeasured, which is why `print_sequence = by object` refuses
    /// to run until a profile states it: guessing would drive the gantry through
    /// a finished part.
    pub gantry_height: f64,
    /// Nozzle temperature held after the print, in degrees Celsius; 0 leaves the
    /// nozzle as the end G-code finds it.
    pub idle_temp: f64,
    /// Unretraction speed in millimetres per second; 0 uses
    /// [`PrinterConfig::retract_speed`].
    pub deretract_speed: f64,
    /// Extra filament fed on unretraction, in millimetres.
    pub retract_restart_extra: f64,
    /// True to retract before every Z change.
    pub retract_layer_change: bool,
    /// True to drag the nozzle back along the bead it just laid while retracting.
    pub wipe: bool,
    /// Distance to wipe, in millimetres.
    pub wipe_distance: f64,
    /// Start G-code, with `{placeholders}` substituted before emission.
    pub start_gcode: String,
    /// End G-code, with `{placeholders}` substituted before emission.
    pub end_gcode: String,
    /// G-code written between one object and the next, for a plate printed one
    /// object at a time.
    pub between_objects_gcode: String,
    /// G-code written at a pause, with `{placeholders}` substituted.
    pub pause_print_gcode: String,
    /// Heights a pause is wanted at, in printed Z; filled by the caller from the
    /// pause flags, never by a profile.
    pub pause_heights: Vec<f64>,
    /// Heights a filament change is wanted at, in printed Z; filled by the
    /// caller, never by a profile.
    pub colour_change_heights: Vec<f64>,
    /// G-code written at a filament change, with `{placeholders}` substituted.
    pub color_change_gcode: String,
    /// Search-and-replace passes applied to the finished file.
    pub gcode_substitutions: Vec<Substitution>,
    /// Printer reported as silent; kept so a preset round-trips. No G-code
    /// depends on it — it is the host's own display choice.
    pub silent_mode: bool,
    /// Printer model name, written into the file header.
    pub printer_model: String,
    /// Printer variant name, written into the file header.
    pub printer_variant: String,
    /// Printer vendor, written into the file header.
    pub printer_vendor: String,
    /// Free-form printer notes, written into the file header.
    pub printer_notes: String,
    /// Free-form job notes, written into the file header.
    pub notes: String,
    /// Filament family, for the operator and the host: `PLA`, `PETG`, `ABS`.
    pub filament_type: String,
    /// Filament vendor, written into the file header.
    pub filament_vendor: String,
    /// Filament colour as the preset spells it, written into the header.
    pub filament_colour: String,
    /// Free-form filament notes, written into the file header.
    pub filament_notes: String,
    /// True when the filament dissolves in water, written into the header.
    pub filament_soluble: bool,
    /// True for an abrasive filament, written into the header so a brass nozzle
    /// is not used on it.
    pub filament_abrasive: bool,
    /// Filament price per kilogram; 0 leaves the cost out of the report.
    pub filament_cost: f64,
    /// Empty spool weight in grams; used with
    /// [`PrinterConfig::filament_cost`] to report what a spool has left.
    pub filament_spool_weight: f64,
    /// Name the output file is written under, with `{placeholders}`; empty keeps
    /// the name the caller asked for.
    pub output_filename_format: String,
    /// Command run on the finished file, `;`-separated; empty runs nothing.
    pub post_process: String,
    /// Host parameters carried through the preset unchanged.
    pub custom_parameters_print: String,
    /// Host parameters carried through the preset unchanged.
    pub custom_parameters_filament: String,
    /// Host parameters carried through the preset unchanged.
    pub custom_parameters_printer: String,
    /// Template the host fills in; carried through the preset unchanged.
    pub template_custom_gcode: String,
    /// Which print profile a GUI would preselect; carried through unchanged.
    pub default_print: String,
    /// Which filament profile a GUI would preselect; carried through unchanged.
    pub default_material: String,
    /// Which tool preset a GUI would preselect; carried through unchanged.
    pub default_tool_print: String,
    /// How far infill may reach into the shell, as a fraction of the bead
    /// width; negative uses the dialect's 15 percent.
    pub infill_overlap: f64,
    /// Minimum thickness of the top skin, in millimetres; 0 uses
    /// [`PrinterConfig::top_layers`].
    pub top_solid_min_thickness: f64,
    /// Minimum thickness of the bottom skin, in millimetres; 0 uses
    /// [`PrinterConfig::bottom_layers`].
    pub bottom_solid_min_thickness: f64,
}

/// One search-and-replace pass over the finished G-code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Substitution {
    /// Text or regular expression to find.
    pub pattern: String,
    /// Replacement; a regular expression may refer to its groups as `$1`.
    pub format: String,
    /// Flag letters: `r` regular expression, `i` ignore case, `w` whole word,
    /// `s` single line.
    pub flags: String,
    /// Note written next to the substitution's own comment line.
    pub note: String,
}

impl Default for PrinterConfig {
    fn default() -> PrinterConfig {
        PrinterConfig {
            name: "generic".to_owned(),
            bed_x: 220.0,
            bed_y: 220.0,
            bed_shape: Vec::new(),
            max_z: 250.0,
            origin_center: false,
            flavor: Flavor::Marlin,
            relative_e: true,
            nozzle_diameter: 0.4,
            filament_diameter: 1.75,
            filament_density: 1.24,
            line_width: 0.4,
            min_bead_width: 0.2,
            layer_height: 0.2,
            variable_layer_height: true,
            layer_height_profile: Vec::new(),
            spiral_vase: false,
            avoid_crossing_perimeters: false,
            avoid_crossing_curled_overhangs: false,
            travel_lift_before_obstacle: false,
            avoid_crossing_perimeters_max_detour: 0.0,
            thin_walls: true,
            min_feature_size: -1.0,
            infill_anchor: -1.0,
            infill_anchor_max: 50.0,
            min_layer_height: 0.07,
            max_layer_height: 0.0,
            first_layer_height: 0.2,
            walls: 3,
            infill_density: 0.15,
            infill_pattern: "rectilinear".to_owned(),
            top_layers: 5,
            bottom_layers: 4,
            nozzle_temp: 210.0,
            first_nozzle_temp: 215.0,
            bed_temp: 60.0,
            first_layer_bed_temp: 0.0,
            chamber_temp: 0.0,
            fan_speed: 100.0,
            cooling: true,
            fan_always_on: false,
            initial_layer_fan_speed: -1.0,
            ironing_fan_speed: -1.0,
            support_interface_fan_speed: -1.0,
            dynamic_fan_speeds: false,
            overhang_fan_speeds: [0.0; 4],
            fan_start_layer: 2,
            retract_len: 0.8,
            retract_speed: 35.0,
            retract_min_travel: 1.5,
            infill_retract_min_travel: 12.0,
            z_hop: 0.0,
            travel_speed: 150.0,
            travel_speed_z: 0.0,
            dynamic_overhang_speeds: false,
            overhang_speeds: [0.0; 4],
            first_layer_speed: 20.0,
            first_layer_perimeter_speed: 0.0,
            first_layer_outer_speed: 0.0,
            first_layer_inner_speed: 0.0,
            first_layer_thin_speed: 0.0,
            first_layer_infill_speed: 0.0,
            first_layer_solid_speed: 0.0,
            first_layer_top_solid_speed: 0.0,
            first_layer_support_speed: 0.0,
            outer_wall_speed: 40.0,
            inner_wall_speed: 60.0,
            thin_speed: 30.0,
            infill_speed: 80.0,
            solid_speed: 60.0,
            top_solid_speed: 0.0,
            support_speed: 60.0,
            support_interface_speed: 40.0,
            support: false,
            support_threshold_angle: 60.0,
            support_xy_gap: 0.35,
            support_z_gap: 0.2,
            support_interface_layers: 2,
            support_spacing: -1.0,
            support_interface_spacing: -1.0,
            support_density: 0.15,
            support_interface_density: 0.8,
            support_on_build_plate_only: false,
            support_min_area: 2.0,
            support_style: "normal".to_owned(),
            support_branch_angle: 40.0,
            support_branch_distance: 3.0,
            support_tip_diameter: 0.8,
            support_branch_diameter_angle: 2.0,
            support_max_branch_diameter: 8.0,
            max_volumetric_speed: 12.0,
            max_volumetric_rate_slope_positive: 0.0,
            max_volumetric_rate_slope_negative: 0.0,
            max_speed: 300.0,
            min_layer_time: 8.0,
            min_speed_factor: 0.25,
            min_print_speed: 0.0,
            cooling_slowdown_logic: "uniform".to_owned(),
            cooling_perimeter_transition_distance: 0.0,
            flow: 1.0,
            seam: Seam::Aligned,
            inner_walls_first: true,
            ensure_vertical_shell_thickness: "enabled".to_owned(),
            skirt_loops: 2,
            skirt_distance: 3.0,
            brim_loops: 2,
            brim_width: 0.0,
            brim_gap: 0.1,
            brim_type: "outer_only".to_owned(),
            brim_separation: 0.0,
            min_skirt_length: 0.0,
            elefant_foot_compensation: 0.0,
            xy_size_compensation: 0.0,
            slice_closing_radius: 0.049,
            resolution: 0.0,
            raft_layers: 0,
            raft_first_layer_density: 0.9,
            raft_first_layer_expansion: 2.0,
            raft_contact_distance: 0.1,
            draft_shield: "disabled".to_owned(),
            draft_shield_height: 0.0,
            travel_slope: 0.0,
            travel_ramping_lift: false,
            travel_max_lift: 0.0,
            retract_lift_above: 0.0,
            retract_lift_below: 0.0,
            seam_gap: 0.0,
            staggered_inner_seams: false,
            scarf_seam_length: 0.0,
            scarf_seam_max_segment: 0.8,
            scarf_seam_inner: false,
            scarf_seam_placement: "nowhere".to_owned(),
            scarf_seam_entire_loop: false,
            scarf_seam_only_on_smooth: true,
            scarf_seam_start_height: 0.0,
            support_pattern: "rectilinear".to_owned(),
            support_interface_pattern: String::new(),
            support_interface_loops: 0,
            support_interface_shells: false,
            support_sheath: false,
            support_first_layer_expansion: 0.0,
            support_enforce_layers: 0,
            support_bottom_interface_layers: 0,
            support_tree_angle_slow: 40.0,
            support_tree_top_rate: 30.0,
            support_tree_double_wall: false,
            support_closing_radius: 0.0,
            support_bottom_contact_distance: 0.0,
            support_skip_bridges: false,
            support_first_layer_density: 0.0,
            fuzzy_skin: "none".to_owned(),
            fuzzy_skin_thickness: 0.3,
            fuzzy_skin_point_dist: 0.8,
            bridge_speed: 30.0,
            bridge_flow: 1.0,
            bridge_density: 1.0,
            bridge_line_width: 0.0,
            wall_width: 0.0,
            outer_wall_width: 0.0,
            infill_width: 0.0,
            solid_infill_width: 0.0,
            first_layer_width: 0.0,
            top_infill_width: 0.0,
            automatic_extrusion_widths: false,
            top_fill_pattern: "monotonic".to_owned(),
            bottom_fill_pattern: "monotonic".to_owned(),
            solid_infill_every_layers: 0,
            solid_infill_below_area: 70.0,
            filament_max_volumetric_speed: 0.0,
            filament_infill_max_speed: 0.0,
            filament_infill_max_crossing_speed: 0.0,
            over_bridge_speed: 0.0,
            internal_bridge_fan_speed: -1.0,
            enable_overhang_bridge_fan: true,
            retract_before_wipe: 100.0,
            only_retract_when_crossing_perimeters: false,
            raft_expansion: 1.5,
            first_layer_speed_over_raft: 30.0,
            first_layer_acceleration_over_raft: 0.0,
            only_one_perimeter_first_layer: false,
            top_one_perimeter_type: "none".to_owned(),
            travel_acceleration: 0.0,
            travel_short_distance_acceleration: 0.0,
            machine_limits_usage: "emit_to_gcode".to_owned(),
            stuck_filament_detection: false,
            shrinkage_xy: 0.0,
            shrinkage_z: 0.0,
            perimeter_generator: "arachne".to_owned(),
            wall_transition_length: 0.0,
            wall_transition_angle: 0.0,
            wall_transition_filter_deviation: 0.0,
            overhangs: true,
            chamber_minimal_temperature: 0.0,
            infill_every_layers: 1,
            automatic_infill_combination: false,
            infill_combination_max_layer_height: 0.0,
            wall_distribution_count: 1,
            gap_fill_enabled: true,
            infill_first: false,
            extra_perimeters: true,
            extra_perimeters_on_overhangs: false,
            support_width: 0.0,
            support_interface_width: 0.0,
            arc_fitting: false,
            arc_fitting_tolerance: 0.025,
            ironing: false,
            ironing_all_solid: false,
            ironing_type: "top_surfaces".to_owned(),
            ironing_flow: 0.15,
            ironing_spacing: 0.1,
            ironing_speed: 15.0,
            bridge_angle: -1.0,
            thick_bridges: false,
            bridge_cross_hatch: false,
            bridge_fan_speed: 100.0,
            bridge_acceleration: 0.0,
            max_bridge_length: 0.0,
            bridge_span_flow: 0.0,
            bridge_span_speed: 0.0,
            bridge_span_ref: 5.0,
            emit_print_stats: false,
            gcode_comments: true,
            label_objects: true,
            gcode_resolution: 0.0125,
            volumetric_e: false,
            firmware_retraction: false,
            clockwise: false,
            remaining_times: false,
            autoemit_temperatures: true,
            before_layer_gcode: String::new(),
            layer_gcode: String::new(),
            start_filament_gcode: String::new(),
            end_filament_gcode: String::new(),
            thumbnails: Vec::new(),
            exclude_object: false,
            pressure_advance: 0.0,
            default_accel: 0.0,
            outer_wall_accel: 0.0,
            inner_wall_accel: 0.0,
            perimeter_accel: 0.0,
            top_solid_accel: 0.0,
            initial_layer_accel: 0.0,
            solid_accel: 0.0,
            infill_accel: 0.0,
            support_accel: 0.0,
            support_interface_accel: 0.0,
            machine_max_accel_x: 0.0,
            machine_max_accel_y: 0.0,
            machine_max_accel_z: 0.0,
            machine_max_accel_e: 0.0,
            machine_max_feedrate_x: 0.0,
            machine_max_feedrate_y: 0.0,
            machine_max_feedrate_z: 0.0,
            machine_max_feedrate_e: 0.0,
            machine_max_jerk_x: 0.0,
            machine_max_jerk_y: 0.0,
            machine_max_jerk_z: 0.0,
            machine_max_jerk_e: 0.0,
            machine_max_accel_extruding: 0.0,
            machine_max_accel_retracting: 0.0,
            machine_max_accel_travel: 0.0,
            machine_max_junction_deviation: 0.0,
            machine_min_extruding_rate: 0.0,
            machine_min_travel_rate: 0.0,
            z_offset: 0.0,
            fill_angle: 45.0,
            min_fan_speed: 0.0,
            full_fan_speed_layer: 0,
            fan_below_layer_time: 0.0,
            slow_down_layer_time: 0.0,
            slow_down_min_speed: 0.0,
            skirt_height: 1,
            print_sequence: PrintSequence::ByLayer,
            gantry_height: 0.0,
            idle_temp: 0.0,
            deretract_speed: 0.0,
            retract_restart_extra: 0.0,
            retract_layer_change: true,
            wipe: false,
            wipe_distance: 2.0,
            start_gcode:
                "M140 S{bed_temp}\nM104 S150\nG28\nM190 S{bed_temp}\nM109 S{first_nozzle_temp}"
                    .to_owned(),
            end_gcode: "M104 S0\nM140 S0\nM107\nM84".to_owned(),
            between_objects_gcode: String::new(),
            pause_print_gcode: String::new(),
            pause_heights: Vec::new(),
            colour_change_heights: Vec::new(),
            color_change_gcode: String::new(),
            gcode_substitutions: Vec::new(),
            silent_mode: false,
            printer_model: String::new(),
            printer_variant: String::new(),
            printer_vendor: String::new(),
            printer_notes: String::new(),
            notes: String::new(),
            filament_type: String::new(),
            filament_vendor: String::new(),
            filament_colour: String::new(),
            filament_notes: String::new(),
            filament_soluble: false,
            filament_abrasive: false,
            filament_cost: 0.0,
            filament_spool_weight: 0.0,
            output_filename_format: String::new(),
            post_process: String::new(),
            custom_parameters_print: String::new(),
            custom_parameters_filament: String::new(),
            custom_parameters_printer: String::new(),
            template_custom_gcode: String::new(),
            default_print: String::new(),
            default_material: String::new(),
            default_tool_print: String::new(),
            infill_overlap: -1.0,
            top_solid_min_thickness: 0.0,
            bottom_solid_min_thickness: 0.0,
        }
    }
}

impl PrinterConfig {
    /// Thickest layer this machine prints, resolving `0` to 80% of the nozzle.
    pub fn max_layer(&self) -> f64 {
        if self.max_layer_height > 0.0 {
            self.max_layer_height
        } else {
            0.8 * self.nozzle_diameter
        }
    }

    /// Cross-sectional area of the filament, in square millimetres.
    pub fn filament_area(&self) -> f64 {
        std::f64::consts::PI * 0.25 * self.filament_diameter * self.filament_diameter
    }

    /// True when a point lies on the bed.
    ///
    /// An empty outline falls back to the rectangle. The test counts crossings of
    /// a ray fired along +X, which puts the answer on the inside for every
    /// winding; a point exactly on an edge can land either way and that is fine,
    /// because the callers allow a two millimetre margin.
    pub fn bed_contains(&self, x: f64, y: f64) -> bool {
        if self.bed_shape.len() < 3 {
            return x >= 0.0 && y >= 0.0 && x <= self.bed_x && y <= self.bed_y;
        }
        let n = self.bed_shape.len();
        let mut inside = false;
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = self.bed_shape[i];
            let (xj, yj) = self.bed_shape[j];
            if (yi > y) != (yj > y) {
                let t = (y - yi) / (yj - yi);
                if x < xi + t * (xj - xi) {
                    inside = !inside;
                }
            }
            j = i;
        }
        inside
    }

    /// Whether a point is within \p margin of the bed, measuring outward from every
    /// edge of the outline.
    ///
    /// A machine extrudes just past the outline as a matter of course -- a Prusa
    /// profile primes along Y -2.5 before the first layer -- so "inside the bed" as
    /// a limit on plastic is the outline grown by the width of a prime line, not the
    /// outline itself.
    pub fn bed_contains_within(&self, x: f64, y: f64, margin: f64) -> bool {
        if self.bed_contains(x, y) {
            return true;
        }
        let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        for (px, py) in &self.bed_shape {
            min_x = min_x.min(*px);
            max_x = max_x.max(*px);
            min_y = min_y.min(*py);
            max_y = max_y.max(*py);
        }
        if self.bed_shape.len() < 3 {
            (min_x, max_x, min_y, max_y) = (0.0, self.bed_x, 0.0, self.bed_y);
        }
        x >= min_x - margin && x <= max_x + margin && y >= min_y - margin && y <= max_y + margin
    }

    /// The bed outline as `{bed_shape}` spells it: `0x0,350x0,350x350`.
    pub fn bed_shape_text(&self) -> String {
        if self.bed_shape.is_empty() {
            return format!(
                "0x0,{}x0,{}x{},0x{}",
                trim(self.bed_x),
                trim(self.bed_x),
                trim(self.bed_y),
                trim(self.bed_y)
            );
        }
        self.bed_shape
            .iter()
            .map(|&(x, y)| format!("{}x{}", trim(x), trim(y)))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Substitutes `{placeholders}` in a G-code fragment.
    pub fn fill(&self, text: &str, extra: &[(&str, String)]) -> String {
        let mut out = text.to_owned();
        let base = [
            ("{bed_temp}", format!("{}", self.bed_temp)),
            ("{nozzle_temp}", format!("{}", self.nozzle_temp)),
            ("{first_nozzle_temp}", format!("{}", self.first_nozzle_temp)),
            ("{bed_x}", format!("{}", self.bed_x)),
            ("{bed_y}", format!("{}", self.bed_y)),
            ("{bed_shape}", self.bed_shape_text()),
            ("{max_z}", format!("{}", self.max_z)),
            ("{pressure_advance}", format!("{}", self.pressure_advance)),
        ];
        for (key, value) in base.iter().chain(extra.iter()) {
            out = out.replace(key, value);
        }
        // `layer_z`, `layer_num` and the plate's settings are all in hand here, so
        // this is the pass where a reference profile's `{if layer_z > 150}` and its
        // `interpolate_table(layer_z, …)` become real commands. An expression still
        // unresolved here stays as written, and the verifier reports it.
        let values: Vec<(String, String)> = base
            .iter()
            .chain(extra.iter())
            .map(|(key, value)| {
                (
                    key.trim_matches(|c| c == '{' || c == '}').to_owned(),
                    value.clone(),
                )
            })
            .collect();
        let lookup = |name: &str, index: Option<usize>| -> Option<String> {
            values
                .iter()
                .find(|(key, _)| key == name)
                .and_then(|(_, value)| match index {
                    Some(index) => value
                        .split(',')
                        .nth(index)
                        .map(|part| part.trim().to_owned()),
                    None => Some(value.clone()),
                })
                .or_else(|| crate::dialect::intrinsic(self, name))
        };
        let (expanded, _) = crate::expression::expand(&out, &lookup);
        expanded
    }
}
