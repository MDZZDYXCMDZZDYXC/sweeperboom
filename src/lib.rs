//! # sweeperboom — Pumpkin 服务器「命令对话版扫雷」插件
//!
//! 参考 <https://docs.pumpkinmc.org/plugin-dev/rust> 的插件与命令开发指南实现，
//! 结构上与同目录的 `two048` 插件保持一致。
//!
//! 玩家在聊天栏里通过 `/sweeperboom ...` 系列子命令游玩扫雷：
//! 每次操作都会把当前雷区、剩余雷数与可点击的操作按钮重新推送给玩家，
//! 棋盘上的每个格子本身也是可点击的，形成「命令行对话式」的完整对局体验。
//!
//! ## 支持的命令
//!
//! | 命令 | 别名 | 说明 |
//! | --- | --- | --- |
//! | `/sweeperboom` | `ms` `mine` `sweep` | 显示当前雷区（没有对局时给出难度选择） |
//! | `/sweeperboom start [easy\|medium\|hard]` | `new` `restart` `reset` | 开始 / 重开一局，默认中等 |
//! | `/sweeperboom open <格>` | `o` `dig` `reveal` | 打开一格 |
//! | `/sweeperboom flag <格>` | `f` `mark` | 插旗 / 取消插旗 |
//! | `/sweeperboom chord <格>` | `ch` | 数字格周围旗数达标时，连带打开其余相邻格 |
//! | `/sweeperboom mode [open\|flag]` | - | 切换 / 指定「点击棋盘」的行为 |
//! | `/sweeperboom undo` | `back` `z` | 撤销上一步（对局结束后不可用） |
//! | `/sweeperboom show` | `board` `b` | 重新显示雷区 |
//! | `/sweeperboom best` | `top` `rank` `leaderboard` | 全服排行榜（前 10 名 + 自己的名次） |
//! | `/sweeperboom me` | `stats` `profile` | 我的战绩与排名 |
//! | `/sweeperboom stop` | `quit` `end` `exit` | 结束当前对局 |
//! | `/sweeperboom help` | `h` | 玩法说明 |
//!
//! ## 坐标
//!
//! 列用字母 `a` 起（最多到 `p`），行用数字 `1` 起，例如 `c5`。
//! 也接受 `3-5`、`3,5`、`3:5` 这类「列-行」纯数字写法。
//!
//! ## 排行榜
//!
//! 每位玩家的战绩（胜场、负场、各难度最快通关时间）会记录在插件数据目录下的
//! `sweeperboom_leaderboard.tsv` 中，服务端重启后依然保留。
//! 排序规则：胜场 → 困难最快 → 中等最快 → 简单最快 → 玩家名（字典序）。
//!
//! 战绩会在这些时机刷新：踩雷失败时、全清胜利时、`/sweeperboom stop` 时，
//! 以及用 `/sweeperboom start` 放弃一局进行中的对局时（没打开过任何格子的空局不计入）。

use std::sync::{Mutex, MutexGuard};

use pumpkin_plugin_api::command::{
    Arg, ArgumentType, Command, CommandError, CommandNode, CommandSender, ConsumedArgs, StringType,
};
use pumpkin_plugin_api::commands::CommandHandler;
use pumpkin_plugin_api::common::NamedColor;
use pumpkin_plugin_api::permission::{Permission, PermissionDefault};
use pumpkin_plugin_api::permissions::{FS_READ_DATA, FS_WRITE_DATA};
use pumpkin_plugin_api::text::TextComponent;
use pumpkin_plugin_api::{Context, EntityType, Plugin, PluginMetadata, Server};

// ===========================================================================
// 常量
// ===========================================================================

/// 命令所需的权限节点。
const PERM_NODE: &str = "sweeperboom:command.play";
/// 命令的主名与别名。
const COMMAND_NAMES: [&str; 2] = ["sweeperboom", "sweeper"];
/// 命令描述。
///
/// 命令树在 `on_load` 里一次性注册，那时没有任何玩家上下文，因此这条**无法**
/// 按玩家语言渲染。它在客户端的补全提示里对玩家可见，所以两种语言并列写在同一行。
const COMMAND_DESCRIPTION: &str = "在聊天栏里玩 SweeperBoom 小游戏 · Play SweeperBoom in chat";
/// 插件描述，同样在 `on_load` 时注册，无法按玩家语言渲染。
const PLUGIN_DESCRIPTION: &str =
    "在聊天栏里用命令对话玩 SweeperBoom 小游戏（含全服排行榜）· Command-driven SweeperBoom in chat, with a server-wide leaderboard";
/// 排行榜最多展示多少名。
const TOP_N: usize = 10;
/// 排行榜文件名（位于插件数据目录内）。
///
/// 有意与 `two048` 的 `leaderboard.tsv` 区分开：当宿主把插件数据目录折叠成同一个
/// `data/` 时也不会互相覆盖。
const BOARD_FILE: &str = "sweeperboom_leaderboard.tsv";

/// 单元格参数名。
const CELL_ARG: &str = "cell";

// ---------------------------------------------------------------------------
// 纯逻辑区（可被 tools/logic_harness.py 抽出后用本机 rustc 单独编译测试）
// >>> PURE-LOGIC-BEGIN
// ---------------------------------------------------------------------------

/// 每个玩家最多保留多少步可撤销历史。
const MAX_HISTORY: usize = 16;
/// 难度数量（简单 / 中等 / 困难）。
const DIFF_COUNT: usize = 3;
/// 帮助里的命令条数。
const HELP_COUNT: usize = 11;

/// 难度。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Difficulty {
    Easy,
    Medium,
    Hard,
}

impl Difficulty {
    /// 全部难度，顺序固定。
    fn all() -> [Difficulty; DIFF_COUNT] {
        [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard]
    }

    /// 命令字面量 / TSV 键。
    fn key(self) -> &'static str {
        match self {
            Difficulty::Easy => "easy",
            Difficulty::Medium => "medium",
            Difficulty::Hard => "hard",
        }
    }

    /// 数组下标，用于按难度存放成绩。
    fn slot(self) -> usize {
        match self {
            Difficulty::Easy => 0,
            Difficulty::Medium => 1,
            Difficulty::Hard => 2,
        }
    }

    /// 棋盘尺寸与雷数：`(宽, 高, 雷数)`。
    ///
    /// 宽度上限 16 是为了让一整行在聊天栏里不会被折行：
    /// 每格渲染为「字形 + 空格」，加上 3 字符的行号前缀，16 列共 35 字符。
    fn size(self) -> (usize, usize, usize) {
        match self {
            Difficulty::Easy => (8, 8, 10),
            Difficulty::Medium => (12, 12, 25),
            Difficulty::Hard => (16, 16, 50),
        }
    }
}

/// 点击棋盘格子时的行为。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Open,
    Flag,
}

impl Mode {
    fn toggled(self) -> Self {
        match self {
            Mode::Open => Mode::Flag,
            Mode::Flag => Mode::Open,
        }
    }

    /// 命令字面量。
    fn key(self) -> &'static str {
        match self {
            Mode::Open => "open",
            Mode::Flag => "flag",
        }
    }
}

/// 对局状态。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Playing,
    Won,
    Lost,
}

/// 玩家在某一格上发起的动作。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Slot {
    Open,
    Flag,
    Chord,
}

/// 一次动作的结果。只描述「发生了什么」，不含任何渲染信息。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    /// 对局已经结束。
    Over,
    /// 这一格已经打开过了。
    AlreadyOpen,
    /// 想打开的格子插着旗子。
    Flagged,
    /// chord 只能作用在已打开的数字格上。
    NotOpen,
    /// chord 周围没有可连带打开的格子。
    NothingToChord,
    /// chord 周围旗数与数字不符。
    FlagsMismatch { need: u8, have: u8 },
    /// 成功打开。
    Opened { gained: u32, first: bool },
    /// 踩到雷。
    HitMine,
    /// 全部排清。
    Won,
    /// 插旗状态变化。
    Toggled { flagged: bool },
    /// chord 连带打开。
    Chorded { gained: u32 },
}

/// 撤销的结果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Undo {
    Done,
    Empty,
    Over,
}

// ---------------------------------------------------------------------------
// 伪随机数：uuid + 时间混合播种的 splitmix64
// ---------------------------------------------------------------------------

struct Rng {
    state: u64,
}

impl Rng {
    /// 用给定种子构造。种子由调用方在**不持有任何锁**时从宿主取得。
    fn seeded(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// splitmix64：每调用一次推进一步，返回质量良好的 64 位随机数。
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 返回 `[0, bound)` 内的下标；`bound == 0` 时返回 0。
    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }
}

// ---------------------------------------------------------------------------
// 雷区
// ---------------------------------------------------------------------------

/// 一块雷区。地雷在**第一次打开**时才布置，因此第一次点击永远安全。
#[derive(Clone)]
struct Board {
    width: usize,
    height: usize,
    /// 总雷数。
    mines: usize,
    /// 每格是否为雷。
    mine: Vec<bool>,
    /// 每格是否已打开。
    revealed: Vec<bool>,
    /// 每格是否插旗。
    flagged: Vec<bool>,
    /// 每格周围 8 格的雷数（雷格固定为 0）。
    adjacent: Vec<u8>,
    /// 是否已经布置过地雷。
    placed: bool,
}

impl Board {
    fn new(width: usize, height: usize, mines: usize) -> Self {
        let cells = width * height;
        Self {
            width,
            height,
            mines,
            mine: vec![false; cells],
            revealed: vec![false; cells],
            flagged: vec![false; cells],
            adjacent: vec![0; cells],
            placed: false,
        }
    }

    fn cells(&self) -> usize {
        self.mine.len()
    }

    /// 最多 8 个相邻格的线性下标。
    fn neighbors(&self, index: usize) -> Vec<usize> {
        let x = (index % self.width) as isize;
        let y = (index / self.width) as isize;
        let mut out = Vec::with_capacity(8);
        for dy in -1..=1isize {
            for dx in -1..=1isize {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = x + dx;
                let ny = y + dy;
                if nx < 0 || ny < 0 || nx >= self.width as isize || ny >= self.height as isize {
                    continue;
                }
                out.push(ny as usize * self.width + nx as usize);
            }
        }
        out
    }

    /// 由 `mine` 重算 `adjacent`。
    fn recompute_adjacent(&mut self) {
        for i in 0..self.cells() {
            if self.mine[i] {
                self.adjacent[i] = 0;
                continue;
            }
            let count = self.neighbors(i).iter().filter(|&&j| self.mine[j]).count();
            self.adjacent[i] = count as u8;
        }
    }

    /// 布置地雷：`safe` 格以及（容量允许时）它的 8 个邻居都不会是雷。
    ///
    /// 采用部分 Fisher–Yates 抽取，因此每一组可行的布雷方案等概率出现。
    fn place_mines(&mut self, safe: usize, rng: &mut Rng) {
        let mut pool: Vec<usize> = (0..self.cells())
            .filter(|&i| i != safe && !self.neighbors(safe).contains(&i))
            .collect();
        if pool.len() < self.mines {
            // 雷数过密时退化为「只保护首次点击的那一格」。
            pool = (0..self.cells()).filter(|&i| i != safe).collect();
        }

        let picks = self.mines.min(pool.len());
        for k in 0..picks {
            let j = k + rng.below(pool.len() - k);
            pool.swap(k, j);
        }
        for &i in pool.iter().take(picks) {
            self.mine[i] = true;
        }

        self.recompute_adjacent();
        self.placed = true;
    }

    /// 从 `start` 开始泛洪展开，返回新打开的格子数。
    ///
    /// 已插旗的格子不会被自动打开（这是玩家自己的标记，交给玩家处理）。
    fn reveal_flood(&mut self, start: usize) -> usize {
        let mut gained = 0usize;
        let mut stack = vec![start];
        let mut queued = vec![false; self.cells()];
        queued[start] = true;

        while let Some(index) = stack.pop() {
            if self.revealed[index] || self.flagged[index] {
                continue;
            }
            self.revealed[index] = true;
            gained += 1;

            if self.adjacent[index] == 0 {
                for n in self.neighbors(index) {
                    if !self.revealed[n] && !self.flagged[n] && !queued[n] {
                        queued[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        gained
    }

    /// 已插旗的格子数。
    fn flags(&self) -> usize {
        self.flagged.iter().filter(|f| **f).count()
    }

    /// 非雷格总数，也就是通关需要打开的格子数。
    fn safe_cells(&self) -> usize {
        self.cells() - self.mines
    }
}

// ---------------------------------------------------------------------------
// 单局游戏
// ---------------------------------------------------------------------------

/// 一步之前可回滚的状态。
#[derive(Clone)]
struct Snapshot {
    board: Board,
    status: Status,
    exploded: Option<usize>,
    revealed_total: usize,
    moves: u32,
    elapsed: u32,
    started_at: u64,
}

/// 单局扫雷的完整状态。
#[derive(Clone)]
struct Game {
    board: Board,
    difficulty: Difficulty,
    status: Status,
    /// 踩中的那颗雷（仅失败时有值，用于把它标红）。
    exploded: Option<usize>,
    /// 已经打开的格子数，用于判定通关。
    revealed_total: usize,
    /// 操作步数（打开 / 插旗 / chord 都算）。
    moves: u32,
    /// 点击棋盘时的行为。
    mode: Mode,
    /// 第一次打开格子的时间戳（秒）；0 表示计时尚未开始。
    started_at: u64,
    /// 结束时记录的通关 / 失败用时（秒）。
    elapsed: u32,
    /// 本局是否已经计入排行榜，避免同一局被重复累加。
    recorded: bool,
    history: Vec<Snapshot>,
}

impl Game {
    /// 新开一局。地雷留到第一次打开时才布置。
    fn new(difficulty: Difficulty) -> Self {
        let (width, height, mines) = difficulty.size();
        Self {
            board: Board::new(width, height, mines),
            difficulty,
            status: Status::Playing,
            exploded: None,
            revealed_total: 0,
            moves: 0,
            mode: Mode::Open,
            started_at: 0,
            elapsed: 0,
            recorded: false,
            history: Vec::new(),
        }
    }

    fn is_over(&self) -> bool {
        self.status != Status::Playing
    }

    /// 剩余雷数（可能为负，说明旗子插多了）。
    fn mines_left(&self) -> i32 {
        self.board.mines as i32 - self.board.flags() as i32
    }

    fn elapsed_at(&self, now: u64) -> u32 {
        if self.started_at == 0 {
            0
        } else {
            now.saturating_sub(self.started_at).min(u32::MAX as u64) as u32
        }
    }

    fn push_history(&mut self) {
        self.history.push(Snapshot {
            board: self.board.clone(),
            status: self.status,
            exploded: self.exploded,
            revealed_total: self.revealed_total,
            moves: self.moves,
            elapsed: self.elapsed,
            started_at: self.started_at,
        });
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
    }

    /// 打开一格。
    fn open(&mut self, index: usize, rng: &mut Rng, now: u64) -> Step {
        if self.is_over() {
            return Step::Over;
        }
        if self.board.revealed[index] {
            return Step::AlreadyOpen;
        }
        if self.board.flagged[index] {
            return Step::Flagged;
        }

        let first = !self.board.placed;
        self.push_history();
        if first {
            self.board.place_mines(index, rng);
            self.started_at = now;
        }
        self.moves += 1;

        if self.board.mine[index] {
            self.status = Status::Lost;
            self.exploded = Some(index);
            self.elapsed = self.elapsed_at(now);
            return Step::HitMine;
        }

        let gained = self.board.reveal_flood(index);
        self.revealed_total += gained;
        if self.check_win(now) {
            return Step::Won;
        }
        Step::Opened {
            gained: gained as u32,
            first,
        }
    }

    /// 插旗 / 取消插旗。
    fn flag(&mut self, index: usize) -> Step {
        if self.is_over() {
            return Step::Over;
        }
        if self.board.revealed[index] {
            return Step::AlreadyOpen;
        }
        self.push_history();
        self.moves += 1;
        let flagged = !self.board.flagged[index];
        self.board.flagged[index] = flagged;
        Step::Toggled { flagged }
    }

    /// 数字格周围旗数已达标时，连带打开其余相邻格。旗数与数字不符时不动作。
    fn chord(&mut self, index: usize, now: u64) -> Step {
        if self.is_over() {
            return Step::Over;
        }
        if !self.board.revealed[index] {
            return Step::NotOpen;
        }
        let need = self.board.adjacent[index];
        if need == 0 {
            return Step::NothingToChord;
        }

        let neighbors = self.board.neighbors(index);
        let have = neighbors.iter().filter(|&&n| self.board.flagged[n]).count() as u8;
        if have != need {
            return Step::FlagsMismatch { need, have };
        }

        let targets: Vec<usize> = neighbors
            .iter()
            .copied()
            .filter(|&n| !self.board.revealed[n] && !self.board.flagged[n])
            .collect();
        if targets.is_empty() {
            return Step::NothingToChord;
        }

        self.push_history();
        self.moves += 1;

        let mut gained = 0usize;
        for target in targets {
            if self.board.mine[target] {
                self.status = Status::Lost;
                self.exploded = Some(target);
                self.elapsed = self.elapsed_at(now);
                return Step::HitMine;
            }
            gained += self.board.reveal_flood(target);
        }
        self.revealed_total += gained;
        if self.check_win(now) {
            return Step::Won;
        }
        Step::Chorded {
            gained: gained as u32,
        }
    }

    /// 通关判定：所有非雷格都已打开。
    fn check_win(&mut self, now: u64) -> bool {
        if self.revealed_total >= self.board.safe_cells() {
            self.status = Status::Won;
            self.elapsed = self.elapsed_at(now);
            true
        } else {
            false
        }
    }

    /// 回滚上一步。对局结束后不再允许撤销，以免已计入排行榜的成绩被改写。
    fn undo(&mut self) -> Undo {
        if self.is_over() {
            return Undo::Over;
        }
        match self.history.pop() {
            Some(snapshot) => {
                self.board = snapshot.board;
                self.status = snapshot.status;
                self.exploded = snapshot.exploded;
                self.revealed_total = snapshot.revealed_total;
                self.moves = snapshot.moves;
                self.elapsed = snapshot.elapsed;
                self.started_at = snapshot.started_at;
                Undo::Done
            }
            None => Undo::Empty,
        }
    }
}

// ---------------------------------------------------------------------------
// 坐标解析
// ---------------------------------------------------------------------------

/// 线性下标 → `c5` 形式的坐标。
fn cell_label(index: usize, width: usize) -> String {
    let x = index % width;
    let y = index / width;
    let letter = char::from(b'a' + (x as u8).min(25));
    format!("{letter}{}", y + 1)
}

/// 坐标 → 线性下标。接受 `c5`、`C5`、`3-5`、`3,5`、`3:5` 等写法。
fn parse_cell(input: &str, width: usize, height: usize) -> Option<usize> {
    let text = input.trim();
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let (column, rest) = if bytes[0].is_ascii_alphabetic() {
        let letter = bytes[0].to_ascii_lowercase();
        ((letter - b'a') as usize, &text[1..])
    } else {
        // 纯数字列号必须带分隔符，否则 `35` 无法判断是「第 3 列第 5 行」还是别的意思。
        let split = text.find(['-', ',', ':', ' '])?;
        let column: usize = text[..split].trim().parse().ok()?;
        if column == 0 {
            return None;
        }
        (column - 1, text[split + 1..].trim())
    };

    let row: usize = rest.trim().parse().ok()?;
    if row == 0 {
        return None;
    }

    let (x, y) = (column, row - 1);
    if x >= width || y >= height {
        return None;
    }
    Some(y * width + x)
}

// ---------------------------------------------------------------------------
// 排行榜数据
// ---------------------------------------------------------------------------

/// 一位玩家的累计战绩。
#[derive(Clone)]
struct Profile {
    name: String,
    wins: u32,
    losses: u32,
    /// 各难度最快通关用时（秒）；索引见 `Difficulty::slot`，0 表示暂无记录。
    best: [u32; DIFF_COUNT],
}

impl Profile {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            wins: 0,
            losses: 0,
            best: [0; DIFF_COUNT],
        }
    }

    fn best_of(&self, difficulty: Difficulty) -> u32 {
        self.best[difficulty.slot()]
    }

    fn games(&self) -> u32 {
        self.wins + self.losses
    }

    fn win_rate(&self) -> u32 {
        let games = self.games();
        if games == 0 {
            0
        } else {
            (self.wins as u64 * 100 / games as u64) as u32
        }
    }
}

/// 没有记录的用时排在有记录的后面。
fn time_key(seconds: u32) -> u32 {
    if seconds == 0 { u32::MAX } else { seconds }
}

/// 榜单排序：胜场 → 困难最快 → 中等最快 → 简单最快 → 玩家名。
///
/// 先比胜场，保证「打得又多又好」排在前面；胜场相同时再比通关速度。
fn sort_profiles(list: &mut [Profile]) {
    list.sort_by(|a, b| {
        b.wins
            .cmp(&a.wins)
            .then(time_key(a.best_of(Difficulty::Hard)).cmp(&time_key(b.best_of(Difficulty::Hard))))
            .then(
                time_key(a.best_of(Difficulty::Medium)).cmp(&time_key(b.best_of(Difficulty::Medium))),
            )
            .then(time_key(a.best_of(Difficulty::Easy)).cmp(&time_key(b.best_of(Difficulty::Easy))))
            .then(a.name.cmp(&b.name))
    });
}

/// 名次（从 1 开始）。
fn rank_in(sorted: &[Profile], name: &str) -> Option<usize> {
    sorted.iter().position(|p| p.name == name).map(|i| i + 1)
}

/// 排行榜 TSV 编码。
fn encode_profiles(list: &[Profile]) -> String {
    let mut out = String::new();
    for profile in list {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            profile.name,
            profile.wins,
            profile.losses,
            profile.best[0],
            profile.best[1],
            profile.best[2]
        ));
    }
    out
}

/// 排行榜 TSV 解码。损坏的行会被静默跳过。
fn decode_profiles(text: &str) -> Vec<Profile> {
    let mut loaded = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 3 + DIFF_COUNT || fields[0].is_empty() {
            continue;
        }
        let (Ok(wins), Ok(losses)) = (fields[1].parse::<u32>(), fields[2].parse::<u32>()) else {
            continue;
        };
        let mut best = [0u32; DIFF_COUNT];
        let mut ok = true;
        for (slot, value) in best.iter_mut().enumerate() {
            match fields[3 + slot].parse::<u32>() {
                Ok(parsed) => *value = parsed,
                Err(_) => ok = false,
            }
        }
        if !ok {
            continue;
        }
        loaded.push(Profile {
            name: fields[0].to_string(),
            wins,
            losses,
            best,
        });
    }
    loaded
}

/// 把一局的结果累加进一位玩家的战绩（纯函数，便于测试）。
///
/// 返回是否为该难度的新纪录。
fn accumulate(profile: &mut Profile, game: &Game, won: bool) -> bool {
    if won {
        profile.wins += 1;
        let slot = game.difficulty.slot();
        let seconds = game.elapsed;
        if seconds > 0 && (profile.best[slot] == 0 || seconds < profile.best[slot]) {
            profile.best[slot] = seconds;
            return true;
        }
    } else {
        profile.losses += 1;
    }
    false
}

// ---------------------------------------------------------------------------
// 多语言
// ---------------------------------------------------------------------------

/// 帮助里的命令列表。命令字面量本身不可本地化，因此单独放一份共享常量。
const HELP_COMMANDS: [&str; HELP_COUNT] = [
    "sweeperboom start",
    "sweeperboom open c5",
    "sweeperboom flag c5",
    "sweeperboom chord c5",
    "sweeperboom mode",
    "sweeperboom undo",
    "sweeperboom show",
    "sweeperboom best",
    "sweeperboom me",
    "sweeperboom stop",
    "sweeperboom help",
];

/// 插件支持的语言。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lang {
    /// 简体中文。
    ZhCn,
    /// 英文，同时是所有未收录语言的回落目标。
    EnUs,
}

impl Lang {
    /// 依据客户端语言串选择语言。
    ///
    /// 只做前缀判断：`zh_cn`、`zh_tw`、`zh_hk` 都归入简体中文——对繁体玩家来说，
    /// 简中远比英文易读；其余语言一律回落英文。
    fn from_locale(locale: &str) -> Self {
        if locale.to_ascii_lowercase().starts_with("zh") {
            Lang::ZhCn
        } else {
            Lang::EnUs
        }
    }

    fn strings(self) -> &'static dyn Strings {
        match self {
            Lang::ZhCn => &ZH_CN,
            Lang::EnUs => &EN_US,
        }
    }
}

/// 面向玩家的全部文案。
///
/// 做成 trait 而不是静态字符串表，有三个理由：
///
/// 1. `format!` 要求格式串是**字面量**，静态表里的模板是运行期字符串，用不了。
///    写成方法后模板仍是字面量，占位符个数或类型写错会直接编译失败，
///    而不是运行期拼出半截句子。
/// 2. 每种语言集中在一个 impl 里，便于整体校对译文。
/// 3. 新增语言只需补一个 impl；漏实现任何一条都是编译错误。
///
/// 注意：方法名与命令字面量不参与本地化，只有面向玩家的散文是。
trait Strings {
    // --- 通用 ---
    /// 可点击组件的悬停提示。
    fn button_hint(&self, command: &str) -> String;

    // --- 棋盘状态 ---
    fn status_title(&self) -> &'static str;
    fn status_size(&self, difficulty: &str, width: usize, height: usize, mines: usize) -> String;
    fn status_remaining(&self, mines_left: i32) -> String;
    fn status_progress(&self, opened: usize, total: usize) -> String;
    fn status_moves(&self, moves: u32) -> String;
    fn status_rank(&self, rank: usize) -> String;
    fn status_mode(&self, mode: &str) -> String;
    fn info_time(&self, seconds: u32) -> String;
    fn info_won(&self, seconds: &str) -> String;
    fn info_lost(&self, seconds: &str) -> String;
    fn info_hint(&self) -> &'static str;

    // --- 单元格悬停 ---
    fn cell_hover_open(&self, coord: &str) -> String;
    fn cell_hover_flag(&self, coord: &str) -> String;
    fn cell_hover_unflag(&self, coord: &str) -> String;
    fn cell_hover_number(&self, coord: &str, adjacent: u8) -> String;
    fn cell_hover_locked(&self, coord: &str) -> String;

    // --- 操作栏 ---
    fn ctl_mode(&self, mode: &str) -> String;
    fn ctl_undo(&self) -> &'static str;
    fn ctl_restart(&self) -> &'static str;
    fn ctl_stop(&self) -> &'static str;
    fn ctl_best(&self) -> &'static str;
    fn ctl_me(&self) -> &'static str;
    fn ctl_help(&self) -> &'static str;

    // --- 名称 ---
    fn diff_label(&self, difficulty: Difficulty) -> &'static str;
    fn mode_label(&self, mode: Mode) -> &'static str;
    fn mode_action(&self, mode: Mode) -> &'static str;
    fn mode_changed(&self, action: &str) -> String;
    fn diff_button(&self, difficulty: &str, width: usize, height: usize, mines: usize) -> String;

    // --- 没有对局 ---
    fn no_game(&self) -> &'static str;
    fn no_game_pick(&self) -> &'static str;

    // --- 帮助 ---
    fn help_title(&self) -> &'static str;
    fn help_descriptions(&self) -> [&'static str; HELP_COUNT];
    fn help_rules(&self) -> &'static str;
    fn help_coords(&self) -> &'static str;
    fn help_click(&self) -> &'static str;
    fn help_safe(&self) -> &'static str;
    fn help_leaderboard(&self) -> &'static str;

    // --- 排行榜 ---
    fn rank_title(&self) -> &'static str;
    fn rank_empty(&self) -> &'static str;
    fn rank_start_button(&self) -> &'static str;
    fn rank_count(&self, players: usize) -> String;
    fn rank_wins(&self, wins: u32) -> String;
    fn rank_losses(&self, losses: u32) -> String;
    fn rank_hard(&self, seconds: &str) -> String;
    fn rank_ellipsis(&self) -> &'static str;
    fn rank_not_listed(&self) -> &'static str;
    fn rank_more(&self) -> &'static str;
    fn rank_me_button(&self) -> &'static str;

    // --- 个人战绩 ---
    fn me_title(&self) -> &'static str;
    fn me_empty(&self) -> &'static str;
    fn me_rank_label(&self) -> &'static str;
    fn me_rank_value(&self, rank: usize, total: usize) -> String;
    fn me_wins_label(&self) -> &'static str;
    fn me_losses_label(&self) -> &'static str;
    fn me_games_label(&self) -> &'static str;
    fn me_rate_label(&self) -> &'static str;
    fn me_best_label(&self, difficulty: &str) -> String;
    fn me_more(&self) -> &'static str;
    fn me_best_button(&self) -> &'static str;

    // --- 交互反馈 ---
    fn bad_cell(&self, raw: &str) -> String;
    fn bad_cell_hint(&self, max_column: char, max_row: usize) -> String;
    fn step_over(&self) -> &'static str;
    fn step_already_open(&self) -> &'static str;
    fn step_flagged(&self) -> &'static str;
    fn step_not_open(&self) -> &'static str;
    fn step_nothing_to_chord(&self) -> &'static str;
    fn step_flags_mismatch(&self, need: u8, have: u8) -> String;
    fn step_first_open(&self, gained: u32) -> String;
    fn step_opened(&self, gained: u32) -> String;
    fn step_flagged_on(&self) -> &'static str;
    fn step_flagged_off(&self) -> &'static str;
    fn step_chorded(&self, gained: u32) -> String;
    fn step_hit_mine(&self, seconds: &str) -> String;
    fn step_won(&self, seconds: &str) -> String;
    fn step_rank(&self, rank: usize, total: usize) -> String;
    fn step_rank_short(&self, rank: usize, total: usize) -> String;
    fn step_new_best(&self, difficulty: &str) -> String;
    /// 踩雷后「脚下被放了 TNT」的提示。
    fn step_tnt(&self) -> &'static str;
    fn step_again(&self) -> &'static str;
    fn undo_done(&self) -> &'static str;
    fn undo_empty(&self) -> &'static str;
    fn undo_over(&self) -> &'static str;
    fn start_abandoned(
        &self,
        difficulty: &str,
        width: usize,
        height: usize,
        mines: usize,
    ) -> String;
    fn start_fresh(&self) -> &'static str;
    fn stop_summary(&self, opened: usize, seconds: &str) -> String;
    fn player_only(&self) -> &'static str;

    // --- 时间 ---
    /// 把秒数渲染成易读的用时；`0` 表示没有记录。
    fn format_seconds(&self, seconds: u32) -> String;
}

struct ZhCn;
struct EnUs;

static ZH_CN: ZhCn = ZhCn;
static EN_US: EnUs = EnUs;

impl Strings for ZhCn {
    fn button_hint(&self, command: &str) -> String {
        format!("点击执行 /{command}")
    }

    fn status_title(&self) -> &'static str {
        "SweeperBoom"
    }
    fn status_size(&self, difficulty: &str, width: usize, height: usize, mines: usize) -> String {
        format!(" {difficulty} {width}×{height} 雷{mines}")
    }
    fn status_remaining(&self, mines_left: i32) -> String {
        format!("  剩余 {mines_left}")
    }
    fn status_progress(&self, opened: usize, total: usize) -> String {
        format!("  已开 {opened}/{total}")
    }
    fn status_moves(&self, moves: u32) -> String {
        format!("  操作 {moves}")
    }
    fn status_rank(&self, rank: usize) -> String {
        format!("  排名 #{rank}")
    }
    fn status_mode(&self, mode: &str) -> String {
        format!("  模式:{mode}")
    }
    fn info_time(&self, seconds: u32) -> String {
        format!("用时 {seconds}s")
    }
    fn info_won(&self, seconds: &str) -> String {
        format!("通关！用时 {seconds}")
    }
    fn info_lost(&self, seconds: &str) -> String {
        format!("踩雷结束 · 用时 {seconds}")
    }
    fn info_hint(&self) -> &'static str {
        "   格子可点击；/sweeperboom help 看玩法"
    }

    fn cell_hover_open(&self, coord: &str) -> String {
        format!("打开 {coord}")
    }
    fn cell_hover_flag(&self, coord: &str) -> String {
        format!("标记 {coord}")
    }
    fn cell_hover_unflag(&self, coord: &str) -> String {
        format!("取消标记 {coord}")
    }
    fn cell_hover_number(&self, coord: &str, adjacent: u8) -> String {
        format!("{coord} · 周围 {adjacent} 雷")
    }
    fn cell_hover_locked(&self, coord: &str) -> String {
        format!("{coord} · 已插旗，用 /sweeperboom mode 切到标记模式可取消")
    }

    fn ctl_mode(&self, mode: &str) -> String {
        format!("模式:{mode}")
    }
    fn ctl_undo(&self) -> &'static str {
        "撤销"
    }
    fn ctl_restart(&self) -> &'static str {
        "重开"
    }
    fn ctl_stop(&self) -> &'static str {
        "结束"
    }
    fn ctl_best(&self) -> &'static str {
        "排行"
    }
    fn ctl_me(&self) -> &'static str {
        "战绩"
    }
    fn ctl_help(&self) -> &'static str {
        "帮助"
    }

    fn diff_label(&self, difficulty: Difficulty) -> &'static str {
        match difficulty {
            Difficulty::Easy => "简单",
            Difficulty::Medium => "中等",
            Difficulty::Hard => "困难",
        }
    }
    fn mode_label(&self, mode: Mode) -> &'static str {
        match mode {
            Mode::Open => "打开",
            Mode::Flag => "标记",
        }
    }
    fn mode_action(&self, mode: Mode) -> &'static str {
        match mode {
            Mode::Open => "打开格子",
            Mode::Flag => "插旗 / 取消插旗",
        }
    }
    fn mode_changed(&self, action: &str) -> String {
        format!("点击棋盘现在会「{action}」。")
    }
    fn diff_button(&self, difficulty: &str, width: usize, height: usize, mines: usize) -> String {
        format!("{difficulty} {width}×{height}·{mines}雷")
    }

    fn no_game(&self) -> &'static str {
        "你当前没有进行中的 SweeperBoom 对局。"
    }
    fn no_game_pick(&self) -> &'static str {
        "选择难度开始："
    }

    fn help_title(&self) -> &'static str {
        "===== SweeperBoom 小游戏 ====="
    }
    fn help_descriptions(&self) -> [&'static str; HELP_COUNT] {
        [
            "开始 / 重开一局，可接 easy|medium|hard（别名 new、restart）",
            "打开 c5 这一格（别名 o、dig、reveal）",
            "给 c5 插旗 / 取消插旗（别名 f、mark）",
            "c5 周围旗数达标时，连带打开其余相邻格（别名 ch）",
            "切换「点击棋盘」是打开还是插旗",
            "撤销上一步（别名 back、z）",
            "重新显示雷区（别名 board、b）",
            "全服排行榜前 10 名（别名 top、rank、leaderboard）",
            "我的战绩与排名（别名 stats、profile）",
            "结束当前对局（别名 quit、end、exit）",
            "显示这份说明（别名 h）",
        ]
    }
    fn help_rules(&self) -> &'static str {
        "玩法：打开所有非雷格即通关；踩到雷就失败。"
    }
    fn help_coords(&self) -> &'static str {
        "坐标：列用字母 a 起，行用数字 1 起（如 c5）；也接受 3-5、3,5、3:5。"
    }
    fn help_click(&self) -> &'static str {
        "棋盘上每个格子都可以直接点击：默认是「打开」，点【模式:打开】可切到「标记」再点格子插旗。"
    }
    fn help_safe(&self) -> &'static str {
        "第一次点击永远安全：地雷在你落子之后才布置。"
    }
    fn help_leaderboard(&self) -> &'static str {
        "通关用时会计入排行榜，重开服务器也不会丢。"
    }

    fn rank_title(&self) -> &'static str {
        "===== SweeperBoom 全服排行榜 ====="
    }
    fn rank_empty(&self) -> &'static str {
        "还没有任何记录，完成一局就能登上榜首！"
    }
    fn rank_start_button(&self) -> &'static str {
        "开始新的一局"
    }
    fn rank_count(&self, players: usize) -> String {
        format!("共 {players} 位玩家上榜 · 按胜场排序")
    }
    fn rank_wins(&self, wins: u32) -> String {
        format!("  胜 {wins}")
    }
    fn rank_losses(&self, losses: u32) -> String {
        format!("  负 {losses}")
    }
    fn rank_hard(&self, seconds: &str) -> String {
        format!("  困难 {seconds}")
    }
    fn rank_ellipsis(&self) -> &'static str {
        "  ……"
    }
    fn rank_not_listed(&self) -> &'static str {
        "你还没有上榜，完成一局即可参与排名。"
    }
    fn rank_more(&self) -> &'static str {
        "查看个人详情："
    }
    fn rank_me_button(&self) -> &'static str {
        "我的战绩"
    }

    fn me_title(&self) -> &'static str {
        "===== 我的 SweeperBoom 战绩 ====="
    }
    fn me_empty(&self) -> &'static str {
        "你还没有完成过任何一局 SweeperBoom。"
    }
    fn me_rank_label(&self) -> &'static str {
        "全服排名"
    }
    fn me_rank_value(&self, rank: usize, total: usize) -> String {
        format!("#{rank} / {total} 人")
    }
    fn me_wins_label(&self) -> &'static str {
        "胜利局数"
    }
    fn me_losses_label(&self) -> &'static str {
        "失败局数"
    }
    fn me_games_label(&self) -> &'static str {
        "总对局数"
    }
    fn me_rate_label(&self) -> &'static str {
        "胜率"
    }
    fn me_best_label(&self, difficulty: &str) -> String {
        format!("{difficulty}最快")
    }
    fn me_more(&self) -> &'static str {
        "查看全服排名："
    }
    fn me_best_button(&self) -> &'static str {
        "排行榜"
    }

    fn bad_cell(&self, raw: &str) -> String {
        format!("看不懂的坐标「{raw}」。")
    }
    fn bad_cell_hint(&self, max_column: char, max_row: usize) -> String {
        format!("用法：列字母 a-{max_column} + 行数字 1-{max_row}，例如 a1、c5；也可以写 3-5。")
    }
    fn step_over(&self) -> &'static str {
        "本局已经结束了，点【重开】或输入 /sweeperboom start 再来一局。"
    }
    fn step_already_open(&self) -> &'static str {
        "这一格已经打开过了。"
    }
    fn step_flagged(&self) -> &'static str {
        "这一格插着旗子，先取消标记再打开。"
    }
    fn step_not_open(&self) -> &'static str {
        "chord 只能用在已经打开的数字格上。"
    }
    fn step_nothing_to_chord(&self) -> &'static str {
        "这一格周围没有可以连带打开的格子。"
    }
    fn step_flags_mismatch(&self, need: u8, have: u8) -> String {
        format!("周围要插满 {need} 面旗子才能 chord，现在只有 {have} 面。")
    }
    fn step_first_open(&self, gained: u32) -> String {
        format!("第一击安全，打开了 {gained} 格。")
    }
    fn step_opened(&self, gained: u32) -> String {
        format!("打开了 {gained} 格。")
    }
    fn step_flagged_on(&self) -> &'static str {
        "已插旗。"
    }
    fn step_flagged_off(&self) -> &'static str {
        "已取消插旗。"
    }
    fn step_chorded(&self, gained: u32) -> String {
        format!("chord 连带打开了 {gained} 格。")
    }
    fn step_hit_mine(&self, seconds: &str) -> String {
        format!("踩到雷了，本局失败。用时 {seconds}。")
    }
    fn step_won(&self, seconds: &str) -> String {
        format!("全部排清，通关！用时 {seconds}。")
    }
    fn step_rank(&self, rank: usize, total: usize) -> String {
        format!("成绩已计入排行榜：当前排名 #{rank} / {total} 人。")
    }
    fn step_rank_short(&self, rank: usize, total: usize) -> String {
        format!("当前排名 #{rank} / {total} 人。")
    }
    fn step_new_best(&self, difficulty: &str) -> String {
        format!("{difficulty}新纪录！")
    }
    fn step_tnt(&self) -> &'static str {
        "轰！你站立处出现了一个引燃的 TNT。"
    }
    fn step_again(&self) -> &'static str {
        "再来一局："
    }
    fn undo_done(&self) -> &'static str {
        "已撤销一步。"
    }
    fn undo_empty(&self) -> &'static str {
        "没有可以撤销的步骤了。"
    }
    fn undo_over(&self) -> &'static str {
        "本局已经结束，撤销会破坏战绩，因此不再可用。"
    }
    fn start_abandoned(
        &self,
        difficulty: &str,
        width: usize,
        height: usize,
        mines: usize,
    ) -> String {
        format!(
            "新的一局开始：{difficulty} {width}×{height}·{mines}雷。上一局中途放弃，已计入排行榜。"
        )
    }
    fn start_fresh(&self) -> &'static str {
        "新的一局开始：点格子（或 /sweeperboom open c5）开出第一块安全区。"
    }
    fn stop_summary(&self, opened: usize, seconds: &str) -> String {
        format!("本局结束 · 打开 {opened} 格 · 用时 {seconds}。")
    }
    fn player_only(&self) -> &'static str {
        "SweeperBoom 是玩家专属的小游戏，请在游戏内由玩家执行该命令。"
    }

    fn format_seconds(&self, seconds: u32) -> String {
        match seconds {
            0 => "-".to_string(),
            s if s < 60 => format!("{s}s"),
            s => format!("{}分{}秒", s / 60, s % 60),
        }
    }
}

impl Strings for EnUs {
    fn button_hint(&self, command: &str) -> String {
        format!("Click to run /{command}")
    }

    fn status_title(&self) -> &'static str {
        "SweeperBoom"
    }
    fn status_size(&self, difficulty: &str, width: usize, height: usize, mines: usize) -> String {
        format!(" {difficulty} {width}×{height}, {mines} mines")
    }
    fn status_remaining(&self, mines_left: i32) -> String {
        format!("  Left {mines_left}")
    }
    fn status_progress(&self, opened: usize, total: usize) -> String {
        format!("  Opened {opened}/{total}")
    }
    fn status_moves(&self, moves: u32) -> String {
        format!("  Moves {moves}")
    }
    fn status_rank(&self, rank: usize) -> String {
        format!("  Rank #{rank}")
    }
    fn status_mode(&self, mode: &str) -> String {
        format!("  Mode:{mode}")
    }
    fn info_time(&self, seconds: u32) -> String {
        format!("Time {seconds}s")
    }
    fn info_won(&self, seconds: &str) -> String {
        format!("Cleared! Time {seconds}")
    }
    fn info_lost(&self, seconds: &str) -> String {
        format!("Hit a mine · Time {seconds}")
    }
    fn info_hint(&self) -> &'static str {
        "   Cells are clickable; see /sweeperboom help"
    }

    fn cell_hover_open(&self, coord: &str) -> String {
        format!("Open {coord}")
    }
    fn cell_hover_flag(&self, coord: &str) -> String {
        format!("Flag {coord}")
    }
    fn cell_hover_unflag(&self, coord: &str) -> String {
        format!("Unflag {coord}")
    }
    fn cell_hover_number(&self, coord: &str, adjacent: u8) -> String {
        format!("{coord} · {adjacent} adjacent mines")
    }
    fn cell_hover_locked(&self, coord: &str) -> String {
        format!("{coord} · flagged; use /sweeperboom mode to unflag")
    }

    fn ctl_mode(&self, mode: &str) -> String {
        format!("Mode:{mode}")
    }
    fn ctl_undo(&self) -> &'static str {
        "Undo"
    }
    fn ctl_restart(&self) -> &'static str {
        "Restart"
    }
    fn ctl_stop(&self) -> &'static str {
        "End"
    }
    fn ctl_best(&self) -> &'static str {
        "Ranking"
    }
    fn ctl_me(&self) -> &'static str {
        "Stats"
    }
    fn ctl_help(&self) -> &'static str {
        "Help"
    }

    fn diff_label(&self, difficulty: Difficulty) -> &'static str {
        match difficulty {
            Difficulty::Easy => "Easy",
            Difficulty::Medium => "Medium",
            Difficulty::Hard => "Hard",
        }
    }
    fn mode_label(&self, mode: Mode) -> &'static str {
        match mode {
            Mode::Open => "Open",
            Mode::Flag => "Flag",
        }
    }
    fn mode_action(&self, mode: Mode) -> &'static str {
        match mode {
            Mode::Open => "open cells",
            Mode::Flag => "flag / unflag cells",
        }
    }
    fn mode_changed(&self, action: &str) -> String {
        format!("Clicking the board now will {action}.")
    }
    fn diff_button(&self, difficulty: &str, width: usize, height: usize, mines: usize) -> String {
        format!("{difficulty} {width}×{height}·{mines} mines")
    }

    fn no_game(&self) -> &'static str {
        "You have no SweeperBoom game in progress."
    }
    fn no_game_pick(&self) -> &'static str {
        "Pick a difficulty to start:"
    }

    fn help_title(&self) -> &'static str {
        "===== SweeperBoom ====="
    }
    fn help_descriptions(&self) -> [&'static str; HELP_COUNT] {
        [
            "Start or restart a game; may be followed by easy|medium|hard (aliases new, restart)",
            "Reveal cell c5 (aliases o, dig, reveal)",
            "Flag or unflag c5 (aliases f, mark)",
            "Reveal c5's remaining neighbours once it has enough adjacent flags (alias ch)",
            "Toggle whether clicking the board opens or flags",
            "Undo the previous action (aliases back, z)",
            "Re-display the board (aliases board, b)",
            "Server-wide top 10 (aliases top, rank, leaderboard)",
            "Your own statistics and rank (aliases stats, profile)",
            "End the current game and record it (aliases quit, end, exit)",
            "Show this reference (alias h)",
        ]
    }
    fn help_rules(&self) -> &'static str {
        "Rules: open every safe cell to win; the first mine you open loses the game."
    }
    fn help_coords(&self) -> &'static str {
        "Coordinates: columns are letters from a, rows are numbers from 1 (e.g. c5); 3-5, 3,5 and 3:5 also work."
    }
    fn help_click(&self) -> &'static str {
        "Every cell is clickable: by default a click opens it. Use [Mode:Open] to switch to flagging, then click to place flags."
    }
    fn help_safe(&self) -> &'static str {
        "The first click is always safe: mines are placed only after you commit to a cell."
    }
    fn help_leaderboard(&self) -> &'static str {
        "Winning times go to the leaderboard and survive server restarts."
    }

    fn rank_title(&self) -> &'static str {
        "===== SweeperBoom leaderboard ====="
    }
    fn rank_empty(&self) -> &'static str {
        "No records yet - finish one game to top the board!"
    }
    fn rank_start_button(&self) -> &'static str {
        "Start a game"
    }
    fn rank_count(&self, players: usize) -> String {
        format!("{players} players on the board · sorted by wins")
    }
    fn rank_wins(&self, wins: u32) -> String {
        format!("  W {wins}")
    }
    fn rank_losses(&self, losses: u32) -> String {
        format!("  L {losses}")
    }
    fn rank_hard(&self, seconds: &str) -> String {
        format!("  Hard {seconds}")
    }
    fn rank_ellipsis(&self) -> &'static str {
        "  ..."
    }
    fn rank_not_listed(&self) -> &'static str {
        "You are not on the board yet - finish a game to join."
    }
    fn rank_more(&self) -> &'static str {
        "Your own details:"
    }
    fn rank_me_button(&self) -> &'static str {
        "My stats"
    }

    fn me_title(&self) -> &'static str {
        "===== My SweeperBoom stats ====="
    }
    fn me_empty(&self) -> &'static str {
        "You have not finished a SweeperBoom game yet."
    }
    fn me_rank_label(&self) -> &'static str {
        "Server rank"
    }
    fn me_rank_value(&self, rank: usize, total: usize) -> String {
        format!("#{rank} / {total} players")
    }
    fn me_wins_label(&self) -> &'static str {
        "Wins"
    }
    fn me_losses_label(&self) -> &'static str {
        "Losses"
    }
    fn me_games_label(&self) -> &'static str {
        "Games"
    }
    fn me_rate_label(&self) -> &'static str {
        "Win rate"
    }
    fn me_best_label(&self, difficulty: &str) -> String {
        format!("Best {difficulty}")
    }
    fn me_more(&self) -> &'static str {
        "Server ranking:"
    }
    fn me_best_button(&self) -> &'static str {
        "Leaderboard"
    }

    fn bad_cell(&self, raw: &str) -> String {
        format!("Could not read the coordinate \"{raw}\".")
    }
    fn bad_cell_hint(&self, max_column: char, max_row: usize) -> String {
        format!(
            "Use a column letter a-{max_column} with a row number 1-{max_row}, e.g. a1 or c5; 3-5 also works."
        )
    }
    fn step_over(&self) -> &'static str {
        "This game is over - click [Restart] or run /sweeperboom start."
    }
    fn step_already_open(&self) -> &'static str {
        "That cell is already open."
    }
    fn step_flagged(&self) -> &'static str {
        "That cell is flagged; unflag it before opening."
    }
    fn step_not_open(&self) -> &'static str {
        "chord only works on an opened numbered cell."
    }
    fn step_nothing_to_chord(&self) -> &'static str {
        "No neighbours can be opened from this cell."
    }
    fn step_flags_mismatch(&self, need: u8, have: u8) -> String {
        format!("chord needs {need} adjacent flags, but there are {have}.")
    }
    fn step_first_open(&self, gained: u32) -> String {
        format!("Safe first click - opened {gained} cells.")
    }
    fn step_opened(&self, gained: u32) -> String {
        format!("Opened {gained} cells.")
    }
    fn step_flagged_on(&self) -> &'static str {
        "Flagged."
    }
    fn step_flagged_off(&self) -> &'static str {
        "Flag removed."
    }
    fn step_chorded(&self, gained: u32) -> String {
        format!("chord opened {gained} more cells.")
    }
    fn step_hit_mine(&self, seconds: &str) -> String {
        format!("You hit a mine - game over. Time {seconds}.")
    }
    fn step_won(&self, seconds: &str) -> String {
        format!("Board cleared - you win! Time {seconds}.")
    }
    fn step_rank(&self, rank: usize, total: usize) -> String {
        format!("Recorded on the leaderboard: rank #{rank} / {total} players.")
    }
    fn step_rank_short(&self, rank: usize, total: usize) -> String {
        format!("Rank #{rank} / {total} players.")
    }
    fn step_new_best(&self, difficulty: &str) -> String {
        format!("New {difficulty} record!")
    }
    fn step_tnt(&self) -> &'static str {
        "Boom! A primed TNT appeared where you stood."
    }
    fn step_again(&self) -> &'static str {
        "Play again:"
    }
    fn undo_done(&self) -> &'static str {
        "Undid one step."
    }
    fn undo_empty(&self) -> &'static str {
        "Nothing left to undo."
    }
    fn undo_over(&self) -> &'static str {
        "The game is over; undo is disabled so recorded results stay accurate."
    }
    fn start_abandoned(
        &self,
        difficulty: &str,
        width: usize,
        height: usize,
        mines: usize,
    ) -> String {
        format!(
            "New game: {difficulty} {width}×{height}·{mines} mines. The previous game was abandoned and recorded."
        )
    }
    fn start_fresh(&self) -> &'static str {
        "New game - click a cell (or /sweeperboom open c5) to open the first safe area."
    }
    fn stop_summary(&self, opened: usize, seconds: &str) -> String {
        format!("Game ended · {opened} cells opened · time {seconds}.")
    }
    fn player_only(&self) -> &'static str {
        "SweeperBoom is a player-only mini-game; run this command in game as a player."
    }

    fn format_seconds(&self, seconds: u32) -> String {
        match seconds {
            0 => "-".to_string(),
            s if s < 60 => format!("{s}s"),
            s => format!("{}m{}s", s / 60, s % 60),
        }
    }
}

// ---------------------------------------------------------------------------
// <<< PURE-LOGIC-END
// ---------------------------------------------------------------------------

// ===========================================================================
// 全局状态
// ===========================================================================

/// 所有在线玩家的对局，键为玩家名。
static GAMES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
/// 全服排行榜。
static PROFILES: Mutex<Vec<Profile>> = Mutex::new(Vec::new());
/// 排行榜文件路径，由 `on_load` 写入。
static DATA_FILE: Mutex<Option<String>> = Mutex::new(None);

/// 一个玩家名对应的对局。
struct Entry {
    owner: String,
    game: Game,
}

fn games() -> MutexGuard<'static, Vec<Entry>> {
    GAMES.lock().unwrap_or_else(|e| e.into_inner())
}

fn profiles() -> MutexGuard<'static, Vec<Profile>> {
    PROFILES.lock().unwrap_or_else(|e| e.into_inner())
}

fn data_file() -> Option<String> {
    DATA_FILE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn index_of(list: &[Entry], owner: &str) -> Option<usize> {
    list.iter().position(|e| e.owner == owner)
}

/// 读取某位玩家当前对局的快照。
fn snapshot_of(owner: &str) -> Option<Game> {
    let list = games();
    index_of(&list, owner).map(|i| list[i].game.clone())
}

/// 按榜单规则排好序的副本。
fn sorted_profiles() -> Vec<Profile> {
    let mut list = profiles().clone();
    sort_profiles(&mut list);
    list
}

fn rank_of(owner: &str) -> Option<usize> {
    rank_in(&sorted_profiles(), owner)
}

/// 一局结束后交给玩家看的排名结果。
struct GameResult {
    rank: Option<usize>,
    total: usize,
    new_best: bool,
}

// ===========================================================================
// 排行榜读写
// ===========================================================================

/// 从磁盘载入排行榜。
fn load_profiles(path: &str) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    *profiles() = decode_profiles(&text);
}

/// 把排行榜写回磁盘。失败时只记日志，不影响游戏。
fn save_profiles() {
    let Some(path) = data_file() else {
        return;
    };

    // 先在锁内序列化，出锁之后再落盘。
    let out = {
        let list = profiles();
        encode_profiles(&list)
    };

    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(err) = std::fs::write(&path, out) {
        tracing::warn!("sweeperboom: 排行榜写入 {path} 失败：{err}");
    }
}

/// 把一局的结果累加进排行榜并落盘。
///
/// 必须在**不持有 `GAMES` 锁**时调用：内部会拿 `PROFILES` 锁并做文件 IO。
fn finish_game(owner: &str, game: &Game) -> GameResult {
    let won = game.status == Status::Won;

    let new_best = {
        let mut list = profiles();
        let index = match list.iter().position(|p| p.name == owner) {
            Some(i) => i,
            None => {
                list.push(Profile::new(owner));
                list.len() - 1
            }
        };
        accumulate(&mut list[index], game, won)
    };

    save_profiles();

    let sorted = sorted_profiles();
    GameResult {
        rank: rank_in(&sorted, owner),
        total: sorted.len(),
        new_best,
    }
}

// ===========================================================================
// 宿主交互
// ===========================================================================

/// 从宿主取随机种子：random UUID 混合墙钟时间。
///
/// 这是宿主调用，因此在**不持有任何锁**时执行。
fn host_seed() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let uid = pumpkin_plugin_api::uuid::generate();
    now ^ uid.high ^ uid.low.rotate_left(32)
}

/// 新建一个随机数发生器。
fn new_rng() -> Rng {
    Rng::seeded(host_seed())
}

/// 当前墙钟时间（秒）。
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// 发送者使用的语言。
///
/// 从玩家客户端上报的 locale 取；控制台、命令方块等没有语言的来源一律回落英文
/// （服务端日志与控制台的惯用语言）。
fn sender_lang(sender: &CommandSender) -> Lang {
    sender
        .as_player()
        .map_or(Lang::EnUs, |player| Lang::from_locale(&player.get_locale()))
}

/// 踩雷后在玩家站立处放下一个引燃的 TNT。
///
/// 返回是否真的放下了。除了「踩雷」之外没有任何条件限制——这就是一记惩罚，
/// 玩家最好赶快跑开。
///
/// 这是宿主调用（读发送者位置、生成实体），因此只在 `GAMES` 锁释放之后、
/// 且只在真的踩雷时执行。
fn ignite_tnt(sender: &CommandSender) -> bool {
    let (Some(position), Some(world)) = (sender.position(), sender.world()) else {
        // 玩家发送者一定同时有位置与世界；拿不到就安静跳过，不去猜一个位置。
        return false;
    };
    // `minecraft:tnt` 这个实体就是引燃状态的 TNT，自带默认引信。
    // 生成后不再需要句柄，直接让它被丢弃。
    drop(world.spawn_entity(EntityType::Tnt, position));
    true
}

// ===========================================================================
// 文本渲染
// ===========================================================================

fn colored(text: &str, color: NamedColor) -> TextComponent {
    TextComponent::text(text).color_named(color)
}

fn good(text: &str) -> TextComponent {
    colored(text, NamedColor::Green)
}

fn bad(text: &str) -> TextComponent {
    colored(text, NamedColor::Red)
}

fn tip(text: &str) -> TextComponent {
    colored(text, NamedColor::Yellow)
}

fn dim(text: &str) -> TextComponent {
    colored(text, NamedColor::Gray)
}

/// 一个可点击的按钮：点击后自动执行对应命令。
fn button(lang: Lang, command: &str, label: &str, color: NamedColor) -> TextComponent {
    let hint = colored(&lang.strings().button_hint(command), NamedColor::Gray);
    TextComponent::text(&format!("[{label}] "))
        .color_named(color)
        .bold(true)
        .click_run_command(command)
        .hover_show_text(hint)
}

/// 数字对应的颜色，沿用经典扫雷的配色习惯（为了在聊天栏暗底上可读，做了替换）。
fn number_color(value: u8) -> NamedColor {
    match value {
        1 => NamedColor::Blue,
        2 => NamedColor::Green,
        3 => NamedColor::Red,
        4 => NamedColor::LightPurple,
        5 => NamedColor::Gold,
        6 => NamedColor::Aqua,
        7 => NamedColor::White,
        _ => NamedColor::Gray,
    }
}

/// 一格要显示的字符、颜色与是否加粗。
///
/// 全部使用在 Minecraft 默认字体中同为 5px 墨迹宽度的字形，
/// 配合「字形 + 一个空格」的固定排布，整块棋盘严格对齐。
fn cell_glyph(game: &Game, index: usize) -> (char, NamedColor, bool) {
    let board = &game.board;

    if board.revealed[index] {
        let value = board.adjacent[index];
        if value == 0 {
            return ('-', NamedColor::DarkGray, false);
        }
        let digit = char::from(b'0' + value.min(9));
        return (digit, number_color(value), false);
    }

    match game.status {
        Status::Lost if board.mine[index] => {
            if game.exploded == Some(index) {
                ('X', NamedColor::Red, true)
            } else {
                ('*', NamedColor::DarkRed, false)
            }
        }
        // 通关时剩下的格子必然是雷，直接以插旗形态收尾。
        Status::Won if board.mine[index] => ('F', NamedColor::Red, false),
        _ if board.flagged[index] => ('F', NamedColor::Red, false),
        _ => ('#', NamedColor::DarkGray, false),
    }
}

/// 一格（含点击与悬停提示）。
fn cell_component(lang: Lang, game: &Game, index: usize) -> TextComponent {
    let s = lang.strings();
    let (glyph, color, bold) = cell_glyph(game, index);
    let coord = cell_label(index, game.board.width);
    let mut cell = TextComponent::text(&format!("{glyph} ")).color_named(color);
    if bold {
        cell = cell.bold(true);
    }

    if game.status != Status::Playing {
        return cell;
    }

    let revealed = game.board.revealed[index];
    match game.mode {
        Mode::Open if !revealed && !game.board.flagged[index] => cell
            .click_run_command(&format!("sweeperboom open {coord}"))
            .hover_show_text(colored(&s.cell_hover_open(&coord), NamedColor::Yellow)),
        Mode::Flag if !revealed => {
            let hint = if game.board.flagged[index] {
                s.cell_hover_unflag(&coord)
            } else {
                s.cell_hover_flag(&coord)
            };
            cell.click_run_command(&format!("sweeperboom flag {coord}"))
                .hover_show_text(colored(&hint, NamedColor::Yellow))
        }
        _ if revealed => cell.hover_show_text(colored(
            &s.cell_hover_number(&coord, game.board.adjacent[index]),
            NamedColor::Gray,
        )),
        _ => cell.hover_show_text(colored(&s.cell_hover_locked(&coord), NamedColor::Gray)),
    }
}

/// 列标头：`a b c ...`。
fn header_line(game: &Game) -> TextComponent {
    let mut row = TextComponent::text("   ").color_named(NamedColor::DarkGray);
    for x in 0..game.board.width {
        let letter = char::from(b'a' + (x as u8).min(25));
        row = row.add_child(colored(&format!("{letter} "), NamedColor::DarkAqua));
    }
    row
}

/// 顶部状态行。
fn status_line(lang: Lang, game: &Game, rank: Option<usize>) -> TextComponent {
    let s = lang.strings();
    let (width, height, mines) = game.difficulty.size();
    let mut line = TextComponent::text("");
    line = line.add_child(
        TextComponent::text(s.status_title())
            .color_named(NamedColor::Gold)
            .bold(true),
    );
    line = line.add_child(colored(
        &s.status_size(s.diff_label(game.difficulty), width, height, mines),
        NamedColor::Aqua,
    ));
    line = line.add_child(colored(
        &s.status_remaining(game.mines_left()),
        if game.mines_left() < 0 {
            NamedColor::Red
        } else {
            NamedColor::Yellow
        },
    ));
    line = line.add_child(colored(
        &s.status_progress(game.revealed_total, game.board.safe_cells()),
        NamedColor::Green,
    ));
    line = line.add_child(colored(&s.status_moves(game.moves), NamedColor::Gray));
    if let Some(rank) = rank {
        line = line.add_child(colored(&s.status_rank(rank), NamedColor::Blue));
    }
    if game.status == Status::Playing {
        line = line.add_child(colored(
            &s.status_mode(s.mode_label(game.mode)),
            NamedColor::LightPurple,
        ));
    }
    line
}

/// 第二行：计时与结果。
fn info_line(lang: Lang, game: &Game) -> TextComponent {
    let s = lang.strings();
    let mut line = TextComponent::text("");
    match game.status {
        Status::Playing => {
            line = line.add_child(colored(
                &s.info_time(game.elapsed_at(now_secs())),
                NamedColor::Aqua,
            ));
        }
        Status::Won => {
            line = line.add_child(
                TextComponent::text(&s.info_won(&s.format_seconds(game.elapsed)))
                    .color_named(NamedColor::Green)
                    .bold(true),
            );
        }
        Status::Lost => {
            line = line.add_child(
                TextComponent::text(&s.info_lost(&s.format_seconds(game.elapsed)))
                    .color_named(NamedColor::Red)
                    .bold(true),
            );
        }
    }
    line = line.add_child(dim(s.info_hint()));
    line
}

/// 底部操作栏。
fn control_row(lang: Lang, game: &Game) -> TextComponent {
    let s = lang.strings();
    let mut row = TextComponent::text("");
    row = row.add_child(button(
        lang,
        "sweeperboom mode",
        &s.ctl_mode(s.mode_label(game.mode)),
        NamedColor::LightPurple,
    ));
    row = row.add_child(button(
        lang,
        "sweeperboom undo",
        s.ctl_undo(),
        NamedColor::Yellow,
    ));
    row = row.add_child(button(
        lang,
        "sweeperboom start",
        s.ctl_restart(),
        NamedColor::Gold,
    ));
    row = row.add_child(button(
        lang,
        "sweeperboom stop",
        s.ctl_stop(),
        NamedColor::Red,
    ));
    row = row.add_child(TextComponent::text("  "));
    row = row.add_child(button(
        lang,
        "sweeperboom best",
        s.ctl_best(),
        NamedColor::Aqua,
    ));
    row = row.add_child(button(
        lang,
        "sweeperboom me",
        s.ctl_me(),
        NamedColor::Green,
    ));
    row = row.add_child(button(
        lang,
        "sweeperboom help",
        s.ctl_help(),
        NamedColor::Gray,
    ));
    row
}

/// 完整的雷区消息：状态行 + 列标头 + 棋盘 + 操作栏 + 可选提示。
fn board_message(
    lang: Lang,
    game: &Game,
    rank: Option<usize>,
    note: Option<TextComponent>,
) -> TextComponent {
    let mut root = TextComponent::text("");
    root = root.add_child(status_line(lang, game, rank));
    root = root.add_child(TextComponent::text("\n"));
    root = root.add_child(info_line(lang, game));
    root = root.add_child(TextComponent::text("\n"));
    root = root.add_child(header_line(game));

    for y in 0..game.board.height {
        root = root.add_child(TextComponent::text("\n"));
        let mut row = TextComponent::text(&format!("{:02} ", y + 1)).color_named(NamedColor::DarkGray);
        for x in 0..game.board.width {
            row = row.add_child(cell_component(lang, game, y * game.board.width + x));
        }
        root = root.add_child(row);
    }

    root = root.add_child(TextComponent::text("\n"));
    root = root.add_child(control_row(lang, game));

    if let Some(note) = note {
        root = root.add_child(TextComponent::text("\n"));
        root = root.add_child(note);
    }
    root
}

/// 带名次信息的雷区（渲染时实时查询榜单）。
fn board_with(
    lang: Lang,
    game: &Game,
    owner: &str,
    note: Option<TextComponent>,
) -> TextComponent {
    board_message(lang, game, rank_of(owner), note)
}

/// 没有对局时的引导。
fn no_game_message(lang: Lang) -> TextComponent {
    let s = lang.strings();
    let mut root = tip(s.no_game());
    root = root.add_child(TextComponent::text("\n"));
    root = root.add_child(dim(s.no_game_pick()));
    for difficulty in Difficulty::all() {
        let (width, height, mines) = difficulty.size();
        root = root.add_child(button(
            lang,
            &format!("sweeperboom start {}", difficulty.key()),
            &s.diff_button(s.diff_label(difficulty), width, height, mines),
            difficulty_color(difficulty),
        ));
    }
    root
}

fn difficulty_color(difficulty: Difficulty) -> NamedColor {
    match difficulty {
        Difficulty::Easy => NamedColor::Green,
        Difficulty::Medium => NamedColor::Yellow,
        Difficulty::Hard => NamedColor::Red,
    }
}

/// 玩法说明。
fn help_message(lang: Lang) -> TextComponent {
    let s = lang.strings();
    let mut root = TextComponent::text("");
    root = root.add_child(
        TextComponent::text(s.help_title())
            .color_named(NamedColor::Gold)
            .bold(true),
    );

    let descriptions = s.help_descriptions();
    for (command, description) in HELP_COMMANDS.iter().zip(descriptions.iter()) {
        let mut line = TextComponent::text("\n");
        line = line.add_child(dim("  /"));
        line = line.add_child(button(lang, command, command, NamedColor::Aqua));
        line = line.add_child(dim(&format!("- {description}")));
        root = root.add_child(line);
    }

    let notes: [&str; 5] = [
        s.help_rules(),
        s.help_coords(),
        s.help_click(),
        s.help_safe(),
        s.help_leaderboard(),
    ];
    root = root.add_child(TextComponent::text("\n"));
    root = root.add_child(tip(notes[0]));
    for note in &notes[1..] {
        root = root.add_child(TextComponent::text("\n"));
        root = root.add_child(dim(note));
    }
    root
}

/// 排行榜中的一行。
fn rank_line(lang: Lang, rank: usize, profile: &Profile, highlight: bool) -> TextComponent {
    let s = lang.strings();
    let mut line = TextComponent::text("\n");
    line = line.add_child(colored(&format!("{rank:>2}. "), rank_color(rank)));

    let mut name = colored(
        &profile.name,
        if highlight {
            NamedColor::Green
        } else {
            NamedColor::White
        },
    );
    if highlight {
        name = name.bold(true);
    }
    line = line.add_child(name);

    line = line.add_child(colored(&s.rank_wins(profile.wins), NamedColor::Yellow));
    line = line.add_child(colored(&s.rank_losses(profile.losses), NamedColor::Gray));
    line = line.add_child(colored(
        &s.rank_hard(&s.format_seconds(profile.best_of(Difficulty::Hard))),
        NamedColor::Red,
    ));
    line
}

fn rank_color(rank: usize) -> NamedColor {
    match rank {
        1 => NamedColor::Gold,
        2 => NamedColor::Gray,
        3 => NamedColor::DarkRed,
        _ => NamedColor::DarkGray,
    }
}

/// 「标签 值」形式的一行战绩。
fn stat_line(label: &str, value: &str, color: NamedColor) -> TextComponent {
    let mut line = TextComponent::text("\n");
    line = line.add_child(colored(&format!("  {label} "), NamedColor::Gray));
    line = line.add_child(colored(value, color));
    line
}

/// 全服排行榜。
fn ranking_message(lang: Lang, owner: &str) -> TextComponent {
    let s = lang.strings();
    let sorted = sorted_profiles();

    let mut root = TextComponent::text("");
    root = root.add_child(
        TextComponent::text(s.rank_title())
            .color_named(NamedColor::Gold)
            .bold(true),
    );

    if sorted.is_empty() {
        root = root.add_child(TextComponent::text("\n"));
        root = root.add_child(dim(s.rank_empty()));
        root = root.add_child(TextComponent::text("\n"));
        root = root.add_child(button(
            lang,
            "sweeperboom start",
            s.rank_start_button(),
            NamedColor::Green,
        ));
        return root;
    }

    root = root.add_child(TextComponent::text("\n"));
    root = root.add_child(dim(&s.rank_count(sorted.len())));

    for (index, profile) in sorted.iter().take(TOP_N).enumerate() {
        root = root.add_child(rank_line(lang, index + 1, profile, profile.name == owner));
    }

    match rank_in(&sorted, owner) {
        Some(rank) if rank > TOP_N => {
            root = root.add_child(TextComponent::text("\n"));
            root = root.add_child(dim(s.rank_ellipsis()));
            root = root.add_child(rank_line(lang, rank, &sorted[rank - 1], true));
        }
        Some(_) => {}
        None => {
            root = root.add_child(TextComponent::text("\n"));
            root = root.add_child(dim(s.rank_not_listed()));
        }
    }

    root = root.add_child(TextComponent::text("\n"));
    root = root.add_child(dim(s.rank_more()));
    root = root.add_child(button(
        lang,
        "sweeperboom me",
        s.rank_me_button(),
        NamedColor::Green,
    ));
    root
}

/// 个人战绩详情。
fn me_message(lang: Lang, owner: &str) -> TextComponent {
    let s = lang.strings();
    let sorted = sorted_profiles();

    let mut root = TextComponent::text("");
    root = root.add_child(
        TextComponent::text(s.me_title())
            .color_named(NamedColor::Gold)
            .bold(true),
    );

    let Some(profile) = sorted.iter().find(|p| p.name == owner) else {
        root = root.add_child(TextComponent::text("\n"));
        root = root.add_child(dim(s.me_empty()));
        root = root.add_child(TextComponent::text("\n"));
        root = root.add_child(button(
            lang,
            "sweeperboom start",
            s.rank_start_button(),
            NamedColor::Green,
        ));
        return root;
    };

    let rank = rank_in(&sorted, owner).unwrap_or(1);

    root = root.add_child(stat_line(
        s.me_rank_label(),
        &s.me_rank_value(rank, sorted.len()),
        NamedColor::Aqua,
    ));
    root = root.add_child(stat_line(
        s.me_wins_label(),
        &profile.wins.to_string(),
        NamedColor::Green,
    ));
    root = root.add_child(stat_line(
        s.me_losses_label(),
        &profile.losses.to_string(),
        NamedColor::Red,
    ));
    root = root.add_child(stat_line(
        s.me_games_label(),
        &profile.games().to_string(),
        NamedColor::Gray,
    ));
    root = root.add_child(stat_line(
        s.me_rate_label(),
        &format!("{}%", profile.win_rate()),
        NamedColor::Yellow,
    ));
    for difficulty in Difficulty::all() {
        root = root.add_child(stat_line(
            &s.me_best_label(s.diff_label(difficulty)),
            &s.format_seconds(profile.best_of(difficulty)),
            difficulty_color(difficulty),
        ));
    }

    root = root.add_child(TextComponent::text("\n"));
    root = root.add_child(dim(s.me_more()));
    root = root.add_child(button(
        lang,
        "sweeperboom best",
        s.me_best_button(),
        NamedColor::Aqua,
    ));
    root
}

// ===========================================================================
// 命令分发
// ===========================================================================

/// 玩家可以触发的动作。
#[derive(Clone)]
enum Action {
    Start(Difficulty),
    /// 在某一格上执行动作，参数是玩家输入的原始坐标文本。
    CellAt(Slot, String),
    /// 需要在 `handle` 里读取坐标参数。
    Cell(Slot),
    SetMode(Mode),
    ToggleMode,
    Undo,
    Show,
    Stop,
    Best,
    Me,
    Help,
}

/// 所有子命令共用的执行器。
#[derive(Clone)]
struct Executor(Action);

impl CommandHandler for Executor {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let action = match &self.0 {
            Action::Cell(slot) => Action::CellAt(*slot, read_cell(&args)),
            other => other.clone(),
        };
        play(&sender, action);
        Ok(1)
    }
}

/// 从命令参数里取出坐标文本。
fn read_cell(args: &ConsumedArgs) -> String {
    match args.get_value(CELL_ARG) {
        Arg::Simple(text) | Arg::Msg(text) => text,
        _ => String::new(),
    }
}

/// 无法解析坐标时的提示。
fn bad_cell_note(lang: Lang, raw: &str, game: &Game) -> TextComponent {
    let s = lang.strings();
    let mut note = bad(&s.bad_cell(raw));
    note = note.add_child(TextComponent::text("\n"));
    note = note.add_child(dim(&s.bad_cell_hint(
        char::from(b'a' + (game.board.width as u8 - 1).min(25)),
        game.board.height,
    )));
    note
}

/// 把一次动作的结果翻译成给玩家看的提示。
///
/// `tnt` 表示踩雷后 TNT 是否真的已经放下（由调用方在释放锁之后完成）。
fn step_note(
    lang: Lang,
    step: Step,
    game: &Game,
    result: Option<&GameResult>,
    tnt: bool,
) -> TextComponent {
    let s = lang.strings();
    let mut note = match step {
        Step::Over => bad(s.step_over()),
        Step::AlreadyOpen => tip(s.step_already_open()),
        Step::Flagged => tip(s.step_flagged()),
        Step::NotOpen => tip(s.step_not_open()),
        Step::NothingToChord => dim(s.step_nothing_to_chord()),
        Step::FlagsMismatch { need, have } => tip(&s.step_flags_mismatch(need, have)),
        Step::Opened { gained, first } => {
            if first {
                good(&s.step_first_open(gained))
            } else {
                dim(&s.step_opened(gained))
            }
        }
        Step::Toggled { flagged } => {
            if flagged {
                dim(s.step_flagged_on())
            } else {
                dim(s.step_flagged_off())
            }
        }
        Step::Chorded { gained } => dim(&s.step_chorded(gained)),
        Step::HitMine => {
            let mut line = bad(&s.step_hit_mine(&s.format_seconds(game.elapsed)));
            if let Some(result) = result
                && let Some(rank) = result.rank
            {
                line = line.add_child(TextComponent::text("\n"));
                line = line.add_child(dim(&s.step_rank(rank, result.total)));
            }
            if tnt {
                line = line.add_child(TextComponent::text("\n"));
                line = line.add_child(tip(s.step_tnt()));
            }
            line
        }
        Step::Won => {
            let mut line = good(&s.step_won(&s.format_seconds(game.elapsed)));
            if let Some(result) = result {
                if result.new_best {
                    line = line.add_child(TextComponent::text("\n"));
                    line = line.add_child(good(
                        &s.step_new_best(s.diff_label(game.difficulty)),
                    ));
                }
                if let Some(rank) = result.rank {
                    line = line.add_child(TextComponent::text("\n"));
                    line = line.add_child(dim(&s.step_rank_short(rank, result.total)));
                }
            }
            line
        }
    };

    if game.is_over() {
        note = note.add_child(TextComponent::text("\n"));
        note = note.add_child(dim(s.step_again()));
        for difficulty in Difficulty::all() {
            note = note.add_child(button(
                lang,
                &format!("sweeperboom start {}", difficulty.key()),
                s.diff_label(difficulty),
                difficulty_color(difficulty),
            ));
        }
    }
    note
}

/// 统一的入口：先取出需要的数据（避免持有锁时调用宿主 API），再渲染并回复玩家。
fn play(sender: &CommandSender, action: Action) {
    let lang = sender_lang(sender);

    if !sender.is_player() {
        sender.send_message(colored(lang.strings().player_only(), NamedColor::Red));
        return;
    }

    let owner = sender.get_name();
    let message = match action {
        Action::Help => help_message(lang),
        Action::Best => ranking_message(lang, &owner),
        Action::Me => me_message(lang, &owner),
        Action::Show => match snapshot_of(&owner) {
            Some(game) => board_with(lang, &game, &owner, None),
            None => no_game_message(lang),
        },
        Action::Start(difficulty) => {
            let (game, abandoned) = {
                let mut list = games();
                match index_of(&list, &owner) {
                    Some(i) => {
                        let old = std::mem::replace(&mut list[i].game, Game::new(difficulty));
                        let abandoned = if old.moves > 0 && !old.recorded {
                            Some(old)
                        } else {
                            None
                        };
                        (list[i].game.clone(), abandoned)
                    }
                    None => {
                        let fresh = Game::new(difficulty);
                        list.push(Entry {
                            owner: owner.clone(),
                            game: fresh.clone(),
                        });
                        (fresh, None)
                    }
                }
            };

            let s = lang.strings();
            let note = match abandoned {
                Some(old) => {
                    let result = finish_game(&owner, &old);
                    let (width, height, mines) = difficulty.size();
                    let mut line = good(&s.start_abandoned(
                        s.diff_label(difficulty),
                        width,
                        height,
                        mines,
                    ));
                    if let Some(rank) = result.rank {
                        line = line.add_child(TextComponent::text("\n"));
                        line = line.add_child(dim(&s.step_rank_short(rank, result.total)));
                    }
                    line
                }
                None => good(s.start_fresh()),
            };
            board_with(lang, &game, &owner, Some(note))
        }
        Action::SetMode(requested) => apply_mode(lang, &owner, Some(requested)),
        Action::ToggleMode => apply_mode(lang, &owner, None),
        Action::Undo => {
            let outcome = {
                let mut list = games();
                match index_of(&list, &owner) {
                    Some(i) => {
                        let game = &mut list[i].game;
                        Some((game.undo(), game.clone()))
                    }
                    None => None,
                }
            };
            let s = lang.strings();
            match outcome {
                None => no_game_message(lang),
                Some((Undo::Done, game)) => {
                    board_with(lang, &game, &owner, Some(dim(s.undo_done())))
                }
                Some((Undo::Empty, game)) => {
                    board_with(lang, &game, &owner, Some(tip(s.undo_empty())))
                }
                Some((Undo::Over, game)) => {
                    board_with(lang, &game, &owner, Some(bad(s.undo_over())))
                }
            }
        }
        Action::Stop => {
            let removed = {
                let mut list = games();
                index_of(&list, &owner).map(|i| list.remove(i).game)
            };
            let s = lang.strings();
            match removed {
                None => no_game_message(lang),
                Some(game) => {
                    let elapsed = if game.status == Status::Playing {
                        game.elapsed_at(now_secs())
                    } else {
                        game.elapsed
                    };
                    let mut line = dim(&s.stop_summary(
                        game.revealed_total,
                        &s.format_seconds(elapsed),
                    ));
                    if game.moves > 0 && !game.recorded {
                        let result = finish_game(&owner, &game);
                        if let Some(rank) = result.rank {
                            line = line.add_child(TextComponent::text("\n"));
                            line = line.add_child(dim(&s.step_rank(rank, result.total)));
                        }
                    }
                    board_with(lang, &game, &owner, Some(line))
                }
            }
        }
        Action::CellAt(slot, raw) => {
            // 随机种子与时间都来自宿主接口，必须在持锁之前取好。
            let mut rng = new_rng();
            let now = now_secs();

            let outcome = {
                let mut list = games();
                match index_of(&list, &owner) {
                    None => CellOutcome::NoGame,
                    Some(i) => {
                        let game = &mut list[i].game;
                        match parse_cell(&raw, game.board.width, game.board.height) {
                            None => {
                                let snapshot = game.clone();
                                let note = bad_cell_note(lang, &raw, &snapshot);
                                CellOutcome::BadCell(snapshot, note)
                            }
                            Some(index) => {
                                let step = match slot {
                                    Slot::Open => game.open(index, &mut rng, now),
                                    Slot::Flag => game.flag(index),
                                    Slot::Chord => game.chord(index, now),
                                };
                                let record = game.is_over() && !game.recorded && game.moves > 0;
                                if record {
                                    game.recorded = true;
                                }
                                CellOutcome::Stepped {
                                    game: game.clone(),
                                    step,
                                    record,
                                }
                            }
                        }
                    }
                }
            };

            match outcome {
                CellOutcome::NoGame => no_game_message(lang),
                CellOutcome::BadCell(game, note) => board_with(lang, &game, &owner, Some(note)),
                CellOutcome::Stepped {
                    game,
                    step,
                    record,
                } => {
                    let result = if record {
                        Some(finish_game(&owner, &game))
                    } else {
                        None
                    };
                    // 踩雷就把玩家脚下的 TNT 点着。这里是宿主调用，此时
                    // `GAMES` 锁已经释放，符合「持锁期间不碰宿主」的约定。
                    let tnt = matches!(step, Step::HitMine) && ignite_tnt(sender);
                    let note = step_note(lang, step, &game, result.as_ref(), tnt);
                    board_with(lang, &game, &owner, Some(note))
                }
            }
        }
        // `Cell` 只作为命令树上的占位，进入 `play` 之前已被 `handle` 解析成 `CellAt`。
        Action::Cell(slot) => {
            let _ = slot;
            no_game_message(lang)
        }
    };

    sender.send_message(message);
}

/// 格子动作的执行结果。
enum CellOutcome {
    NoGame,
    BadCell(Game, TextComponent),
    Stepped {
        game: Game,
        step: Step,
        record: bool,
    },
}

/// 切换或指定「点击棋盘」的行为。
///
/// `requested` 为 `None` 表示切换，为 `Some` 表示直接指定。
fn apply_mode(lang: Lang, owner: &str, requested: Option<Mode>) -> TextComponent {
    let outcome = {
        let mut list = games();
        match index_of(&list, owner) {
            Some(i) => {
                let game = &mut list[i].game;
                game.mode = match requested {
                    Some(mode) => mode,
                    None => game.mode.toggled(),
                };
                Some(game.clone())
            }
            None => None,
        }
    };

    match outcome {
        None => no_game_message(lang),
        Some(game) => {
            let s = lang.strings();
            let note = dim(&s.mode_changed(s.mode_action(game.mode)));
            board_with(lang, &game, owner, Some(note))
        }
    }
}

// ===========================================================================
// 插件入口
// ===========================================================================

struct Sweeperboom;

impl Plugin for Sweeperboom {
    fn new() -> Self {
        Sweeperboom
    }

    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "sweeperboom".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            authors: vec!["WorkBuddy".into()],
            description: PLUGIN_DESCRIPTION.into(),
            dependencies: Vec::new(),
            // 排行榜需要读写插件私有数据目录。
            permissions: vec![FS_READ_DATA.to_string(), FS_WRITE_DATA.to_string()],
        }
    }

    fn on_load(&self, context: Context) -> pumpkin_plugin_api::Result<()> {
        context.register_permission(&Permission {
            node: PERM_NODE.to_string(),
            description: "允许玩家游玩扫雷小游戏".to_string(),
            default: PermissionDefault::Allow,
            children: Vec::new(),
        })?;

        let path = format!("{}/{BOARD_FILE}", context.get_data_folder());
        {
            let mut slot = DATA_FILE.lock().unwrap_or_else(|e| e.into_inner());
            *slot = Some(path.clone());
        }
        load_profiles(&path);
        tracing::info!(
            "sweeperboom: 排行榜已从 {path} 载入 {} 条记录",
            profiles().len()
        );

        let names: Vec<String> = COMMAND_NAMES.iter().map(|n| n.to_string()).collect();
        let mut command = Command::new(&names, COMMAND_DESCRIPTION);

        // 直接执行 /sweeperboom 时展示雷区（或引导选择难度）。
        command = command.execute(Executor(Action::Show));

        // 开始 / 重开：裸命令默认中等，另接 easy|medium|hard。
        for alias in ["start", "new", "restart", "reset"] {
            let mut node = CommandNode::literal(alias)
                .execute(Executor(Action::Start(Difficulty::Medium)));
            for difficulty in Difficulty::all() {
                node = node.then(
                    CommandNode::literal(difficulty.key())
                        .execute(Executor(Action::Start(difficulty))),
                );
            }
            command = command.then(node);
        }

        // 打开 / 插旗 / chord：每个别名下挂一个 cell 参数节点。
        let cell_nodes: [(&str, Slot); 9] = [
            ("open", Slot::Open),
            ("o", Slot::Open),
            ("dig", Slot::Open),
            ("reveal", Slot::Open),
            ("flag", Slot::Flag),
            ("f", Slot::Flag),
            ("mark", Slot::Flag),
            ("chord", Slot::Chord),
            ("ch", Slot::Chord),
        ];
        for (alias, slot) in cell_nodes {
            command = command.then(
                CommandNode::literal(alias).then(
                    CommandNode::argument(
                        CELL_ARG,
                        &ArgumentType::String(StringType::SingleWord),
                    )
                    .execute(Executor(Action::Cell(slot))),
                ),
            );
        }

        // 点击行为切换。
        let mut mode_node = CommandNode::literal("mode").execute(Executor(Action::ToggleMode));
        for mode in [Mode::Open, Mode::Flag] {
            mode_node = mode_node
                .then(CommandNode::literal(mode.key()).execute(Executor(Action::SetMode(mode))));
        }
        command = command.then(mode_node);

        // 撤销 / 重新显示 / 结束 / 排行榜 / 战绩 / 帮助。
        for alias in ["undo", "back", "z"] {
            command = command.then(CommandNode::literal(alias).execute(Executor(Action::Undo)));
        }
        for alias in ["show", "board", "b"] {
            command = command.then(CommandNode::literal(alias).execute(Executor(Action::Show)));
        }
        for alias in ["stop", "quit", "end", "exit"] {
            command = command.then(CommandNode::literal(alias).execute(Executor(Action::Stop)));
        }
        for alias in ["best", "top", "rank", "leaderboard"] {
            command = command.then(CommandNode::literal(alias).execute(Executor(Action::Best)));
        }
        for alias in ["me", "stats", "profile"] {
            command = command.then(CommandNode::literal(alias).execute(Executor(Action::Me)));
        }
        for alias in ["help", "h"] {
            command = command.then(CommandNode::literal(alias).execute(Executor(Action::Help)));
        }

        context.register_command(command, PERM_NODE);

        tracing::info!("sweeperboom 已加载：使用 /sweeperboom start 开始一局扫雷");
        Ok(())
    }

    fn on_unload(&self, _context: Context) -> pumpkin_plugin_api::Result<()> {
        save_profiles();
        games().clear();
        tracing::info!("sweeperboom 已卸载，排行榜已保存");
        Ok(())
    }
}

pumpkin_plugin_api::register_plugin!(Sweeperboom);
