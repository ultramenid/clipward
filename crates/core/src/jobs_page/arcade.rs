//! Grid games for the wait screen (arcade/games.ts), after MonoCode's
//! empty-session arcade. Each plays itself until someone takes control, and
//! draws in grid cells so the board renders as one dot matrix.
use std::collections::VecDeque;
use std::f32::consts::PI;
use std::path::Path;

/// Where a game draws. Coordinates are grid cells; fractions are fine.
pub trait Draw {
    /// A lit cell; `size` < 1 draws a smaller centered dot.
    fn cell(&mut self, x: f32, y: f32, alpha: f32, size: f32);
    /// A filled rectangle `w` x `h` cells; cell (x, y) covers x..x + 1, y..y + 1.
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, alpha: f32);
    /// A filled circle `r` cells in radius, centered on cell (x, y).
    fn disc(&mut self, x: f32, y: f32, r: f32, alpha: f32);
    /// Pac-man centered on cell (x, y), facing (dx, dy), the mouth open
    /// `mouth` radians either side; at π nothing is left.
    #[allow(clippy::too_many_arguments)]
    fn pac(&mut self, x: f32, y: f32, r: f32, dx: f32, dy: f32, mouth: f32, alpha: f32);
    fn ghost(&mut self, ghost: &Sprite);
}

/// How a ghost looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Body,
    /// Running from pac-man; `flash` alternates as the time runs out.
    Scared { flash: bool },
    /// Eaten: only the eyes, on their way home.
    Eyes,
}

/// A ghost to draw, centered on cell (x, y), `r` cells in radius.
pub struct Sprite {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    /// Where the eyes look: its heading.
    pub dir: (f32, f32),
    pub look: Look,
    /// The skirt's two frames.
    pub feet: bool,
    pub alpha: f32,
}

pub trait Game {
    fn score(&self) -> u32;
    /// 0 for games without lives.
    fn lives(&self) -> u32;
    fn over(&self) -> bool;
    fn resize(&mut self, cols: i32, rows: i32);
    fn reset(&mut self);
    /// A heading from the keyboard; (0, 0) when a horizontal key is let go.
    fn steer(&mut self, x: i32, y: i32);
    /// Pointer column, for games that follow the mouse.
    fn point(&mut self, _x: f64) {}
    fn step(&mut self, controlled: bool);
    fn draw(&self, d: &mut dyn Draw);
}

pub struct GameDef {
    pub id: &'static str,
    pub label: &'static str,
    /// Grid pitch in CSS pixels.
    pub pitch: f32,
    /// A board of fixed size instead: the pitch stretches so this many cells
    /// fill the space (full screen), or fill the width (the idle band).
    pub fit: Option<(i32, i32)>,
    /// Whether the unlit dot matrix shows behind the game.
    pub matrix: bool,
    pub tick_ms: f64,
    /// Idle brightness; busy boards run quieter.
    pub idle_dim: f32,
    pub hint: &'static str,
    pub create: fn(u64) -> Box<dyn Game>,
}

/// Games the wait-screen slider rotates through. Add one here to add it everywhere.
pub const GAMES: [GameDef; 3] = [
    GameDef { id: "snake", label: "snake", pitch: 8., fit: None, matrix: true, tick_ms: 60., idle_dim: 0.8, hint: "arrows / wasd to turn", create: |seed| Box::new(Snake::new(seed)) },
    // The 28 x 31 arcade maze with a tile of margin around it.
    GameDef { id: "pacman", label: "pac-man", pitch: 14., fit: Some((30, 33)), matrix: false, tick_ms: PAC_TICK_MS, idle_dim: 0.5, hint: "arrows / wasd to move", create: |seed| Box::new(Pacman::new(seed)) },
    GameDef { id: "breaker", label: "block breaker", pitch: 8., fit: None, matrix: true, tick_ms: 16., idle_dim: 0.6, hint: "arrows / a d / mouse to move", create: |seed| Box::new(Breaker::new(seed)) },
];

/// Difficulty scales every game's tick: hard runs everything faster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Low,
    Mid,
    Hard,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Low, Mode::Mid, Mode::Hard];

    pub fn tick_scale(self) -> f64 {
        match self {
            Mode::Low => 1.4,
            Mode::Mid => 1.0,
            Mode::Hard => 0.7,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Low => "low",
            Mode::Mid => "mid",
            Mode::Hard => "hard",
        }
    }
}

const MODE_FILE: &str = "arcade.json";

/// The last picked mode, kept in the app's data folder; mid when unset.
pub fn load_mode(dir: &Path) -> Mode {
    std::fs::read_to_string(dir.join(MODE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| {
            let mode = value["mode"].as_str()?.to_owned();
            Mode::ALL.into_iter().find(|m| m.as_str() == mode)
        })
        .unwrap_or(Mode::Mid)
}

pub fn save_mode(dir: &Path, mode: Mode) -> std::io::Result<()> {
    std::fs::write(dir.join(MODE_FILE), serde_json::json!({ "mode": mode.as_str() }).to_string())
}

/// Keys the games read: a heading for arrows and WASD (GPUI key names).
pub fn heading(key: &str) -> Option<(i32, i32)> {
    match key.to_lowercase().as_str() {
        "up" | "w" => Some((0, -1)),
        "down" | "s" => Some((0, 1)),
        "left" | "a" => Some((-1, 0)),
        "right" | "d" => Some((1, 0)),
        _ => None,
    }
}

/// xorshift64*: the games only need cheap, unpredictable-looking choices.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    /// Like `Math.random()`: [0, 1).
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, n: i32) -> i32 {
        (self.next() * n as f64).floor() as i32
    }
}

/// `Math.round` for the non-negative values here.
fn round(v: f64) -> f64 {
    (v + 0.5).floor()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cell {
    x: i32,
    y: i32,
}

const fn c(x: i32, y: i32) -> Cell {
    Cell { x, y }
}

// ---- snake ------------------------------------------------------------------

struct Snake {
    cols: i32,
    rows: i32,
    body: VecDeque<Cell>,
    dir: Cell,
    queued: Option<Cell>,
    pellet: Cell,
    score: u32,
    over: bool,
    rng: Rng,
}

impl Snake {
    fn new(seed: u64) -> Self {
        Self { cols: 0, rows: 0, body: VecDeque::new(), dir: c(1, 0), queued: None, pellet: c(0, 0), score: 0, over: false, rng: Rng::new(seed) }
    }

    fn hits(&self, x: i32, y: i32) -> bool {
        self.body.iter().any(|b| b.x == x && b.y == y)
    }

    fn place(&mut self) {
        for _ in 0..100 {
            let p = c(self.rng.below(self.cols), self.rng.below(self.rows));
            if !self.hits(p.x, p.y) {
                self.pellet = p;
                return;
            }
        }
    }

    /// Greedy autopilot: straight, left or right, whichever gets closer without biting itself.
    fn think(&mut self) -> Cell {
        let head = self.body[0];
        let dir = self.dir;
        let mut best = dir;
        let mut best_score = f64::NEG_INFINITY;
        for d in [dir, c(-dir.y, dir.x), c(dir.y, -dir.x)] {
            let x = (head.x + d.x).rem_euclid(self.cols);
            let y = (head.y + d.y).rem_euclid(self.rows);
            if self.hits(x, y) {
                continue;
            }
            let dx = (self.pellet.x - x).abs().min(self.cols - (self.pellet.x - x).abs());
            let dy = (self.pellet.y - y).abs().min(self.rows - (self.pellet.y - y).abs());
            let s = -((dx + dy) as f64) + self.rng.next() * 0.5;
            if s > best_score {
                best_score = s;
                best = d;
            }
        }
        best
    }
}

impl Game for Snake {
    fn score(&self) -> u32 {
        self.score
    }
    fn lives(&self) -> u32 {
        0
    }
    fn over(&self) -> bool {
        self.over
    }
    fn resize(&mut self, cols: i32, rows: i32) {
        self.cols = cols.max(8);
        self.rows = rows.max(4);
        self.reset();
    }
    fn reset(&mut self) {
        let y = self.rows / 2;
        let x = (self.cols / 4).max(2);
        self.body = [c(x, y), c(x - 1, y), c(x - 2, y)].into();
        self.dir = c(1, 0);
        self.queued = None;
        self.score = 0;
        self.over = false;
        self.place();
    }
    fn steer(&mut self, x: i32, y: i32) {
        if x == 0 && y == 0 {
            return;
        }
        let last = self.queued.unwrap_or(self.dir);
        if x != -last.x || y != -last.y {
            self.queued = Some(c(x, y));
        }
    }
    fn step(&mut self, controlled: bool) {
        if self.over || self.body.is_empty() {
            return;
        }
        if !controlled {
            self.dir = self.think();
        } else if let Some(queued) = self.queued.take() {
            self.dir = queued;
        }
        let head = c(
            (self.body[0].x + self.dir.x).rem_euclid(self.cols),
            (self.body[0].y + self.dir.y).rem_euclid(self.rows),
        );
        // The tail moves out of the way this tick, so it can't be bitten.
        if self.body.iter().take(self.body.len() - 1).any(|b| *b == head) {
            if controlled {
                self.over = true;
            } else {
                self.reset();
            }
            return;
        }
        self.body.push_front(head);
        if head == self.pellet {
            if controlled {
                self.score += 1;
            } else if self.body.len() as f64 > (self.cols as f64 * 0.4).max(16.0) {
                self.body.pop_back();
            }
            self.place();
        } else {
            self.body.pop_back();
        }
    }
    fn draw(&self, d: &mut dyn Draw) {
        let len = self.body.len() as f32;
        for (i, b) in self.body.iter().enumerate() {
            d.cell(b.x as f32, b.y as f32, 0.72 - 0.5 * i as f32 / len, 1.0);
        }
        d.cell(self.pellet.x as f32, self.pellet.y as f32, 0.9, 1.0);
    }
}

// ---- pac-man ----------------------------------------------------------------

/// The arcade maze, 28 x 31 tiles: `#` wall, `.` dot, `o` energizer, `-` the
/// ghost-house door, `_` outside the maze, spaces open floor. Row 14 runs off
/// both sides into the tunnel.
const MAZE: [&str; 31] = [
    "############################",
    "#............##............#",
    "#.####.#####.##.#####.####.#",
    "#o####.#####.##.#####.####o#",
    "#.####.#####.##.#####.####.#",
    "#..........................#",
    "#.####.##.########.##.####.#",
    "#.####.##.########.##.####.#",
    "#......##....##....##......#",
    "######.##### ## #####.######",
    "_____#.##### ## #####.#_____",
    "_____#.##          ##.#_____",
    "_____#.## ###--### ##.#_____",
    "######.## #      # ##.######",
    "      .   #      #   .      ",
    "######.## #      # ##.######",
    "_____#.## ######## ##.#_____",
    "_____#.##          ##.#_____",
    "_____#.## ######## ##.#_____",
    "######.## ######## ##.######",
    "#............##............#",
    "#.####.#####.##.#####.####.#",
    "#.####.#####.##.#####.####.#",
    "#o..##.......  .......##..o#",
    "###.##.##.########.##.##.###",
    "###.##.##.########.##.##.###",
    "#......##....##....##......#",
    "#.##########.##.##########.#",
    "#.##########.##.##########.#",
    "#..........................#",
    "############################",
];
const MAZE_W: i32 = 28;
const MAZE_H: i32 = 31;

/// Pac-man steps 120 times a second, so everyone glides a fraction of a tile
/// at a time instead of jumping whole cells.
const PAC_TICK_MS: f64 = 1000.0 / 120.0;
/// Seconds per tick at mid difficulty; the other modes run the clock faster or slower.
const DT: f32 = (PAC_TICK_MS / 1000.0) as f32;
/// Full speed in tiles a second (the arcade's 75.75 px/s on 8 px tiles); the
/// actors run at fractions of it.
const FULL_SPEED: f32 = 9.47;
const EPS: f32 = 1e-3;

const UP: Cell = c(0, -1);
const LEFT: Cell = c(-1, 0);
const DOWN: Cell = c(0, 1);
const RIGHT: Cell = c(1, 0);
/// Ghosts weigh turns in this order, so ties go the arcade's way.
const TURNS: [Cell; 4] = [UP, LEFT, DOWN, RIGHT];

/// The ghost house: its door column, the row above the door and the row inside.
const DOOR_X: f32 = 13.5;
const OUTSIDE_Y: f32 = 11.0;
const INSIDE_Y: f32 = 14.0;
/// Where eaten ghosts' eyes head before dropping back into the house.
const HOME: Cell = c(13, 11);
/// The corners Blinky, Pinky, Inky and Clyde scatter to.
const CORNERS: [Cell; 4] = [c(25, -3), c(2, -3), c(27, 31), c(0, 31)];
const FRUIT: (f32, f32) = (13.5, 17.0);
/// Wall line thickness, in tiles.
const WALL_LINE: f32 = 0.14;
/// Seconds the death takes: a beat, the shrink, then a pause.
const DEATH: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tile {
    Floor,
    Wall,
    Door,
    Void,
}

/// The tile at (x, y); columns wrap, for the tunnel.
fn tile_at(x: i32, y: i32) -> Tile {
    if !(0..MAZE_H).contains(&y) {
        return Tile::Void;
    }
    match MAZE[y as usize].as_bytes()[x.rem_euclid(MAZE_W) as usize] {
        b'#' => Tile::Wall,
        b'-' => Tile::Door,
        b'_' => Tile::Void,
        _ => Tile::Floor,
    }
}

fn floor(x: i32, y: i32) -> bool {
    tile_at(x, y) == Tile::Floor
}

/// The maze drawn the arcade's way: a line through the middle of each wall tile
/// along open floor, as rectangles in tile units (tile (x, y) covers x..x + 1).
fn outline() -> Vec<[f32; 4]> {
    let at = |x: i32, y: i32| {
        ((0..MAZE_W).contains(&x) && (0..MAZE_H).contains(&y)).then(|| MAZE[y as usize].as_bytes()[x as usize])
    };
    let wall = |x, y| at(x, y) == Some(b'#');
    // The door and the outside count as solid: no line faces them.
    let open = |x, y| matches!(at(x, y), Some(b' ' | b'.' | b'o'));
    let lo = 0.5 - WALL_LINE / 2.0;
    let hi = lo + WALL_LINE;
    let mut rects = Vec::new();
    for y in 0..MAZE_H {
        for x in 0..MAZE_W {
            if !wall(x, y) {
                continue;
            }
            // The line runs on to a neighboring wall tile when open floor lies
            // along either side of the step between them.
            let arms = [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|(dx, dy)| {
                wall(x + dx, y + dy)
                    && (open(x + dy, y + dx)
                        || open(x + dx + dy, y + dy + dx)
                        || open(x - dy, y - dx)
                        || open(x + dx - dy, y + dy - dx))
            });
            if !arms.contains(&true) {
                continue;
            }
            let (fx, fy) = (x as f32, y as f32);
            rects.push([fx + lo, fy + lo, WALL_LINE, WALL_LINE]);
            for (arm, rect) in arms.into_iter().zip([
                [fx + hi, fy + lo, 1.0 - hi, WALL_LINE],
                [fx, fy + lo, lo, WALL_LINE],
                [fx + lo, fy + hi, WALL_LINE, 1.0 - hi],
                [fx + lo, fy, WALL_LINE, lo],
            ]) {
                if arm {
                    rects.push(rect);
                }
            }
        }
    }
    rects
}

/// First step of the shortest way from `from` to a tile `goal` accepts,
/// around the tiles `blocked` rejects.
fn route(from: Cell, goal: &dyn Fn(Cell) -> bool, blocked: &dyn Fn(Cell) -> bool) -> Option<Cell> {
    let index = |t: Cell| (t.y * MAZE_W + t.x) as usize;
    let mut first: Vec<Option<Cell>> = vec![None; (MAZE_W * MAZE_H) as usize];
    let mut seen = vec![false; first.len()];
    seen[index(from)] = true;
    let mut queue = VecDeque::from([from]);
    while let Some(t) = queue.pop_front() {
        for d in TURNS {
            let n = c((t.x + d.x).rem_euclid(MAZE_W), t.y + d.y);
            if !floor(n.x, n.y) || seen[index(n)] || blocked(n) {
                continue;
            }
            seen[index(n)] = true;
            let step = first[index(t)].unwrap_or(d);
            if goal(n) {
                return Some(step);
            }
            first[index(n)] = Some(step);
            queue.push_back(n);
        }
    }
    None
}

/// Something that moves through the maze, in tile units: tile centers sit on
/// whole numbers, and it's always on a center or between two along `dir`.
#[derive(Clone, Copy, Debug)]
struct Mover {
    x: f32,
    y: f32,
    dir: Cell,
}

impl Mover {
    fn tile(&self) -> Cell {
        c((self.x.round() as i32).rem_euclid(MAZE_W), self.y.round() as i32)
    }

    fn centered(&self) -> bool {
        (self.x - self.x.round()).abs() < EPS && (self.y - self.y.round()).abs() < EPS
    }

    fn reverse(&mut self) {
        self.dir = c(-self.dir.x, -self.dir.y);
    }

    /// Distance to (x, y), across the tunnel too.
    fn near(&self, x: f32, y: f32) -> f32 {
        let dx = (self.x - x).abs();
        dx.min(MAZE_W as f32 - dx).hypot(self.y - y)
    }

    /// Runs `dist` tiles along the corridors. At each tile center `turn` picks
    /// the heading, or stops there with `None`; a wall ahead stops it too.
    /// Returns the distance covered.
    fn run(&mut self, dist: f32, mut turn: impl FnMut(&Mover) -> Option<Cell>) -> f32 {
        let mut left = dist;
        while left > EPS {
            if self.centered() {
                self.x = self.x.round();
                self.y = self.y.round();
                let Some(dir) = turn(self) else { break };
                self.dir = dir;
                let t = self.tile();
                if !floor(t.x + dir.x, t.y + dir.y) {
                    break;
                }
            }
            let along = if self.dir.x != 0 { self.x * self.dir.x as f32 } else { self.y * self.dir.y as f32 };
            let gap = along.floor() + 1.0 - along;
            let step = left.min(gap);
            self.x += self.dir.x as f32 * step;
            self.y += self.dir.y as f32 * step;
            left -= step;
            if gap - step < EPS {
                self.x = self.x.round();
                self.y = self.y.round();
            }
            // Out one side of the tunnel, in the other.
            let w = MAZE_W as f32;
            if self.x < -0.5 {
                self.x += w;
            } else if self.x > w - 0.5 {
                self.x -= w;
            }
        }
        dist - left
    }

    /// Slides straight to (x, y), off the grid (in and out of the ghost house),
    /// spending `budget`; true once there.
    fn glide(&mut self, x: f32, y: f32, budget: &mut f32) -> bool {
        let (dx, dy) = (x - self.x, y - self.y);
        let gap = dx.abs().max(dy.abs());
        if gap > EPS {
            self.dir = if dx.abs() > dy.abs() { c(dx.signum() as i32, 0) } else { c(0, dy.signum() as i32) };
            let step = gap.min(*budget);
            *budget -= step;
            self.x += self.dir.x as f32 * step;
            self.y += self.dir.y as f32 * step;
        }
        let there = (x - self.x).abs().max((y - self.y).abs()) < EPS;
        if there {
            self.x = x;
            self.y = y;
        }
        there
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Haunt {
    /// Bobbing in the house, waiting to be let out.
    House,
    Leaving,
    Out,
    /// Eaten: the eyes run home.
    Eyes,
    Entering,
}

#[derive(Clone, Copy, Debug)]
struct Ghost {
    at: Mover,
    state: Haunt,
    scared: bool,
    /// Dots eaten while it was first in line to leave the house.
    dots: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Everyone holds still before play starts.
    Ready(f32),
    Play,
    /// The board stops for a beat after a ghost is eaten.
    Eat(f32),
    Dying(f32),
    /// The maze flashes before the next level.
    Cleared(f32),
}

/// Speeds as fractions of full: pac-man, pac-man while ghosts are scared,
/// ghosts, scared ghosts and ghosts in the tunnel.
fn speeds(level: u32) -> [f32; 5] {
    match level {
        1 => [0.80, 0.90, 0.75, 0.50, 0.40],
        2..=4 => [0.90, 0.95, 0.85, 0.55, 0.45],
        _ => [1.00, 1.00, 0.95, 0.60, 0.50],
    }
}

/// Seconds ghosts stay scared after an energizer.
fn fright_time(level: u32) -> f32 {
    const TIMES: [f32; 18] = [6., 5., 4., 3., 2., 5., 2., 2., 1., 5., 2., 1., 1., 3., 1., 1., 0., 1.];
    TIMES.get(level as usize - 1).copied().unwrap_or(0.0)
}

/// Scatter and chase spans in seconds, alternating; chase for good after the last.
fn waves(level: u32) -> &'static [f32] {
    match level {
        1 => &[7., 20., 7., 20., 5., 20., 5.],
        2..=4 => &[7., 20., 7., 20., 5., 1033., 1. / 60.],
        _ => &[5., 20., 5., 20., 5., 1037., 1. / 60.],
    }
}

fn fruit_points(level: u32) -> u32 {
    const POINTS: [u32; 12] = [100, 300, 500, 500, 700, 700, 1000, 1000, 2000, 2000, 3000, 3000];
    POINTS.get(level as usize - 1).copied().unwrap_or(5000)
}

/// The arcade game on its own maze: four ghosts with their own chase targets,
/// scatter and chase waves, energizers, the ghost house, the tunnel and fruit.
struct Pacman {
    /// Where tile (0, 0) sits on the board grid: the maze is centered.
    ox: f32,
    oy: f32,
    /// Board rows; fewer than the maze's and the view pans to follow pac-man.
    rows: i32,
    walls: Vec<[f32; 4]>,
    /// 1 for a dot, 2 for an energizer, by tile.
    dots: Vec<u8>,
    left: u32,
    eaten: u32,
    pac: Mover,
    /// The last heading asked for; taken at the first tile that allows it.
    want: Option<Cell>,
    /// Distance run, for the mouth.
    chomp: f32,
    ghosts: [Ghost; 4],
    phase: Phase,
    level: u32,
    /// Seconds into the scatter and chase waves; stopped while ghosts are scared.
    clock: f32,
    fright: f32,
    /// Ghosts eaten on this energizer.
    combo: u32,
    fruit: f32,
    /// Seconds since the last dot.
    idle: f32,
    /// Dots since a death; while set, it lets the ghosts out instead of their own counts.
    regroup: Option<u32>,
    time: f32,
    score: u32,
    lives: u32,
    over: bool,
    rng: Rng,
}

impl Pacman {
    fn new(seed: u64) -> Self {
        let at = Mover { x: 0.0, y: 0.0, dir: LEFT };
        let ghost = Ghost { at, state: Haunt::House, scared: false, dots: 0 };
        Self { ox: 0.0, oy: 0.0, rows: 0, walls: outline(), dots: Vec::new(), left: 0, eaten: 0, pac: at, want: None, chomp: 0.0, ghosts: [ghost; 4], phase: Phase::Ready(0.0), level: 1, clock: 0.0, fright: 0.0, combo: 0, fruit: 0.0, idle: 0.0, regroup: None, time: 0.0, score: 0, lives: 3, over: false, rng: Rng::new(seed) }
    }

    /// A full maze for the current level.
    fn fill(&mut self) {
        self.dots = MAZE
            .iter()
            .flat_map(|row| row.bytes())
            .map(|b| match b {
                b'.' => 1,
                b'o' => 2,
                _ => 0,
            })
            .collect();
        self.left = self.dots.iter().filter(|&&d| d > 0).count() as u32;
        self.eaten = 0;
        self.regroup = None;
        for g in &mut self.ghosts {
            g.dots = 0;
        }
        self.place();
    }

    /// Everyone back to their starts: Blinky above the door, the rest inside.
    fn place(&mut self) {
        self.pac = Mover { x: DOOR_X, y: 23.0, dir: LEFT };
        self.want = None;
        self.chomp = 0.0;
        let starts = [
            (DOOR_X, OUTSIDE_Y, LEFT, Haunt::Out),
            (DOOR_X, INSIDE_Y, DOWN, Haunt::House),
            (11.5, INSIDE_Y, UP, Haunt::House),
            (15.5, INSIDE_Y, UP, Haunt::House),
        ];
        for (g, (x, y, dir, state)) in self.ghosts.iter_mut().zip(starts) {
            g.at = Mover { x, y, dir };
            g.state = state;
            g.scared = false;
        }
        self.fright = 0.0;
        self.combo = 0;
        self.fruit = 0.0;
        self.clock = 0.0;
        self.idle = 0.0;
        self.phase = Phase::Ready(2.0);
    }

    fn award(&mut self, points: u32, controlled: bool) {
        if !controlled {
            return;
        }
        // One extra life at 10,000.
        if self.score < 10_000 && self.score + points >= 10_000 {
            self.lives += 1;
        }
        self.score += points;
    }

    /// Whether the ghosts are chasing rather than scattering to their corners.
    fn chasing(&self) -> bool {
        let mut t = self.clock;
        for (i, span) in waves(self.level).iter().enumerate() {
            if t < *span {
                return i % 2 == 1;
            }
            t -= span;
        }
        true
    }

    /// The tile ghost `k` heads for. Blinky goes for pac-man, Pinky four tiles
    /// ahead of him, Inky past him from Blinky, and Clyde for him until close.
    fn target(&self, k: usize) -> Cell {
        let g = &self.ghosts[k];
        if g.state == Haunt::Eyes {
            return HOME;
        }
        if !self.chasing() {
            return CORNERS[k];
        }
        let p = self.pac.tile();
        let d = self.pac.dir;
        // Looking ahead of pac-man facing up also looks left: the arcade's overflow bug.
        let ahead = |n: i32| c(p.x + d.x * n - if d == UP { n } else { 0 }, p.y + d.y * n);
        match k {
            0 => p,
            1 => ahead(4),
            2 => {
                let (a, b) = (ahead(2), self.ghosts[0].at.tile());
                c(2 * a.x - b.x, 2 * a.y - b.y)
            }
            _ => {
                let t = g.at.tile();
                if (t.x - p.x).pow(2) + (t.y - p.y).pow(2) > 64 { p } else { CORNERS[3] }
            }
        }
    }

    /// An energizer: the ghosts turn around and run scared.
    fn frighten(&mut self) {
        self.combo = 0;
        self.fright = fright_time(self.level);
        let scare = self.fright > 0.0;
        for g in &mut self.ghosts {
            if g.state == Haunt::Out {
                g.at.reverse();
            }
            if scare && matches!(g.state, Haunt::House | Haunt::Leaving | Haunt::Out) {
                g.scared = true;
            }
        }
    }

    /// Lets the next ghost out: Pinky, Inky, then Clyde, each once enough dots
    /// are eaten, or when pac-man goes a while without eating.
    fn release(&mut self) {
        let Some(k) = (1..4).find(|&k| self.ghosts[k].state == Haunt::House) else {
            self.regroup = None;
            return;
        };
        let due = match self.regroup {
            // After a death one shared count lets them out at 7, 17 and 32 dots.
            Some(n) => n >= [0, 7, 17, 32][k],
            None => self.ghosts[k].dots >= match (self.level, k) {
                (1, 2) => 30,
                (1, 3) => 60,
                (2, 3) => 50,
                _ => 0,
            },
        };
        if due || self.idle >= if self.level < 5 { 4.0 } else { 3.0 } {
            self.ghosts[k].state = Haunt::Leaving;
            self.idle = 0.0;
        }
    }

    fn timers(&mut self) {
        if self.fright > 0.0 {
            self.fright -= DT;
            if self.fright <= 0.0 {
                self.fright = 0.0;
                for g in &mut self.ghosts {
                    g.scared = false;
                }
            }
        } else {
            // Ghosts turn around whenever they switch between scatter and chase.
            let chasing = self.chasing();
            self.clock += DT;
            if self.chasing() != chasing {
                for g in self.ghosts.iter_mut().filter(|g| g.state == Haunt::Out) {
                    g.at.reverse();
                }
            }
        }
        self.fruit = (self.fruit - DT).max(0.0);
        self.idle += DT;
        self.release();
    }

    /// The attract-mode player: at each tile, the shortest way to a dot (or a
    /// scared ghost) that keeps clear of the rest; between tiles, it backs off
    /// from a ghost just ahead.
    fn autopilot(&mut self) {
        let threats: Vec<Cell> = self
            .ghosts
            .iter()
            .filter(|g| matches!(g.state, Haunt::Out | Haunt::Leaving) && !g.scared)
            .map(|g| g.at.tile())
            .collect();
        let (t, d) = (self.pac.tile(), self.pac.dir);
        if !self.pac.centered() {
            let ahead = |n: i32| c((t.x + d.x * n).rem_euclid(MAZE_W), t.y + d.y * n);
            if threats.iter().any(|g| *g == ahead(1) || *g == ahead(2)) {
                self.want = Some(c(-d.x, -d.y));
            }
            return;
        }
        let prey: Vec<Cell> = self
            .ghosts
            .iter()
            .filter(|g| g.state == Haunt::Out && g.scared && self.fright > 1.0)
            .map(|g| g.at.tile())
            .collect();
        let dots = &self.dots;
        let goal = |n: Cell| dots[(n.y * MAZE_W + n.x) as usize] > 0 || prey.contains(&n);
        let danger = |n: Cell| threats.iter().any(|g| (g.x - n.x).abs() + (g.y - n.y).abs() <= 2);
        if let Some(step) = route(t, &goal, &danger).or_else(|| route(t, &goal, &|_| false)) {
            self.want = Some(step);
        }
    }

    fn eat(&mut self, controlled: bool) {
        let t = self.pac.tile();
        let i = (t.y * MAZE_W + t.x) as usize;
        let dot = self.dots[i];
        if dot == 0 {
            return;
        }
        self.dots[i] = 0;
        self.left -= 1;
        self.eaten += 1;
        self.idle = 0.0;
        match &mut self.regroup {
            Some(n) => *n += 1,
            None => {
                if let Some(g) = self.ghosts[1..].iter_mut().find(|g| g.state == Haunt::House) {
                    g.dots += 1;
                }
            }
        }
        if dot == 2 {
            self.award(50, controlled);
            self.frighten();
        } else {
            self.award(10, controlled);
        }
        if self.eaten == 70 || self.eaten == 170 {
            self.fruit = 9.5;
        }
        if self.left == 0 {
            self.phase = Phase::Cleared(2.0);
        }
    }

    fn move_pac(&mut self, controlled: bool) {
        if !controlled {
            self.autopilot();
        }
        // Pac-man can turn back anywhere, not only on a tile center.
        if self.want == Some(c(-self.pac.dir.x, -self.pac.dir.y)) {
            self.pac.reverse();
        }
        let [normal, scared, ..] = speeds(self.level);
        let speed = if self.fright > 0.0 { scared } else { normal };
        let want = self.want;
        self.chomp += self.pac.run(speed * FULL_SPEED * DT, |m| {
            let t = m.tile();
            Some(want.filter(|w| floor(t.x + w.x, t.y + w.y)).unwrap_or(m.dir))
        });
        self.eat(controlled);
        if self.fruit > 0.0 && self.pac.near(FRUIT.0, FRUIT.1) < 0.6 {
            self.fruit = 0.0;
            self.award(fruit_points(self.level), controlled);
        }
    }

    fn move_ghost(&mut self, k: usize) {
        let [_, _, normal, scared, tunnel] = speeds(self.level);
        let g = self.ghosts[k];
        let in_tunnel = g.at.tile().y == 14 && (g.at.x < 5.5 || g.at.x > 21.5);
        let mut budget = FULL_SPEED
            * DT
            * match g.state {
                Haunt::Eyes | Haunt::Entering => 1.6,
                Haunt::House | Haunt::Leaving => 0.4,
                Haunt::Out if in_tunnel => tunnel,
                Haunt::Out if g.scared => scared,
                Haunt::Out => normal,
            };
        match g.state {
            Haunt::House => {
                let m = &mut self.ghosts[k].at;
                m.y += m.dir.y as f32 * budget;
                if m.y <= INSIDE_Y - 0.3 {
                    m.dir = DOWN;
                } else if m.y >= INSIDE_Y + 0.3 {
                    m.dir = UP;
                }
            }
            Haunt::Leaving => {
                // Over to the door, then up and out.
                let m = &mut self.ghosts[k].at;
                let x = m.x;
                let lined_up = (x - DOOR_X).abs() < EPS
                    || (m.glide(x, INSIDE_Y, &mut budget) && m.glide(DOOR_X, INSIDE_Y, &mut budget));
                if lined_up && m.glide(DOOR_X, OUTSIDE_Y, &mut budget) {
                    m.dir = LEFT;
                    self.ghosts[k].state = Haunt::Out;
                }
            }
            Haunt::Entering => {
                let m = &mut self.ghosts[k].at;
                if m.glide(DOOR_X, OUTSIDE_Y, &mut budget) && m.glide(DOOR_X, INSIDE_Y, &mut budget) {
                    self.ghosts[k].state = Haunt::Leaving;
                    self.ghosts[k].scared = false;
                }
            }
            Haunt::Out | Haunt::Eyes => {
                let target = self.target(k);
                let eyes = g.state == Haunt::Eyes;
                // The eyes stop over the door, on either of its two tiles.
                let home = |t: Cell| eyes && t.y == HOME.y && (t.x == HOME.x || t.x == HOME.x + 1);
                let frightened = g.scared && !eyes;
                let rng = &mut self.rng;
                let m = &mut self.ghosts[k].at;
                m.run(budget, |m| (!home(m.tile())).then(|| choose(m, target, frightened, rng)));
                if home(m.tile()) && m.centered() {
                    self.ghosts[k].state = Haunt::Entering;
                }
            }
        }
    }

    /// Pac-man meets a ghost: eats it when scared, dies otherwise.
    fn collide(&mut self, controlled: bool) {
        for k in 0..self.ghosts.len() {
            let g = self.ghosts[k];
            if self.phase != Phase::Play || g.state != Haunt::Out || self.pac.near(g.at.x, g.at.y) > 0.6 {
                continue;
            }
            if !g.scared {
                self.phase = Phase::Dying(DEATH);
                return;
            }
            self.ghosts[k].state = Haunt::Eyes;
            self.ghosts[k].scared = false;
            self.combo += 1;
            // 200, 400, 800, then 1600.
            self.award(100 << self.combo, controlled);
            self.phase = Phase::Eat(0.5);
        }
    }

    /// After the death: a life gone, then everyone back to their starts.
    fn revive(&mut self, controlled: bool) {
        if controlled {
            self.lives = self.lives.saturating_sub(1);
            if self.lives == 0 {
                self.over = true;
                return;
            }
        }
        self.place();
        self.regroup = Some(0);
    }
}

/// A ghost's turn on a tile center: never back the way it came, never up from
/// the four tiles by the house and pac-man's start unless scared, then the way
/// closest to its target, or any way at random while scared.
fn choose(m: &Mover, target: Cell, scared: bool, rng: &mut Rng) -> Cell {
    let t = m.tile();
    let back = c(-m.dir.x, -m.dir.y);
    let no_up = !scared && (t.y == 11 || t.y == 23) && (t.x == 12 || t.x == 15);
    let ways: Vec<Cell> = TURNS
        .into_iter()
        .filter(|&d| d != back && floor(t.x + d.x, t.y + d.y) && !(no_up && d == UP))
        .collect();
    if ways.is_empty() {
        return back;
    }
    if scared {
        return ways[rng.below(ways.len() as i32) as usize];
    }
    let dist = |d: &Cell| (t.x + d.x - target.x).pow(2) + (t.y + d.y - target.y).pow(2);
    ways.into_iter().min_by_key(dist).unwrap_or(back)
}

impl Game for Pacman {
    fn score(&self) -> u32 {
        self.score
    }
    fn lives(&self) -> u32 {
        self.lives
    }
    fn over(&self) -> bool {
        self.over
    }
    fn resize(&mut self, cols: i32, rows: i32) {
        self.ox = (cols - MAZE_W) as f32 / 2.0;
        self.oy = (rows - MAZE_H) as f32 / 2.0;
        self.rows = rows;
        self.reset();
    }
    fn reset(&mut self) {
        self.score = 0;
        self.lives = 3;
        self.over = false;
        self.level = 1;
        self.fill();
    }
    fn steer(&mut self, x: i32, y: i32) {
        if x != 0 || y != 0 {
            self.want = Some(c(x, y));
        }
    }
    fn step(&mut self, controlled: bool) {
        if self.over || self.dots.is_empty() {
            return;
        }
        self.time += DT;
        match self.phase {
            Phase::Play => {}
            Phase::Ready(t) => {
                self.phase = if t > DT { Phase::Ready(t - DT) } else { Phase::Play };
                return;
            }
            Phase::Eat(t) => {
                self.phase = if t > DT { Phase::Eat(t - DT) } else { Phase::Play };
                return;
            }
            Phase::Dying(t) if t > DT => {
                self.phase = Phase::Dying(t - DT);
                return;
            }
            Phase::Dying(_) => return self.revive(controlled),
            Phase::Cleared(t) if t > DT => {
                self.phase = Phase::Cleared(t - DT);
                return;
            }
            Phase::Cleared(_) => {
                self.level += 1;
                return self.fill();
            }
        }
        self.timers();
        self.move_pac(controlled);
        self.collide(controlled);
        if self.phase != Phase::Play {
            return;
        }
        for k in 0..self.ghosts.len() {
            self.move_ghost(k);
        }
        self.collide(controlled);
    }
    fn draw(&self, d: &mut dyn Draw) {
        let ox = self.ox;
        // A board shorter than the maze shows the slice around pac-man, a tile of margin at the ends.
        let oy = if self.rows < MAZE_H + 2 {
            (self.rows as f32 / 2.0 - self.pac.y - 0.5).clamp((self.rows - MAZE_H - 1) as f32, 1.0)
        } else {
            self.oy
        };
        // The maze flashes once it's cleared.
        let flash = matches!(self.phase, Phase::Cleared(t) if (t * 4.0) as i32 % 2 == 1);
        let line = if flash { 0.2 } else { 0.7 };
        for &[x, y, w, h] in &self.walls {
            d.rect(ox + x, oy + y, w, h, line);
        }
        // The door, between the house walls' line ends.
        d.rect(ox + 12.5 + WALL_LINE / 2.0, oy + 12.5 - WALL_LINE / 4.0, 3.0 - WALL_LINE, WALL_LINE / 2.0, 0.35);

        let blink = self.phase == Phase::Play && (self.time * 3.0).fract() < 0.5;
        for (i, &dot) in self.dots.iter().enumerate() {
            let x = ox + (i as i32 % MAZE_W) as f32;
            let y = oy + (i as i32 / MAZE_W) as f32;
            match dot {
                1 => d.cell(x, y, 0.8, 0.22),
                2 if !blink => d.disc(x, y, 0.4, 0.9),
                _ => {}
            }
        }
        if self.fruit > 0.0 {
            // Cherries.
            let (x, y) = (ox + FRUIT.0, oy + FRUIT.1);
            d.rect(x + 0.45, y + 0.05, 0.08, 0.45, 0.6);
            d.rect(x + 0.45, y + 0.05, 0.3, 0.08, 0.6);
            d.disc(x - 0.25, y + 0.25, 0.28, 0.9);
            d.disc(x + 0.25, y + 0.3, 0.28, 0.9);
        }

        // Seconds into the death, while it plays.
        let dying = match self.phase {
            Phase::Dying(t) => Some(DEATH - t),
            _ => None,
        };
        let ghosts_shown = !dying.is_some_and(|e| e > 0.5) && !matches!(self.phase, Phase::Cleared(_));
        if ghosts_shown {
            // Scared ghosts flash as their time runs out.
            let flash = self.fright < 2.0 && (self.fright * 4.0).fract() < 0.5;
            let feet = (self.time * 8.0) as i32 % 2 == 0;
            for (g, alpha) in self.ghosts.iter().zip([0.95, 0.85, 0.75, 0.65]) {
                let look = match g.state {
                    Haunt::Eyes | Haunt::Entering => Look::Eyes,
                    _ if g.scared => Look::Scared { flash },
                    _ => Look::Body,
                };
                let dir = (g.at.dir.x as f32, g.at.dir.y as f32);
                d.ghost(&Sprite { x: ox + g.at.x, y: oy + g.at.y, r: 0.82, dir, look, feet, alpha });
            }
        }

        let p = self.pac;
        let (mouth, dir) = match dying {
            Some(e) if e > 1.7 => return,
            // Mouth opens all the way round until nothing is left.
            Some(e) if e > 0.5 => (0.15 + (PI - 0.15) * ((e - 0.5) / 1.2).min(1.0), UP),
            _ if matches!(self.phase, Phase::Ready(_)) => (0.0, p.dir),
            _ => {
                let open = 1.0 - ((self.chomp * 1.5).fract() * 2.0 - 1.0).abs();
                (0.08 + 0.8 * open, p.dir)
            }
        };
        d.pac(ox + p.x, oy + p.y, 0.8, dir.x as f32, dir.y as f32, mouth, 1.0);
    }
}

// ---- block breaker ----------------------------------------------------------

const BRICK_W: i32 = 4;
const BRICK_TOP: i32 = 2;

struct Ball {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
}

struct Breaker {
    cols: f64,
    rows: f64,
    bricks: Vec<u8>,
    brick_cols: i32,
    brick_rows: i32,
    bx0: i32,
    left: i32,
    px: f64,
    pw: f64,
    shift: f64,
    target: Option<f64>,
    speed: f64,
    ball: Ball,
    score: u32,
    lives: u32,
    over: bool,
    rng: Rng,
}

impl Breaker {
    fn new(seed: u64) -> Self {
        Self { cols: 0., rows: 0., bricks: Vec::new(), brick_cols: 0, brick_rows: 0, bx0: 0, left: 0, px: 0., pw: 6., shift: 0., target: None, speed: 0.3, ball: Ball { x: 0., y: 0., vx: 0., vy: 0. }, score: 0, lives: 3, over: false, rng: Rng::new(seed) }
    }

    fn brick_at(&self, x: f64, y: f64) -> Option<usize> {
        let r = y.floor() as i32 - BRICK_TOP;
        let col = ((x - self.bx0 as f64) / BRICK_W as f64).floor() as i32;
        if r < 0 || r >= self.brick_rows || col < 0 || col >= self.brick_cols || x < self.bx0 as f64 {
            return None;
        }
        let i = (r * self.brick_cols + col) as usize;
        (self.bricks[i] == 1).then_some(i)
    }

    fn serve(&mut self) {
        self.px = self.cols / 2.0;
        let side = if self.rng.next() < 0.5 { -1.0 } else { 1.0 };
        let vx = self.speed * 0.5 * side;
        self.ball = Ball { x: self.px, y: self.rows - 3.0, vx, vy: -(self.speed.powi(2) - vx.powi(2)).sqrt() };
    }

    fn level(&mut self) {
        self.brick_cols = self.cols as i32 / BRICK_W;
        self.brick_rows = (self.rows as i32 / 6).clamp(2, 6);
        self.bx0 = (self.cols as i32 - self.brick_cols * BRICK_W) / 2;
        self.bricks = vec![1; (self.brick_cols * self.brick_rows) as usize];
        self.left = self.bricks.len() as i32;
        self.serve();
    }

    fn lose(&mut self, controlled: bool) {
        if controlled {
            self.lives = self.lives.saturating_sub(1);
            if self.lives == 0 {
                self.over = true;
                return;
            }
        }
        self.serve();
    }
}

impl Game for Breaker {
    fn score(&self) -> u32 {
        self.score
    }
    fn lives(&self) -> u32 {
        self.lives
    }
    fn over(&self) -> bool {
        self.over
    }
    fn resize(&mut self, cols: i32, rows: i32) {
        self.cols = cols.max(12) as f64;
        self.rows = rows.max(10) as f64;
        self.pw = round(self.cols / 12.0).max(5.0);
        self.speed = (self.rows / 140.0).max(0.25);
        self.reset();
    }
    fn reset(&mut self) {
        self.score = 0;
        self.lives = 3;
        self.over = false;
        self.target = None;
        self.shift = 0.0;
        self.level();
    }
    fn steer(&mut self, x: i32, _: i32) {
        self.shift = x as f64;
        self.target = None;
    }
    fn point(&mut self, x: f64) {
        self.target = Some(x);
    }
    fn step(&mut self, controlled: bool) {
        if self.over || self.bricks.is_empty() {
            return;
        }
        let aim = if controlled {
            self.target
        } else {
            Some(self.ball.x + self.ball.y.sin() * self.pw * 0.3)
        };
        match aim {
            Some(aim) => self.px += (aim - self.px).clamp(-1.0, 1.0),
            None => self.px += self.shift,
        }
        self.px = self.px.min(self.cols - self.pw / 2.0).max(self.pw / 2.0);

        let mut nx = self.ball.x + self.ball.vx;
        let mut ny = self.ball.y + self.ball.vy;
        if nx < 0.0 || nx >= self.cols {
            self.ball.vx = -self.ball.vx;
            nx = self.ball.x;
        }
        if ny < 0.0 {
            self.ball.vy = -self.ball.vy;
            ny = self.ball.y;
        }
        if let Some(hit) = self.brick_at(nx, ny) {
            self.bricks[hit] = 0;
            self.left -= 1;
            if controlled {
                self.score += 1;
            }
            if self.brick_at(nx, self.ball.y) == Some(hit) {
                self.ball.vx = -self.ball.vx;
            } else {
                self.ball.vy = -self.ball.vy;
            }
            if self.left == 0 {
                self.level();
            }
            return;
        }
        let py = self.rows - 2.0;
        if self.ball.vy > 0.0 && self.ball.y < py && ny >= py && (nx - self.px).abs() <= self.pw / 2.0 + 0.5 {
            // Where it lands on the paddle sets the angle, as in the arcade game.
            self.ball.vx = ((nx - self.px) / (self.pw / 2.0)).clamp(-0.8, 0.8) * self.speed;
            self.ball.vy = -(self.speed.powi(2) - self.ball.vx.powi(2)).sqrt();
            return;
        }
        if ny >= self.rows {
            return self.lose(controlled);
        }
        self.ball.x = nx;
        self.ball.y = ny;
    }
    fn draw(&self, d: &mut dyn Draw) {
        for r in 0..self.brick_rows {
            for col in 0..self.brick_cols {
                if self.bricks[(r * self.brick_cols + col) as usize] == 1 {
                    for k in 0..BRICK_W - 1 {
                        let x = self.bx0 + col * BRICK_W + k;
                        d.cell(x as f32, (BRICK_TOP + r) as f32, 0.75 - r as f32 * 0.08, 1.0);
                    }
                }
            }
        }
        let start = round(self.px - self.pw / 2.0) as f32;
        for k in 0..self.pw as i32 {
            d.cell(start + k as f32, self.rows as f32 - 2.0, 0.8, 1.0);
        }
        d.cell(self.ball.x as f32 - 0.5, self.ball.y as f32 - 0.5, 1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Count {
        cells: usize,
        pacs: usize,
        ghosts: usize,
    }

    impl Draw for Count {
        fn cell(&mut self, _: f32, _: f32, _: f32, _: f32) {
            self.cells += 1;
        }
        fn rect(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32) {
            self.cells += 1;
        }
        fn disc(&mut self, _: f32, _: f32, _: f32, _: f32) {
            self.cells += 1;
        }
        fn pac(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
            self.pacs += 1;
        }
        fn ghost(&mut self, _: &Sprite) {
            self.ghosts += 1;
        }
    }

    #[test]
    fn every_game_plays_itself_at_any_size() {
        for def in &GAMES {
            for (cols, rows) in [(0, 0), (3, 2), (40, 12), (150, 60)] {
                let mut game = (def.create)(7);
                game.resize(cols, rows);
                for _ in 0..3000 {
                    game.step(false);
                }
                assert!(!game.over(), "{} never ends on its own", def.id);
                assert_eq!(game.score(), 0, "{} only scores under control", def.id);
                let mut count = Count::default();
                game.draw(&mut count);
                assert!(count.cells > 0);
            }
        }
    }

    #[test]
    fn snake_ignores_reversing_and_dies_on_itself() {
        let mut snake = Snake::new(3);
        snake.resize(20, 10);
        snake.steer(-1, 0);
        assert_eq!(snake.queued, None);
        // A tight loop into its own body: down, left, up.
        snake.body = [c(5, 5), c(4, 5), c(3, 5), c(3, 6), c(4, 6), c(5, 6), c(6, 6)].into();
        snake.pellet = c(0, 0);
        snake.steer(0, 1);
        snake.step(true);
        assert!(snake.over);
    }

    fn playing_pacman() -> Pacman {
        let mut pac = Pacman::new(5);
        pac.resize(30, 33);
        pac.phase = Phase::Play;
        pac
    }

    #[test]
    fn pacman_has_the_arcade_maze() {
        assert!(MAZE.iter().all(|row| row.len() == MAZE_W as usize));
        let pac = playing_pacman();
        assert_eq!(pac.left, 244, "240 dots and 4 energizers");
        assert_eq!((pac.ox, pac.oy), (1.0, 1.0));
        let mut count = Count::default();
        pac.draw(&mut count);
        assert_eq!((count.pacs, count.ghosts), (1, 4));
    }

    #[test]
    fn pacman_glides_and_wraps_through_the_tunnel() {
        let mut pac = playing_pacman();
        let x = pac.pac.x;
        pac.step(true);
        assert!(pac.pac.x < x && pac.pac.x > x - 0.2, "moves a fraction of a tile a tick");
        let mut m = Mover { x: 1.0, y: 14.0, dir: LEFT };
        for _ in 0..12 {
            m.run(0.15, |m| Some(m.dir));
        }
        assert!(m.x > 27.0 && m.x < 27.5, "came out the right side at {}", m.x);
        // A wall ahead stops it on the tile center.
        let mut m = Mover { x: 1.0, y: 1.0, dir: LEFT };
        assert_eq!(m.run(0.5, |m| Some(m.dir)), 0.0);
    }

    #[test]
    fn pacman_takes_a_queued_turn_at_the_next_corner() {
        let mut pac = playing_pacman();
        pac.pac = Mover { x: 2.6, y: 5.0, dir: LEFT };
        pac.steer(0, -1);
        for _ in 0..30 {
            pac.move_pac(true);
        }
        assert_eq!(pac.pac.x, 1.0);
        assert!(pac.pac.y < 5.0 && pac.pac.dir == UP);
    }

    #[test]
    fn ghosts_target_like_the_arcade() {
        let mut pac = playing_pacman();
        pac.clock = 1e6;
        pac.pac = Mover { x: 10.0, y: 23.0, dir: LEFT };
        pac.ghosts[0].at = Mover { x: 6.0, y: 20.0, dir: LEFT };
        assert_eq!(pac.target(0), c(10, 23));
        assert_eq!(pac.target(1), c(6, 23));
        // Inky doubles the line from Blinky to two tiles ahead of pac-man.
        assert_eq!(pac.target(2), c(10, 26));
        pac.pac.dir = UP;
        assert_eq!(pac.target(1), c(6, 19));
        pac.clock = 0.0;
        assert_eq!(pac.target(0), CORNERS[0]);
    }

    #[test]
    fn pacman_eats_scared_ghosts_and_loses_lives_to_the_rest() {
        let mut pac = playing_pacman();
        pac.frighten();
        assert!(pac.ghosts.iter().all(|g| g.scared));
        pac.ghosts[0].at = pac.pac;
        pac.collide(true);
        assert_eq!(pac.score, 200);
        assert_eq!(pac.ghosts[0].state, Haunt::Eyes);
        assert_eq!(pac.phase, Phase::Eat(0.5));

        pac.phase = Phase::Play;
        pac.ghosts[1].state = Haunt::Out;
        pac.ghosts[1].scared = false;
        pac.ghosts[1].at = pac.pac;
        pac.collide(true);
        assert_eq!(pac.phase, Phase::Dying(DEATH));
        for _ in 0..3 {
            pac.revive(true);
        }
        assert!(pac.over);
    }

    #[test]
    fn ghosts_all_get_out_and_eaten_ones_come_back() {
        let mut pac = playing_pacman();
        let mut out = [false; 4];
        for _ in 0..120 * 40 {
            pac.step(false);
            for (seen, g) in out.iter_mut().zip(&pac.ghosts) {
                *seen |= g.state == Haunt::Out;
            }
        }
        assert_eq!(out, [true; 4]);

        let mut pac = playing_pacman();
        pac.ghosts[0].state = Haunt::Eyes;
        pac.ghosts[0].at = Mover { x: 6.0, y: 5.0, dir: RIGHT };
        for _ in 0..120 * 10 {
            pac.move_ghost(0);
        }
        assert_ne!(pac.ghosts[0].state, Haunt::Eyes);
    }

    #[test]
    fn breaker_scores_bricks_and_ends_after_three_misses() {
        let mut game = Breaker::new(9);
        game.resize(40, 30);
        let bricks = game.left;
        game.ball = Ball { x: game.bx0 as f64 + 1.0, y: (BRICK_TOP + 1) as f64 + 0.5, vx: 0.0, vy: -0.3 };
        game.step(true);
        assert_eq!((game.score, game.left), (1, bricks - 1));
        for _ in 0..3 {
            game.ball = Ball { x: 0.5, y: game.rows - 0.1, vx: 0.0, vy: 0.3 };
            game.px = game.cols - game.pw;
            game.step(true);
        }
        assert!(game.over);
    }

    #[test]
    fn keys_and_mode_file() {
        assert_eq!(heading("Up"), Some((0, -1)));
        assert_eq!(heading("d"), Some((1, 0)));
        assert_eq!(heading("space"), None);
        let dir = std::env::temp_dir().join(format!("clipward-arcade-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load_mode(&dir), Mode::Mid);
        save_mode(&dir, Mode::Hard).unwrap();
        assert_eq!(load_mode(&dir), Mode::Hard);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

