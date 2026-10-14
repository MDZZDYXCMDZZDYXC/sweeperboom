# sweeperboom

A sweeperboom mini-game plugin for the [Pumpkin](https://pumpkinmc.org) Minecraft server, played
entirely from the chat bar through the `/sweeperboom` command tree.

Every command re-sends the current minefield, the remaining-mine counter, the timer, and a row of
clickable controls. On top of that **every cell on the board is itself a clickable button**, so a
full game can be played without typing a single coordinate. No inventory, no GUI, no external
dependencies.

Structurally this is a sibling of the [`two048`](../two048) plugin next to it: same crate layout,
same rendering approach, same persistence scheme, same permission model.

Built against the official Pumpkin plugin development guide
(`https://docs.pumpkinmc.org/plugin-dev/rust`) and compiled to a `wasm32-wasip2` component.

- Package: `sweeperboom` v0.1.0 (Rust edition 2024)
- Artifact: `target/wasm32-wasip2/release/sweeperboom.wasm`
- Dependencies: `pumpkin-plugin-api`, `tracing`

## Features

- Playable with zero client-side setup: everything renders as chat text components.
- Fully clickable board — each cell carries a `click_run_command` event.
- "Mode" toggle so one click type can cover both actions: click to **open**, or switch to click to
  **flag**. The current mode is shown in the status line and on the toggle button.
- First click is always safe: mines are placed *after* the first reveal, and neither that cell nor
  its eight neighbours can become a mine.
- Classic flood fill, flag toggling, and **chord** (reveal the remaining neighbours of a number
  once enough flags are around it).
- Undo, up to 16 steps.
- Live timer, win/lose detection, and an exact "revealed / total" progress readout.
- Losing has a physical consequence: a primed TNT is spawned where you were standing.
- Persistent server-wide leaderboard that survives restarts, with per-difficulty best times.
- Per-player statistics: rank, wins, losses, win rate, and best time per difficulty.

Fully bilingual: the plugin ships Simplified Chinese and English text and picks per player from the
client's own language setting. See [Languages](#languages).

## Languages

The plugin is bilingual out of the box: **Simplified Chinese** (`zh_cn`) and **English** (`en_us`).
Text is translated **server-side**, which keeps the plugin's central promise that a player needs
zero client-side setup - no resource pack, no language files, nothing to install.

Selection is by prefix on the locale the client reports:

| Client locale | Language |
| --- | --- |
| `zh_cn`, `zh_tw`, `zh_hk`, anything starting with `zh` | Simplified Chinese |
| everything else, including unknown and empty values | English |

Traditional Chinese deliberately falls back to Simplified rather than English: for a `zh_tw` reader
Simplified is far more readable than English, and a third translation would not change the fact
that the plugin has to pick one.

There is no locale at all on the console, a command block, or RCON, so those fall back to English,
the conventional language for server-side output.

### How it is implemented

Text lives behind a `Strings` trait with one `impl` per language, rather than in a table of
`&'static str` values. The reason is specific: `format!` requires its format string to be a
**literal**, so a runtime-loaded template cannot be used with it at all. Keeping every message as a
method means the interpolation stays a literal and the compiler checks placeholder count and types
- a mismatch becomes a build failure rather than a sentence with a hole in it. The side benefits are
that a translation can be proofread as one contiguous block, and that adding a language means
adding a single `impl`: forgetting one message is a compile error.

What is deliberately **not** translated:

- **Command names and arguments.** `open`, `flag`, `chord`, `mode`, `best` and the `easy` /
  `medium` / `hard` difficulty keys are identifiers, as is `/sweeperboom` itself. Translating them
  would silently change the command syntax whenever a player switches language.
- **Coordinates and the board glyph set.** Column letters, row numbers, `#`, `F`, `-`, `*`, `X` and
  the digits `1`-`8` are symbols rather than prose, and the grid's alignment depends on them being
  exactly these characters.
- **Server log lines.** `tracing::info!` output stays in Simplified Chinese; it is operator-facing
  and not part of the player experience.

Two strings are player-visible but still cannot be per-player, because both are registered a single
time in `on_load`, where there is no player context to read a locale from: the **command-tree
description** (shown in the client's command suggestions) and the **plugin metadata
description**. Both carry the two languages in one line separated by `·` instead, so neither
audience is left out. The permission node's description is operator-only and stays in Simplified
Chinese.

## Requirements

- A Pumpkin server whose plugin API matches the pinned crate version
  (`pumpkin-plugin-api = "0.2.0+26.3-26.51"`).
- To build from source: a Rust toolchain with the `wasm32-wasip2` target installed.

```sh
rustup target add wasm32-wasip2
```

## Installation

```sh
cargo build --release
```

`.cargo/config.toml` already pins `wasm32-wasip2` as the default build target, so a plain
`cargo build --release` produces the plugin. The `[profile.release]` block enables LTO, which
shrinks the wasm binary considerably.

Then copy the artifact into your server's plugin directory and restart:

```sh
cp target/wasm32-wasip2/release/sweeperboom.wasm /path/to/server/plugins/
```

In game, run `/sweeperboom start`. Your first board is pushed straight into chat.

## Commands

The root command `/sweeperboom` has the alias `sweeper`.

| Command | Aliases | Description |
| --- | --- | --- |
| `/sweeperboom` | - | Re-display the current board, or a difficulty picker if none is running |
| `/sweeperboom start [easy\|medium\|hard]` | `new`, `restart`, `reset` | Start a new game (defaults to medium) |
| `/sweeperboom open <cell>` | `o`, `dig`, `reveal` | Reveal a cell |
| `/sweeperboom flag <cell>` | `f`, `mark` | Flag / unflag a cell |
| `/sweeperboom chord <cell>` | `ch` | Reveal the remaining neighbours of a satisfied number |
| `/sweeperboom mode [open\|flag]` | - | Toggle, or explicitly set, what clicking the board does |
| `/sweeperboom undo` | `back`, `z` | Undo the previous action |
| `/sweeperboom show` | `board`, `b` | Re-display the board |
| `/sweeperboom best` | `top`, `rank`, `leaderboard` | Server-wide leaderboard (top 10 plus your own rank) |
| `/sweeperboom me` | `stats`, `profile` | Your own statistics and rank |
| `/sweeperboom stop` | `quit`, `end`, `exit` | End the current game and record it |
| `/sweeperboom help` | `h` | Command reference and rules |

Every subcommand literal is a separate node in the Brigadier-style command tree, which is why
clients get working tab-completion and argument highlighting for free. Game responses are sent
only to the player who invoked the command - nothing is broadcast to the rest of the server.

## Coordinates

Columns are letters starting at `a` (up to `p`), rows are numbers starting at `1`, so `c5` is
column 3, row 5. The `3-5`, `3,5`, and `3:5` forms are accepted too, for players who would rather
count than map letters.

Each cell's hover tooltip shows its own coordinate, so you never have to count columns by eye.

## Rules

Standard sweeperboom with the conventional safety guarantees:

- Three difficulties: **easy** 8×8 with 10 mines, **medium** 12×12 with 25, **hard** 16×16 with 50.
- Mines are placed on the first reveal. The clicked cell and, whenever the board is large enough
  to allow it, all eight of its neighbours are guaranteed to be mine-free. Placement is a partial
  Fisher–Yates over the eligible cells, so every valid layout is equally likely.
- A cell showing `0` floods: it and every reachable cell are opened, stopping at numbered cells.
  Flagged cells are never opened automatically.
- Flagging a revealed cell is refused; opening a flagged cell is refused.
- `chord` only fires when the number of adjacent flags equals the number on the cell. Mismatched
  counts are reported rather than silently ignored, which is what makes it safe to type.
- The game is won when every non-mine cell is open (flags on the mines are applied for you in the
  final render). It is lost on the first mine you open, whether directly or via `chord`.

The timer starts on the first reveal, not on `/sweeperboom start`, so thinking time before the
opening move is not counted.

## Losing has consequences

Stepping on a mine - directly, or through `chord` - does two things. It ends the game and records
the loss, and it **spawns a primed TNT at the player's position**: the spot they were standing on
when they ran the command. The entity keeps Minecraft's default fuse, so the natural reaction is to
start running.

The TNT is a real `minecraft:tnt` entity, created through the host's `world.spawn_entity` - not a
cosmetic effect. `create-explosion` is deliberately *not* called: the entity brings its own blast,
and firing a second one would double it. That also means what happens after the spawn is the
server's business rather than the plugin's; the plugin's responsibility ends at placing the entity.

Two implementation details worth knowing:

- The spawn happens **only after the game lock is released**, because reading the sender's position
  and world, and spawning the entity, are all host calls. The rule that no host call happens under
  a lock is what dictates the shape of that code path.
- The note is only printed if the spawn actually happened. If the sender has no position or world
  the plugin stays silent rather than guessing a location - in practice both are always present for
  a player, so this is a guard, not a normal branch.

There is no configuration switch: every loss is punished. If you want it opt-in per player or per
world, the single hook is the `ignite_tnt` call in `play`.

## The board as rendered

Chinese (`zh_cn`):

```
SweeperBoom 中等 12×12 雷25  剩余 22  已开 18/119  操作 18  排名 #3  模式:打开
用时 34s   格子可点击；/sweeperboom help 看玩法
   a b c d e f g h i j k l
01 # # # # # # # # # # # #
02 # 1 - - - 1 # # # # # #
03 # 1 F - 1 2 # # # # # #
04 # 1 1 2 2 # # # # # # #
...
[模式:打开] [撤销] [重开] [结束]  [排行] [战绩] [帮助]
```

English (`en_us`), same game:

```
SweeperBoom  Medium 12×12, 25 mines  Left 22  Opened 18/119  Moves 18  Rank #3  Mode:Open
Time 34s   Cells are clickable; see /sweeperboom help
   a b c d e f g h i j k l
01 # # # # # # # # # # # #
02 # 1 - - - 1 # # # # # #
...
[Mode:Open] [Undo] [Restart] [End]  [Ranking] [Stats] [Help]
```

Legend: `#` hidden, `F` flagged, `-` revealed and empty, `1`–`8` revealed with that many adjacent
mines, `*` a mine shown after a loss, `X` the mine that ended the game. The board itself is
identical in both languages - only the surrounding prose changes.

## Leaderboard

Statistics live in the plugin's private data folder, which the host resolves to
`plugins/data/sweeperboom/sweeperboom_leaderboard.tsv` under the server root, so they survive
server restarts. The file is a tab-separated table with one row per player:

```
<name>  <wins>  <losses>  <best_easy>  <best_medium>  <best_hard>
```

Best times are in whole seconds; `0` means "no record yet". A row from a real server looks like
this - one abandoned medium game, nothing else:

```
233bit	0	1	0	0	0
``` Ties are broken in this order:

1. Wins (descending)
2. Best hard time, then medium, then easy (ascending; a missing record sorts last)
3. Player name (lexicographic)

A run is recorded when it is won, when it is lost, when `/sweeperboom stop` is used, or when a
game in progress is abandoned via `/sweeperboom start`. Runs with zero actions are never recorded,
and a `recorded` flag on each game prevents the same run from being counted twice.

`/sweeperboom best` shows the top 10. Your own row is highlighted in green and bold; if you rank
below the cut-off, your row is appended after an ellipsis so you always see where you stand.
`/sweeperboom me` shows your rank as `#3 / 14 人` along with your full statistics.

The current rank is also appended to the board's status line (`排名 #12`) and refreshes on every
action, so you can watch it climb without leaving the game.

### On undo and integrity

Undo is deliberately disabled once a game has ended. Without that restriction you could open a
mine, undo, and carry on, or lose and then undo back into a winning position - either way the
already-recorded result would stop matching the board. `/sweeperboom undo` therefore reports that
the game is over instead of rewinding it.

## Permissions

The command tree is registered against the permission node `sweeperboom:command.play` with
`PermissionDefault::Allow`, so every player can play out of the box. Server operators can revoke
it to lock the game down.

The plugin also requests two plugin-level permissions, used only for the leaderboard file:

| Permission | Why |
| --- | --- |
| `fs.read.data` | Read `sweeperboom_leaderboard.tsv` on load |
| `fs.write.data` | Write `sweeperboom_leaderboard.tsv` when a game is recorded, and on unload |

No network permissions are requested, and no files outside the plugin's own data folder are
touched. The host sandboxes each plugin into its own directory (`plugins/data/<name>/`), so the
filename cannot actually collide with `two048`'s `leaderboard.tsv`; giving it a distinct name is
defensive in case that mapping ever changes.

## Project layout

The whole plugin is a single file, organised into labelled sections:

| Section | Contents |
| --- | --- |
| Constants | Difficulty sizes, history depth, leaderboard file name, permission node |
| Pure logic | `Difficulty`, `Mode`, `Status`, `Slot`, `Step`, `Rng`, `Board`, `Game`, coordinate helpers, `Profile` and the TSV codec |
| Languages | `Lang`, the `Strings` trait, and the `ZhCn` / `EnUs` implementations |
| Global state | `GAMES`, `PROFILES`, `DATA_FILE` statics and their accessors |
| Leaderboard | Sorting, ranking, accumulation, save/load |
| Host interaction | RNG seeding and wall-clock reads, kept out of any critical section |
| Rendering | `TextComponent` builders for cells, the board, the control row, and the panels |
| Dispatch | The `Action` enum, the `CommandHandler` implementation, and `play` |
| Plugin entry | `PluginMetadata`, `on_load`, `on_unload`, `register_plugin!` |

## Implementation notes

- `Plugin::on_load` takes `&self` rather than `&mut self`, so all mutable state lives in
  `static Mutex<...>` values and is accessed through small guard-returning helpers.
- No host API calls happen while a lock is held. State is mutated under the lock, an owned
  snapshot is cloned out, the guard is dropped, and only then is the message rendered and sent.
  The RNG seed, the wall clock, and the leaderboard file I/O are all arranged to sit outside the
  critical section. The Pumpkin docs explicitly warn against holding a non-reentrant lock across a
  host call; this layout keeps that invariant easy to audit.
- There is no `rand` dependency. Randomness comes from a `splitmix64` generator whose seed mixes
  a host-provided random UUID with the wall clock, which avoids relying on a `getrandom` backend
  under `wasm32-wasip2`.
- The board is a single text component with newline children. Every cell renders as **one glyph
  plus one space**, and the glyph set is restricted to `#`, `F`, `-`, `*`, `X` and `0`–`8`.
  Minecraft's default font is proportional, so mixing wide and narrow glyphs (or padding with
  spaces, which are narrower than digits) would make columns drift; keeping every cell to one
  glyph of the same ink width plus one separator keeps the grid square. Row labels are zero-padded
  (`01`–`16`) for the same reason. This is why the grid deliberately avoids `.` and similar narrow
  glyphs. The layout is designed against that property; it has not been pixel-measured in a live
  client, so the easy and medium boards are the ones to eyeball first.
- Randomness is injected into `Board::place_mines`, `Game::open`, and `now` as parameters rather
  than being read internally, which is what makes the "no host calls under a lock" rule hold and
  also makes the whole pure-logic section deterministic under test.
- The language is resolved once per command, in `sender_lang`, and then threaded down as a `Lang`
  value. The rendering functions receive it as an ordinary argument rather than re-reading the
  player's locale, so there is exactly one host call per command and no locale lookup can happen
  inside a critical section.

## Testing

`cargo test` is not usable for this crate: `crate-type` is `cdylib` and `.cargo/config.toml` pins
the `wasm32-wasip2` target, so no test binary can run on the host.

Instead, `tools/logic_harness.py` extracts the section of `src/lib.rs` delimited by the
`PURE-LOGIC-BEGIN` / `PURE-LOGIC-END` markers (the part with no `pumpkin_plugin_api` dependency),
concatenates `tools/logic_tests.rs`, and compiles the result with the host `rustc --test`. Because
the extraction is automated, the tests always run against the current source and cannot drift from
the plugin implementation.

```sh
python tools/logic_harness.py
```

35 tests run, covering:

- Mine placement: exact mine count, first-click cell and neighbours always clear, `adjacent`
  consistent with `mine`, and a 400-round uniformity check on the cell-by-cell mine frequency.
- Flood fill: a mine-free board opens completely; the result is compared cell-for-cell against an
  independently written reference implementation (a fixpoint over `adjacent == 0`) across 5
  deterministic layouts plus 120 randomly generated boards; flags are never opened.
- Open / flag / chord: first click places mines and starts the clock; opening a mine sets the loss
  state and records the exploded cell; opening the last safe cell wins; opening a flagged or
  already-open cell is refused; a cell ringed by mines is never revealed by the flood.
- Chord: mismatched flag counts are reported; a satisfied count opens the rest and can win; a
  wrongly-placed pair of flags detonates the mine underneath.
- Undo: restores board and counters, is bounded by `MAX_HISTORY`, and is refused once the game is
  over.
- Coordinates: round-trip of every cell on all three grid sizes, upper case, the numbered-column
  forms, and rejection of junk input (`""`, `a`, `a0`, `a9`, `q1`, `abc`, `35`, `-5`, `3-`).
- Leaderboard: four-level tie-break ordering, deterministic ordering under a full tie, TSV
  round-trip, tolerance of corrupt and short rows, best-time accumulation (a slower win must not
  overwrite the record), win-rate arithmetic, and time formatting.
- Languages: locale-to-language mapping for `zh` variants, unknown, and empty values; every
  constant string non-empty and free of stray placeholder braces in both languages; the help table
  matched against the command list; identical command literals across languages; and a
  **sentinel check** that calls all 38 templated messages with unmistakable marker values and
  asserts every argument actually appears in the output - which is the one failure mode `format!`
  cannot catch, since a correct argument list can still be omitted from the template.
- Wording lock: the fully composed sentences are asserted byte-for-byte against the expected output
  (`SweeperBoom 中等 12×12 雷25  剩余 22  已开 18/119`, `第一击安全，打开了 12 格。`, and so on).
  Apart from the brand name this is still exactly what the plugin printed before the translation
  layer existed, so the test doubles as a regression guard on the refactor, and it is the closest
  thing to verifying Chinese output without a Minecraft client - the `zh_cn` branch cannot be
  reached end to end without one.
- Brand name: the on-screen name is deliberately identical in both languages, which is why
  `status_title` is the one entry excluded from the "the two languages differ" check and is pinned
  by its own test instead.

### Note on names

The plugin was originally called `minesweeper` and was renamed to `sweeperboom` afterwards. The
rename changed the crate and the wasm artifact, the command and its aliases (`ms` / `mine` /
`sweep` became `sweeper`), the permission node, the leaderboard file name, and the on-screen name -
not the mechanics or any game logic. The log excerpts below are reproduced exactly as they were
captured, so the ones recorded before the rename still show the old name; the final section is the
post-rename run.

### Load check in a throwaway instance

The plugin was loaded into a throwaway Pumpkin instance (a separate copy of the server binary,
world, and data with a different port, so the running server was untouched). The log confirms the
load sequence as it was at the time:

```
INFO minesweeper: 排行榜已从 data/minesweeper_leaderboard.tsv 载入 0 条记录
INFO minesweeper 已加载：使用 /minesweeper start 开始一局扫雷
INFO Loaded minesweeper (0.1.0)
WARN Plugin "minesweeper" uses the following permissions: ["fs.read.data", "fs.write.data"]
```

Dispatching `minesweeper help`, the alias `ms`, and `minesweeper me` from that instance's console
all reached the plugin and produced its reply, while `2048` in the same instance correctly reported
`Unknown command: 2048` - confirming the command tree and aliases were resolved by this plugin
rather than matching something else.

That same console run is the end-to-end check of the language wiring. Before the translation was
added, those commands printed the Chinese player-only notice; afterwards the very same commands
print:

```
Minesweeper is a player-only mini-game; run this command in game as a player.
```

The console has no locale, so it takes the English fallback - which means the selection path
(`sender_lang` -> `Lang::from_locale` -> `EnUs`) was exercised against the real host rather than
only in the harness. The `zh_cn` branch of the catalog is covered by the unit tests, since reaching
it in-game would require a Minecraft client.

The plugin itself builds with zero warnings and a clean `cargo clippy`.

### Startup and dispatch after the rename

The final check was a restart of the real server with the renamed build installed. The migrated
leaderboard was read back on load, so the rename and the data migration are both confirmed:

```
INFO sweeperboom: 排行榜已从 data/sweeperboom_leaderboard.tsv 载入 1 条记录
INFO sweeperboom 已加载：使用 /sweeperboom start 开始一局扫雷
INFO Loaded sweeperboom (0.1.0)
WARN Plugin "sweeperboom" uses the following permissions: ["fs.read.data", "fs.write.data"]
```

`载入 1 条记录` is the row carried over from the old plugin's data folder, so the write-then-read
round trip across a restart survives the rename.

Four console commands were then dispatched - `sweeperboom best`, `sweeper help`, `2048 best`, and
`mine help` - covering the new name, the new alias, the neighbouring plugin, and a retired alias:

```
SweeperBoom is a player-only mini-game; run this command in game as a player.
SweeperBoom is a player-only mini-game; run this command in game as a player.
2048 是玩家专属的小游戏，请在游戏内由玩家执行该命令。
Unknown command: mine. Please check that the command exists and that you have permission to use it.
```

The first two lines come from the renamed plugin, brand name included. The third is the
neighbouring `two048`, which has no translation layer and still answers in Chinese - the contrast
being the clearest evidence that language selection is really running rather than printing
whichever string happened to be first. The fourth is the useful one: `mine` was an alias before the
rename and now resolves to nothing, so the alias list was genuinely replaced rather than extended.

### Live play on the real server

After the plugin was deployed, the game was also played for real on the server it was built for.
The log shows the whole session:

```
INFO minesweeper 已加载：使用 /minesweeper start 开始一局扫雷
INFO 233bit joined the game
INFO Player (233bit): executed command /minesweeper start
INFO Player (233bit): executed command /minesweeper open c3
... open i5, j5, k5, k4, i4, i6
INFO Player (233bit): executed command /minesweeper start easy
INFO Player (233bit): executed command /minesweeper help
INFO Player (233bit): executed command /minesweeper undo
INFO Player (233bit): executed command /minesweeper me
INFO Player (233bit): executed command /minesweeper start
INFO minesweeper 已卸载，排行榜已保存
```

Every subcommand in that sequence resolved - including the `easy` difficulty literal, and `open`
with a letter-and-digit coordinate. The recorded result on disk was:

```
233bit	0	1	0	0	0
```

One loss and nothing else, which is exactly right: the first (medium) game was abandoned by
`/minesweeper start easy` after eight moves and therefore recorded as a loss, while the easy game
that followed was abandoned by a plain `/minesweeper start` having made **zero** moves and was
therefore correctly not recorded. That single line exercises the abandon path, the record-once
guard, the no-zero-move rule, and the `fs.write.data` persistence in one go.

### What the tests do not cover

The TNT spawn cannot be exercised without a client: it needs a sender with a position, and the
console has none. The load-time checks above confirm the new code path is compiled in and that the
plugin still loads and registers cleanly, and the engine was separately confirmed to accept a
`tnt` spawn - `/summon tnt 0 100 0` from the console answers `Object successfully summoned`, so
`minecraft:tnt` is a type this server can actually create. Whether the fuse ticks down and the
blast lands is engine behaviour and is left for in-game verification.

## Troubleshooting

**`/sweeperboom` is not a known command.** The wasm file is not in the server's `plugins/`
directory, or its plugin API version does not match the server. Confirm that the pinned
`pumpkin-plugin-api` version in `Cargo.toml` lines up with your server release.

**The leaderboard resets after a restart.** The plugin needs the `fs.read.data` and
`fs.write.data` permissions to persist its data folder. Check the server log for a line starting
with `sweeperboom:`; a failed write is reported there as a warning.

**A player sees English (or Chinese) instead of their own language.** The plugin only ships
`zh_cn` and `en_us`, and everything that is not a `zh` locale falls back to English, so a `ja_jp`
or `de_de` client gets English by design. Check what the client reported before assuming a bug: a
client set to a variant the plugin does not carry - or a modded client reporting an unexpected
locale - will fall back. Adding a language means adding one `Strings` impl in `src/lib.rs`.

**The board wraps onto a second line.** The hard board is 35 characters wide plus its controls.
Half of a maximised window, or GUI scale above 4, can be too narrow for it. Easy and medium fit
comfortably at any normal scale.

**Cargo prints a warning about semver metadata.** `pumpkin-plugin-api = "0.2.0+26.3-26.51"`
carries build metadata after the `+`, which Cargo ignores for version resolution and warns about.
It is harmless. The version is pinned deliberately so that `cargo update` cannot silently move the
plugin onto an API track your server does not implement.

## Credits

Game and command structure follow the Pumpkin plugin development guide, and the plugin mirrors the
layout of the neighbouring `two048` plugin. Plugin API signatures were verified against the
`pumpkin-plugin-api` WIT definitions rather than the guide's snippets, since several of the
guide's examples had drifted from the current API.
