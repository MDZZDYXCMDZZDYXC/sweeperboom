// 纯逻辑测试：由 tools/logic_harness.py 抽出 src/lib.rs 的 PURE-LOGIC 区段后拼接，
// 再用本机 rustc 编译执行（该 crate 本身是 cdylib + wasm32-wasip2，cargo test 不可用）。
//
// 测试只依赖纯逻辑，不触碰 pumpkin_plugin_api。

#[cfg(test)]
mod tests {
    use super::*;

    /// 直接按给定下标布置地雷，绕开随机布雷，便于构造确定性的局面。
    fn board_of(width: usize, height: usize, mines: &[usize]) -> Board {
        let mut board = Board::new(width, height, mines.len());
        for &m in mines {
            board.mine[m] = true;
        }
        board.recompute_adjacent();
        board.placed = true;
        board
    }

    /// 独立实现的泛洪参考解：从起点取闭包 —— 已打开且 `adjacent == 0` 的格子会打开其全部邻居。
    fn reference_reveal(board: &Board, start: usize) -> Vec<bool> {
        let mut revealed = vec![false; board.cells()];
        if board.mine[start] || board.flagged[start] {
            return revealed;
        }
        revealed[start] = true;
        loop {
            let mut changed = false;
            for i in 0..board.cells() {
                if !revealed[i] || board.mine[i] || board.adjacent[i] != 0 {
                    continue;
                }
                for n in board.neighbors(i) {
                    if !revealed[n] && !board.flagged[n] {
                        revealed[n] = true;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        revealed
    }

    // -----------------------------------------------------------------------
    // 布雷
    // -----------------------------------------------------------------------

    #[test]
    fn placements_have_exact_mine_count_and_safe_opening() {
        for difficulty in Difficulty::all() {
            let (width, height, mines) = difficulty.size();
            assert_eq!(width * height > mines, true);

            for seed in 0..300u64 {
                let mut rng = Rng::seeded(seed);
                let mut board = Board::new(width, height, mines);
                let safe = (seed as usize * 7 + 3) % board.cells();
                board.place_mines(safe, &mut rng);

                assert!(board.placed);
                assert_eq!(
                    board.mine.iter().filter(|m| **m).count(),
                    mines,
                    "{difficulty:?} seed {seed}: 雷数不符"
                );
                assert!(!board.mine[safe], "首次点击的格子不能是雷");
                for n in board.neighbors(safe) {
                    assert!(!board.mine[n], "首次点击的邻居不能是雷");
                }

                // 周围雷数必须与 `mine` 一致。
                for i in 0..board.cells() {
                    if board.mine[i] {
                        assert_eq!(board.adjacent[i], 0);
                        continue;
                    }
                    let expected =
                        board.neighbors(i).iter().filter(|&&j| board.mine[j]).count() as u8;
                    assert_eq!(board.adjacent[i], expected);
                }
            }
        }
    }

    #[test]
    fn placement_distribution_is_reasonably_uniform() {
        // 固定首击格为 0，因此只有它和它的邻居永远不可能是雷；其余 252 格机会均等。
        let (width, height, mines) = (16, 16, 50);
        let safe = 0usize;
        let excluded: Vec<usize> = std::iter::once(safe)
            .chain(Board::new(width, height, mines).neighbors(safe))
            .collect();
        let pool = width * height - excluded.len();

        let mut hits = vec![0u32; width * height];
        let rounds = 400u32;
        for seed in 0..u64::from(rounds) {
            let mut rng = Rng::seeded(seed);
            let mut board = Board::new(width, height, mines);
            board.place_mines(safe, &mut rng);
            for (i, is_mine) in board.mine.iter().enumerate() {
                if *is_mine {
                    hits[i] += 1;
                }
            }
        }

        let expected = f64::from(rounds) * mines as f64 / pool as f64;
        for (i, hit) in hits.iter().enumerate() {
            if excluded.contains(&i) {
                assert_eq!(*hit, 0, "格子 {i} 不应该成为雷");
                continue;
            }
            let deviation = (*hit as f64 - expected).abs() / expected;
            assert!(
                deviation < 0.35,
                "格子 {i} 成雷频率偏差过大：{hit}（期望约 {expected:.1}）"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 泛洪展开
    // -----------------------------------------------------------------------

    #[test]
    fn flood_on_mine_free_board_reveals_everything() {
        let mut board = Board::new(8, 8, 0);
        let mut rng = Rng::seeded(1);
        board.place_mines(0, &mut rng);

        assert_eq!(board.reveal_flood(0), 64);
        assert!(board.revealed.iter().all(|r| *r));
    }

    #[test]
    fn flood_matches_independent_reference() {
        // 若干确定性局面 + 若干随机局面，逐一与独立参考解对比。
        let cases: [(usize, usize, &[usize]); 5] = [
            (3, 3, &[0]),
            (5, 5, &[4, 9, 14, 19]),
            (5, 5, &[6, 7, 8, 11, 13, 16, 17, 18]),
            (6, 6, &[0, 1, 6, 7]),
            (4, 4, &[5]),
        ];

        for (width, height, mines) in cases {
            for start in 0..width * height {
                let mut board = board_of(width, height, mines);
                if board.mine[start] {
                    continue;
                }
                let expected = reference_reveal(&board, start);
                let gained = board.reveal_flood(start);

                assert_eq!(
                    board.revealed, expected,
                    "局面 {width}x{height} 起点 {start} 的展开结果与参考解不一致"
                );
                assert_eq!(gained, expected.iter().filter(|r| **r).count());
            }
        }

        for seed in 0..120u64 {
            let (width, height, mines) = (9, 9, 12);
            let mut rng = Rng::seeded(seed);
            let mut board = Board::new(width, height, mines);
            let start = (seed as usize * 11) % board.cells();
            board.place_mines(start, &mut rng);

            let expected = reference_reveal(&board, start);
            let gained = board.reveal_flood(start);
            assert_eq!(board.revealed, expected, "随机局面 seed {seed} 展开结果不一致");
            assert_eq!(gained, expected.iter().filter(|r| **r).count());
        }
    }

    #[test]
    fn flood_stops_at_flags() {
        let mut board = board_of(3, 3, &[0]);
        board.flagged[5] = true; // 把右侧唯一通道标掉
        board.reveal_flood(8);
        assert!(!board.revealed[5], "插旗的格子不应被自动打开");
    }

    // -----------------------------------------------------------------------
    // 打开 / 插旗 / 胜负
    // -----------------------------------------------------------------------

    #[test]
    fn first_click_places_mines_and_starts_the_clock() {
        let mut game = Game::new(Difficulty::Easy);
        assert!(!game.board.placed);
        assert_eq!(game.started_at, 0);

        let mut rng = Rng::seeded(7);
        let step = game.open(27, &mut rng, 1_000);

        assert!(game.board.placed);
        assert_eq!(game.started_at, 1_000);
        assert_eq!(game.moves, 1);
        assert!(!game.board.mine[27]);
        match step {
            Step::Opened { first, gained } => {
                assert!(first);
                assert!(gained >= 1);
            }
            other => panic!("首击应返回 Opened，实际 {other:?}"),
        }
    }

    #[test]
    fn losing_reveals_the_exploded_mine() {
        let mut game = Game::new(Difficulty::Easy);
        game.board = board_of(3, 3, &[4]);
        game.started_at = 100;

        let mut rng = Rng::seeded(3);
        let step = game.open(4, &mut rng, 160);

        assert_eq!(step, Step::HitMine);
        assert_eq!(game.status, Status::Lost);
        assert_eq!(game.exploded, Some(4));
        assert_eq!(game.elapsed, 60);
        assert!(game.is_over());
    }

    #[test]
    fn opening_the_last_safe_cell_wins() {
        let mut game = Game::new(Difficulty::Easy);
        game.board = board_of(3, 3, &[0]);
        game.started_at = 500;

        let mut rng = Rng::seeded(9);
        let step = game.open(8, &mut rng, 512);

        assert_eq!(step, Step::Won);
        assert_eq!(game.status, Status::Won);
        assert_eq!(game.revealed_total, 8);
        assert_eq!(game.elapsed, 12);
    }

    #[test]
    fn opening_a_flagged_or_opened_cell_is_refused() {
        // 用一圈雷把 24 围起来：泛洪会推平其余 40 格，只留下 24 没打开。
        let mut game = Game::new(Difficulty::Easy);
        game.board = board_of(7, 7, &[16, 17, 18, 23, 25, 30, 31, 32]);
        game.started_at = 1_000;
        let mut rng = Rng::seeded(5);

        game.open(0, &mut rng, 1_010);
        assert_eq!(game.status, Status::Playing);
        assert!(game.board.revealed[0]);
        assert!(!game.board.revealed[24], "被雷环围住的格子不该被推开");

        // 已打开的格子：打开与插旗都被拒绝。
        assert_eq!(game.open(0, &mut rng, 1_020), Step::AlreadyOpen);
        assert_eq!(game.flag(0), Step::AlreadyOpen);

        // 插旗的格子不能打开。
        assert_eq!(game.flag(24), Step::Toggled { flagged: true });
        assert_eq!(game.open(24, &mut rng, 1_030), Step::Flagged);
        assert_eq!(game.flag(24), Step::Toggled { flagged: false });

        // 取消插旗后可以打开，而这一步正好排清全盘。
        assert_eq!(game.open(24, &mut rng, 1_040), Step::Won);

        // 结束之后一切动作都被拒绝。
        assert_eq!(game.open(1, &mut rng, 1_050), Step::Over);
        assert_eq!(game.flag(1), Step::Over);
        assert_eq!(game.chord(0, 1_050), Step::Over);
    }

    #[test]
    fn flag_toggling_counts_and_mine_counter() {
        let mut game = Game::new(Difficulty::Easy);
        assert_eq!(game.mines_left(), 10);
        assert_eq!(game.board.flags(), 0);

        game.flag(0);
        game.flag(1);
        assert_eq!(game.board.flags(), 2);
        assert_eq!(game.mines_left(), 8);
        assert_eq!(game.flag(0), Step::Toggled { flagged: false });
        assert_eq!(game.board.flags(), 1);
        assert_eq!(game.moves, 3);
    }

    // -----------------------------------------------------------------------
    // chord
    // -----------------------------------------------------------------------

    /// 5x5，第 e 列前四格是雷：泛洪只推进到 d 列，e5 会一直留着，正好用来测 chord。
    fn chord_fixture() -> Game {
        let mut game = Game::new(Difficulty::Easy);
        game.board = board_of(5, 5, &[4, 9, 14, 19]);
        game.started_at = 1_000;
        game
    }

    #[test]
    fn chord_requires_matching_flag_count() {
        let mut game = chord_fixture();
        let mut rng = Rng::seeded(2);
        game.open(0, &mut rng, 1_005);

        assert_eq!(game.status, Status::Playing);
        assert!(!game.board.revealed[24], "e5 应该还没被打开");

        // d5（下标 23）周围只有 e4 是雷，需要 1 面旗。
        assert_eq!(game.board.adjacent[23], 1);
        assert_eq!(
            game.chord(23, 1_010),
            Step::FlagsMismatch { need: 1, have: 0 }
        );

        game.flag(19);
        assert_eq!(game.chord(23, 1_020), Step::Won);
        assert!(game.board.revealed[24]);
        assert_eq!(game.elapsed, 20);
    }

    #[test]
    fn chord_can_explode() {
        let mut game = chord_fixture();
        let mut rng = Rng::seeded(2);
        game.open(0, &mut rng, 1_005);

        // d4（下标 18）周围有 e3、e4 两颗雷。把旗插在 e3 与 e5 上凑够 2 面，
        // chord 就会去打开剩下的 e4 —— 那是一颗雷。
        assert_eq!(game.board.adjacent[18], 2);
        game.flag(14);
        game.flag(24);
        assert_eq!(game.chord(18, 1_030), Step::HitMine);
        assert_eq!(game.status, Status::Lost);
        assert_eq!(game.exploded, Some(19));
    }

    #[test]
    fn chord_only_works_on_opened_numbered_cells() {
        let mut game = chord_fixture();
        let mut rng = Rng::seeded(2);
        game.open(0, &mut rng, 1_005);

        assert_eq!(game.chord(24, 1_010), Step::NotOpen);
        assert_eq!(game.chord(0, 1_010), Step::NothingToChord);
    }

    // -----------------------------------------------------------------------
    // 撤销
    // -----------------------------------------------------------------------

    #[test]
    fn undo_rolls_back_board_and_counters() {
        let mut game = chord_fixture();
        let mut rng = Rng::seeded(4);

        assert_eq!(game.undo(), Undo::Empty);
        game.open(0, &mut rng, 2_000);
        let revealed_after_open = game.revealed_total;
        assert!(revealed_after_open > 0);

        assert_eq!(game.undo(), Undo::Done);
        assert_eq!(game.revealed_total, 0);
        assert_eq!(game.moves, 0);
        assert!(game.board.revealed.iter().all(|r| !*r));

        game.flag(3);
        assert_eq!(game.moves, 1);
        assert_eq!(game.undo(), Undo::Done);
        assert_eq!(game.moves, 0);
        assert!(!game.board.flagged[3]);
    }

    #[test]
    fn undo_is_disabled_after_the_game_ends() {
        let mut game = Game::new(Difficulty::Easy);
        game.board = board_of(3, 3, &[4]);
        game.started_at = 10;
        let mut rng = Rng::seeded(6);

        assert_eq!(game.open(4, &mut rng, 20), Step::HitMine);
        assert_eq!(game.undo(), Undo::Over);
        assert_eq!(game.status, Status::Lost);
    }

    #[test]
    fn undo_history_is_bounded() {
        let mut game = Game::new(Difficulty::Easy);
        let mut rng = Rng::seeded(8);
        for i in 0..MAX_HISTORY + 20 {
            game.flag(i % 20);
        }
        assert!(game.history.len() <= MAX_HISTORY);
        // 仍然可用。
        assert_eq!(game.undo(), Undo::Done);
        let _ = &mut rng;
    }

    // -----------------------------------------------------------------------
    // 坐标
    // -----------------------------------------------------------------------

    #[test]
    fn cell_labels_round_trip() {
        for (width, height) in [(8usize, 8usize), (12, 12), (16, 16)] {
            for index in 0..width * height {
                let label = cell_label(index, width);
                assert_eq!(parse_cell(&label, width, height), Some(index));
                assert_eq!(parse_cell(&label.to_uppercase(), width, height), Some(index));
            }
        }
    }

    #[test]
    fn cell_parsing_accepts_numbered_columns() {
        assert_eq!(parse_cell("3-5", 12, 12), parse_cell("c5", 12, 12));
        assert_eq!(parse_cell("3,5", 12, 12), parse_cell("c5", 12, 12));
        assert_eq!(parse_cell("3:5", 12, 12), parse_cell("c5", 12, 12));
        assert_eq!(parse_cell(" 3-5 ", 12, 12), parse_cell("c5", 12, 12));
        assert_eq!(parse_cell("16-16", 16, 16), Some(255));
    }

    #[test]
    fn cell_parsing_rejects_junk() {
        for bad in ["", "a", "0a", "a0", "a9", "q1", "abc", "35", "-5", "3-"] {
            assert_eq!(parse_cell(bad, 8, 8), None, "「{bad}」不应被解析成功");
        }
        assert_eq!(parse_cell("p16", 16, 16), Some(255));
        assert_eq!(parse_cell("p16", 8, 8), None);
    }

    #[test]
    fn coordinate_labels_use_letters_within_range() {
        assert_eq!(cell_label(0, 8), "a1");
        assert_eq!(cell_label(7, 8), "h1");
        assert_eq!(cell_label(8, 8), "a2");
        assert_eq!(cell_label(255, 16), "p16");
    }

    // -----------------------------------------------------------------------
    // 排行榜
    // -----------------------------------------------------------------------

    fn profile(name: &str, wins: u32, losses: u32, hard: u32) -> Profile {
        let mut p = Profile::new(name);
        p.wins = wins;
        p.losses = losses;
        p.best[Difficulty::Hard.slot()] = hard;
        p
    }

    #[test]
    fn ranking_prefers_wins_then_speed() {
        let mut list = vec![
            profile("slow_many", 3, 1, 0),
            profile("fast_few", 1, 0, 40),
            profile("many_fast", 3, 4, 55),
            profile("zero_wins", 0, 9, 30),
        ];
        sort_profiles(&mut list);
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["many_fast", "slow_many", "fast_few", "zero_wins"]);

        assert_eq!(rank_in(&list, "many_fast"), Some(1));
        assert_eq!(rank_in(&list, "zero_wins"), Some(4));
        assert_eq!(rank_in(&list, "nobody"), None);
    }

    #[test]
    fn ranking_is_deterministic_for_full_ties() {
        let mut list = vec![profile("b", 1, 0, 20), profile("a", 1, 0, 20)];
        sort_profiles(&mut list);
        assert_eq!(list[0].name, "a");
    }

    #[test]
    fn tsv_round_trip_and_corruption_tolerance() {
        let mut list = vec![
            profile("233bit", 4, 2, 71),
            profile("someone", 0, 1, 0),
        ];
        list[1].best[Difficulty::Easy.slot()] = 12;
        let text = encode_profiles(&list);
        assert_eq!(text.lines().count(), 2);

        let decoded = decode_profiles(&text);
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].name, "233bit");
        assert_eq!(decoded[0].wins, 4);
        assert_eq!(decoded[0].losses, 2);
        assert_eq!(decoded[0].best_of(Difficulty::Hard), 71);
        assert_eq!(decoded[1].best_of(Difficulty::Easy), 12);
        assert_eq!(decoded[1].best_of(Difficulty::Hard), 0);

        // 坏行、空行、字段不足的行都应被跳过而不是 panic。
        let messy = "\nnot-enough\tfields\nbad\tnums\tx\t1\t2\t3\ngood\t1\t0\t9\t0\t0\n\t1\t1\t1\t1\t1\n";
        let decoded = decode_profiles(messy);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name, "good");
    }

    #[test]
    fn accumulating_records_keeps_the_best_time() {
        let mut p = Profile::new("player");

        let mut game = Game::new(Difficulty::Medium);
        game.status = Status::Won;
        game.elapsed = 90;

        assert!(accumulate(&mut p, &game, true));
        assert_eq!(p.wins, 1);
        assert_eq!(p.best_of(Difficulty::Medium), 90);

        // 更慢的一局：胜场增加，但最快记录不变，也不算新纪录。
        game.elapsed = 120;
        assert!(!accumulate(&mut p, &game, true));
        assert_eq!(p.wins, 2);
        assert_eq!(p.best_of(Difficulty::Medium), 90);

        // 更快的一局：刷新记录。
        game.elapsed = 45;
        assert!(accumulate(&mut p, &game, true));
        assert_eq!(p.best_of(Difficulty::Medium), 45);

        // 失败：只加负场，不动时间。
        let mut lost = Game::new(Difficulty::Hard);
        lost.status = Status::Lost;
        lost.elapsed = 5;
        assert!(!accumulate(&mut p, &lost, false));
        assert_eq!(p.losses, 1);
        assert_eq!(p.best_of(Difficulty::Hard), 0);

        assert_eq!(p.games(), 4);
        assert_eq!(p.win_rate(), 75);
    }

    #[test]
    fn elapsed_and_win_rate_edge_cases() {
        let mut game = Game::new(Difficulty::Easy);
        // 还没开始计时。
        assert_eq!(game.elapsed_at(9_999), 0);
        game.started_at = 1_000;
        assert_eq!(game.elapsed_at(1_045), 45);
        // 时间倒退也不应下溢出负数。
        assert_eq!(game.elapsed_at(900), 0);

        let empty = Profile::new("nobody");
        assert_eq!(empty.win_rate(), 0);
        assert_eq!(empty.games(), 0);
    }

    #[test]
    fn seconds_are_rendered_readably() {
        assert_eq!(ZH_CN.format_seconds(0), "-");
        assert_eq!(ZH_CN.format_seconds(9), "9s");
        assert_eq!(ZH_CN.format_seconds(59), "59s");
        assert_eq!(ZH_CN.format_seconds(60), "1分0秒");
        assert_eq!(ZH_CN.format_seconds(125), "2分5秒");

        // 英文用 m/s，同样的数字在两种语言下的写法必须不同。
        assert_eq!(EN_US.format_seconds(0), "-");
        assert_eq!(EN_US.format_seconds(9), "9s");
        assert_eq!(EN_US.format_seconds(59), "59s");
        assert_eq!(EN_US.format_seconds(60), "1m0s");
        assert_eq!(EN_US.format_seconds(125), "2m5s");
    }

    // -----------------------------------------------------------------------
    // 多语言
    // -----------------------------------------------------------------------

    /// 所有无参文案，用于检查有没有漏译成空串。
    fn constant_texts(s: &dyn Strings) -> Vec<&'static str> {
        vec![
            s.status_title(),
            s.info_hint(),
            s.ctl_undo(),
            s.ctl_restart(),
            s.ctl_stop(),
            s.ctl_best(),
            s.ctl_me(),
            s.ctl_help(),
            s.no_game(),
            s.no_game_pick(),
            s.help_title(),
            s.help_rules(),
            s.help_coords(),
            s.help_click(),
            s.help_safe(),
            s.help_leaderboard(),
            s.rank_title(),
            s.rank_empty(),
            s.rank_start_button(),
            s.rank_ellipsis(),
            s.rank_not_listed(),
            s.rank_more(),
            s.rank_me_button(),
            s.me_title(),
            s.me_empty(),
            s.me_rank_label(),
            s.me_wins_label(),
            s.me_losses_label(),
            s.me_games_label(),
            s.me_rate_label(),
            s.me_more(),
            s.me_best_button(),
            s.step_over(),
            s.step_already_open(),
            s.step_flagged(),
            s.step_not_open(),
            s.step_nothing_to_chord(),
            s.step_flagged_on(),
            s.step_flagged_off(),
            s.step_tnt(),
            s.step_again(),
            s.undo_done(),
            s.undo_empty(),
            s.undo_over(),
            s.start_fresh(),
            s.player_only(),
        ]
    }

    #[test]
    fn locale_maps_to_the_right_language() {
        for locale in ["zh_cn", "zh_CN", "zh_tw", "zh_hk", "zh"] {
            assert_eq!(Lang::from_locale(locale), Lang::ZhCn, "「{locale}」应为简中");
        }
        for locale in ["en_us", "en_US", "en_gb", "", "ja_jp", "ko_kr", "de_de", "fr_fr"] {
            assert_eq!(Lang::from_locale(locale), Lang::EnUs, "「{locale}」应为英文");
        }
    }

    #[test]
    fn every_text_exists_in_both_languages() {
        for lang in [Lang::ZhCn, Lang::EnUs] {
            let s = lang.strings();

            for text in constant_texts(s) {
                assert!(!text.trim().is_empty(), "{lang:?} 存在空白文案");
                assert!(
                    !text.contains('{'),
                    "{lang:?} 的常量文案里不该出现占位符：{text:?}"
                );
            }

            let descriptions = s.help_descriptions();
            assert_eq!(descriptions.len(), HELP_COMMANDS.len());
            for description in descriptions {
                assert!(!description.trim().is_empty(), "{lang:?} 存在空白帮助文案");
                assert!(
                    !description.contains('{'),
                    "{lang:?} 的帮助文案里不该出现占位符：{description:?}"
                );
            }
        }
    }

    #[test]
    fn the_two_languages_are_actually_different() {
        // 防止「翻译」其实是把中文原样抄进英文表。
        let pairs: Vec<(String, String)> = vec![
            (ZH_CN.no_game().to_string(), EN_US.no_game().to_string()),
            (ZH_CN.player_only().to_string(), EN_US.player_only().to_string()),
            (ZH_CN.step_won("5s"), EN_US.step_won("5s")),
            (ZH_CN.undo_over().to_string(), EN_US.undo_over().to_string()),
            (ZH_CN.ctl_undo().to_string(), EN_US.ctl_undo().to_string()),
            (ZH_CN.rank_title().to_string(), EN_US.rank_title().to_string()),
            (
                ZH_CN.diff_label(Difficulty::Hard).to_string(),
                EN_US.diff_label(Difficulty::Hard).to_string(),
            ),
        ];
        for (zh, en) in pairs {
            assert_ne!(zh, en, "中英文词条不应完全相同：{zh}");
        }
    }

    #[test]
    fn the_brand_name_is_never_translated() {
        // 品牌名是专有名词，两种语言下必须逐字符相同——这正是上面那条测试里
        // 唯一被排除的一项，单独钉住以免日后被误「翻译」。
        assert_eq!(ZH_CN.status_title(), EN_US.status_title());
        assert_eq!(ZH_CN.status_title(), "SweeperBoom");
        assert!(ZH_CN.player_only().contains("SweeperBoom"));
        assert!(EN_US.player_only().contains("SweeperBoom"));
        assert!(ZH_CN.me_title().contains("SweeperBoom"));
        assert!(EN_US.me_title().contains("SweeperBoom"));
    }

    #[test]
    fn chinese_wording_is_locked_down() {
        // 没有客户端就无法端到端验证中文渲染，所以这里把整句拼接结果钉死。
        // 特别注意状态行：重构时把「扫雷」后面的空格从标题挪到了 status_size 里，
        // 拼接结果必须与改造前逐字节一致。
        let status = format!(
            "{}{}{}{}",
            ZH_CN.status_title(),
            ZH_CN.status_size("中等", 12, 12, 25),
            ZH_CN.status_remaining(22),
            ZH_CN.status_progress(18, 119),
        );
        assert_eq!(status, "SweeperBoom 中等 12×12 雷25  剩余 22  已开 18/119");

        assert_eq!(ZH_CN.info_won("1分23秒"), "通关！用时 1分23秒");
        assert_eq!(ZH_CN.step_won("45s"), "全部排清，通关！用时 45s。");
        assert_eq!(ZH_CN.step_hit_mine("45s"), "踩到雷了，本局失败。用时 45s。");
        assert_eq!(ZH_CN.step_first_open(12), "第一击安全，打开了 12 格。");
        assert_eq!(ZH_CN.step_opened(1), "打开了 1 格。");
        assert_eq!(ZH_CN.step_flags_mismatch(2, 1), "周围要插满 2 面旗子才能 chord，现在只有 1 面。");
        assert_eq!(ZH_CN.step_rank(3, 14), "成绩已计入排行榜：当前排名 #3 / 14 人。");
        assert_eq!(ZH_CN.step_new_best("困难"), "困难新纪录！");
        assert_eq!(ZH_CN.step_tnt(), "轰！你站立处出现了一个引燃的 TNT。");
        assert_eq!(ZH_CN.rank_hard("-"), "  困难 -");
        assert_eq!(ZH_CN.me_best_label("中等"), "中等最快");
        assert_eq!(ZH_CN.bad_cell("zz"), "看不懂的坐标「zz」。");
        assert_eq!(ZH_CN.cell_hover_number("c5", 3), "c5 · 周围 3 雷");
        assert_eq!(ZH_CN.start_abandoned("中等", 12, 12, 25),
            "新的一局开始：中等 12×12·25雷。上一局中途放弃，已计入排行榜。");
        assert_eq!(ZH_CN.diff_button("困难", 16, 16, 50), "困难 16×16·50雷");
        assert_eq!(
            ZH_CN.player_only(),
            "SweeperBoom 是玩家专属的小游戏，请在游戏内由玩家执行该命令。"
        );

        // 英文侧同样钉几条，确认不是机械替换。
        let status_en = format!(
            "{}{}",
            EN_US.status_title(),
            EN_US.status_size("Medium", 12, 12, 25),
        );
        assert_eq!(status_en, "SweeperBoom Medium 12×12, 25 mines");
        assert_eq!(EN_US.info_won("1m23s"), "Cleared! Time 1m23s");
        assert_eq!(EN_US.step_first_open(12), "Safe first click - opened 12 cells.");
        assert_eq!(EN_US.rank_hard("-"), "  Hard -");
        assert_eq!(EN_US.me_best_label("Medium"), "Best Medium");
        assert_eq!(EN_US.step_tnt(), "Boom! A primed TNT appeared where you stood.");
        assert_eq!(
            EN_US.player_only(),
            "SweeperBoom is a player-only mini-game; run this command in game as a player."
        );
    }

    #[test]
    fn command_literals_are_not_localised() {
        // 命令名是标识符，不能被本地化；难度与模式的命令键同理。
        assert_eq!(Difficulty::Easy.key(), "easy");
        assert_eq!(Difficulty::Medium.key(), "medium");
        assert_eq!(Difficulty::Hard.key(), "hard");
        assert_eq!(Mode::Open.key(), "open");
        assert_eq!(Mode::Flag.key(), "flag");
        for command in HELP_COMMANDS {
            assert!(command.starts_with("sweeperboom "), "{command:?}");
        }
    }

    #[test]
    fn templates_actually_interpolate_their_arguments() {
        // `format!` 会检查占位符与参数是否匹配，但「参数写对了却忘了放进模板」
        // 编译期查不出来。这里用哨兵值逐个确认每个参数都真的出现在输出里。
        for lang in [Lang::ZhCn, Lang::EnUs] {
            let s = lang.strings();
            let word = "ZZZZ";

            let cases: Vec<(&str, String, Vec<&str>)> = vec![
                ("button_hint", s.button_hint(word), vec![word]),
                (
                    "status_size",
                    s.status_size(word, 7111, 7222, 7333),
                    vec![word, "7111", "7222", "7333"],
                ),
                ("status_remaining", s.status_remaining(7444), vec!["7444"]),
                (
                    "status_progress",
                    s.status_progress(7555, 7666),
                    vec!["7555", "7666"],
                ),
                ("status_moves", s.status_moves(7777), vec!["7777"]),
                ("status_rank", s.status_rank(7888), vec!["7888"]),
                ("status_mode", s.status_mode(word), vec![word]),
                ("info_time", s.info_time(7999), vec!["7999"]),
                ("info_won", s.info_won(word), vec![word]),
                ("info_lost", s.info_lost(word), vec![word]),
                ("cell_hover_open", s.cell_hover_open(word), vec![word]),
                ("cell_hover_flag", s.cell_hover_flag(word), vec![word]),
                ("cell_hover_unflag", s.cell_hover_unflag(word), vec![word]),
                (
                    "cell_hover_number",
                    s.cell_hover_number(word, 1),
                    vec![word],
                ),
                ("cell_hover_locked", s.cell_hover_locked(word), vec![word]),
                ("ctl_mode", s.ctl_mode(word), vec![word]),
                ("mode_changed", s.mode_changed(word), vec![word]),
                (
                    "mode_action",
                    s.mode_action(Mode::Flag).to_string(),
                    vec![],
                ),
                ("mode_label", s.mode_label(Mode::Open).to_string(), vec![]),
                ("diff_label", s.diff_label(Difficulty::Easy).to_string(), vec![]),
                (
                    "diff_button",
                    s.diff_button(word, 8111, 8222, 8333),
                    vec![word, "8111", "8222", "8333"],
                ),
                ("rank_count", s.rank_count(8444), vec!["8444"]),
                ("rank_wins", s.rank_wins(8555), vec!["8555"]),
                ("rank_losses", s.rank_losses(8666), vec!["8666"]),
                ("rank_hard", s.rank_hard(word), vec![word]),
                ("me_rank_value", s.me_rank_value(8777, 8888), vec!["8777", "8888"]),
                ("me_best_label", s.me_best_label(word), vec![word]),
                ("bad_cell", s.bad_cell(word), vec![word]),
                ("bad_cell_hint", s.bad_cell_hint('p', 8999), vec!["p", "8999"]),
                (
                    "step_flags_mismatch",
                    s.step_flags_mismatch(2, 1),
                    vec!["2", "1"],
                ),
                ("step_first_open", s.step_first_open(9111), vec!["9111"]),
                ("step_opened", s.step_opened(9222), vec!["9222"]),
                ("step_chorded", s.step_chorded(9333), vec!["9333"]),
                ("step_hit_mine", s.step_hit_mine(word), vec![word]),
                ("step_won", s.step_won(word), vec![word]),
                ("step_rank", s.step_rank(9444, 9555), vec!["9444", "9555"]),
                (
                    "step_rank_short",
                    s.step_rank_short(9666, 9777),
                    vec!["9666", "9777"],
                ),
                ("step_new_best", s.step_new_best(word), vec![word]),
                (
                    "start_abandoned",
                    s.start_abandoned(word, 9888, 9999, 1234),
                    vec![word, "9888", "9999", "1234"],
                ),
                ("stop_summary", s.stop_summary(2345, word), vec!["2345", word]),
                // format_seconds 会把秒数折算成分与秒，因此分开校验两个分支。
                ("format_seconds", s.format_seconds(34), vec!["34"]),
                ("format_seconds", s.format_seconds(3456), vec!["57", "36"]),
            ];

            for (name, output, expected) in cases {
                for needle in expected {
                    assert!(
                        output.contains(needle),
                        "{lang:?} 的 {name} 丢掉了参数 {needle}：{output:?}"
                    );
                }
                assert!(
                    !output.contains('{'),
                    "{lang:?} 的 {name} 留下了未替换的占位符：{output:?}"
                );
            }
        }
    }

    #[test]
    fn help_table_is_aligned_with_commands() {
        // HELP_COMMANDS 与每种语言的描述必须一一对应，否则 zip 会静默截断。
        assert_eq!(HELP_COMMANDS.len(), HELP_COUNT);
        for lang in [Lang::ZhCn, Lang::EnUs] {
            let descriptions = lang.strings().help_descriptions();
            assert_eq!(descriptions.len(), HELP_COUNT);

            let joined: Vec<(&str, &str)> = HELP_COMMANDS
                .iter()
                .zip(descriptions.iter())
                .map(|(command, description)| (*command, *description))
                .collect();
            assert_eq!(joined.len(), HELP_COUNT);
            for (command, description) in joined {
                assert!(command.starts_with("sweeperboom "), "{command:?}");
                assert!(!description.trim().is_empty(), "{command:?} 没有描述");
            }
        }
    }

    #[test]
    fn time_keys_sort_missing_records_last() {
        assert!(time_key(0) > time_key(u32::MAX - 1));
        assert!(time_key(30) < time_key(31));
    }
}
