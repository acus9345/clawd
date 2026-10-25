//! Just enough chess for a friendly game against Clawd: legal move generation (castling,
//! en passant, promotion to a queen), game-end detection and an alpha-beta search that
//! stops when its time budget runs out.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

pub const PAWN: i8 = 1;
pub const KNIGHT: i8 = 2;
pub const BISHOP: i8 = 3;
pub const ROOK: i8 = 4;
pub const QUEEN: i8 = 5;
pub const KING: i8 = 6;
pub const WHITE: i8 = 1;
pub const BLACK: i8 = -1;

const MATE: i32 = 100_000;
const MAX_DEPTH: i32 = 6;
const VALUE: [i32; 7] = [0, 100, 320, 330, 500, 900, 0];
const KNIGHT_STEPS: [(i32, i32); 8] = [(1, 2), (2, 1), (2, -1), (1, -2), (-1, -2), (-2, -1), (-2, 1), (-1, 2)];
const KING_STEPS: [(i32, i32); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
const ROOK_DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const BISHOP_DIRS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];

// Piece-square tables (the "simplified evaluation function"), written with a8 first, from white's side.
#[rustfmt::skip]
const PST: [[i32; 64]; 6] = [
    [ // pawn
         0,  0,  0,  0,  0,  0,  0,  0,
        50, 50, 50, 50, 50, 50, 50, 50,
        10, 10, 20, 30, 30, 20, 10, 10,
         5,  5, 10, 25, 25, 10,  5,  5,
         0,  0,  0, 20, 20,  0,  0,  0,
         5, -5,-10,  0,  0,-10, -5,  5,
         5, 10, 10,-20,-20, 10, 10,  5,
         0,  0,  0,  0,  0,  0,  0,  0,
    ],
    [ // knight
        -50,-40,-30,-30,-30,-30,-40,-50,
        -40,-20,  0,  0,  0,  0,-20,-40,
        -30,  0, 10, 15, 15, 10,  0,-30,
        -30,  5, 15, 20, 20, 15,  5,-30,
        -30,  0, 15, 20, 20, 15,  0,-30,
        -30,  5, 10, 15, 15, 10,  5,-30,
        -40,-20,  0,  5,  5,  0,-20,-40,
        -50,-40,-30,-30,-30,-30,-40,-50,
    ],
    [ // bishop
        -20,-10,-10,-10,-10,-10,-10,-20,
        -10,  0,  0,  0,  0,  0,  0,-10,
        -10,  0,  5, 10, 10,  5,  0,-10,
        -10,  5,  5, 10, 10,  5,  5,-10,
        -10,  0, 10, 10, 10, 10,  0,-10,
        -10, 10, 10, 10, 10, 10, 10,-10,
        -10,  5,  0,  0,  0,  0,  5,-10,
        -20,-10,-10,-10,-10,-10,-10,-20,
    ],
    [ // rook
         0,  0,  0,  0,  0,  0,  0,  0,
         5, 10, 10, 10, 10, 10, 10,  5,
        -5,  0,  0,  0,  0,  0,  0, -5,
        -5,  0,  0,  0,  0,  0,  0, -5,
        -5,  0,  0,  0,  0,  0,  0, -5,
        -5,  0,  0,  0,  0,  0,  0, -5,
        -5,  0,  0,  0,  0,  0,  0, -5,
         0,  0,  0,  5,  5,  0,  0,  0,
    ],
    [ // queen
        -20,-10,-10, -5, -5,-10,-10,-20,
        -10,  0,  0,  0,  0,  0,  0,-10,
        -10,  0,  5,  5,  5,  5,  0,-10,
         -5,  0,  5,  5,  5,  5,  0, -5,
          0,  0,  5,  5,  5,  5,  0, -5,
        -10,  5,  5,  5,  5,  5,  0,-10,
        -10,  0,  5,  0,  0,  0,  0,-10,
        -20,-10,-10, -5, -5,-10,-10,-20,
    ],
    [ // king, middlegame
        -30,-40,-40,-50,-50,-40,-40,-30,
        -30,-40,-40,-50,-50,-40,-40,-30,
        -30,-40,-40,-50,-50,-40,-40,-30,
        -30,-40,-40,-50,-50,-40,-40,-30,
        -20,-30,-30,-40,-40,-30,-30,-20,
        -10,-20,-20,-20,-20,-20,-20,-10,
         20, 20,  0,  0,  0,  0, 20, 20,
         20, 30, 10,  0,  0, 10, 30, 20,
    ],
];
#[rustfmt::skip]
const KING_ENDGAME: [i32; 64] = [
    -50,-40,-30,-20,-20,-30,-40,-50,
    -30,-20,-10,  0,  0,-10,-20,-30,
    -30,-10, 20, 30, 30, 20,-10,-30,
    -30,-10, 30, 40, 40, 30,-10,-30,
    -30,-10, 30, 40, 40, 30,-10,-30,
    -30,-10, 20, 30, 30, 20,-10,-30,
    -30,-30,  0,  0,  0,  0,-30,-30,
    -50,-30,-30,-30,-30,-30,-30,-50,
];

/// A move from one square to another; squares count from a1 = 0 to h8 = 63.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    pub from: u8,
    pub to: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Playing,
    Checkmate { winner: i8 },
    Stalemate,
    Draw, // repetition, fifty moves, or not enough material to mate
}

#[derive(Clone, Debug)]
pub struct Position {
    /// Piece kinds, positive for white and negative for black, a1 first.
    pub board: [i8; 64],
    pub side: i8,
    castling: u8, // 1 white short, 2 white long, 4 black short, 8 black long
    ep: Option<u8>,
    halfmove: u32,
}

fn square(rank: i32, file: i32) -> Option<usize> {
    ((0..8).contains(&rank) && (0..8).contains(&file)).then_some((rank * 8 + file) as usize)
}

/// Castling rights that survive a move touching this square.
fn castle_mask(sq: usize) -> u8 {
    match sq {
        0 => !2,
        4 => !3,
        7 => !1,
        56 => !8,
        60 => !12,
        63 => !4,
        _ => !0,
    }
}

fn is_promotion(piece: i8, to: u8) -> bool {
    piece.abs() == PAWN && (to / 8 == 0 || to / 8 == 7)
}

impl Position {
    pub fn start() -> Self {
        Self::from_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1").expect("valid start position")
    }

    pub fn from_fen(fen: &str) -> Option<Self> {
        let mut parts = fen.split_whitespace();
        let mut board = [0i8; 64];
        for (i, row) in parts.next()?.split('/').enumerate() {
            let rank = 7 - i as i32;
            let mut file = 0;
            for ch in row.chars() {
                if let Some(empty) = ch.to_digit(10) {
                    file += empty as i32;
                    continue;
                }
                let kind = match ch.to_ascii_lowercase() {
                    'p' => PAWN,
                    'n' => KNIGHT,
                    'b' => BISHOP,
                    'r' => ROOK,
                    'q' => QUEEN,
                    'k' => KING,
                    _ => return None,
                };
                board[square(rank, file)?] = if ch.is_ascii_uppercase() { kind } else { -kind };
                file += 1;
            }
        }
        let side = if parts.next()? == "w" { WHITE } else { BLACK };
        let castling = parts.next()?.chars().fold(0, |rights, ch| {
            rights
                | match ch {
                    'K' => 1,
                    'Q' => 2,
                    'k' => 4,
                    'q' => 8,
                    _ => 0,
                }
        });
        let ep = parts.next().and_then(|s| match s.as_bytes() {
            [file @ b'a'..=b'h', rank @ b'1'..=b'8'] => Some((rank - b'1') * 8 + (file - b'a')),
            _ => None,
        });
        let halfmove = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        Some(Position {
            board,
            side,
            castling,
            ep,
            halfmove,
        })
    }

    /// Identifies the position for repetition checks (the move counters don't matter).
    pub fn key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        (self.board, self.side, self.castling, self.ep).hash(&mut hasher);
        hasher.finish()
    }

    pub fn king_square(&self, side: i8) -> Option<usize> {
        self.board.iter().position(|&p| p == KING * side)
    }

    pub fn in_check(&self, side: i8) -> bool {
        self.king_square(side).is_some_and(|k| self.attacked(k, -side))
    }

    /// Whether any piece of `by` attacks `sq`.
    fn attacked(&self, sq: usize, by: i8) -> bool {
        let (r, f) = ((sq / 8) as i32, (sq % 8) as i32);
        let has = |rank, file, piece| square(rank, file).is_some_and(|s| self.board[s] == piece);
        // A pawn of `by` attacks diagonally forward, so it would stand one rank behind.
        if has(r - by as i32, f - 1, PAWN * by) || has(r - by as i32, f + 1, PAWN * by) {
            return true;
        }
        if KNIGHT_STEPS.iter().any(|&(dr, df)| has(r + dr, f + df, KNIGHT * by))
            || KING_STEPS.iter().any(|&(dr, df)| has(r + dr, f + df, KING * by))
        {
            return true;
        }
        let slider = |dirs: &[(i32, i32)], kind| {
            dirs.iter().any(|&(dr, df)| {
                let (mut rank, mut file) = (r + dr, f + df);
                while let Some(s) = square(rank, file) {
                    let p = self.board[s];
                    if p != 0 {
                        return p == kind * by || p == QUEEN * by;
                    }
                    rank += dr;
                    file += df;
                }
                false
            })
        };
        slider(&ROOK_DIRS, ROOK) || slider(&BISHOP_DIRS, BISHOP)
    }

    /// The piece a move takes, including a pawn captured en passant.
    pub fn captured(&self, m: Move) -> i8 {
        let target = self.board[m.to as usize];
        if target == 0 && self.board[m.from as usize].abs() == PAWN && Some(m.to) == self.ep {
            -PAWN * self.side
        } else {
            target
        }
    }

    fn pseudo_moves(&self, out: &mut Vec<Move>) {
        let side = self.side;
        for from in 0..64 {
            let piece = self.board[from];
            if piece * side <= 0 {
                continue;
            }
            match piece.abs() {
                PAWN => self.pawn_moves(from, out),
                KNIGHT => self.steps(from, &KNIGHT_STEPS, out),
                BISHOP => self.slides(from, &BISHOP_DIRS, out),
                ROOK => self.slides(from, &ROOK_DIRS, out),
                QUEEN => {
                    self.slides(from, &ROOK_DIRS, out);
                    self.slides(from, &BISHOP_DIRS, out);
                }
                _ => {
                    self.steps(from, &KING_STEPS, out);
                    self.castle_moves(from, out);
                }
            }
        }
    }

    fn pawn_moves(&self, from: usize, out: &mut Vec<Move>) {
        let side = self.side as i32;
        let (r, f) = ((from / 8) as i32, (from % 8) as i32);
        let mut add = |to: usize| {
            out.push(Move {
                from: from as u8,
                to: to as u8,
            })
        };
        if let Some(to) = square(r + side, f).filter(|&to| self.board[to] == 0) {
            add(to);
            let home = if side == 1 { 1 } else { 6 };
            if let Some(to) = square(r + 2 * side, f).filter(|&to| r == home && self.board[to] == 0) {
                add(to);
            }
        }
        for df in [-1, 1] {
            if let Some(to) = square(r + side, f + df) {
                if self.board[to] as i32 * side < 0 || self.ep == Some(to as u8) {
                    add(to);
                }
            }
        }
    }

    fn steps(&self, from: usize, deltas: &[(i32, i32)], out: &mut Vec<Move>) {
        let (r, f) = ((from / 8) as i32, (from % 8) as i32);
        for &(dr, df) in deltas {
            if let Some(to) = square(r + dr, f + df) {
                if self.board[to] * self.side <= 0 {
                    out.push(Move {
                        from: from as u8,
                        to: to as u8,
                    });
                }
            }
        }
    }

    fn slides(&self, from: usize, dirs: &[(i32, i32)], out: &mut Vec<Move>) {
        let (r, f) = ((from / 8) as i32, (from % 8) as i32);
        for &(dr, df) in dirs {
            let (mut rank, mut file) = (r + dr, f + df);
            while let Some(to) = square(rank, file) {
                let target = self.board[to];
                if target * self.side > 0 {
                    break;
                }
                out.push(Move {
                    from: from as u8,
                    to: to as u8,
                });
                if target != 0 {
                    break;
                }
                rank += dr;
                file += df;
            }
        }
    }

    fn castle_moves(&self, from: usize, out: &mut Vec<Move>) {
        let side = self.side;
        let (short, long, home) = if side == WHITE { (1, 2, 4) } else { (4, 8, 60) };
        if from != home || self.castling & (short | long) == 0 || self.in_check(side) {
            return;
        }
        let clear = |squares: &[usize]| squares.iter().all(|&s| self.board[s] == 0);
        let safe = |squares: &[usize]| squares.iter().all(|&s| !self.attacked(s, -side));
        if self.castling & short != 0
            && self.board[home + 3] == ROOK * side
            && clear(&[home + 1, home + 2])
            && safe(&[home + 1, home + 2])
        {
            out.push(Move {
                from: home as u8,
                to: home as u8 + 2,
            });
        }
        if self.castling & long != 0
            && self.board[home - 4] == ROOK * side
            && clear(&[home - 1, home - 2, home - 3])
            && safe(&[home - 1, home - 2])
        {
            out.push(Move {
                from: home as u8,
                to: home as u8 - 2,
            });
        }
    }

    pub fn legal_moves(&self) -> Vec<Move> {
        let mut moves = Vec::with_capacity(48);
        self.pseudo_moves(&mut moves);
        moves.retain(|&m| !self.play(m).in_check(self.side));
        moves
    }

    /// The position after `m`, which must be one of `legal_moves`. Pawns always promote to queens.
    pub fn play(&self, m: Move) -> Position {
        let (from, to) = (m.from as usize, m.to as usize);
        let mut next = self.clone();
        let piece = self.board[from];
        let captured = self.captured(m);
        if captured != 0 && self.board[to] == 0 {
            next.board[(to as i32 - 8 * self.side as i32) as usize] = 0; // en passant
        }
        next.board[to] = if is_promotion(piece, m.to) {
            QUEEN * self.side
        } else {
            piece
        };
        next.board[from] = 0;
        if piece.abs() == KING && from.abs_diff(to) == 2 {
            let (rook_from, rook_to) = if to > from {
                (from + 3, from + 1)
            } else {
                (from - 4, from - 1)
            };
            next.board[rook_to] = next.board[rook_from];
            next.board[rook_from] = 0;
        }
        next.castling &= castle_mask(from) & castle_mask(to);
        next.ep = (piece.abs() == PAWN && from.abs_diff(to) == 16).then_some(((from + to) / 2) as u8);
        next.halfmove = if piece.abs() == PAWN || captured != 0 {
            0
        } else {
            self.halfmove + 1
        };
        next.side = -self.side;
        next
    }

    fn insufficient_material(&self) -> bool {
        let mut others = self.board.iter().filter(|&&p| p != 0 && p.abs() != KING);
        match (others.next(), others.next()) {
            (None, _) => true,
            (Some(p), None) => p.abs() == KNIGHT || p.abs() == BISHOP,
            _ => false,
        }
    }

    /// `history` holds the key of every position so far, including this one.
    pub fn status(&self, history: &[u64]) -> Status {
        if self.legal_moves().is_empty() {
            return if self.in_check(self.side) {
                Status::Checkmate { winner: -self.side }
            } else {
                Status::Stalemate
            };
        }
        let key = self.key();
        if self.halfmove >= 100 || self.insufficient_material() || history.iter().filter(|&&h| h == key).count() >= 3 {
            return Status::Draw;
        }
        Status::Playing
    }

    /// Material plus piece placement, from the point of view of the side to move.
    fn evaluate(&self) -> i32 {
        let heavy: i32 = self
            .board
            .iter()
            .map(|&p| {
                if p.abs() == PAWN {
                    0
                } else {
                    VALUE[p.unsigned_abs() as usize]
                }
            })
            .sum();
        let endgame = heavy <= 1300;
        let mut score = 0;
        for (sq, &p) in self.board.iter().enumerate() {
            if p == 0 {
                continue;
            }
            let kind = p.unsigned_abs() as usize;
            // The tables are drawn from white's side with a8 first, so white mirrors the rank.
            let index = if p > 0 { (7 - sq / 8) * 8 + sq % 8 } else { sq };
            let table = if kind == KING as usize && endgame {
                &KING_ENDGAME
            } else {
                &PST[kind - 1]
            };
            let value = VALUE[kind] + table[index];
            score += if p > 0 { value } else { -value };
        }
        score * self.side as i32
    }
}

/// Captures first, biggest victim by smallest attacker; promotions before everything.
fn order_moves(pos: &Position, moves: &mut [Move]) {
    moves.sort_by_cached_key(|&m| {
        let attacker = pos.board[m.from as usize];
        let victim = VALUE[pos.captured(m).unsigned_abs() as usize];
        let promotion = if is_promotion(attacker, m.to) { 10_000 } else { 0 };
        -(promotion + victim * 10 - VALUE[attacker.unsigned_abs() as usize] / 10)
    });
}

struct Search {
    deadline: Instant,
    nodes: u64,
    stopped: bool,
    seen: Vec<u64>, // keys of every position on the way here, for spotting repetitions
    seed: u64,
}

impl Search {
    /// A small fixed nudge per position (-8..=7) so Clawd doesn't play identical games.
    /// It depends only on the position, which keeps the alpha-beta bounds sound.
    fn jitter(&self, pos: &Position) -> i32 {
        ((pos.key() ^ self.seed).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 60) as i32 - 8
    }

    fn out_of_time(&mut self) -> bool {
        self.nodes += 1;
        if self.nodes % 1024 == 0 && Instant::now() >= self.deadline {
            self.stopped = true;
        }
        self.stopped
    }

    fn negamax(&mut self, pos: &Position, depth: i32, mut alpha: i32, beta: i32, ply: i32) -> i32 {
        if self.out_of_time() {
            return 0;
        }
        let key = self.seen[self.seen.len() - 1];
        if pos.halfmove >= 100 || self.seen[..self.seen.len() - 1].contains(&key) {
            return 0;
        }
        let in_check = pos.in_check(pos.side);
        let depth = if in_check && ply < 12 { depth + 1 } else { depth }; // look a little deeper at checks
        if depth <= 0 {
            return self.quiesce(pos, alpha, beta, ply);
        }
        let mut moves = pos.legal_moves();
        if moves.is_empty() {
            return if in_check { -(MATE - ply) } else { 0 };
        }
        order_moves(pos, &mut moves);
        for m in moves {
            let child = pos.play(m);
            self.seen.push(child.key());
            let score = -self.negamax(&child, depth - 1, -beta, -alpha, ply + 1);
            self.seen.pop();
            if self.stopped {
                return 0;
            }
            if score >= beta {
                return beta;
            }
            alpha = alpha.max(score);
        }
        alpha
    }

    /// Plays out captures until the position is quiet, so the search doesn't stop mid-trade.
    fn quiesce(&mut self, pos: &Position, mut alpha: i32, beta: i32, ply: i32) -> i32 {
        if self.out_of_time() {
            return 0;
        }
        let stand_pat = pos.evaluate() + self.jitter(pos);
        if stand_pat >= beta {
            return beta;
        }
        alpha = alpha.max(stand_pat);
        if ply > 40 {
            return alpha;
        }
        let mut captures = Vec::new();
        pos.pseudo_moves(&mut captures);
        captures.retain(|&m| pos.captured(m) != 0 && !pos.play(m).in_check(pos.side));
        order_moves(pos, &mut captures);
        for m in captures {
            let score = -self.quiesce(&pos.play(m), -beta, -alpha, ply + 1);
            if self.stopped {
                return 0;
            }
            if score >= beta {
                return beta;
            }
            alpha = alpha.max(score);
        }
        alpha
    }
}

/// Searches deeper and deeper until `budget` runs out and returns the best move found.
/// `seed` adds a little noise so equal-looking moves vary from game to game.
pub fn best_move(pos: &Position, history: &[u64], budget: Duration, seed: u64) -> Option<Move> {
    let mut moves = pos.legal_moves();
    order_moves(pos, &mut moves);
    let mut best = *moves.first()?;
    let mut search = Search {
        deadline: Instant::now() + budget,
        nodes: 0,
        stopped: false,
        seen: history.to_vec(),
        seed,
    };
    for depth in 1..=MAX_DEPTH {
        let mut alpha = -MATE - 1;
        let mut depth_best = None;
        for &m in &moves {
            let child = pos.play(m);
            search.seen.push(child.key());
            let score = -search.negamax(&child, depth - 1, -MATE - 1, -alpha, 1);
            search.seen.pop();
            if search.stopped {
                break;
            }
            if score > alpha {
                alpha = score;
                depth_best = Some(m);
            }
        }
        if search.stopped {
            break; // an unfinished depth can't be trusted
        }
        if let Some(m) = depth_best {
            best = m;
            let at = moves.iter().position(|&x| x == m).unwrap_or(0);
            moves[..=at].rotate_right(1); // search the favourite first next time
        }
        if alpha >= MATE - 100 {
            break;
        }
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perft(pos: &Position, depth: u32) -> u64 {
        if depth == 0 {
            return 1;
        }
        pos.legal_moves().iter().map(|&m| perft(&pos.play(m), depth - 1)).sum()
    }

    // Reference move counts; none of these depths involve promotions, which this engine limits to queens.
    #[test]
    fn move_generation_matches_reference_counts() {
        assert_eq!(perft(&Position::start(), 4), 197_281);
        let kiwipete =
            Position::from_fen("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1").unwrap();
        assert_eq!(perft(&kiwipete, 3), 97_862);
        let endgame = Position::from_fen("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1").unwrap();
        assert_eq!(perft(&endgame, 4), 43_238);
    }

    #[test]
    fn finds_mate_in_one() {
        let pos = Position::from_fen("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1").unwrap();
        let m = best_move(&pos, &[pos.key()], Duration::from_millis(500), 7).unwrap();
        assert_eq!((m.from, m.to), (3, 59)); // Rd1-d8#
        assert_eq!(pos.play(m).status(&[]), Status::Checkmate { winner: WHITE });
    }

    #[test]
    fn grabs_a_hanging_queen() {
        let pos = Position::from_fen("4k3/8/8/3q4/8/8/3R4/4K3 w - - 0 1").unwrap();
        let m = best_move(&pos, &[pos.key()], Duration::from_millis(500), 7).unwrap();
        assert_eq!((m.from, m.to), (11, 35)); // Rd2xd5
    }

    #[test]
    fn detects_stalemate_and_bare_kings() {
        let stalemate = Position::from_fen("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1").unwrap();
        assert_eq!(stalemate.status(&[]), Status::Stalemate);
        let bare = Position::from_fen("7k/8/6K1/8/8/8/8/8 w - - 0 1").unwrap();
        assert_eq!(bare.status(&[]), Status::Draw);
    }
}
