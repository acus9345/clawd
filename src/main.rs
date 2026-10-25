//! Clawd, a desktop pet.
//!
//! A little pixel Clawd that lives on top of your taskbar. It idles, wanders,
//! waves, dances, writes code on a tiny laptop, does backflips, naps, and can
//! be picked up, dragged and tossed around.
//!
//!     Left-drag     pick up / throw (it bounces off screen edges)
//!     Left-click    pet (it hops and shows a heart); click again mid-hop to backflip
//!     Wiggle cursor tickle (move the pointer back and forth over Clawd)
//!     Right-click   menu (ping pong, chess, pet, dance, code, backflip, nap, wander toggle, quit)
//!
//! Ping pong: on a table a quarter of the screen, your paddle on the left
//! follows the mouse up and down and Clawd plays the right side. First to 5
//! wins; Esc or the menu stops the game.
//!
//! Chess: you play white by clicking a piece and then its destination; Clawd
//! plays black, typing on its laptop while it thinks. Pawns promote to queens.
//!
//! Every frame waits for the vertical blank of the monitor Clawd is on, so
//! animation and dragging run at that monitor's exact refresh rate. While
//! dragging, the cursor is sampled once per frame right after the vblank,
//! which keeps Clawd glued to the pointer without event-driven jitter.
//!
//! Set CLAWD_LOG=<path> to append the measured frame rate once a second.

#![windows_subsystem = "windows"]

mod badminton;
mod chess;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::f64::consts::PI;
use std::io::Write;
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetMonitorInfoW, MonitorFromPoint, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
    HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;

// Colors as premultiplied 0xAARRGGBB; 0 is fully transparent (and click-through).
const BODY: u32 = 0xFFD9_7757;
const EYE: u32 = 0xFF14_1414;
const HEART: u32 = 0xFFF2_667A;
const ZZZ: u32 = 0xFF7F_A7F5;
const NOTE: u32 = 0xFFF5_C542;
const STAR: u32 = 0xFFFF_D75E;
const CODE_BIT: u32 = 0xFF7C_D992;
const CHECK: u32 = 0xFF4F_C26F;
const QUESTION: u32 = 0xFF9B_8CFF;
const LAPTOP_LID: u32 = 0xFFC4_C8CF;
const LAPTOP_BASE: u32 = 0xFF8B_9099;
const PLAYER_PADDLE: u32 = 0xFF7F_A7F5;
const PLAYER_PADDLE_EDGE: u32 = 0xFF1F_2A44;
const CLAWD_PADDLE: u32 = 0xFFD9_443B;
const CLAWD_PADDLE_EDGE: u32 = 0xFF4A_1A12;
const BALL_FILL: u32 = 0xFFF7_F4EE;
const BALL_EDGE: u32 = 0xFF2E_2E2E;
const TABLE: u32 = 0xD019_4C30; // translucent green
const TABLE_LINE: u32 = 0xFFF2_F2F2;
const NET: u32 = 0xFFC8_C8C8;
const SCOREBOARD: u32 = 0xB00A_0A0A; // translucent black
const SCORE_DASH: u32 = 0xFFDD_DDDD;
const BOARD_FRAME: u32 = 0xFF4E_3A2B;
const BOARD_LIGHT: u32 = 0xFFEE_EED2;
const BOARD_DARK: u32 = 0xFF76_9656;
const BOARD_LAST_LIGHT: u32 = 0xFFF6_F68A;
const BOARD_LAST_DARK: u32 = 0xFFBA_CA44;
const BOARD_SELECTED: u32 = 0xFF8F_C1E3;
const BOARD_CHECK: u32 = 0xFFE0_524A;
const WHITE_PIECE: u32 = 0xFFFA_FAFA;
const WHITE_PIECE_EDGE: u32 = 0xFF1E_1E1E;
const CLAWD_PIECE_EDGE: u32 = 0xFF3A_1A10;
const BANNER_TEXT: u32 = 0xFFF2_F2F2;
const BANNER_HINT: u32 = 0xFFB0_B0B0;

// Canvas size in sprite pixels, and where Clawd's 16x9 sprite sits on it.
// The empty rows above Clawd leave room for raised arms, hearts and Zs.
const GRID_W: i32 = 24;
const GRID_H: i32 = 22;
const OX: i32 = 4;
const OY: i32 = GRID_H - 9;
const LEG_X: [i32; 4] = [3, 5, 10, 12];
// Rotations turn the sprite about this point; it sits on the grid so turned pixels stay aligned.
const PIVOT: (i32, i32) = (OX + 8, OY + 5);

// Arm rows relative to the top of the body, (left, right).
const ARMS_SIDE: (i32, i32) = (3, 3);
const ARMS_UP: (i32, i32) = (-1, -1);

// Physics, in sprite pixels per second (scaled to screen pixels at runtime).
const GRAVITY: f64 = 420.0;
const WALK_SPEED: f64 = 9.0;
const HOP_SPEED: f64 = 95.0;
const FLIP_SPEED: f64 = 140.0;
const FLIP_TIME: f64 = 0.5; // seconds for one full turn
const CURIOUS_TIME: f64 = 4.0; // look left, look right, then wonder "?"
const BOUNCE_SPEED: f64 = 150.0; // landings faster than this bounce a little
const DIZZY_SPEED: f64 = 230.0; // landings faster than this leave Clawd dizzy
const THROW_WINDOW: f64 = 0.06; // seconds of drag history used for the throw velocity
const TICKLE_AMOUNT: f64 = 40.0; // cursor travel over Clawd, in sprite pixels, that tickles

// Ping pong, in sprite pixels (per second).
const WIN_SCORE: u32 = 5;
const PADDLE_W: i32 = 2;
const PADDLE_H: i32 = 14;
const BALL_START: f64 = 70.0;
const BALL_MAX: f64 = 180.0;
const BALL_SPEEDUP: f64 = 1.08; // per paddle hit
const CLAWD_REACH: f64 = 95.0; // how fast Clawd can move its paddle
const CLAWD_REACTS_AT: f64 = 0.45; // fraction of the table the ball must cross before Clawd moves for it
const SERVE_DELAY: f64 = 1.2;

// Chess. The board is 100 units across: a 2-unit frame around 8 squares of 12 units.
const SQUARE: i32 = 12;
const BOARD_UNITS: i32 = 8 * SQUARE + 4;
const THINK_MIN: f64 = 0.8; // Clawd always pauses at least this long, even when the answer is instant
const THINK_BUDGET: Duration = Duration::from_millis(1200);
const PIECE_SLIDE: f64 = 0.18;
const MAX_BOUNCE_ANGLE: f64 = 0.95; // radians, for a hit on the paddle's very edge

const HEART_SHAPE: &[&str] = &[".X.X.", "XXXXX", "XXXXX", ".XXX.", "..X.."];
const MINI_HEART_SHAPE: &[&str] = &["X.X", "XXX", ".X."];
const Z_SHAPE: &[&str] = &["XXXX", "..X.", ".X..", "XXXX"];
const NOTE_SHAPE: &[&str] = &[".XXX", ".X.X", ".X..", "XX..", "XX.."];
const STAR_SHAPE: &[&str] = &[".X.", "XXX", ".X."];
const BIT_SHAPE: &[&str] = &["XX"];
const CHECK_SHAPE: &[&str] = &["....X", "...X.", "X.X..", ".X..."];
const QUESTION_SHAPE: &[&str] = &["XXX", "..X", ".XX", "...", ".X."];
const BALL_SHAPE: &[&str] = &[
    "..OOO..", ".OWWWO.", "OWWWWWO", "OWWWWWO", "OWWWWWO", ".OWWWO.", "..OOO..",
];
const DIGITS: [[&str; 5]; 10] = [
    ["XXX", "X.X", "X.X", "X.X", "XXX"],
    [".X.", "XX.", ".X.", ".X.", "XXX"],
    ["XXX", "..X", "XXX", "X..", "XXX"],
    ["XXX", "..X", "XXX", "..X", "XXX"],
    ["X.X", "X.X", "XXX", "..X", "..X"],
    ["XXX", "X..", "XXX", "..X", "XXX"],
    ["XXX", "X..", "XXX", "X.X", "XXX"],
    ["XXX", "..X", "..X", "..X", "..X"],
    ["XXX", "X.X", "XXX", "X.X", "XXX"],
    ["XXX", "X.X", "XXX", "..X", "XXX"],
];

// Eye pixels relative to each eye's anchor; the right eye is mirrored.
const HAPPY_EYE: &[(i32, i32)] = &[(-1, 1), (0, 0), (1, 1)];
const SQUINT_EYE: &[(i32, i32)] = &[(-1, 0), (0, 1), (-1, 2)];
const DIZZY_EYE: &[(i32, i32)] = &[(-1, 0), (1, 0), (0, 1), (-1, 2), (1, 2)];

const MENU_PET: usize = 1;
const MENU_DANCE: usize = 2;
const MENU_CODE: usize = 3;
const MENU_FLIP: usize = 4;
const MENU_NAP: usize = 5;
const MENU_WANDER: usize = 6;
const MENU_QUIT: usize = 7;
const MENU_PONG: usize = 8;
const MENU_CHESS: usize = 9;
const MENU_STOP: usize = 10;
const MENU_BADMINTON: usize = 11;

// Chess pieces on a 12x12 grid: '#' outline, 'o' fill. Indexed by piece kind - 1.
const PIECES: [[&str; 12]; 6] = [
    [
        "............",
        "............",
        "............",
        "....####....",
        "...#oooo#...",
        "...#oooo#...",
        "....#oo#....",
        "....#oo#....",
        "...#oooo#...",
        "..#oooooo#..",
        "..########..",
        "............",
    ],
    [
        "............",
        "....#.#.....",
        "...#o#o#....",
        "..#ooooo#...",
        ".#oo#oooo#..",
        ".#oooooooo#.",
        "..##.#oooo#.",
        ".....#oooo#.",
        "....#ooooo#.",
        "...#oooooo#.",
        "..#########.",
        "............",
    ],
    [
        "............",
        ".....##.....",
        "....#oo#....",
        "...#oo#o#...",
        "...#o#oo#...",
        "...#oooo#...",
        "....#oo#....",
        "....#oo#....",
        "...#oooo#...",
        "..#oooooo#..",
        "..########..",
        "............",
    ],
    [
        "............",
        "..##.##.##..",
        "..#o#oo#o#..",
        "..########..",
        "...#oooo#...",
        "...#oooo#...",
        "...#oooo#...",
        "...#oooo#...",
        "..#oooooo#..",
        ".#oooooooo#.",
        ".##########.",
        "............",
    ],
    [
        "............",
        "..#..##..#..",
        "..#o#oo#o#..",
        "..#oooooo#..",
        "...#oooo#...",
        "...#oooo#...",
        "....#oo#....",
        "....#oo#....",
        "...#oooo#...",
        "..#oooooo#..",
        "..########..",
        "............",
    ],
    [
        ".....##.....",
        "....####....",
        ".....##.....",
        "...######...",
        "..#oooooo#..",
        "..#oooooo#..",
        "...#oooo#...",
        "....#oo#....",
        "...#oooo#...",
        "..#oooooo#..",
        "..########..",
        "............",
    ],
];

/// 5x5 capitals for the game-over banner.
fn glyph(ch: char) -> Option<[&'static str; 5]> {
    Some(match ch {
        'A' => [".###.", "#...#", "#####", "#...#", "#...#"],
        'C' => [".####", "#....", "#....", "#....", ".####"],
        'D' => ["####.", "#...#", "#...#", "#...#", "####."],
        'E' => ["#####", "#....", "####.", "#....", "#####"],
        'I' => ["#####", "..#..", "..#..", "..#..", "#####"],
        'K' => ["#...#", "#..#.", "###..", "#..#.", "#...#"],
        'L' => ["#....", "#....", "#....", "#....", "#####"],
        'M' => ["#...#", "##.##", "#.#.#", "#...#", "#...#"],
        'N' => ["#...#", "##..#", "#.#.#", "#..##", "#...#"],
        'O' => [".###.", "#...#", "#...#", "#...#", ".###."],
        'R' => ["####.", "#...#", "####.", "#..#.", "#...#"],
        'S' => [".####", "#....", ".###.", "....#", "####."],
        'T' => ["#####", "..#..", "..#..", "..#..", "..#.."],
        'U' => ["#...#", "#...#", "#...#", "#...#", ".###."],
        'W' => ["#...#", "#...#", "#.#.#", "##.##", "#...#"],
        'Y' => ["#...#", ".#.#.", "..#..", "..#..", "..#.."],
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Idle,
    Walk,
    Wave,
    Happy,
    Drag,
    Fall,
    Dizzy,
    Yawn,
    Sleep,
    Dance,
    Code,
    Curious,
    Tickle,
    Flip,
    Tada,
    Pong, // hovering at the right edge, paddle in hand
}

#[derive(Clone, Copy, PartialEq)]
enum Eyes {
    Open,
    Blink,
    Closed,
    Happy,
    Squint,
    Dizzy,
}

enum Input {
    Press(POINT), // cursor position when the button went down
    Release,
    Menu,
    BoardClick(POINT),
}

thread_local! {
    // Filled by the window procedure, drained once per frame by the main loop.
    static INPUTS: RefCell<Vec<Input>> = const { RefCell::new(Vec::new()) };
}

struct Particle {
    shape: &'static [&'static str],
    color: u32,
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    life: f64,
}

struct Pet {
    s: i32,
    w: i32,
    h: i32,
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    dir: i32, // walking direction
    airborne: bool,
    now: f64,
    state: State,
    state_start: f64,
    state_end: f64,
    cue_done: bool, // one-shot effect of the current state has fired
    particles: Vec<Particle>,
    press: Option<(POINT, f64, f64)>, // cursor at press, grab offset
    dragging: bool,
    trail: VecDeque<(f64, f64, f64)>,
    last_cursor: POINT,
    tickle: f64,
    squash_until: f64,
    blink_until: f64,
    next_blink: f64,
    next_fx: f64,
    last_touch: f64,
    wander: bool,
    in_game: bool, // a ping pong game positions Clawd and picks its moods
    rng: u64,
}

impl Pet {
    fn new(s: i32, area: RECT) -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        let (w, h) = (GRID_W * s, GRID_H * s);
        Pet {
            s,
            w,
            h,
            x: (area.right - (OX + 16) * s - 120) as f64,
            y: (area.bottom - h - 30 * s) as f64, // drop in from just above the taskbar
            vx: 0.0,
            vy: 0.0,
            dir: -1,
            airborne: true,
            now: 0.0,
            state: State::Fall,
            state_start: 0.0,
            state_end: f64::INFINITY,
            cue_done: false,
            particles: Vec::new(),
            press: None,
            dragging: false,
            trail: VecDeque::new(),
            last_cursor: POINT::default(),
            tickle: 0.0,
            squash_until: 0.0,
            blink_until: 0.0,
            next_blink: 2.0,
            next_fx: 0.0,
            last_touch: 0.0,
            wander: true,
            in_game: false,
            rng: seed | 1,
        }
    }

    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }

    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.random()
    }

    fn center(&self) -> POINT {
        POINT {
            x: self.x as i32 + (OX + 8) * self.s,
            y: self.y as i32 + (OY + 4) * self.s,
        }
    }

    fn over_body(&self, p: POINT) -> bool {
        let (x, y) = (p.x as f64 - self.x, p.y as f64 - self.y);
        let s = self.s as f64;
        x >= (OX + 2) as f64 * s && x < (OX + 14) as f64 * s && y >= OY as f64 * s && y < (OY + 9) as f64 * s
    }

    // ----- state -----

    fn set_state(&mut self, state: State, duration: Option<f64>) {
        self.state = state;
        self.state_start = self.now;
        self.state_end = duration.map_or(f64::INFINITY, |d| self.now + d);
        self.cue_done = false;
        self.next_fx = self.now;
    }

    fn choose_behavior(&mut self) {
        if self.now - self.last_touch > 120.0 && self.random() < 0.3 {
            self.nap();
            return;
        }
        // About half the time Clawd just hangs out, so the tricks stay special.
        let roll = self.random();
        if roll < 0.06 {
            self.set_state(State::Wave, Some(3.0));
        } else if roll < 0.11 {
            self.dance();
        } else if roll < 0.17 {
            self.code();
        } else if roll < 0.23 {
            self.set_state(State::Curious, Some(CURIOUS_TIME));
        } else if roll < 0.25 {
            self.flip();
        } else if roll < 0.50 && self.wander {
            self.dir = if self.random() < 0.5 { -1 } else { 1 };
            let duration = self.uniform(5.0, 12.0);
            self.set_state(State::Walk, Some(duration));
        } else {
            let duration = self.uniform(6.0, 12.0);
            self.set_state(State::Idle, Some(duration));
        }
    }

    fn pet(&mut self) {
        self.last_touch = self.now;
        match self.state {
            State::Drag => {}
            State::Happy if self.airborne => self.flip(),
            _ => {
                self.set_state(State::Happy, Some(1.4));
                self.vy = -HOP_SPEED * self.s as f64;
                self.spawn(HEART_SHAPE, HEART, (OX + 5) as f64, (OY - 6) as f64, 0.0, -4.0, 1.4);
            }
        }
    }

    fn flip(&mut self) {
        self.set_state(State::Flip, Some(3.0)); // normally ends on landing
        self.vy = -FLIP_SPEED * self.s as f64;
    }

    fn dance(&mut self) {
        self.set_state(State::Dance, Some(8.0));
    }

    fn code(&mut self) {
        let duration = self.uniform(10.0, 16.0);
        self.set_state(State::Code, Some(duration));
    }

    fn nap(&mut self) {
        self.set_state(State::Yawn, Some(1.6));
    }

    fn enter_game(&mut self) {
        self.in_game = true;
        self.press = None;
        self.dragging = false;
        self.set_state(State::Pong, None);
    }

    fn leave_game(&mut self) {
        self.in_game = false;
        self.vx = 0.0;
        self.vy = 0.0;
        self.set_state(State::Fall, None);
    }

    fn spawn(&mut self, shape: &'static [&'static str], color: u32, x: f64, y: f64, vx: f64, vy: f64, life: f64) {
        self.particles.push(Particle {
            shape,
            color,
            x,
            y,
            vx,
            vy,
            life,
        });
    }

    // ----- input -----

    fn on_press(&mut self, cursor: POINT) {
        if self.in_game {
            return;
        }
        self.press = Some((cursor, cursor.x as f64 - self.x, cursor.y as f64 - self.y));
        self.dragging = false;
    }

    fn on_release(&mut self) {
        if self.press.take().is_none() {
            return;
        }
        self.last_touch = self.now;
        if !self.dragging {
            self.pet();
            return;
        }
        self.dragging = false;
        (self.vx, self.vy) = self.throw_velocity();
        self.set_state(State::Fall, None);
    }

    /// Follows the cursor; called once per frame so motion is paced by the display, not by mouse events.
    fn follow_cursor(&mut self, cursor: POINT) {
        let Some((start, off_x, off_y)) = self.press else {
            return;
        };
        if !self.dragging {
            if (cursor.x - start.x).abs() + (cursor.y - start.y).abs() < 5 {
                return;
            }
            self.dragging = true;
            self.vx = 0.0;
            self.vy = 0.0;
            self.trail.clear();
            self.set_state(State::Drag, None);
        }
        self.x = cursor.x as f64 - off_x;
        self.y = cursor.y as f64 - off_y;
        self.trail.push_back((self.now, cursor.x as f64, cursor.y as f64));
        while self.trail.len() > 2 && self.now - self.trail[0].0 > THROW_WINDOW {
            self.trail.pop_front();
        }
    }

    fn throw_velocity(&self) -> (f64, f64) {
        let (Some(&(t0, x0, y0)), Some(&(t1, x1, y1))) = (self.trail.front(), self.trail.back()) else {
            return (0.0, 0.0);
        };
        if t1 - t0 < 0.005 {
            return (0.0, 0.0);
        }
        let limit = 400.0 * self.s as f64;
        (
            ((x1 - x0) / (t1 - t0)).clamp(-limit, limit),
            ((y1 - y0) / (t1 - t0)).clamp(-limit, limit),
        )
    }

    /// Wiggling the pointer back and forth over Clawd's body tickles it.
    fn detect_tickle(&mut self, cursor: POINT, dt: f64) {
        let moved = ((cursor.x - self.last_cursor.x).abs() + (cursor.y - self.last_cursor.y).abs()) as f64;
        self.last_cursor = cursor;
        self.tickle *= (-1.5 * dt).exp();
        if self.press.is_some() || !self.over_body(cursor) {
            return;
        }
        self.tickle += moved.min(20.0 * self.s as f64);
        let ticklish = matches!(
            self.state,
            State::Idle
                | State::Walk
                | State::Wave
                | State::Sleep
                | State::Yawn
                | State::Curious
                | State::Dance
                | State::Code
        );
        if ticklish && self.tickle > TICKLE_AMOUNT * self.s as f64 {
            self.tickle = 0.0;
            self.last_touch = self.now;
            self.set_state(State::Tickle, Some(1.6));
        }
    }

    // ----- simulation -----

    fn update(&mut self, now: f64, dt: f64, cursor: POINT, area: RECT) {
        self.now = now;
        let floor = (area.bottom - self.h) as f64;

        if !self.in_game {
            self.follow_cursor(cursor);
            self.detect_tickle(cursor, dt);
            if self.state != State::Drag {
                self.step_physics(dt, area, floor);
            }
        }

        let t = now - self.state_start;
        if now >= self.state_end && !self.in_game {
            match self.state {
                State::Idle | State::Walk | State::Wave | State::Dance | State::Curious => self.choose_behavior(),
                State::Code => {
                    self.spawn(CHECK_SHAPE, CHECK, (OX + 6) as f64, (OY - 7) as f64, 0.0, -1.5, 1.2);
                    self.set_state(State::Tada, Some(0.9));
                }
                State::Yawn => {
                    let duration = self.uniform(45.0, 90.0);
                    self.set_state(State::Sleep, Some(duration));
                }
                _ => {
                    self.last_touch = now;
                    let duration = self.uniform(2.0, 4.0);
                    self.set_state(State::Idle, Some(duration));
                }
            }
        }

        if now >= self.next_blink {
            self.blink_until = now + 0.12;
            self.next_blink = now + self.uniform(2.0, 6.0);
        }
        self.state_effects(t);

        for p in &mut self.particles {
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.life -= dt;
        }
        self.particles.retain(|p| p.life > 0.0);
    }

    fn step_physics(&mut self, dt: f64, area: RECT, floor: f64) {
        let s = self.s as f64;
        let min_x = (area.left - OX * self.s) as f64;
        let max_x = (area.right - (OX + 16) * self.s) as f64;

        self.airborne = self.y < floor - 0.5 || self.vy < 0.0;
        if self.airborne {
            self.vy += GRAVITY * s * dt;
            self.x += self.vx * dt;
            self.y += self.vy * dt;
            let ceiling = (area.top - OY * self.s) as f64;
            if self.y < ceiling {
                self.y = ceiling;
                self.vy = self.vy.abs() * 0.3;
            }
            if self.y >= floor {
                self.land(floor);
            }
        } else {
            self.y = floor;
            if self.state == State::Walk {
                self.x += self.dir as f64 * WALK_SPEED * s * dt;
            } else {
                self.x += self.vx * dt;
                self.vx *= (1.0 - 8.0 * dt).max(0.0);
                if self.vx.abs() < 5.0 * s {
                    self.vx = 0.0;
                    if self.state == State::Fall {
                        let duration = self.uniform(1.5, 3.0);
                        self.set_state(State::Idle, Some(duration));
                    }
                }
            }
        }

        if self.x < min_x || self.x > max_x {
            self.x = self.x.clamp(min_x, max_x.max(min_x));
            self.vx = -self.vx * 0.5;
            self.dir = if self.x <= min_x { 1 } else { -1 };
        }
    }

    fn land(&mut self, floor: f64) {
        let impact = self.vy / self.s as f64;
        self.y = floor;
        self.vy = if impact > BOUNCE_SPEED { -self.vy * 0.3 } else { 0.0 };
        self.squash_until = self.now + 0.15;
        if impact > DIZZY_SPEED {
            self.set_state(State::Dizzy, Some(1.8));
        } else if self.state == State::Flip {
            self.set_state(State::Tada, Some(0.9));
            self.spawn(STAR_SHAPE, STAR, (OX + 1) as f64, (OY + 6) as f64, -5.0, -3.0, 0.5);
            self.spawn(STAR_SHAPE, STAR, (OX + 12) as f64, (OY + 6) as f64, 5.0, -3.0, 0.5);
        }
    }

    /// Particles each state gives off while it lasts.
    fn state_effects(&mut self, t: f64) {
        let now = self.now;
        if self.state == State::Curious && t > CURIOUS_TIME * 0.6 && !self.cue_done {
            self.cue_done = true;
            self.spawn(
                QUESTION_SHAPE,
                QUESTION,
                (OX + 13) as f64,
                (OY - 6) as f64,
                0.0,
                -1.5,
                1.0,
            );
        }
        if now < self.next_fx {
            return;
        }
        match self.state {
            State::Sleep => {
                self.next_fx = now + 1.3;
                self.spawn(Z_SHAPE, ZZZ, (OX + 13) as f64, (OY - 3) as f64, 1.2, -3.0, 2.6);
            }
            State::Dance => {
                self.next_fx = now + 0.55;
                let (x, vx) = if self.random() < 0.5 {
                    (OX - 2, -1.5)
                } else {
                    (OX + 14, 1.5)
                };
                self.spawn(NOTE_SHAPE, NOTE, x as f64, (OY - 4) as f64, vx, -3.5, 1.6);
            }
            State::Code => {
                self.next_fx = now + 0.2;
                let x = OX as f64 + 4.0 + self.random() * 7.0;
                self.spawn(BIT_SHAPE, CODE_BIT, x, (OY - 1) as f64, 0.0, -5.0, 0.8);
            }
            State::Tickle => {
                self.next_fx = now + 0.3;
                let x = OX as f64 + 1.0 + self.random() * 12.0;
                self.spawn(MINI_HEART_SHAPE, HEART, x, (OY - 4) as f64, 0.0, -4.0, 0.9);
            }
            _ => {}
        }
    }

    // ----- drawing -----

    fn look_at(&self, cursor: POINT) -> (i32, i32) {
        let s = self.s as f64;
        let dx = cursor.x as f64 - (self.x + ((OX + 8) * self.s) as f64);
        let dy = cursor.y as f64 - (self.y + ((OY + 3) * self.s) as f64);
        let step = |d: f64, reach: f64| {
            if d < -reach {
                -1
            } else if d > reach {
                1
            } else {
                0
            }
        };
        (step(dx, 6.0 * s), step(dy, 8.0 * s))
    }

    fn render(&self, cursor: POINT, canvas: &mut Canvas) {
        let t = self.now - self.state_start;
        let phase = |period: f64| (t / period) as i64 % 2 == 1;
        let (mut crouch, mut shake, mut turn, mut mouth, mut laptop) = (0, 0, 0, 0, false);
        let (mut arms, mut eyes, mut look) = (ARMS_SIDE, Eyes::Open, (0, 0));
        let mut legs = [(0, 2); 4]; // (x offset, height) per leg

        match self.state {
            State::Idle => {
                look = self.look_at(cursor);
                crouch = (t % 1.6 > 1.25) as i32;
            }
            State::Walk => {
                look = (self.dir, 0);
                let step = phase(0.14) as usize;
                for (i, leg) in legs.iter_mut().enumerate() {
                    leg.1 = if (i + step) % 2 == 1 { 1 } else { 2 };
                }
            }
            State::Wave => {
                look = self.look_at(cursor);
                eyes = Eyes::Happy;
                arms = if phase(0.22) { (3, -1) } else { (3, 1) };
            }
            State::Happy | State::Tada => (eyes, arms) = (Eyes::Happy, ARMS_UP),
            State::Drag => {
                (eyes, arms) = (Eyes::Squint, ARMS_UP);
                legs = [(if phase(0.1) { 1 } else { -1 }, 2); 4];
            }
            State::Fall => (arms, look) = (ARMS_UP, (0, 1)),
            State::Dizzy => {
                eyes = Eyes::Dizzy;
                shake = if phase(0.12) { 1 } else { -1 };
            }
            State::Yawn => {
                (eyes, arms) = (Eyes::Closed, ARMS_UP);
                mouth = if t < 0.3 || t > 1.3 { 1 } else { 2 };
            }
            State::Sleep => (eyes, crouch) = (Eyes::Closed, 1),
            State::Dance => {
                let beat = phase(0.3);
                shake = if beat { 1 } else { -1 };
                crouch = (t % 0.3 < 0.1) as i32;
                eyes = Eyes::Happy;
                arms = if beat { (3, -1) } else { (-1, 3) };
                for (i, leg) in legs.iter_mut().enumerate() {
                    leg.1 = if (i + beat as usize) % 2 == 1 { 1 } else { 2 };
                }
            }
            State::Code => {
                (crouch, laptop) = (1, true);
                arms = if phase(0.08) { (4, 5) } else { (5, 4) };
            }
            State::Curious => {
                look = if t < CURIOUS_TIME * 0.3 {
                    (-1, 0)
                } else if t < CURIOUS_TIME * 0.6 {
                    (1, 0)
                } else {
                    (0, -1)
                }
            }
            State::Tickle => {
                eyes = Eyes::Happy;
                shake = if phase(0.06) { 1 } else { -1 };
                crouch = phase(0.18) as i32;
                arms = if phase(0.12) { (3, -1) } else { (-1, 3) };
            }
            State::Flip => {
                (eyes, arms) = (Eyes::Happy, ARMS_UP);
                turn = (t / FLIP_TIME * 4.0).min(4.0) as u8 % 4;
            }
            State::Pong => {
                look = self.look_at(cursor); // the game passes the ball here
                legs = [(if phase(0.2) { 1 } else { -1 }, 2); 4];
            }
        }

        if self.now < self.squash_until && turn == 0 {
            crouch = 1;
        }
        if eyes == Eyes::Open && self.now < self.blink_until {
            eyes = Eyes::Blink;
        }

        canvas.clear();
        canvas.turn = turn;
        let (ox, oy) = (OX + shake, OY + crouch);
        for (leg_x, (off, h)) in LEG_X.iter().zip(legs) {
            canvas.px(ox + leg_x + off, oy + 7, 1, h.min(2 - crouch), BODY);
        }
        canvas.px(ox + 2, oy, 12, 7, BODY);
        draw_eyes(canvas, ox, oy, eyes, look);
        if mouth > 0 {
            canvas.px(ox + 7, oy + 4, 2, mouth, EYE);
        }
        if laptop {
            canvas.px(ox + 4, oy + 4, 8, 2, LAPTOP_LID);
            canvas.px(ox + 7, oy + 4, 2, 1, BODY); // logo
            canvas.px(ox + 3, oy + 6, 10, 1, LAPTOP_BASE);
        }
        canvas.px(ox, oy + arms.0, 2, 2, BODY);
        canvas.px(ox + 14, oy + arms.1, 2, 2, BODY);

        canvas.turn = 0;
        if self.state == State::Dizzy {
            for k in 0..2 {
                let angle = t * 7.0 + k as f64 * PI;
                let x = (OX + 7) as f64 + 6.0 * angle.cos();
                let y = (OY - 5) as f64 + 1.5 * angle.sin();
                canvas.shape(STAR_SHAPE, STAR, x.round() as i32, y.round() as i32);
            }
        }
        for p in &self.particles {
            canvas.shape(p.shape, p.color, p.x as i32, p.y as i32);
        }
    }
}

fn draw_eyes(canvas: &mut Canvas, ox: i32, oy: i32, eyes: Eyes, (lx, ly): (i32, i32)) {
    for (eye_x, mirror) in [(4, 1), (11, -1)] {
        let (x, y) = (ox + eye_x, oy + 2);
        let shape = match eyes {
            Eyes::Open => {
                canvas.px(x + lx, y + ly, 1, 2, EYE);
                continue;
            }
            Eyes::Blink => {
                canvas.px(x + lx, y + ly + 1, 1, 1, EYE);
                continue;
            }
            Eyes::Closed => {
                canvas.px(if mirror == 1 { x - 1 } else { x }, y + 1, 2, 1, EYE);
                continue;
            }
            Eyes::Happy => HAPPY_EYE,
            Eyes::Squint => SQUINT_EYE,
            Eyes::Dizzy => DIZZY_EYE,
        };
        for &(dx, dy) in shape {
            canvas.px(x + dx * mirror, y + dy, 1, 1, EYE);
        }
    }
}

/// A view of the window's pixels that draws in sprite-pixel units.
struct Canvas<'a> {
    pixels: &'a mut [u32],
    w: i32,
    h: i32,
    s: i32,
    turn: u8, // quarter turns clockwise applied to everything drawn
}

impl Canvas<'_> {
    fn clear(&mut self) {
        self.pixels.fill(0);
    }

    /// Turns a rectangle about PIVOT, shifted up so the turned sprite still ends at the canvas bottom.
    fn rotate(&self, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
        let (cx, cy) = PIVOT;
        match self.turn % 4 {
            0 => (x, y, w, h),
            1 => (cx + cy - y - h, cy - cx + x - 4, h, w),
            2 => (2 * cx - x - w, 2 * cy - y - h - 2, w, h),
            _ => (cx - cy + y, cx + cy - x - w - 4, h, w),
        }
    }

    fn px(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        let (x, y, w, h) = self.rotate(x, y, w, h);
        let s = self.s;
        let (x0, y0) = ((x * s).max(0), (y * s).max(0));
        let (x1, y1) = (((x + w) * s).min(self.w), ((y + h) * s).min(self.h));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        for row in y0..y1 {
            let start = (row * self.w) as usize;
            self.pixels[start + x0 as usize..start + x1 as usize].fill(color);
        }
    }

    fn shape(&mut self, shape: &[&str], color: u32, x: i32, y: i32) {
        self.shape_part(shape, b'X', color, x, y);
    }

    /// Draws a pattern with `cell`-sized squares at canvas position (x, y), coloring each mark via `color`.
    fn pattern(&mut self, shape: &[&str], x: i32, y: i32, cell: i32, color: impl Fn(u8) -> Option<u32>) {
        for (row, line) in shape.iter().enumerate() {
            for (col, ch) in line.bytes().enumerate() {
                if let Some(c) = color(ch) {
                    self.px(x + col as i32 * cell, y + row as i32 * cell, cell, cell, c);
                }
            }
        }
    }

    /// Darkens what's already drawn; `keep` is out of 256.
    fn shade(&mut self, x: i32, y: i32, w: i32, h: i32, keep: u32) {
        let s = self.s;
        let (x0, y0) = ((x * s).max(0), (y * s).max(0));
        let (x1, y1) = (((x + w) * s).min(self.w), ((y + h) * s).min(self.h));
        for row in y0..y1 {
            for col in x0..x1 {
                let p = &mut self.pixels[(row * self.w + col) as usize];
                let channel = |shift: u32| ((*p >> shift & 0xFF) * keep / 256) << shift;
                *p = (*p & 0xFF00_0000) | channel(16) | channel(8) | channel(0);
            }
        }
    }

    /// Writes `text` in 5x5 capitals centered on `center_x`; spaces are blank.
    fn text(&mut self, text: &str, center_x: i32, y: i32, cell: i32, color: u32) {
        let width = text.len() as i32 * 6 * cell - cell;
        let mut x = center_x - width / 2;
        for ch in text.chars() {
            if let Some(g) = glyph(ch) {
                self.pattern(&g, x, y, cell, |m| (m == b'#').then_some(color));
            }
            x += 6 * cell;
        }
    }

    /// Draws the cells of `shape` marked with `mark`, so one pattern can carry several colors.
    fn shape_part(&mut self, shape: &[&str], mark: u8, color: u32, x: i32, y: i32) {
        for (row, line) in shape.iter().enumerate() {
            for (col, ch) in line.bytes().enumerate() {
                if ch == mark {
                    self.px(x + col as i32, y + row as i32, 1, 1, color);
                }
            }
        }
    }
}

/// A 32-bit premultiplied-alpha bitmap pushed to the layered window each frame.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u32,
    w: i32,
    h: i32,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.dc);
            let _ = DeleteObject(self.bitmap);
        }
    }
}

impl Surface {
    unsafe fn new(w: i32, h: i32) -> Result<Self> {
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        SelectObject(dc, bitmap);
        Ok(Surface {
            dc,
            bitmap,
            bits: bits.cast(),
            w,
            h,
        })
    }

    fn canvas(&mut self, s: i32) -> Canvas<'_> {
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize) };
        Canvas {
            pixels,
            w: self.w,
            h: self.h,
            s,
            turn: 0,
        }
    }

    /// Moves the window and swaps in the new pixels in one atomic call.
    unsafe fn present(&self, hwnd: HWND, x: i32, y: i32) {
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            hwnd,
            None,
            Some(&POINT { x, y }),
            Some(&SIZE { cx: self.w, cy: self.h }),
            self.dc,
            Some(&POINT::default()),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sound {
    Paddle,
    Wall,
    PlayerPoint,
    ClawdPoint,
    PlayerWins,
    ClawdWins,
    Move,
    Capture,
}

/// Plays a short synthesized blip without blocking.
fn play(sound: Sound) {
    static WAVS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    let wavs = WAVS.get_or_init(|| {
        vec![
            tone(880.0, 45, 0.22),
            tone(440.0, 25, 0.12),
            tone(660.0, 160, 0.2),
            tone(262.0, 200, 0.2),
            tone(988.0, 350, 0.2),
            tone(196.0, 400, 0.2),
            tone(520.0, 35, 0.15),
            tone(300.0, 70, 0.2),
        ]
    });
    let wav = &wavs[sound as usize];
    unsafe {
        let _ = PlaySoundW(
            PCWSTR(wav.as_ptr().cast()),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

/// A mono 16-bit WAV of a sine tone that fades out.
fn tone(freq: f64, ms: u32, volume: f64) -> Vec<u8> {
    const RATE: u32 = 22050;
    let samples = RATE * ms / 1000;
    let mut wav = Vec::with_capacity(44 + samples as usize * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples * 2).to_le_bytes());
    for i in 0..samples {
        let fade = 1.0 - i as f64 / samples as f64;
        let v = (2.0 * PI * freq * i as f64 / RATE as f64).sin() * fade * volume * i16::MAX as f64;
        wav.extend_from_slice(&(v as i16).to_le_bytes());
    }
    wav
}

/// Ping pong rules and physics, in screen pixels. You play the left side, Clawd the right.
/// The table (left/top/right/bottom) is a quarter of the screen, centered.
struct Pong {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    s: f64,
    ball_unit: i32,
    ball_size: f64,
    paddle_w: f64,
    paddle_h: f64,
    ball: (f64, f64), // top-left corner
    vel: (f64, f64),
    speed: f64, // 0 while waiting to serve
    player_x: f64,
    player_y: f64,
    player_vy: f64,
    clawd_x: f64,
    clawd_y: f64,
    aim_error: f64,   // where on its paddle Clawd is trying to meet the ball
    scores: [u32; 2], // you, Clawd
    hits: [u32; 2],
    serve_at: f64,
    serve_dir: f64,
    mood_until: f64,
    over_at: Option<f64>,
    sound: Option<Sound>,
}

impl Pong {
    fn new(area: RECT, pet: &mut Pet) -> Self {
        let s = pet.s;
        let (w, h) = ((area.right - area.left) as f64, (area.bottom - area.top) as f64);
        let (left, top) = (area.left as f64 + w / 4.0, area.top as f64 + h / 4.0);
        let (right, bottom) = (left + w / 2.0, top + h / 2.0);
        let ball_unit = (s / 3).max(2);
        let (paddle_w, paddle_h) = ((PADDLE_W * s) as f64, (PADDLE_H * s) as f64);
        let paddle_y = (top + bottom - paddle_h) / 2.0;
        let mut pong = Pong {
            left,
            top,
            right,
            bottom,
            s: s as f64,
            ball_unit,
            ball_size: (7 * ball_unit) as f64,
            paddle_w,
            paddle_h,
            ball: (0.0, 0.0),
            vel: (0.0, 0.0),
            speed: 0.0,
            player_x: left + s as f64,
            player_y: paddle_y,
            player_vy: 0.0,
            clawd_x: right - s as f64 - paddle_w, // Clawd stands just off the table's right edge
            clawd_y: paddle_y,
            aim_error: 0.0,
            scores: [0, 0],
            hits: [0, 0],
            serve_at: pet.now + 1.5,
            serve_dir: -1.0,
            mood_until: 0.0,
            over_at: None,
            sound: None,
        };
        pong.center_ball();
        pet.enter_game();
        pong.place_clawd(pet);
        pong
    }

    fn center_ball(&mut self) {
        self.speed = 0.0;
        self.ball = (
            (self.left + self.right - self.ball_size) / 2.0,
            (self.top + self.bottom - self.ball_size) / 2.0,
        );
    }

    fn ball_center(&self) -> POINT {
        let half = self.ball_size / 2.0;
        POINT {
            x: (self.ball.0 + half) as i32,
            y: (self.ball.1 + half) as i32,
        }
    }

    fn overlaps(&self, paddle_y: f64) -> bool {
        self.ball.1 + self.ball_size > paddle_y && self.ball.1 < paddle_y + self.paddle_h
    }

    /// Advances the game; returns false once it's over and Clawd has finished reacting.
    fn update(&mut self, now: f64, dt: f64, cursor_y: f64, pet: &mut Pet) -> bool {
        self.sound = None;
        let target = (cursor_y - self.paddle_h / 2.0).clamp(self.top, self.bottom - self.paddle_h);
        if dt > 0.0 {
            self.player_vy = (target - self.player_y) / dt;
        }
        self.player_y = target;

        if let Some(end) = self.over_at {
            if now >= end {
                return false;
            }
        } else if self.speed == 0.0 {
            if now >= self.serve_at {
                self.serve(pet);
            }
        } else {
            self.move_ball(now, dt, pet);
        }
        self.move_clawd(dt);
        self.place_clawd(pet);
        if self.over_at.is_none() && now >= self.mood_until && pet.state != State::Pong {
            pet.set_state(State::Pong, None);
        }
        true
    }

    fn serve(&mut self, pet: &mut Pet) {
        self.speed = BALL_START * self.s;
        let angle = pet.uniform(-0.45, 0.45);
        self.vel = (self.serve_dir * self.speed * angle.cos(), self.speed * angle.sin());
        self.pick_aim(pet);
    }

    /// Clawd aims for a random spot on its paddle; faster balls make it sloppier, until it can whiff entirely.
    fn pick_aim(&mut self, pet: &mut Pet) {
        let sloppiness = 0.3 + 0.55 * self.speed / (BALL_MAX * self.s);
        self.aim_error = pet.uniform(-sloppiness, sloppiness) * self.paddle_h;
    }

    fn move_ball(&mut self, now: f64, dt: f64, pet: &mut Pet) {
        let prev_x = self.ball.0;
        self.ball.0 += self.vel.0 * dt;
        self.ball.1 += self.vel.1 * dt;
        let lowest = self.bottom - self.ball_size;
        if self.ball.1 < self.top {
            self.ball.1 = 2.0 * self.top - self.ball.1;
            self.vel.1 = self.vel.1.abs();
            self.sound = Some(Sound::Wall);
        } else if self.ball.1 > lowest {
            self.ball.1 = 2.0 * lowest - self.ball.1;
            self.vel.1 = -self.vel.1.abs();
            self.sound = Some(Sound::Wall);
        }

        // Compare with last frame's position too, so a fast ball can't tunnel through a paddle.
        let player_face = self.player_x + self.paddle_w;
        let clawd_face = self.clawd_x - self.ball_size;
        if self.vel.0 < 0.0 && prev_x >= player_face && self.ball.0 < player_face && self.overlaps(self.player_y) {
            self.ball.0 = player_face;
            self.bounce(1.0, self.player_y, self.player_vy * 0.15);
            self.hits[0] += 1;
            self.pick_aim(pet);
        } else if self.vel.0 > 0.0 && prev_x <= clawd_face && self.ball.0 > clawd_face && self.overlaps(self.clawd_y) {
            self.ball.0 = clawd_face;
            self.bounce(-1.0, self.clawd_y, 0.0);
            self.hits[1] += 1;
            pet.spawn(STAR_SHAPE, STAR, (OX - 2) as f64, (OY + 2) as f64, -4.0, -2.0, 0.4);
        }

        if self.ball.0 + self.ball_size < self.left {
            self.point(false, now, pet);
        } else if self.ball.0 > self.right {
            self.point(true, now, pet);
        }
    }

    /// Classic pong: where the ball meets the paddle sets its angle, and every hit speeds it up.
    fn bounce(&mut self, dir: f64, paddle_y: f64, spin: f64) {
        self.speed = (self.speed * BALL_SPEEDUP).min(BALL_MAX * self.s);
        let offset = (self.ball.1 + self.ball_size / 2.0) - (paddle_y + self.paddle_h / 2.0);
        let angle = (offset / ((self.paddle_h + self.ball_size) / 2.0)).clamp(-1.0, 1.0) * MAX_BOUNCE_ANGLE;
        let vy = (self.speed * angle.sin() + spin).clamp(-0.85 * self.speed, 0.85 * self.speed);
        self.vel = (dir * (self.speed * self.speed - vy * vy).sqrt(), vy);
        self.sound = Some(Sound::Paddle);
    }

    fn point(&mut self, player_scored: bool, now: f64, pet: &mut Pet) {
        let side = if player_scored { 0 } else { 1 };
        self.scores[side] += 1;
        self.center_ball();
        self.serve_at = now + SERVE_DELAY;
        self.serve_dir = if player_scored { 1.0 } else { -1.0 }; // serve to whoever lost the point
        self.mood_until = now + SERVE_DELAY;
        if player_scored {
            pet.set_state(State::Dizzy, None);
            self.sound = Some(Sound::PlayerPoint);
        } else {
            pet.set_state(State::Happy, None);
            pet.spawn(HEART_SHAPE, HEART, (OX + 5) as f64, (OY - 6) as f64, 0.0, -4.0, 1.2);
            self.sound = Some(Sound::ClawdPoint);
        }
        if self.scores[side] < WIN_SCORE {
            return;
        }
        self.over_at = Some(now + 4.0);
        if player_scored {
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

    fn move_clawd(&mut self, dt: f64) {
        let rest = (self.top + self.bottom - self.paddle_h) / 2.0;
        let incoming = self.speed > 0.0
            && self.vel.0 > 0.0
            && self.ball.0 > self.left + (self.right - self.left) * CLAWD_REACTS_AT;
        let target = if incoming {
            self.predict_y(self.clawd_x - self.ball_size) + (self.ball_size - self.paddle_h) / 2.0 + self.aim_error
        } else {
            rest
        };
        let reach = CLAWD_REACH * self.s * dt;
        self.clawd_y =
            (self.clawd_y + (target - self.clawd_y).clamp(-reach, reach)).clamp(self.top, self.bottom - self.paddle_h);
    }

    /// Where the ball's top edge will be when it reaches `x`, folding in bounces off the top and bottom.
    fn predict_y(&self, x: f64) -> f64 {
        let span = self.bottom - self.ball_size - self.top;
        let travel = (x - self.ball.0) / self.vel.0 * self.vel.1;
        let y = (self.ball.1 - self.top + travel).rem_euclid(2.0 * span);
        self.top + if y > span { 2.0 * span - y } else { y }
    }

    fn place_clawd(&self, pet: &mut Pet) {
        // Clawd's left arm (sprite column OX) holds the back of the paddle.
        pet.x = self.clawd_x + self.paddle_w - OX as f64 * self.s;
        pet.y = self.clawd_y + self.paddle_h / 2.0 - (OY + 4) as f64 * self.s;
    }
}

/// A layered window showing one game piece. Most are click-through; the chess board takes clicks.
struct Layer {
    hwnd: HWND,
    surface: Surface,
}

impl Layer {
    unsafe fn new(instance: HMODULE, w: i32, h: i32) -> Result<Self> {
        Self::with_class(instance, w, h, w!("ClawdLayer"), WS_EX_TRANSPARENT)
    }

    unsafe fn with_class(instance: HMODULE, w: i32, h: i32, class: PCWSTR, extra: WINDOW_EX_STYLE) -> Result<Self> {
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | extra,
            class,
            w!("Clawd ping pong"),
            WS_POPUP,
            0,
            0,
            w,
            h,
            None,
            None,
            instance,
            None,
        )?;
        let surface = Surface::new(w, h)?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Ok(Layer { hwnd, surface })
    }

    fn present(&self, x: f64, y: f64) {
        unsafe { self.surface.present(self.hwnd, x.round() as i32, y.round() as i32) }
    }
}

impl Drop for Layer {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// The windows that draw a ping pong game: the table, ball, both paddles and the scoreboard.
struct PongView {
    _table: Layer,
    ball: Layer,
    player: Layer,
    clawd: Layer,
    score: Layer,
    score_unit: i32,
    shown_scores: Option<[u32; 2]>,
}

impl PongView {
    unsafe fn new(instance: HMODULE, pong: &Pong) -> Result<Self> {
        let (paddle_w, paddle_h) = (pong.paddle_w as i32, pong.paddle_h as i32);
        let (table_w, table_h) = ((pong.right - pong.left) as i32, (pong.bottom - pong.top) as i32);
        let line = pong.ball_unit;
        let score_unit = (pong.s as i32 * 2 / 3).max(3);
        // Created first so everything else sits on top of it.
        let mut table = Layer::new(instance, table_w, table_h)?;
        {
            let mut canvas = table.surface.canvas(1);
            canvas.px(0, 0, table_w, table_h, TABLE);
            for (x, y, w, h) in [
                (0, 0, table_w, line),
                (0, table_h - line, table_w, line),
                (0, 0, line, table_h),
                (table_w - line, 0, line, table_h),
            ] {
                canvas.px(x, y, w, h, TABLE_LINE);
            }
            canvas.px(0, (table_h - line / 2) / 2, table_w, (line / 2).max(1), TABLE_LINE); // center line
            canvas.px((table_w - line) / 2, 0, line, table_h, NET);
        }
        table.present(pong.left, pong.top);
        let mut player = Layer::new(instance, paddle_w, paddle_h)?;
        let mut clawd = Layer::new(instance, paddle_w, paddle_h)?;
        paint_paddle(&mut player.surface, PLAYER_PADDLE, PLAYER_PADDLE_EDGE);
        paint_paddle(&mut clawd.surface, CLAWD_PADDLE, CLAWD_PADDLE_EDGE);
        Ok(PongView {
            _table: table,
            ball: Layer::new(instance, 7 * pong.ball_unit, 7 * pong.ball_unit)?,
            player,
            clawd,
            score: Layer::new(instance, 17 * score_unit, 9 * score_unit)?,
            score_unit,
            shown_scores: None,
        })
    }

    fn render(&mut self, pong: &Pong, now: f64) {
        let blink_off = ((pong.serve_at - now) * 4.0).max(0.0) as i64 % 2 == 1; // the ball blinks before a serve
        let visible = pong.over_at.is_none() && (pong.speed > 0.0 || !blink_off);
        {
            let mut canvas = self.ball.surface.canvas(pong.ball_unit);
            canvas.clear();
            if visible {
                canvas.shape_part(BALL_SHAPE, b'O', BALL_EDGE, 0, 0);
                canvas.shape_part(BALL_SHAPE, b'W', BALL_FILL, 0, 0);
            }
        }
        self.ball.present(pong.ball.0, pong.ball.1);
        self.player.present(pong.player_x, pong.player_y);
        self.clawd.present(pong.clawd_x, pong.clawd_y);

        if self.shown_scores != Some(pong.scores) {
            self.shown_scores = Some(pong.scores);
            draw_scoreboard(&mut self.score.surface.canvas(self.score_unit), pong.scores);
        }
        // Just above the table.
        let x = (pong.left + pong.right - (17 * self.score_unit) as f64) / 2.0;
        self.score.present(x, pong.top - (9 * self.score_unit) as f64 - pong.s);
    }
}

/// "you - Clawd" on a dark rounded panel, 17x9 cells.
fn draw_scoreboard(canvas: &mut Canvas, scores: [u32; 2]) {
    canvas.clear();
    canvas.px(1, 0, 15, 9, SCOREBOARD); // rounded corners
    canvas.px(0, 1, 17, 7, SCOREBOARD);
    canvas.shape(&DIGITS[scores[0] as usize % 10], PLAYER_PADDLE, 2, 2);
    canvas.px(7, 4, 3, 1, SCORE_DASH);
    canvas.shape(&DIGITS[scores[1] as usize % 10], BODY, 12, 2);
}

/// A chess game against Clawd: you play white, Clawd searches for its reply on a background thread.
struct Chess {
    pos: chess::Position,
    history: Vec<u64>, // every position so far, for repetition draws
    x: f64,            // board window's top-left corner
    y: f64,
    unit: i32,
    selected: Option<u8>,
    targets: Vec<u8>, // where the selected piece may go
    last_move: Option<chess::Move>,
    slide: Option<(chess::Move, f64)>, // the piece currently gliding to its square, and when it set off
    thinking: Option<(mpsc::Receiver<Option<chess::Move>>, f64)>,
    status: chess::Status,
    mood_until: f64,
    version: u64, // bumped whenever the board needs redrawing
    sound: Option<Sound>,
}

impl Chess {
    fn new(area: RECT, pet: &mut Pet) -> Self {
        let s = pet.s;
        let unit = ((s as f64 * 0.65).round() as i32).max(3);
        let size = (BOARD_UNITS * unit) as f64;
        let with_clawd = size + (18 * s) as f64; // center the board and Clawd beside it together
        let pos = chess::Position::start();
        let game = Chess {
            history: vec![pos.key()],
            pos,
            x: (area.left + area.right) as f64 / 2.0 - with_clawd / 2.0,
            y: (area.top + area.bottom) as f64 / 2.0 - size / 2.0,
            unit,
            selected: None,
            targets: Vec::new(),
            last_move: None,
            slide: None,
            thinking: None,
            status: chess::Status::Playing,
            mood_until: 0.0,
            version: 0,
            sound: None,
        };
        pet.enter_game();
        pet.set_state(State::Idle, None);
        game.place_clawd(pet);
        game
    }

    fn size(&self) -> f64 {
        (BOARD_UNITS * self.unit) as f64
    }

    /// Top-left pixel of a square inside the board window (white at the bottom).
    fn square_origin(&self, sq: u8) -> (i32, i32) {
        let (rank, file) = ((sq / 8) as i32, (sq % 8) as i32);
        ((2 + file * SQUARE) * self.unit, (2 + (7 - rank) * SQUARE) * self.unit)
    }

    fn square_at(&self, p: POINT) -> Option<u8> {
        let col = ((p.x as f64 - self.x) / self.unit as f64 - 2.0) / SQUARE as f64;
        let row = ((p.y as f64 - self.y) / self.unit as f64 - 2.0) / SQUARE as f64;
        ((0.0..8.0).contains(&col) && (0.0..8.0).contains(&row)).then(|| (7 - row as u8) * 8 + col as u8)
    }

    /// Handles a click on the board; returns false when the finished game should close.
    fn click(&mut self, p: POINT, pet: &mut Pet) -> bool {
        if self.status != chess::Status::Playing {
            return false;
        }
        if self.pos.side != chess::WHITE || self.thinking.is_some() {
            return true;
        }
        let Some(sq) = self.square_at(p) else { return true };
        self.version += 1;
        if let Some(from) = self.selected {
            if self.targets.contains(&sq) {
                self.play(chess::Move { from, to: sq }, pet);
                if self.status == chess::Status::Playing {
                    self.start_thinking(pet);
                }
                return true;
            }
        }
        if self.pos.board[sq as usize] > 0 && self.selected != Some(sq) {
            self.selected = Some(sq);
            self.targets = self
                .pos
                .legal_moves()
                .iter()
                .filter(|m| m.from == sq)
                .map(|m| m.to)
                .collect();
        } else {
            self.selected = None;
            self.targets.clear();
        }
        true
    }

    fn play(&mut self, m: chess::Move, pet: &mut Pet) {
        let now = pet.now;
        let mover = self.pos.side;
        let captured = self.pos.captured(m).abs();
        self.pos = self.pos.play(m);
        self.history.push(self.pos.key());
        self.last_move = Some(m);
        self.slide = Some((m, now));
        self.selected = None;
        self.targets.clear();
        self.version += 1;
        self.sound = Some(if captured != 0 { Sound::Capture } else { Sound::Move });
        self.status = self.pos.status(&self.history);
        self.mood_until = now + 1.2;

        match self.status {
            chess::Status::Checkmate { winner } if winner == chess::WHITE => {
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
            }
            chess::Status::Checkmate { .. } => {
                pet.dance();
                self.sound = Some(Sound::ClawdWins);
            }
            chess::Status::Stalemate | chess::Status::Draw => pet.set_state(State::Curious, None),
            chess::Status::Playing if mover == chess::WHITE => {
                // You took one of Clawd's pieces: a big loss makes it dizzy, a small one puzzled.
                match captured {
                    0 => self.mood_until = now,
                    c if c >= chess::ROOK => pet.set_state(State::Dizzy, None),
                    _ => pet.set_state(State::Curious, None),
                }
            }
            chess::Status::Playing => {
                if captured != 0 {
                    pet.set_state(State::Happy, None);
                    pet.spawn(HEART_SHAPE, HEART, (OX + 5) as f64, (OY - 6) as f64, 0.0, -4.0, 1.2);
                } else if self.pos.in_check(chess::WHITE) {
                    pet.set_state(State::Wave, None);
                } else {
                    self.mood_until = now;
                }
            }
        }
    }

    fn start_thinking(&mut self, pet: &mut Pet) {
        let (send, receive) = mpsc::channel();
        let (pos, history) = (self.pos.clone(), self.history.clone());
        let seed = (pet.random() * u64::MAX as f64) as u64;
        std::thread::spawn(move || {
            let _ = send.send(chess::best_move(&pos, &history, THINK_BUDGET, seed));
        });
        self.thinking = Some((receive, pet.now));
    }

    fn update(&mut self, pet: &mut Pet) {
        let now = pet.now;
        self.sound = None;
        let reply = match &self.thinking {
            Some((receive, start)) if now - start >= THINK_MIN => match receive.try_recv() {
                Ok(reply) => Some(reply),
                Err(mpsc::TryRecvError::Disconnected) => Some(None),
                Err(mpsc::TryRecvError::Empty) => None,
            },
            _ => None,
        };
        if let Some(reply) = reply {
            self.thinking = None;
            if let Some(m) = reply {
                self.play(m, pet);
            }
        }
        if self.slide.is_some_and(|(_, start)| now - start >= PIECE_SLIDE) {
            self.slide = None;
            self.version += 1;
        }
        // Between reactions Clawd types on its laptop while thinking and watches you otherwise.
        if self.status == chess::Status::Playing && now >= self.mood_until {
            let calm = if self.thinking.is_some() {
                State::Code
            } else {
                State::Idle
            };
            if pet.state != calm {
                pet.set_state(calm, None);
            }
        }
        self.place_clawd(pet);
    }

    fn place_clawd(&self, pet: &mut Pet) {
        // Standing on the same line as the board's bottom edge, just to its right.
        pet.x = self.x + self.size() + pet.s as f64 - (OX * pet.s) as f64;
        pet.y = self.y + self.size() - pet.h as f64;
    }
}

/// The clickable board window.
struct ChessView {
    board: Layer,
    drawn: Option<u64>,
}

impl ChessView {
    unsafe fn new(instance: HMODULE, game: &Chess) -> Result<Self> {
        let size = game.size() as i32;
        let board = Layer::with_class(instance, size, size, w!("ClawdBoard"), WINDOW_EX_STYLE(0))?;
        Ok(ChessView { board, drawn: None })
    }

    fn render(&mut self, game: &Chess, now: f64) {
        let sliding = game.slide.map(|(m, start)| (m, ((now - start) / PIECE_SLIDE).min(1.0)));
        if sliding.is_none() && self.drawn == Some(game.version) {
            return; // nothing changed; the window keeps its last picture
        }
        self.drawn = Some(game.version);
        draw_board(game, sliding, &mut self.board.surface.canvas(1));
        self.board.present(game.x, game.y);
    }
}

/// Paints the board, highlights, pieces (one possibly mid-slide) and any game-over banner, in pixels.
fn draw_board(game: &Chess, sliding: Option<(chess::Move, f64)>, canvas: &mut Canvas) {
    let u = game.unit;
    canvas.px(0, 0, BOARD_UNITS * u, BOARD_UNITS * u, BOARD_FRAME);

    let side = game.pos.side;
    let in_check = game.pos.in_check(side).then(|| game.pos.king_square(side)).flatten();
    for sq in 0..64u8 {
        let (x, y) = game.square_origin(sq);
        let light = (sq / 8 + sq % 8) % 2 == 1;
        let last = game.last_move.is_some_and(|m| m.from == sq || m.to == sq);
        let color = match () {
            _ if game.selected == Some(sq) => BOARD_SELECTED,
            _ if in_check == Some(sq as usize) => BOARD_CHECK,
            _ if last && light => BOARD_LAST_LIGHT,
            _ if last => BOARD_LAST_DARK,
            _ if light => BOARD_LIGHT,
            _ => BOARD_DARK,
        };
        canvas.px(x, y, SQUARE * u, SQUARE * u, color);
        if game.targets.contains(&sq) {
            if game.pos.board[sq as usize] != 0 {
                for (dx, dy) in [(0, 0), (9, 0), (0, 9), (9, 9)] {
                    canvas.shade(x + dx * u, y + dy * u, 3 * u, 3 * u, 160); // capture: dark corners
                }
            } else {
                canvas.shade(x + 5 * u, y + 4 * u, 2 * u, 4 * u, 160); // move: a small round dot
                canvas.shade(x + 4 * u, y + 5 * u, u, 2 * u, 160);
                canvas.shade(x + 7 * u, y + 5 * u, u, 2 * u, 160);
            }
        }
    }

    for sq in 0..64u8 {
        let piece = game.pos.board[sq as usize];
        if piece != 0 && sliding.is_none_or(|(m, _)| m.to != sq) {
            let (x, y) = game.square_origin(sq);
            draw_piece(canvas, piece, x, y, u);
        }
    }
    if let Some((m, t)) = sliding {
        let ((x0, y0), (x1, y1)) = (game.square_origin(m.from), game.square_origin(m.to));
        let ease = t * t * (3.0 - 2.0 * t);
        let lerp = |a: i32, b: i32| a + ((b - a) as f64 * ease).round() as i32;
        draw_piece(canvas, game.pos.board[m.to as usize], lerp(x0, x1), lerp(y0, y1), u);
    }

    if game.status != chess::Status::Playing {
        let (title, color) = match game.status {
            chess::Status::Checkmate { winner } if winner == chess::WHITE => ("YOU WIN", PLAYER_PADDLE),
            chess::Status::Checkmate { .. } => ("CLAWD WINS", BODY),
            chess::Status::Stalemate => ("STALEMATE", BANNER_TEXT),
            _ => ("DRAW", BANNER_TEXT),
        };
        let center = BOARD_UNITS * u / 2;
        canvas.shade(0, 38 * u, BOARD_UNITS * u, 24 * u, 70);
        canvas.text(title, center, 42 * u, u, color);
        canvas.text("CLICK TO CLOSE", center, 54 * u, (u / 2).max(2), BANNER_HINT);
    }
}

fn draw_piece(canvas: &mut Canvas, piece: i8, x: i32, y: i32, u: i32) {
    let (fill, edge) = if piece > 0 {
        (WHITE_PIECE, WHITE_PIECE_EDGE)
    } else {
        (BODY, CLAWD_PIECE_EDGE)
    };
    canvas.pattern(&PIECES[piece.unsigned_abs() as usize - 1], x, y, u, |m| match m {
        b'#' => Some(edge),
        b'o' => Some(fill),
        _ => None,
    });
}

/// The games, with the windows that draw them.
enum Game {
    Pong(Pong, PongView),
    Badminton(badminton::Badminton, badminton::BadmintonView),
    Chess(Chess, ChessView),
}

fn paint_paddle(surface: &mut Surface, fill: u32, edge: u32) {
    let (w, h) = (surface.w, surface.h);
    let mut canvas = surface.canvas(1);
    canvas.clear();
    canvas.px(0, 0, w, h, edge);
    canvas.px(2, 2, w - 4, h - 4, fill);
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Pacing {
    VBlank, // DXGI WaitForVBlank on the monitor Clawd is on
    Dwm,    // DwmFlush, the compositor's own frame clock
    Timer,  // last resort when neither blocks
}

/// Paces the main loop to the refresh rate of whichever monitor Clawd is on.
struct FramePacer {
    outputs: Vec<(HMONITOR, IDXGIOutput)>,
    last_enumerated: Instant,
    pacing: Pacing,
    too_fast: u32,
}

impl FramePacer {
    fn new() -> Self {
        let outputs = Self::enumerate_outputs();
        let pacing = if outputs.is_empty() {
            Pacing::Dwm
        } else {
            Pacing::VBlank
        };
        FramePacer {
            outputs,
            last_enumerated: Instant::now(),
            pacing,
            too_fast: 0,
        }
    }

    fn enumerate_outputs() -> Vec<(HMONITOR, IDXGIOutput)> {
        let mut outputs = Vec::new();
        let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
            return outputs;
        };
        let mut adapter_index = 0;
        while let Ok(adapter) = unsafe { factory.EnumAdapters1(adapter_index) } {
            let mut output_index = 0;
            while let Ok(output) = unsafe { adapter.EnumOutputs(output_index) } {
                if let Ok(desc) = unsafe { output.GetDesc() } {
                    outputs.push((desc.Monitor, output));
                }
                output_index += 1;
            }
            adapter_index += 1;
        }
        outputs
    }

    fn wait(&mut self, monitor: HMONITOR) {
        let start = Instant::now();
        let waited = match self.pacing {
            Pacing::VBlank => self.wait_for_vblank(monitor) || unsafe { DwmFlush() }.is_ok(),
            Pacing::Dwm => unsafe { DwmFlush() }.is_ok(),
            Pacing::Timer => false,
        };
        if !waited {
            std::thread::sleep(Duration::from_millis(7));
        }
        // A wait that keeps returning instantly isn't tracking the display (remote
        // desktop, odd drivers); step down to the next pacing method.
        if start.elapsed() < Duration::from_micros(500) {
            self.too_fast += 1;
            if self.too_fast > 10 {
                self.too_fast = 0;
                self.pacing = match self.pacing {
                    Pacing::VBlank => Pacing::Dwm,
                    _ => Pacing::Timer,
                };
            }
        } else {
            self.too_fast = 0;
        }
    }

    fn wait_for_vblank(&mut self, monitor: HMONITOR) -> bool {
        let mut found = self.outputs.iter().find(|(m, _)| *m == monitor);
        if found.is_none() && self.last_enumerated.elapsed() > Duration::from_secs(1) {
            // Displays were plugged in or rearranged.
            self.outputs = Self::enumerate_outputs();
            self.last_enumerated = Instant::now();
            found = self.outputs.iter().find(|(m, _)| *m == monitor);
        }
        found.is_some_and(|(_, output)| unsafe { output.WaitForVBlank() }.is_ok())
    }
}

/// Appends the measured frame rate to $CLAWD_LOG once a second, when set.
struct FrameLog {
    path: Option<String>,
    frames: u32,
    since: Instant,
}

impl FrameLog {
    fn from_env() -> Self {
        FrameLog {
            path: std::env::var("CLAWD_LOG").ok(),
            frames: 0,
            since: Instant::now(),
        }
    }

    fn frame(&mut self, pacing: Pacing, state: State) {
        let Some(path) = &self.path else { return };
        self.frames += 1;
        let elapsed = self.since.elapsed().as_secs_f64();
        if elapsed < 1.0 {
            return;
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(
                file,
                "fps={:.1} pacing={pacing:?} state={state:?}",
                self.frames as f64 / elapsed
            );
        }
        self.frames = 0;
        self.since = Instant::now();
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let push = |input| INPUTS.with_borrow_mut(|inputs| inputs.push(input));
    match msg {
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            let pos = GetMessagePos();
            push(Input::Press(POINT {
                x: (pos & 0xFFFF) as i16 as i32,
                y: (pos >> 16) as i16 as i32,
            }));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            push(Input::Release);
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            push(Input::Release);
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            push(Input::Menu);
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn layer_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn board_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let push = |input| INPUTS.with_borrow_mut(|inputs| inputs.push(input));
    match msg {
        WM_LBUTTONDOWN => {
            let pos = GetMessagePos();
            push(Input::BoardClick(POINT {
                x: (pos & 0xFFFF) as i16 as i32,
                y: (pos >> 16) as i16 as i32,
            }));
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            push(Input::Menu);
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Shows the right-click menu and acts on pet choices; returns the chosen item so the caller can
/// start or stop games. `stop` labels the stop item while a game is running.
unsafe fn show_menu(hwnd: HWND, pet: &mut Pet, at: POINT, stop: Option<PCWSTR>) -> Result<usize> {
    let menu = CreatePopupMenu()?;
    if let Some(label) = stop {
        AppendMenuW(menu, MF_STRING, MENU_STOP, label)?;
    } else {
        let wander_flags = if pet.wander { MF_STRING | MF_CHECKED } else { MF_STRING };
        AppendMenuW(menu, MF_STRING, MENU_PONG, w!("Play ping pong"))?;
        AppendMenuW(menu, MF_STRING, MENU_BADMINTON, w!("Play badminton"))?;
        AppendMenuW(menu, MF_STRING, MENU_CHESS, w!("Play chess"))?;
        AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())?;
        AppendMenuW(menu, MF_STRING, MENU_PET, w!("Pet"))?;
        AppendMenuW(menu, MF_STRING, MENU_DANCE, w!("Dance"))?;
        AppendMenuW(menu, MF_STRING, MENU_CODE, w!("Write some code"))?;
        AppendMenuW(menu, MF_STRING, MENU_FLIP, w!("Backflip"))?;
        AppendMenuW(menu, MF_STRING, MENU_NAP, w!("Nap time"))?;
        AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())?;
        AppendMenuW(menu, wander_flags, MENU_WANDER, w!("Wander around"))?;
    }
    AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())?;
    AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit"))?;

    // The menu only dismisses properly when its owner is in the foreground.
    let _ = SetForegroundWindow(hwnd);
    let choice = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, at.x, at.y, 0, hwnd, None).0 as usize;
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    DestroyMenu(menu)?;

    match choice {
        MENU_PET => pet.pet(),
        MENU_DANCE => pet.dance(),
        MENU_CODE => pet.code(),
        MENU_FLIP => pet.flip(),
        MENU_NAP => pet.nap(),
        MENU_WANDER => pet.wander = !pet.wander,
        MENU_QUIT => DestroyWindow(hwnd)?,
        _ => {}
    }
    Ok(choice)
}

fn monitor_at(point: POINT) -> HMONITOR {
    unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) }
}

/// The monitor's area minus the taskbar.
fn work_area(monitor: HMONITOR) -> RECT {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetMonitorInfoW(monitor, &mut info);
    }
    info.rcWork
}

fn cursor_pos() -> POINT {
    let mut point = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut point);
    }
    point
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [State; 16] = [
        State::Idle,
        State::Walk,
        State::Wave,
        State::Happy,
        State::Drag,
        State::Fall,
        State::Dizzy,
        State::Yawn,
        State::Sleep,
        State::Dance,
        State::Code,
        State::Curious,
        State::Tickle,
        State::Flip,
        State::Tada,
        State::Pong,
    ];
    const TIMES: [f64; 6] = [0.05, 0.15, 0.3, 0.45, 0.9, 1.5];

    /// Renders every state at a few points in time; set CLAWD_SHEET=<path.ppm> to save them as a contact sheet.
    #[test]
    fn every_state_renders() {
        let s = 4;
        let (fw, fh) = (GRID_W * s, GRID_H * s);
        let area = RECT {
            left: 0,
            top: 0,
            right: 4000,
            bottom: 3000,
        };
        let mut sheet = vec![0xFF30_3030u32; (fw * fh) as usize * STATES.len() * TIMES.len()];
        let sheet_w = fw * TIMES.len() as i32;
        for (row, &state) in STATES.iter().enumerate() {
            for (col, &time) in TIMES.iter().enumerate() {
                let mut pet = Pet::new(s, area);
                pet.y = (area.bottom - pet.h) as f64;
                pet.next_blink = 99.0;
                pet.dir = if col % 2 == 0 { -1 } else { 1 };
                pet.set_state(state, Some(60.0));
                if state == State::Flip {
                    pet.flip();
                }
                let far = POINT { x: -1000, y: -1000 };
                let steps = (time * 60.0) as i32;
                for i in 1..=steps {
                    pet.update(i as f64 / 60.0, 1.0 / 60.0, far, area);
                }
                let mut pixels = vec![0u32; (fw * fh) as usize];
                pet.render(
                    far,
                    &mut Canvas {
                        pixels: &mut pixels,
                        w: fw,
                        h: fh,
                        s,
                        turn: 0,
                    },
                );
                assert!(pixels.iter().any(|&p| p == BODY), "{state:?} at {time}s drew no body");
                for y in 0..fh {
                    for x in 0..fw {
                        let p = pixels[(y * fw + x) as usize];
                        if p != 0 {
                            let (sx, sy) = (col as i32 * fw + x, row as i32 * fh + y);
                            sheet[(sy * sheet_w + sx) as usize] = p;
                        }
                    }
                }
            }
        }
        if let Ok(path) = std::env::var("CLAWD_SHEET") {
            let mut out = format!("P6 {} {} 255\n", sheet_w, fh * STATES.len() as i32).into_bytes();
            for p in sheet {
                out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
            std::fs::write(path, out).unwrap();
        }
    }

    /// Plays a whole ping pong game on a 2560x1500 screen at 165 fps, with `player` choosing the cursor height.
    fn play_pong(player: impl Fn(&Pong) -> f64, max_secs: f64) -> (Pong, f64) {
        let area = RECT {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1500,
        };
        let mut pet = Pet::new(9, area);
        let mut pong = Pong::new(area, &mut pet);
        let dt = 1.0 / 165.0;
        let mut now = 0.0;
        while now < max_secs {
            now += dt;
            pet.now = now;
            if !pong.update(now, dt, player(&pong), &mut pet) {
                break;
            }
            pet.update(now, dt, POINT::default(), area);
        }
        assert!(!pet.in_game || pong.over_at.is_some());
        (pong, now)
    }

    #[test]
    fn clawd_rallies_but_a_perfect_player_wins() {
        let (pong, secs) = play_pong(|p| p.ball.1 + p.ball_size / 2.0, 1200.0);
        println!(
            "perfect player: score {:?} in {secs:.0}s, returns {:?}",
            pong.scores, pong.hits
        );
        assert_eq!(pong.scores[0], WIN_SCORE);
        assert!(pong.hits[1] >= 10, "Clawd should return plenty of balls first");
    }

    /// Plays chess through the same clicks the board window sends, waiting on Clawd's real replies.
    /// Set CLAWD_BOARD=<path.ppm> to save the final board.
    #[test]
    fn chess_plays_through_clicks() {
        let area = RECT {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1500,
        };
        let mut pet = Pet::new(9, area);
        let clock = Instant::now();
        let mut game = Chess::new(area, &mut pet);
        let click = |game: &Chess, sq: u8| {
            let (x, y) = game.square_origin(sq);
            let half = SQUARE * game.unit / 2;
            POINT {
                x: game.x as i32 + x + half,
                y: game.y as i32 + y + half,
            }
        };
        // e4, Nf3, Bc4, d3, Nc3, then whatever is legal.
        let openings = [(12, 28), (6, 21), (5, 26), (11, 19), (1, 18)];
        for round in 0..8 {
            assert_eq!(game.pos.side, chess::WHITE);
            let legal = game.pos.legal_moves();
            let m = openings
                .get(round)
                .map(|&(from, to)| chess::Move { from, to })
                .filter(|m| legal.contains(m))
                .unwrap_or(legal[0]);
            pet.now = clock.elapsed().as_secs_f64();
            assert!(game.click(click(&game, m.from), &mut pet));
            assert_eq!(game.selected, Some(m.from));
            assert!(game.targets.contains(&m.to));
            assert!(game.click(click(&game, m.to), &mut pet));
            if game.status != chess::Status::Playing {
                break;
            }
            assert!(game.thinking.is_some(), "Clawd should start thinking after your move");
            game.update(&mut pet);
            assert!(
                pet.state == State::Code || pet.now < game.mood_until,
                "Clawd types while it thinks"
            );
            while game.thinking.is_some() {
                std::thread::sleep(Duration::from_millis(20));
                pet.now = clock.elapsed().as_secs_f64();
                game.update(&mut pet);
            }
            assert!(pet.now >= THINK_MIN, "Clawd should take a moment before answering");
            if game.status != chess::Status::Playing {
                break;
            }
        }
        assert!(game.history.len() >= 11);
        assert!(
            game.click(POINT { x: -50, y: -50 }, &mut pet),
            "clicks off the board are ignored"
        );

        if let Ok(path) = std::env::var("CLAWD_BOARD") {
            let size = game.size() as i32;
            let mut pixels = vec![0u32; (size * size) as usize];
            game.selected = game
                .pos
                .legal_moves()
                .iter()
                .map(|m| m.from)
                .find(|&sq| game.pos.board[sq as usize] == chess::KNIGHT);
            game.targets = game
                .pos
                .legal_moves()
                .iter()
                .filter(|m| Some(m.from) == game.selected)
                .map(|m| m.to)
                .collect();
            draw_board(
                &game,
                None,
                &mut Canvas {
                    pixels: &mut pixels,
                    w: size,
                    h: size,
                    s: 1,
                    turn: 0,
                },
            );
            game.status = chess::Status::Checkmate { winner: chess::BLACK };
            let mut banner = vec![0u32; (size * size) as usize];
            draw_board(
                &game,
                None,
                &mut Canvas {
                    pixels: &mut banner,
                    w: size,
                    h: size,
                    s: 1,
                    turn: 0,
                },
            );
            let mut out = format!("P6 {} {} 255\n", size * 2, size).into_bytes();
            for row in 0..size {
                for p in pixels[(row * size) as usize..((row + 1) * size) as usize]
                    .iter()
                    .chain(&banner[(row * size) as usize..((row + 1) * size) as usize])
                {
                    out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
                }
            }
            std::fs::write(path, out).unwrap();
        }
    }

    #[test]
    fn clawd_beats_a_player_who_never_moves() {
        let (pong, secs) = play_pong(|p| p.top, 600.0);
        println!(
            "idle player: score {:?} in {secs:.0}s, returns {:?}",
            pong.scores, pong.hits
        );
        assert_eq!(pong.scores[1], WIN_SCORE);
    }
}

fn main() -> Result<()> {
    unsafe {
        // Work in real screen pixels so the sprite stays crisp on scaled displays.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let s = ((6.0 * GetDpiForSystem() as f64 / 96.0).round() as i32).max(3);
        let mut pet = Pet::new(s, work_area(monitor_at(POINT::default())));

        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_HAND)?,
            lpszClassName: w!("ClawdPet"),
            ..Default::default()
        };
        RegisterClassW(&class);
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(layer_wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("ClawdLayer"),
            ..Default::default()
        });
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(board_wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_HAND)?,
            lpszClassName: w!("ClawdBoard"),
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("ClawdPet"),
            w!("Clawd"),
            WS_POPUP,
            pet.x as i32,
            pet.y as i32,
            pet.w,
            pet.h,
            None,
            None,
            instance,
            None,
        )?;
        let mut surface = Surface::new(pet.w, pet.h)?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        let mut pacer = FramePacer::new();
        let mut log = FrameLog::from_env();
        let clock = Instant::now();
        let mut last = 0.0;
        let mut last_raise = 0.0;
        let mut game: Option<Game> = None;
        loop {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return Ok(());
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // Sampled right after the previous vblank, so the drag position is as fresh as it gets.
            let cursor = cursor_pos();
            pet.now = clock.elapsed().as_secs_f64();
            // Esc ends the action games (not chess, where a stray key press would throw away a long game).
            let mut stop_game =
                matches!(game, Some(Game::Pong(..) | Game::Badminton(..))) && GetAsyncKeyState(VK_ESCAPE.0 as i32) < 0;
            let mut start = None;
            for input in INPUTS.take() {
                match input {
                    Input::Press(at) => pet.on_press(at),
                    Input::Release => pet.on_release(),
                    Input::BoardClick(at) => {
                        if let Some(Game::Chess(chess, _)) = &mut game {
                            stop_game |= !chess.click(at, &mut pet);
                        }
                    }
                    Input::Menu => {
                        let stop_label = match game {
                            Some(Game::Pong(..)) => Some(w!("Stop ping pong")),
                            Some(Game::Badminton(..)) => Some(w!("Stop badminton")),
                            Some(Game::Chess(..)) => Some(w!("Stop chess")),
                            None => None,
                        };
                        match show_menu(hwnd, &mut pet, cursor, stop_label)? {
                            MENU_STOP => stop_game = true,
                            choice @ (MENU_PONG | MENU_BADMINTON | MENU_CHESS) => start = Some(choice),
                            _ => {}
                        }
                    }
                }
            }

            let now = clock.elapsed().as_secs_f64();
            let dt = (now - last).min(0.05);
            last = now;
            if stop_game && game.take().is_some() {
                pet.leave_game();
            }
            if let Some(choice) = start.filter(|_| game.is_none()) {
                let area = work_area(monitor_at(pet.center()));
                game = Some(match choice {
                    MENU_PONG => {
                        let pong = Pong::new(area, &mut pet);
                        let view = PongView::new(instance, &pong)?;
                        Game::Pong(pong, view)
                    }
                    MENU_BADMINTON => {
                        let match_ = badminton::Badminton::new(area, &mut pet);
                        let view = badminton::BadmintonView::new(instance, &match_)?;
                        Game::Badminton(match_, view)
                    }
                    _ => {
                        let chess = Chess::new(area, &mut pet);
                        let view = ChessView::new(instance, &chess)?;
                        Game::Chess(chess, view)
                    }
                });
                // Keep Clawd in front of the game's windows.
                let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            let (sound, finished) = match &mut game {
                Some(Game::Pong(pong, _)) => {
                    let playing = pong.update(now, dt, cursor.y as f64, &mut pet);
                    (pong.sound, !playing)
                }
                Some(Game::Badminton(match_, _)) => {
                    let playing = match_.update(now, dt, cursor, &mut pet);
                    (match_.sound, !playing)
                }
                Some(Game::Chess(chess, _)) => {
                    chess.update(&mut pet);
                    (chess.sound, false)
                }
                None => (None, false),
            };
            if let Some(sound) = sound {
                play(sound);
            }
            if finished {
                game = None;
                pet.leave_game();
            }

            let monitor = monitor_at(pet.center());
            pet.update(now, dt, cursor, work_area(monitor));
            // In the action games Clawd keeps its eyes on the ball or shuttle instead of the cursor.
            let look = match &game {
                Some(Game::Pong(pong, _)) => pong.ball_center(),
                Some(Game::Badminton(match_, _)) => match_.shuttle_center(),
                _ => cursor,
            };
            pet.render(look, &mut surface.canvas(s));
            surface.present(hwnd, pet.x.round() as i32, pet.y.round() as i32);
            match &mut game {
                Some(Game::Pong(pong, view)) => view.render(pong, now),
                Some(Game::Badminton(match_, view)) => view.render(match_, now),
                Some(Game::Chess(chess, view)) => view.render(chess, now),
                None => {}
            }

            if now - last_raise > 2.0 {
                last_raise = now;
                let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            log.frame(pacer.pacing, pet.state);
            pacer.wait(monitor);
        }
    }
}
