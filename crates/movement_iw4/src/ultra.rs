//! ULTRAKILL's V1 movement, run in place of IW4's walk and air moves while the
//! command carries `buttons::ULTRA`.
//!
//! Not a port: ULTRAKILL's code isn't used. The moves (dash, slide, slam, slam
//! bounce, wall jump) and their feel come from playing the game, and the
//! numbers below are tuned to IW4's units (1 unit = 1 inch, gravity 800).
//! Collision and ground detection stay IW4's own.

use playerstate_iw4::{ENTITYNUM_NONE, PlayerState, UserCmd, buttons, ultra_flags};

use crate::{
    CollisionBackend, GroundTraceInput, MoveBounds, Pml, project_velocity, step_slide_move,
};

/// Top running speed. IW4 runs at 190.
pub const RUN_SPEED: f32 = 480.0;
const GROUND_ACCEL: f32 = 14.0;
const GROUND_FRICTION: f32 = 10.0;
const AIR_ACCEL: f32 = 4.0;
pub const JUMP_SPEED: f32 = 420.0;

pub const STAMINA_PER_DASH: i32 = 100;
pub const STAMINA_MAX: i32 = 3 * STAMINA_PER_DASH;
/// Stamina regained per second, so one charge takes ~0.7 s.
const STAMINA_REGEN: i32 = 150;
pub const DASH_SPEED: f32 = 1500.0;
pub const DASH_MS: i32 = 170;
/// A jump during a dash keeps this much of the dash's speed.
const DASH_JUMP_KEEP: f32 = 0.85;

pub const SLIDE_SPEED: f32 = 620.0;

pub const SLAM_SPEED: f32 = 2400.0;
/// A jump this soon after a slam lands is a slam bounce.
pub const SLAM_BOUNCE_WINDOW_MS: i32 = 200;
const SLAM_BOUNCE_MAX: f32 = 1500.0;

pub const WALL_JUMPS: i32 = 3;
const WALL_JUMP_UP: f32 = 460.0;
const WALL_JUMP_OUT: f32 = 420.0;
/// How close a wall must be, beyond the player's hull, to jump off it.
const WALL_REACH: f32 = 20.0;
/// Walls are surfaces steeper than this normal z.
const WALL_MAX_NORMAL_Z: f32 = 0.7;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UltraMoveContext {
    pub old_buttons: u32,
}

/// Whether this command moves as V1.
#[must_use]
pub fn active(cmd: &UserCmd) -> bool {
    cmd.buttons & buttons::ULTRA != 0
}

/// Leaving the mode drops every V1 state, so IW4 movement resumes clean.
pub fn clear(ps: &mut PlayerState) {
    ps.ultra_flags = 0;
    ps.ultra_stamina = 0;
    ps.ultra_dash_time = 0;
    ps.ultra_wall_jumps = 0;
    ps.ultra_slam_time = 0;
    ps.ultra_slam_height = 0.0;
}

/// Entering the mode starts with full stamina.
pub fn begin(ps: &mut PlayerState) {
    if ps.ultra_flags & ultra_flags::ACTIVE == 0 {
        clear(ps);
        ps.ultra_flags = ultra_flags::ACTIVE;
        ps.ultra_stamina = STAMINA_MAX;
    }
}

/// The buttons IW4's own stance and sprint code should see: V1 never sprints
/// (sprint is the dash) and never goes prone (prone is the slide too).
#[must_use]
pub fn iw4_buttons(buttons: u32) -> u32 {
    let mut out = buttons & !(buttons::SPRINT | buttons::PRONE);
    if buttons & buttons::PRONE != 0 {
        out |= buttons::CROUCH;
    }
    out
}

fn slide_held(cmd: &UserCmd) -> bool {
    cmd.buttons & (buttons::CROUCH | buttons::PRONE) != 0
}

#[allow(clippy::too_many_lines)]
pub fn ultra_move<C: CollisionBackend>(
    ps: &mut PlayerState,
    pml: &mut Pml,
    cmd: &UserCmd,
    context: UltraMoveContext,
    bounds: MoveBounds,
    collision: &C,
) {
    let dt = pml.frametime;
    let msec = pml.msec;
    let grounded = pml.walking != 0;

    ps.ultra_stamina = (ps.ultra_stamina + STAMINA_REGEN * msec / 1000).min(STAMINA_MAX);

    let dash_held = cmd.buttons & buttons::SPRINT != 0;
    let dash_pressed = dash_held && ps.ultra_flags & ultra_flags::DASH_HELD == 0;
    if dash_held {
        ps.ultra_flags |= ultra_flags::DASH_HELD;
    } else {
        ps.ultra_flags &= !ultra_flags::DASH_HELD;
    }
    let jump_pressed = cmd.buttons & buttons::JUMP != 0 && context.old_buttons & buttons::JUMP == 0;
    let slide = slide_held(cmd);
    let slide_pressed = slide && context.old_buttons & (buttons::CROUCH | buttons::PRONE) == 0;

    let wish = wish_dir(pml, cmd);
    let facing = horizontal(pml.forward);

    if dash_pressed && ps.ultra_stamina >= STAMINA_PER_DASH {
        ps.ultra_stamina -= STAMINA_PER_DASH;
        ps.ultra_dash_time = DASH_MS;
        ps.ultra_flags &= !(ultra_flags::SLIDING | ultra_flags::SLAMMING);
        ps.ultra_slam_height = 0.0;
        let dir = wish.unwrap_or(facing);
        ps.velocity = [dir[0] * DASH_SPEED, dir[1] * DASH_SPEED, 0.0];
    }

    if ps.ultra_dash_time > 0 {
        ps.ultra_dash_time = (ps.ultra_dash_time - msec).max(0);
        if grounded && jump_pressed {
            ps.ultra_dash_time = 0;
            ps.velocity[0] *= DASH_JUMP_KEEP;
            ps.velocity[1] *= DASH_JUMP_KEEP;
            launch(ps, pml, JUMP_SPEED);
            fly(ps, pml, bounds, collision);
            return;
        }
        // Keep the dash's direction and speed; a wall that stops it ends it.
        let speed = horizontal_speed(ps.velocity);
        if speed < 1.0 {
            ps.ultra_dash_time = 0;
        } else {
            ps.velocity[0] *= DASH_SPEED / speed;
            ps.velocity[1] *= DASH_SPEED / speed;
            ps.velocity[2] = 0.0;
            step_slide_move(
                ps,
                pml,
                collision,
                bounds.mins,
                bounds.maxs,
                bounds.tracemask,
                None,
            );
            return;
        }
    }

    if grounded {
        ps.ultra_wall_jumps = 0;
        if ps.ultra_flags & ultra_flags::SLAMMING != 0 {
            ps.ultra_flags &= !ultra_flags::SLAMMING;
            ps.ultra_slam_time = cmd.server_time;
            ps.ultra_slam_height = (ps.ultra_slam_height - ps.origin[2]).max(0.0);
        }

        if slide {
            if ps.ultra_flags & ultra_flags::SLIDING == 0 {
                ps.ultra_flags |= ultra_flags::SLIDING;
                let dir = wish.unwrap_or(facing);
                let speed = horizontal_speed(ps.velocity).max(SLIDE_SPEED);
                ps.velocity[0] = dir[0] * speed;
                ps.velocity[1] = dir[1] * speed;
            }
        } else {
            ps.ultra_flags &= !ultra_flags::SLIDING;
        }

        if jump_pressed {
            ps.ultra_flags &= !ultra_flags::SLIDING;
            let since_slam = cmd.server_time.wrapping_sub(ps.ultra_slam_time);
            let bounce =
                ps.ultra_slam_height > 0.0 && (0..=SLAM_BOUNCE_WINDOW_MS).contains(&since_slam);
            let speed = if bounce {
                slam_bounce_speed(ps)
            } else {
                JUMP_SPEED
            };
            ps.ultra_slam_height = 0.0;
            launch(ps, pml, speed);
            fly(ps, pml, bounds, collision);
            return;
        }

        if ps.ultra_flags & ultra_flags::SLIDING != 0 {
            // A slide holds its speed and direction until crouch is released.
            let speed = horizontal_speed(ps.velocity);
            if speed < SLIDE_SPEED {
                let dir = if speed > 1.0 {
                    [ps.velocity[0] / speed, ps.velocity[1] / speed, 0.0]
                } else {
                    facing
                };
                ps.velocity[0] = dir[0] * SLIDE_SPEED;
                ps.velocity[1] = dir[1] * SLIDE_SPEED;
            }
        } else {
            ground_friction(ps, dt);
            if let Some(dir) = wish {
                crate::accelerate(ps, pml, &dir, RUN_SPEED, GROUND_ACCEL);
            }
        }

        let normal = ground_normal(pml);
        project_velocity(&mut ps.velocity, &normal);
        if ps.velocity[0] != 0.0 || ps.velocity[1] != 0.0 {
            step_slide_move(
                ps,
                pml,
                collision,
                bounds.mins,
                bounds.maxs,
                bounds.tracemask,
                None,
            );
        }
        return;
    }

    ps.ultra_flags &= !ultra_flags::SLIDING;

    if slide_pressed && ps.ultra_flags & ultra_flags::SLAMMING == 0 {
        ps.ultra_flags |= ultra_flags::SLAMMING;
        // Remember where the slam started; landing turns it into a height.
        ps.ultra_slam_height = ps.origin[2];
    }
    if ps.ultra_flags & ultra_flags::SLAMMING != 0 {
        ps.velocity = [0.0, 0.0, -SLAM_SPEED];
        step_slide_move(
            ps,
            pml,
            collision,
            bounds.mins,
            bounds.maxs,
            bounds.tracemask,
            None,
        );
        return;
    }

    if jump_pressed
        && ps.ultra_wall_jumps < WALL_JUMPS
        && let Some(normal) = nearby_wall(ps, bounds, collision)
    {
        ps.ultra_wall_jumps += 1;
        ps.velocity[0] = ps.velocity[0] * 0.3 + normal[0] * WALL_JUMP_OUT;
        ps.velocity[1] = ps.velocity[1] * 0.3 + normal[1] * WALL_JUMP_OUT;
        ps.velocity[2] = WALL_JUMP_UP;
    } else if let Some(dir) = wish {
        crate::accelerate(ps, pml, &dir, RUN_SPEED, AIR_ACCEL);
    }
    fly(ps, pml, bounds, collision);
}

/// Stamina as charges, for the HUD: `(full charges, fraction of the next)`.
#[must_use]
pub fn stamina_charges(ps: &PlayerState) -> (i32, f32) {
    let stamina = ps.ultra_stamina.clamp(0, STAMINA_MAX);
    (
        stamina / STAMINA_PER_DASH,
        (stamina % STAMINA_PER_DASH) as f32 / STAMINA_PER_DASH as f32,
    )
}

fn slam_bounce_speed(ps: &PlayerState) -> f32 {
    // Higher slams bounce higher: half again the speed of falling that far.
    let fall = libm::sqrtf(2.0 * ps.gravity.max(1) as f32 * ps.ultra_slam_height);
    (JUMP_SPEED + fall * 0.5).min(SLAM_BOUNCE_MAX)
}

fn launch(ps: &mut PlayerState, pml: &mut Pml, speed: f32) {
    ps.velocity[2] = speed;
    ps.ground_entity_num = ENTITYNUM_NONE;
    pml.walking = 0;
    pml.ground_plane = 0;
    pml.almost_ground_plane = 0;
    pml.record_jump_animation(crate::JumpAnimation::Forward, false);
}

fn fly<C: CollisionBackend>(ps: &mut PlayerState, pml: &Pml, bounds: MoveBounds, collision: &C) {
    let gravity = ps.gravity as f32;
    step_slide_move(
        ps,
        pml,
        collision,
        bounds.mins,
        bounds.maxs,
        bounds.tracemask,
        Some(gravity),
    );
}

fn ground_friction(ps: &mut PlayerState, dt: f32) {
    let speed = horizontal_speed(ps.velocity);
    if speed < 1.0 {
        ps.velocity[0] = 0.0;
        ps.velocity[1] = 0.0;
        return;
    }
    let drop = speed.max(100.0) * GROUND_FRICTION * dt;
    let scale = (speed - drop).max(0.0) / speed;
    ps.velocity[0] *= scale;
    ps.velocity[1] *= scale;
}

fn nearby_wall<C: CollisionBackend>(
    ps: &PlayerState,
    bounds: MoveBounds,
    collision: &C,
) -> Option<[f32; 3]> {
    const DIRS: [[f32; 2]; 8] = [
        [1.0, 0.0],
        [-1.0, 0.0],
        [0.0, 1.0],
        [0.0, -1.0],
        [
            core::f32::consts::FRAC_1_SQRT_2,
            core::f32::consts::FRAC_1_SQRT_2,
        ],
        [
            core::f32::consts::FRAC_1_SQRT_2,
            -core::f32::consts::FRAC_1_SQRT_2,
        ],
        [
            -core::f32::consts::FRAC_1_SQRT_2,
            core::f32::consts::FRAC_1_SQRT_2,
        ],
        [
            -core::f32::consts::FRAC_1_SQRT_2,
            -core::f32::consts::FRAC_1_SQRT_2,
        ],
    ];
    let mut best: Option<(f32, [f32; 3])> = None;
    for dir in DIRS {
        let trace = collision.trace(GroundTraceInput {
            start: ps.origin,
            end: [
                ps.origin[0] + dir[0] * WALL_REACH,
                ps.origin[1] + dir[1] * WALL_REACH,
                ps.origin[2],
            ],
            mins: bounds.mins,
            maxs: bounds.maxs,
            tracemask: bounds.tracemask,
        });
        if trace.startsolid != 0
            || trace.fraction >= 1.0
            || trace.normal[2].abs() > WALL_MAX_NORMAL_Z
        {
            continue;
        }
        if best.is_none_or(|(fraction, _)| trace.fraction < fraction) {
            let flat = horizontal(trace.normal);
            best = Some((trace.fraction, flat));
        }
    }
    best.map(|(_, normal)| normal)
}

fn wish_dir(pml: &Pml, cmd: &UserCmd) -> Option<[f32; 3]> {
    let forward = horizontal(pml.forward);
    let right = horizontal(pml.right);
    let f = f32::from(cmd.forwardmove);
    let r = f32::from(cmd.rightmove);
    let dir = [
        forward[0] * f + right[0] * r,
        forward[1] * f + right[1] * r,
        0.0,
    ];
    let len = libm::sqrtf(dir[0] * dir[0] + dir[1] * dir[1]);
    (len > 0.0).then(|| [dir[0] / len, dir[1] / len, 0.0])
}

fn horizontal(v: [f32; 3]) -> [f32; 3] {
    let len = libm::sqrtf(v[0] * v[0] + v[1] * v[1]);
    if len <= 0.0 {
        [1.0, 0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, 0.0]
    }
}

fn horizontal_speed(v: [f32; 3]) -> f32 {
    libm::sqrtf(v[0] * v[0] + v[1] * v[1])
}

fn ground_normal(pml: &Pml) -> [f32; 3] {
    [
        f32::from_bits(pml.ground_trace[1]),
        f32::from_bits(pml.ground_trace[2]),
        f32::from_bits(pml.ground_trace[3]),
    ]
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    use super::*;
    use crate::complete_ground_trace;
    use trace_iw4::Trace;

    const MSEC: i32 = 16;
    const BOUNDS: MoveBounds = MoveBounds {
        mins: [-15.0, -15.0, 0.0],
        maxs: [15.0, 15.0, 70.0],
        tracemask: 1,
    };

    /// Axis-aligned boxes, swept against the player's hull.
    struct Boxes(Vec<([f32; 3], [f32; 3])>);

    impl Boxes {
        fn floor() -> Self {
            Self(vec![(
                [-100_000.0, -100_000.0, -64.0],
                [100_000.0, 100_000.0, 0.0],
            )])
        }
    }

    impl CollisionBackend for Boxes {
        fn trace(&self, input: GroundTraceInput) -> Trace {
            let delta = [
                input.end[0] - input.start[0],
                input.end[1] - input.start[1],
                input.end[2] - input.start[2],
            ];
            let mut best = Trace {
                fraction: 1.0,
                endpos: input.end,
                ..Trace::default()
            };
            for (bmin, bmax) in &self.0 {
                // Minkowski: grow the box by the hull, sweep the origin as a ray.
                let lo: [f32; 3] = core::array::from_fn(|i| bmin[i] - input.maxs[i]);
                let hi: [f32; 3] = core::array::from_fn(|i| bmax[i] - input.mins[i]);
                let inside = (0..3).all(|i| input.start[i] > lo[i] && input.start[i] < hi[i]);
                if inside {
                    best.startsolid = 1;
                    best.allsolid = 1;
                    best.fraction = 0.0;
                    best.endpos = input.start;
                    return best;
                }
                let mut enter = -1.0_f32;
                let mut exit = 1.0_f32;
                let mut normal = [0.0; 3];
                let mut hit = true;
                for i in 0..3 {
                    if delta[i] == 0.0 {
                        if input.start[i] <= lo[i] || input.start[i] >= hi[i] {
                            hit = false;
                        }
                        continue;
                    }
                    let (near, far, n) = if delta[i] > 0.0 {
                        (lo[i], hi[i], -1.0)
                    } else {
                        (hi[i], lo[i], 1.0)
                    };
                    let t0 = (near - input.start[i]) / delta[i];
                    let t1 = (far - input.start[i]) / delta[i];
                    if t0 > enter {
                        enter = t0;
                        normal = [0.0; 3];
                        normal[i] = n;
                    }
                    exit = exit.min(t1);
                }
                if !hit || enter > exit || enter < 0.0 || enter >= best.fraction {
                    continue;
                }
                let len = libm::sqrtf(delta.iter().map(|d| d * d).sum());
                let fraction = (enter - 0.03125 / len.max(1e-6)).max(0.0);
                best.fraction = fraction;
                best.normal = normal;
                best.contents = 1;
                best.hit_type = trace_iw4::HITTYPE_ENTITY;
                best.hit_id = trace_iw4::ENTITYNUM_WORLD;
                best.walkable = u8::from(normal[2] >= 0.7);
                best.endpos = core::array::from_fn(|i| input.start[i] + delta[i] * fraction);
            }
            best
        }
    }

    fn player(origin: [f32; 3]) -> PlayerState {
        let mut ps = PlayerState::ZERO;
        ps.origin = origin;
        ps.gravity = 800;
        ps.ground_entity_num = ENTITYNUM_NONE;
        ps.ultra_flags = ultra_flags::ACTIVE;
        ps.ultra_stamina = STAMINA_MAX;
        ps
    }

    struct Sim {
        ps: PlayerState,
        world: Boxes,
        old_buttons: u32,
        time: i32,
    }

    impl Sim {
        fn new(origin: [f32; 3], world: Boxes) -> Self {
            let mut sim = Self {
                ps: player(origin),
                world,
                old_buttons: 0,
                time: 1000,
            };
            for _ in 0..60 {
                sim.tick(0, 0, 0);
            }
            sim
        }

        fn tick(&mut self, forward: i8, right: i8, held: u32) {
            self.time += MSEC;
            let cmd = UserCmd {
                server_time: self.time,
                buttons: held | buttons::ULTRA,
                forwardmove: forward,
                rightmove: right,
                ..UserCmd::default()
            };
            let (f, r, u) = math_iw4::angle_vectors(self.ps.viewangles);
            let mut pml = Pml {
                forward: f,
                right: r,
                up: u,
                frametime: MSEC as f32 * 0.001,
                msec: MSEC,
                walking: 0,
                ground_plane: 0,
                almost_ground_plane: 0,
                ground_trace: [0; 11],
                previous_origin: self.ps.origin,
                previous_velocity: self.ps.velocity,
                holdrand: 0,
                jump_animations: [None; 4],
                mantle_movetype: None,
                landing_animation: false,
                ultra: true,
            };
            complete_ground_trace(&mut self.ps, &mut pml, BOUNDS, forward, &self.world);
            ultra_move(
                &mut self.ps,
                &mut pml,
                &cmd,
                UltraMoveContext {
                    old_buttons: self.old_buttons,
                },
                BOUNDS,
                &self.world,
            );
            complete_ground_trace(&mut self.ps, &mut pml, BOUNDS, forward, &self.world);
            self.old_buttons = cmd.buttons;
        }

        fn run(&mut self, ticks: usize, forward: i8, right: i8, held: u32) {
            for _ in 0..ticks {
                self.tick(forward, right, held);
            }
        }

        fn grounded(&self) -> bool {
            self.ps.ground_entity_num != ENTITYNUM_NONE
        }

        fn speed(&self) -> f32 {
            horizontal_speed(self.ps.velocity)
        }
    }

    #[test]
    fn settles_on_the_floor() {
        let sim = Sim::new([0.0, 0.0, 40.0], Boxes::floor());
        assert!(sim.ps.origin[2] < 20.0, "z={}", sim.ps.origin[2]);
    }

    #[test]
    fn runs_at_v1_speed() {
        let mut sim = Sim::new([0.0, 0.0, 0.0], Boxes::floor());
        sim.run(30, 127, 0, 0);
        assert!(sim.grounded());
        assert!(
            (sim.speed() - RUN_SPEED).abs() < 5.0,
            "speed={}",
            sim.speed()
        );
        sim.run(30, 0, 0, 0);
        assert!(sim.speed() < 1.0, "stops quickly: speed={}", sim.speed());
    }

    #[test]
    fn dash_costs_a_charge_and_keeps_its_speed() {
        let mut sim = Sim::new([0.0, 0.0, 0.0], Boxes::floor());
        let start = sim.ps.origin[0];
        sim.tick(0, 0, buttons::SPRINT);
        assert!(
            (sim.speed() - DASH_SPEED).abs() < 1.0,
            "speed={}",
            sim.speed()
        );
        assert!(sim.ps.ultra_stamina < STAMINA_MAX - STAMINA_PER_DASH + 10);
        sim.run(10, 0, 0, buttons::SPRINT);
        let travelled = sim.ps.origin[0] - start;
        assert!(travelled > 200.0, "dash distance {travelled}");
        assert_eq!(sim.ps.ultra_dash_time, 0);
    }

    #[test]
    fn three_dashes_then_empty() {
        let mut sim = Sim::new([0.0, 0.0, 0.0], Boxes::floor());
        for _ in 0..3 {
            sim.tick(0, 0, buttons::SPRINT);
            assert!(sim.ps.ultra_dash_time > 0);
            sim.tick(0, 0, 0);
        }
        sim.run(12, 0, 0, 0);
        sim.tick(0, 0, buttons::SPRINT);
        assert_eq!(
            sim.ps.ultra_dash_time, 0,
            "no stamina left for a fourth dash"
        );
    }

    #[test]
    fn jump_reaches_about_110_units() {
        let mut sim = Sim::new([0.0, 0.0, 0.0], Boxes::floor());
        sim.tick(0, 0, buttons::JUMP);
        let mut apex: f32 = 0.0;
        for _ in 0..120 {
            sim.tick(0, 0, buttons::JUMP);
            apex = apex.max(sim.ps.origin[2]);
        }
        let expected = JUMP_SPEED * JUMP_SPEED / (2.0 * 800.0);
        assert!(
            (apex - expected).abs() < 8.0,
            "apex={apex} expected={expected}"
        );
        assert!(sim.grounded(), "holding jump doesn't bunny hop");
    }

    #[test]
    fn slide_holds_its_speed() {
        let mut sim = Sim::new([0.0, 0.0, 0.0], Boxes::floor());
        sim.run(120, 0, 0, buttons::CROUCH);
        assert!(sim.ps.ultra_flags & ultra_flags::SLIDING != 0);
        assert!(
            (sim.speed() - SLIDE_SPEED).abs() < 1.0,
            "speed={}",
            sim.speed()
        );
        sim.tick(0, 0, buttons::CROUCH | buttons::JUMP);
        assert!(!sim.grounded());
        assert!(
            sim.speed() > SLIDE_SPEED - 10.0,
            "slide jump keeps momentum: {}",
            sim.speed()
        );
    }

    #[test]
    fn slam_then_bounce_jumps_higher() {
        let mut sim = Sim::new([0.0, 0.0, 0.0], Boxes::floor());
        sim.ps.origin[2] = 400.0;
        sim.ps.ground_entity_num = ENTITYNUM_NONE;
        sim.ps.velocity = [300.0, 0.0, 0.0];
        sim.tick(0, 0, 0);
        sim.tick(0, 0, buttons::CROUCH);
        assert_eq!(sim.ps.velocity[0], 0.0);
        let mut ticks = 0;
        while !sim.grounded() {
            sim.tick(0, 0, buttons::CROUCH);
            ticks += 1;
            assert!(
                ticks < 30,
                "slam lands fast: {:?} {:?}",
                sim.ps.origin,
                sim.ps.velocity
            );
        }
        sim.tick(0, 0, 0);
        assert!(
            sim.ps.ultra_slam_height > 300.0,
            "height={}",
            sim.ps.ultra_slam_height
        );
        sim.tick(0, 0, buttons::JUMP);
        assert!(
            sim.ps.velocity[2] > JUMP_SPEED + 200.0,
            "bounce vz={}",
            sim.ps.velocity[2]
        );
    }

    #[test]
    fn wall_jumps_three_times_then_falls() {
        // A wall at x = 30..60, the player hugging it in the air.
        let mut world = Boxes::floor();
        world.0.push(([30.0, -500.0, 0.0], [60.0, 500.0, 2000.0]));
        let mut sim = Sim::new([0.0, 0.0, 0.0], world);
        sim.ps.origin = [10.0, 0.0, 600.0];
        sim.ps.ground_entity_num = ENTITYNUM_NONE;
        for jump in 0..4 {
            sim.ps.origin[0] = 10.0;
            sim.ps.velocity = [0.0, 0.0, -100.0];
            sim.tick(0, 0, 0);
            sim.tick(0, 0, buttons::JUMP);
            if jump < 3 {
                assert!(
                    sim.ps.velocity[0] < -300.0,
                    "pushed off the wall: {:?}",
                    sim.ps.velocity
                );
                assert!(sim.ps.velocity[2] > 300.0);
            } else {
                assert!(sim.ps.velocity[2] < 0.0, "fourth wall jump refused");
            }
        }
    }
}
