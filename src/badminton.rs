//! Badminton against Clawd, seen from the side. The whole screen is the court: a net stands on
//! the taskbar in the middle, your racket follows the mouse around the left half and Clawd runs
//! about the right half. Swinging through the shuttle adds the racket's speed to the shot. A rally
//! ends when the shuttle lands, flies out or drops off the net; the rally's winner serves next.

use super::*;

const WIN_POINTS: u32 = 7;
// Physics in court widths (W) per second.
const GRAVITY: f64 = 1.6;
const TERMINAL: f64 = 0.75; // a shuttlecock's top falling speed; the heavy drag is what makes its arcs drop steeply
const NET_HEIGHT: f64 = 0.1; // about a real court's proportions
const SERVE_HEIGHT: f64 = 0.15; // where your serve floats, above the floor
const SWING: f64 = 0.35; // share of your racket's speed that goes into the shot
const MAX_TILT: f64 = 1.0; // radians your racket leans at full speed
const FULL_TILT_SPEED: f64 = 0.6; // W/s of sideways cursor movement for the full lean
const TILT_EASE: f64 = 0.08; // seconds for the lean to catch up with the cursor
const MAX_SHOT: f64 = 3.2;
// Reach in sprite pixels, so it matches the drawn rackets whatever the screen size.
const PLAYER_REACH: f64 = 7.0;
const CLAWD_REACH: f64 = 12.0;
const CLAWD_RUN: f64 = 0.55;
const CLAWD_REACTION: f64 = 0.15; // seconds before Clawd starts running for your shot
const SERVE_DELAY: f64 = 1.2;
const SERVE_WAIT: f64 = 4.0; // if you don't hit your serve, it goes by itself after this long
const STEP: f64 = 1.0 / 240.0; // physics substep, shared by play and by every prediction

const NET_TAPE: u32 = 0xFFF2_F2F2;
const NET_POST: u32 = 0xFF9A_9A9A;
const NET_MESH: u32 = 0x9090_9090; // translucent white
const STRINGS: u32 = 0xFFE8_E8E8;
const SHUTTLE_CORK: u32 = 0xFFF7_F4EE;
const SHUTTLE_FEATHER: u32 = 0xFFE0_E0E0;
const SHUTTLE_EDGE: u32 = 0xFF3A_3A3A;

// Cells from the racket head's middle to the edge of its window: far enough for the grip
// (9 cells below the head's middle) at any tilt.
const TILT_REACH: i32 = 10;

// A racket on a 7x13 grid: '#' frame and grip, '+' strings.
const RACKET: [&str; 13] = [
    "..###..", ".#.+.#.", "#.+.+.#", "#+.+.+#", "#.+.+.#", "#+.+.+#", ".#.+.#.", "..###..", "...#...", "...#...",
    "...#...", "..###..", "..###..",
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    You,
    Clawd,
}

impl Side {
    fn other(self) -> Side {
        match self {
            Side::You => Side::Clawd,
            Side::Clawd => Side::You,
        }
    }
}

/// Where a shot would come down, and whether it gets over the net.
struct Flight {
    land_x: f64,
    clears_net: bool,
}

pub(super) struct Badminton {
    left: f64,
    top: f64,
    right: f64,
    floor: f64,
    w: f64,
    s: f64,
    net_x: f64,
    net_top: f64,
    gravity: f64,
    drag: f64,
    unit: i32,         // racket cell size in pixels
    shuttle_unit: i32, // shuttlecock cell size in pixels
    shuttle: (f64, f64),
    vel: (f64, f64),
    flying: bool,
    last_hitter: Option<Side>,
    server: Side,
    serve_at: f64,
    racket: (f64, f64), // center of your racket's head
    trail: VecDeque<(f64, f64, f64)>,
    clawd_x: f64, // where Clawd's racket hand is
    clawd_target: f64,
    react_at: f64,
    swing_until: f64,
    tilt: f64,        // your racket's lean in radians, clockwise; follows the cursor's sideways speed
    scores: [u32; 2], // you, Clawd
    hits: [u32; 2],
    mood_until: f64,
    over_at: Option<f64>,
    pub(super) sound: Option<Sound>,
}

impl Badminton {
    pub(super) fn new(area: RECT, pet: &mut Pet) -> Self {
        // The whole monitor is the court: the taskbar is the floor and the net stands at its middle.
        let (left, top, right, floor) = (area.left as f64, area.top as f64, area.right as f64, area.bottom as f64);
        let w = right - left;
        let net_x = left + w / 2.0;
        let mut game = Badminton {
            left,
            top,
            right,
            floor,
            w,
            s: pet.s as f64,
            net_x,
            net_top: floor - NET_HEIGHT * w,
            gravity: GRAVITY * w,
            drag: GRAVITY / (TERMINAL * TERMINAL * w), // so that drag balances gravity at the terminal speed
            unit: pet.s.max(4),
            shuttle_unit: (pet.s * 2 / 3).max(3),
            shuttle: (0.0, 0.0),
            vel: (0.0, 0.0),
            flying: false,
            last_hitter: None,
            server: Side::You,
            serve_at: 0.0,
            racket: (left + w / 4.0, floor - SERVE_HEIGHT * w),
            trail: VecDeque::new(),
            clawd_x: 0.0,
            clawd_target: 0.0,
            react_at: 0.0,
            swing_until: 0.0,
            tilt: 0.0,
            scores: [0, 0],
            hits: [0, 0],
            mood_until: 0.0,
            over_at: None,
            sound: None,
        };
        game.clawd_x = game.clawd_home();
        game.reset_serve(pet.now);
        pet.enter_game();
        pet.set_state(State::Idle, None);
        game.place_clawd(pet);
        game
    }

    fn clawd_home(&self) -> f64 {
        self.net_x + 0.55 * (self.right - self.net_x)
    }

    /// Where Clawd grips its racket: just outside and above its left arm (which is 5 sprite
    /// pixels up), so the racket head stands clear above Clawd's body.
    fn clawd_hand(&self) -> (f64, f64) {
        (self.clawd_x - 1.5 * self.unit as f64, self.floor - 7.0 * self.s)
    }

    /// The middle of Clawd's racket head, held upright above its hand.
    fn clawd_racket(&self) -> (f64, f64) {
        let (x, y) = self.clawd_hand();
        (x, y - 8.5 * self.unit as f64)
    }

    pub(super) fn shuttle_center(&self) -> POINT {
        POINT {
            x: self.shuttle.0 as i32,
            y: self.shuttle.1 as i32,
        }
    }

    fn reset_serve(&mut self, now: f64) {
        self.flying = false;
        self.last_hitter = None;
        self.vel = (0.0, 0.0);
        self.serve_at = now + SERVE_DELAY;
        self.clawd_target = self.clawd_home();
        self.shuttle = match self.server {
            Side::You => (self.left + 0.22 * self.w, self.floor - SERVE_HEIGHT * self.w),
            Side::Clawd => self.clawd_racket(),
        };
    }

    fn step(&self, p: &mut (f64, f64), v: &mut (f64, f64)) {
        let speed = v.0.hypot(v.1);
        v.0 -= self.drag * speed * v.0 * STEP;
        v.1 += (self.gravity - self.drag * speed * v.1) * STEP;
        p.0 += v.0 * STEP;
        p.1 += v.1 * STEP;
    }

    fn crosses_net(&self, before: (f64, f64), after: (f64, f64)) -> bool {
        (before.0 - self.net_x).signum() != (after.0 - self.net_x).signum() && after.1 > self.net_top
    }

    fn simulate(&self, from: (f64, f64), vel: (f64, f64)) -> Flight {
        let (mut p, mut v) = (from, vel);
        let mut clears_net = true;
        for _ in 0..2400 {
            let before = p;
            self.step(&mut p, &mut v);
            clears_net &= !self.crosses_net(before, p);
            if p.1 >= self.floor {
                break;
            }
        }
        Flight {
            land_x: p.0,
            clears_net,
        }
    }

    /// Follows the shuttle's current flight to where it first drops through `height` on one
    /// side of the net (Clawd's when `clawd_side`), and to where it lands.
    fn predict(&self, height: f64, clawd_side: bool) -> (Option<f64>, f64) {
        let (mut p, mut v) = (self.shuttle, self.vel);
        let mut crossing = None;
        for _ in 0..2400 {
            let before = p;
            self.step(&mut p, &mut v);
            if self.crosses_net(before, p) {
                return (None, self.net_x); // into the net
            }
            let on_side = (p.0 > self.net_x) == clawd_side;
            if crossing.is_none() && on_side && v.1 > 0.0 && before.1 < height && p.1 >= height {
                crossing = Some(p.0);
            }
            if p.1 >= self.floor {
                break;
            }
        }
        (crossing, p.0)
    }

    /// Advances the game; returns false once it's over and Clawd has finished reacting.
    pub(super) fn update(&mut self, now: f64, dt: f64, cursor: POINT, pet: &mut Pet) -> bool {
        self.sound = None;
        let u = self.unit as f64;
        let racket = (
            (cursor.x as f64).clamp(self.left, self.net_x - 0.02 * self.w),
            (cursor.y as f64).clamp(self.top, self.floor - 4.0 * u),
        );
        let swept_from = self.racket;
        self.racket = racket;
        self.trail.push_back((now, racket.0, racket.1));
        while self.trail.len() > 2 && now - self.trail[0].0 > 0.06 {
            self.trail.pop_front();
        }
        // Like a racket held loosely and swept sideways: the head leans the way you move and the
        // handle trails, then it eases back upright when you stop.
        let lean = (self.racket_velocity().0 / (FULL_TILT_SPEED * self.w)).clamp(-1.0, 1.0) * MAX_TILT;
        self.tilt += (lean - self.tilt) * (1.0 - (-dt / TILT_EASE).exp());

        if let Some(end) = self.over_at {
            if now >= end {
                return false;
            }
        } else if self.flying {
            self.fly(now, dt, pet);
        } else if self.server == Side::Clawd {
            self.shuttle = self.clawd_racket();
            if now >= self.serve_at {
                self.clawd_hit(now, true, pet);
            }
        } else {
            // Your serve: the shuttle hangs in the air, bobbing, until you hit it.
            self.shuttle.1 = self.floor - SERVE_HEIGHT * self.w + (now * 3.0).sin() * self.s;
            let reach = PLAYER_REACH * self.s;
            let touched = segment_distance(swept_from, racket, self.shuttle) < reach;
            if now >= self.serve_at && touched || now >= self.serve_at + SERVE_WAIT {
                self.player_hit(now, pet);
            }
        }
        self.move_clawd(now, dt, pet);
        self.place_clawd(pet);
        true
    }

    fn fly(&mut self, now: f64, dt: f64, pet: &mut Pet) {
        let prev = self.shuttle;
        let mut t = 0.0;
        while t < dt {
            let before = self.shuttle;
            let (mut p, mut v) = (self.shuttle, self.vel);
            self.step(&mut p, &mut v);
            (self.shuttle, self.vel) = (p, v);
            if self.crosses_net(before, self.shuttle) {
                // Into the net: it drops back on the hitter's side.
                self.shuttle.0 = self.net_x + (before.0 - self.net_x).signum() * 2.0;
                self.vel = (-self.vel.0 * 0.15, self.vel.1 * 0.3);
                self.sound = Some(Sound::Wall);
            }
            t += STEP;
        }

        if self.last_hitter != Some(Side::You)
            && self.shuttle.0 < self.net_x
            && segment_distance(prev, self.shuttle, self.racket) < PLAYER_REACH * self.s
        {
            self.player_hit(now, pet);
        } else if self.last_hitter != Some(Side::Clawd)
            && self.shuttle.0 > self.net_x
            && segment_distance(prev, self.shuttle, self.clawd_racket()) < CLAWD_REACH * self.s
        {
            self.clawd_hit(now, false, pet);
        }

        let (x, y) = self.shuttle;
        if y >= self.floor || x < self.left - 0.25 * self.w || x > self.right + 0.25 * self.w {
            let hitter = self.last_hitter.unwrap_or(self.server);
            let winner = if x < self.left || x > self.right {
                hitter.other() // out
            } else if x < self.net_x {
                Side::Clawd
            } else {
                Side::You
            };
            self.point(winner, now, pet);
        }
    }

    fn racket_velocity(&self) -> (f64, f64) {
        let (Some(&(t0, x0, y0)), Some(&(t1, x1, y1))) = (self.trail.front(), self.trail.back()) else {
            return (0.0, 0.0);
        };
        if t1 - t0 < 0.005 {
            return (0.0, 0.0);
        }
        let limit = 3.0 * self.w;
        (
            ((x1 - x0) / (t1 - t0)).clamp(-limit, limit),
            ((y1 - y0) / (t1 - t0)).clamp(-limit, limit),
        )
    }

    /// Your shot: with the racket held still it floats to the middle of Clawd's court; swinging
    /// adds the racket's speed, so a fast swing drives or smashes it (and may put it out).
    fn player_hit(&mut self, now: f64, pet: &mut Pet) {
        let (rvx, rvy) = self.racket_velocity();
        let w = self.w;
        let middle = self.net_x + 0.5 * (self.right - self.net_x);
        let (bx, by) = self.aim(middle, &[0.95, 0.7, 1.2]).unwrap_or((0.9 * w, -1.6 * w));
        let mut v = ((bx + rvx * SWING).max(0.35 * w), by + rvy * SWING);
        let speed = v.0.hypot(v.1);
        if speed > MAX_SHOT * w {
            v = (v.0 * MAX_SHOT * w / speed, v.1 * MAX_SHOT * w / speed);
        }
        self.vel = v;
        self.flying = true;
        self.last_hitter = Some(Side::You);
        self.hits[0] += 1;
        self.sound = Some(Sound::Paddle);

        // Clawd reads the shot and heads for where it'll come down to racket height,
        // unless it's flying out or into the net, in which case Clawd lets it go.
        let (crossing, land_x) = self.predict(self.clawd_racket().1, true);
        self.clawd_target = match crossing {
            Some(x) if land_x <= self.right => x + pet.uniform(-0.03, 0.03) * w,
            _ => self.clawd_home(),
        };
        self.react_at = now + CLAWD_REACTION;
    }

    fn clawd_hit(&mut self, now: f64, serve: bool, pet: &mut Pet) {
        self.vel = self.clawd_shot(serve, pet);
        self.flying = true;
        self.last_hitter = Some(Side::Clawd);
        self.hits[1] += 1;
        self.swing_until = now + 0.15;
        self.clawd_target = self.clawd_home();
        self.sound = Some(Sound::Paddle);
    }

    /// Picks a spot in your court and finds a shot that clears the net and lands there.
    /// Now and then Clawd aims long (out) or mishits into the net.
    fn clawd_shot(&self, serve: bool, pet: &mut Pet) -> (f64, f64) {
        let w = self.w;
        let mut target = self.left + w * pet.uniform(0.06, 0.44);
        if !serve && pet.random() < 0.12 {
            target = self.left - w * pet.uniform(0.02, 0.08);
        }
        let flub = !serve && pet.random() < 0.06;
        let mut angles = if serve { [1.05, 0.9, 1.2] } else { [0.35, 0.75, 1.1] };
        if !serve {
            angles.rotate_left((pet.random() * 3.0) as usize); // vary drives, mid shots and clears
        }
        match self.aim(target, &angles) {
            Some((vx, vy)) if flub => (vx * 0.7, vy * 0.7),
            Some(v) => v,
            None => (-0.8 * w, -2.0 * w), // a big clear
        }
    }

    /// A shot from the shuttle's position that clears the net and lands at `target_x`, trying each
    /// launch angle (radians above horizontal) in turn.
    fn aim(&self, target_x: f64, angles: &[f64]) -> Option<(f64, f64)> {
        let w = self.w;
        let toward = (target_x - self.shuttle.0).signum();
        for &angle in angles {
            let dir = (toward * angle.cos(), -angle.sin());
            let (mut lo, mut hi) = (0.2 * w, 4.0 * w);
            for _ in 0..22 {
                let mid = (lo + hi) / 2.0;
                if (self.simulate(self.shuttle, (dir.0 * mid, dir.1 * mid)).land_x - target_x) * toward < 0.0 {
                    lo = mid; // not far enough yet
                } else {
                    hi = mid;
                }
            }
            let speed = (lo + hi) / 2.0;
            let flight = self.simulate(self.shuttle, (dir.0 * speed, dir.1 * speed));
            if flight.clears_net && (flight.land_x - target_x).abs() < 0.03 * w {
                return Some((dir.0 * speed, dir.1 * speed));
            }
        }
        None
    }

    fn point(&mut self, winner: Side, now: f64, pet: &mut Pet) {
        let side = if winner == Side::You { 0 } else { 1 };
        self.scores[side] += 1;
        self.server = winner;
        self.reset_serve(now);
        self.mood_until = now + SERVE_DELAY;
        if winner == Side::You {
            pet.set_state(State::Dizzy, None);
            self.sound = Some(Sound::PlayerPoint);
        } else {
            pet.set_state(State::Happy, None);
            pet.spawn(HEART_SHAPE, HEART, (OX + 5) as f64, (OY - 6) as f64, 0.0, -4.0, 1.2);
            self.sound = Some(Sound::ClawdPoint);
        }
        if self.scores[side] < WIN_POINTS {
            return;
        }
        self.over_at = Some(now + 4.0);
        if winner == Side::You {
            // A good sport: Clawd waves and sends hearts.
            pet.set_state(State::Wave, None);
            for i in 0..3 {
                pet.spawn(
                    HEART_SHAPE,
                    HEART,
                    (OX + 1 + 5 * i) as f64,
                    (OY - 6 - 2 * i) as f64,
                    0.0,
                    -3.0,
                    2.5,
                );
            }
            self.sound = Some(Sound::PlayerWins);
        } else {
            pet.dance();
            self.sound = Some(Sound::ClawdWins);
        }
    }

    fn move_clawd(&mut self, now: f64, dt: f64, pet: &mut Pet) {
        let target = if now >= self.react_at {
            self.clawd_target
        } else {
            self.clawd_x
        };
        let reach = CLAWD_RUN * self.w * dt;
        let step = (target - self.clawd_x).clamp(-reach, reach);
        self.clawd_x = (self.clawd_x + step).clamp(self.net_x + 0.08 * self.w, self.right - 0.03 * self.w);
        if self.over_at.is_some() || now < self.mood_until {
            return; // still celebrating or sulking
        }
        let running = step.abs() > 0.2 * self.s;
        if running {
            pet.dir = if step < 0.0 { -1 } else { 1 };
        }
        let pose = if running { State::Walk } else { State::Idle };
        if pet.state != pose {
            pet.set_state(pose, None);
        }
    }

    fn place_clawd(&self, pet: &mut Pet) {
        // Clawd's left arm (sprite columns OX..OX+2) holds the racket; its feet are on the floor.
        pet.x = self.clawd_x - ((OX + 1) as f64) * self.s;
        pet.y = self.floor - pet.h as f64;
    }
}

/// How close the point `p` comes to the segment from `a` to `b`.
fn segment_distance(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a.0 + t * dx - p.0).hypot(a.1 + t * dy - p.1)
}

/// The windows that draw a badminton game: the net, the shuttlecock, both rackets and the scoreboard.
pub(super) struct BadmintonView {
    _net: Layer,
    shuttle: Layer,
    racket: Layer,
    clawd_racket: Layer,
    score: Layer,
    score_unit: i32,
    shown_scores: Option<[u32; 2]>,
}

impl BadmintonView {
    pub(super) unsafe fn new(instance: HMODULE, game: &Badminton) -> Result<Self> {
        let line = game.shuttle_unit;
        let score_unit = (game.s as i32 * 2 / 3).max(3);
        // Just the net, standing on the taskbar in the middle of the screen: a post, a mesh and a tape.
        let net_h = (game.floor - game.net_top) as i32;
        let mut net = Layer::new(instance, 2 * line, net_h)?;
        {
            let mut canvas = net.surface.canvas(1);
            canvas.clear();
            for row in 0..net_h / line {
                for col in 0..2 {
                    if (row + col) % 2 == 0 {
                        canvas.px(col * line, row * line, line, line, NET_MESH);
                    }
                }
            }
            canvas.px(line / 2, 0, line, net_h, NET_POST);
            canvas.px(0, 0, 2 * line, line, NET_TAPE);
        }
        net.present(game.net_x - line as f64, game.net_top);
        let racket_size = 13 * game.unit;
        Ok(BadmintonView {
            _net: net,
            shuttle: Layer::new(instance, 9 * game.shuttle_unit, 9 * game.shuttle_unit)?,
            // Room for the racket turned any way around its head, which stays on the cursor.
            racket: Layer::new(instance, 2 * TILT_REACH * game.unit, 2 * TILT_REACH * game.unit)?,
            clawd_racket: Layer::new(instance, racket_size, racket_size)?,
            score: Layer::new(instance, 17 * score_unit, 9 * score_unit)?,
            score_unit,
            shown_scores: None,
        })
    }

    pub(super) fn render(&mut self, game: &Badminton, now: f64) {
        let u = game.unit as f64;
        draw_tilted_racket(&mut self.racket.surface.canvas(1), PLAYER_PADDLE, game.tilt, game.unit);
        let half = (TILT_REACH * game.unit) as f64;
        self.racket.present(game.racket.0 - half, game.racket.1 - half); // head centered on the cursor

        // Clawd's racket stands up from its hand, and swings flat for a moment when it hits.
        let swinging = now < game.swing_until;
        draw_racket(&mut self.clawd_racket.surface.canvas(game.unit), CLAWD_PADDLE, swinging);
        let (hx, hy) = game.clawd_hand();
        let (ax, ay) = if swinging { (12.5, 9.5) } else { (9.5, 12.5) }; // where the grip sits in the layer
        self.clawd_racket.present(hx - ax * u, hy - ay * u);

        let su = game.shuttle_unit;
        {
            let mut canvas = self.shuttle.surface.canvas(su);
            canvas.clear();
            if game.over_at.is_none() {
                let heading = if game.flying { game.vel } else { (0.0, 1.0) }; // waiting: cork down
                draw_shuttle(&mut canvas, heading);
            }
        }
        self.shuttle
            .present(game.shuttle.0 - 4.5 * su as f64, game.shuttle.1 - 4.5 * su as f64);

        if self.shown_scores != Some(game.scores) {
            self.shown_scores = Some(game.scores);
            draw_scoreboard(&mut self.score.surface.canvas(self.score_unit), game.scores);
        }
        // Floating just above the net.
        let x = game.net_x - (17 * self.score_unit) as f64 / 2.0;
        self.score
            .present(x, game.net_top - (9 * self.score_unit) as f64 - 3.0 * game.s);
    }
}

/// An upright racket at columns 6..13 of a 13x13 layer, or (swung) lying flat with its head to the left.
/// The racket turned `angle` radians clockwise about the middle of its head, which sits at the
/// center of a square canvas (in pixels) TILT_REACH cells from each edge. Each pixel samples the
/// racket pattern, so the chunky cells stay crisp at any angle.
fn draw_tilted_racket(canvas: &mut Canvas, frame: u32, angle: f64, cell: i32) {
    canvas.clear();
    let (sin, cos) = angle.sin_cos();
    let size = 2 * TILT_REACH * cell;
    let (center, cell) = ((TILT_REACH * cell) as f64, cell as f64);
    for py in 0..size {
        for px in 0..size {
            let (x, y) = (px as f64 + 0.5 - center, py as f64 + 0.5 - center);
            // Undo the turn to find which racket cell this pixel shows; the head's middle is cell (3.5, 4).
            let (col, row) = ((x * cos + y * sin) / cell + 3.5, (-x * sin + y * cos) / cell + 4.0);
            if col < 0.0 || row < 0.0 {
                continue;
            }
            let color = match RACKET
                .get(row as usize)
                .and_then(|line| line.as_bytes().get(col as usize))
            {
                Some(b'#') => frame,
                Some(b'+') => STRINGS,
                _ => continue,
            };
            canvas.px(px, py, 1, 1, color);
        }
    }
}

fn draw_racket(canvas: &mut Canvas, frame: u32, swung: bool) {
    canvas.clear();
    for (row, line) in RACKET.iter().enumerate() {
        for (col, ch) in line.bytes().enumerate() {
            let color = match ch {
                b'#' => frame,
                b'+' => STRINGS,
                _ => continue,
            };
            let (row, col) = (row as i32, col as i32);
            let (x, y) = if swung { (row, col + 6) } else { (col + 6, row) };
            canvas.px(x, y, 1, 1, color);
        }
    }
}

/// A shuttlecock on a 9x9 grid flying cork-first along `heading`: a round cork, a flared feather skirt.
fn draw_shuttle(canvas: &mut Canvas, heading: (f64, f64)) {
    let length = heading.0.hypot(heading.1).max(1e-9);
    let (dx, dy) = (heading.0 / length, heading.1 / length);
    let mut cells = [[0u32; 9]; 9];
    for (row, cells_row) in cells.iter_mut().enumerate() {
        for (col, cell) in cells_row.iter_mut().enumerate() {
            let (x, y) = (col as f64 - 4.0, row as f64 - 4.0);
            let along = x * dx + y * dy;
            let across = (x * dy - y * dx).abs();
            if (x - 2.3 * dx).hypot(y - 2.3 * dy) <= 1.4 {
                *cell = SHUTTLE_CORK;
            } else if (-3.8..=1.0).contains(&along) && across <= 0.9 + (1.0 - along) * 0.42 {
                *cell = SHUTTLE_FEATHER;
            }
        }
    }
    for row in 0..9 {
        for col in 0..9 {
            if cells[row][col] == 0 {
                continue;
            }
            // Outline any cell that touches empty space so the shuttle shows up on light backgrounds.
            let edge = [(0, 1), (2, 1), (1, 0), (1, 2)].iter().any(|&(dr, dc)| {
                let (r, c) = (row as i32 + dr - 1, col as i32 + dc - 1);
                !(0..9).contains(&r) || !(0..9).contains(&c) || cells[r as usize][c as usize] == 0
            });
            let color = if edge { SHUTTLE_EDGE } else { cells[row][col] };
            canvas.px(col as i32, row as i32, 1, 1, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays a whole match on a 2560x1500 screen at 165 fps, with `player` choosing where the cursor goes.
    fn play_match(player: impl Fn(&Badminton) -> POINT, max_secs: f64) -> (Badminton, f64) {
        let area = RECT {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1500,
        };
        let mut pet = Pet::new(9, area);
        pet.rng = 0x2545_F491_4F6C_DD1D; // the same match every run
        let mut game = Badminton::new(area, &mut pet);
        let dt = 1.0 / 165.0;
        let mut now = 0.0;
        while now < max_secs {
            now += dt;
            pet.now = now;
            let cursor = player(&game);
            if !game.update(now, dt, cursor, &mut pet) {
                break;
            }
            pet.update(now, dt, cursor, area);
        }
        (game, now)
    }

    /// Waits where the shuttle will drop to shoulder height (or lower, for flat drives) and holds
    /// the racket still there, leaving shots that are going out.
    fn good_player(game: &Badminton) -> POINT {
        let ready = (game.left + 0.25 * game.w, game.floor - 0.12 * game.w);
        let (x, y) = if !game.flying && game.server == Side::You {
            game.shuttle
        } else if game.flying && game.last_hitter == Some(Side::Clawd) {
            [0.12, 0.06, 0.025]
                .iter()
                .map(|f| game.floor - f * game.w)
                .find_map(|height| match game.predict(height, false) {
                    (Some(x), land_x) if land_x >= game.left => Some((x, height)),
                    _ => None,
                })
                .unwrap_or(ready)
        } else {
            ready
        };
        POINT {
            x: x as i32,
            y: y as i32,
        }
    }

    /// Set CLAWD_RACKETS=<path.ppm> to save the racket drawn at a few tilts.
    #[test]
    fn racket_tilts_with_the_cursor() {
        let area = RECT {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1500,
        };
        let mut pet = Pet::new(9, area);
        let mut game = Badminton::new(area, &mut pet);
        let dt = 1.0 / 165.0;
        let (mut now, mut x) = (0.0, 300.0);
        let mut run = |game: &mut Badminton, pet: &mut Pet, secs: f64, speed: f64| {
            for _ in 0..(secs / dt) as i32 {
                now += dt;
                x += speed * dt;
                pet.now = now;
                game.update(now, dt, POINT { x: x as i32, y: 900 }, pet);
            }
        };
        run(&mut game, &mut pet, 0.3, 0.0);
        assert!(game.tilt.abs() < 0.01, "upright while still");
        run(&mut game, &mut pet, 0.25, 1200.0);
        assert!(game.tilt > 0.4, "leans right moving right, got {}", game.tilt);
        run(&mut game, &mut pet, 0.6, 0.0);
        assert!(game.tilt.abs() < 0.02, "eases back upright, got {}", game.tilt);
        run(&mut game, &mut pet, 0.25, -1200.0);
        assert!(game.tilt < -0.4, "leans left moving left, got {}", game.tilt);

        if let Ok(path) = std::env::var("CLAWD_RACKETS") {
            let size = 2 * TILT_REACH * game.unit;
            let angles = [-MAX_TILT, -0.5, 0.0, 0.5, MAX_TILT];
            let mut sheet = vec![0xFF30_3030u32; (size * size) as usize * angles.len()];
            for (i, &angle) in angles.iter().enumerate() {
                let mut pixels = vec![0u32; (size * size) as usize];
                draw_tilted_racket(
                    &mut Canvas {
                        pixels: &mut pixels,
                        w: size,
                        h: size,
                        s: 1,
                        turn: 0,
                    },
                    PLAYER_PADDLE,
                    angle,
                    game.unit,
                );
                for y in 0..size {
                    for x in 0..size {
                        let p = pixels[(y * size + x) as usize];
                        if p != 0 {
                            sheet[(y * size * angles.len() as i32 + i as i32 * size + x) as usize] = p;
                        }
                    }
                }
            }
            let mut out = format!("P6 {} {} 255\n", size * angles.len() as i32, size).into_bytes();
            for p in sheet {
                out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
            std::fs::write(path, out).unwrap();
        }
    }

    #[test]
    fn rallies_go_back_and_forth_and_the_match_ends() {
        let (game, secs) = play_match(good_player, 900.0);
        println!(
            "good player: score {:?} in {secs:.0}s, hits {:?}",
            game.scores, game.hits
        );
        assert!(game.scores.contains(&WIN_POINTS), "someone should win");
        assert!(game.hits[1] >= 10, "Clawd should return plenty of shots");
        assert!(game.scores[0] >= 2, "a steady player should take some points off Clawd");
    }

    #[test]
    fn clawd_beats_a_player_who_never_moves() {
        let (game, secs) = play_match(
            |g| POINT {
                x: g.left as i32,
                y: g.top as i32,
            },
            600.0,
        );
        println!(
            "idle player: score {:?} in {secs:.0}s, hits {:?}",
            game.scores, game.hits
        );
        assert_eq!(game.scores[1], WIN_POINTS);
    }

    #[test]
    fn clawd_aims_shots_into_your_court() {
        let area = RECT {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1500,
        };
        let mut pet = Pet::new(9, area);
        let mut game = Badminton::new(area, &mut pet);
        let mut landed_in = 0;
        for i in 0..200 {
            game.clawd_x = game.net_x + (0.15 + 0.3 * (i % 7) as f64 / 6.0) * (game.right - game.net_x);
            game.shuttle = game.clawd_racket();
            let v = game.clawd_shot(false, &mut pet);
            let flight = game.simulate(game.shuttle, v);
            if flight.clears_net && (game.left..game.net_x).contains(&flight.land_x) {
                landed_in += 1;
            }
        }
        println!("{landed_in}/200 of Clawd's shots land in");
        assert!((140..=195).contains(&landed_in), "mostly in, with the odd mistake");
    }
}
