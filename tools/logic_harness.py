#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""对 sweeperboom 的纯逻辑做本机单元测试。

这个 crate 的 `crate-type` 是 `cdylib`，且 `.cargo/config.toml` 把默认构建目标钉在了
`wasm32-wasip2`，因此 `cargo test` 无法产出可以在本机运行的可执行文件。

于是这里换一条路：把 `src/lib.rs` 中标记为 PURE-LOGIC 的区段（不含任何
`pumpkin_plugin_api` 依赖）抽出来，拼上 `tools/logic_tests.rs`，用本机 rustc 编译成
一个测试二进制并执行。抽取是自动完成的，所以测试永远针对当前的源码，不会与
插件实现脱节。

用法：
    python tools/logic_harness.py
"""

from __future__ import annotations

import pathlib
import subprocess
import sys
import tempfile

BEGIN = "// >>> PURE-LOGIC-BEGIN"
END = "// <<< PURE-LOGIC-END"

ROOT = pathlib.Path(__file__).resolve().parent.parent
LIB = ROOT / "src" / "lib.rs"
TESTS = ROOT / "tools" / "logic_tests.rs"

PREAMBLE = """\
// 由 tools/logic_harness.py 自动生成，请勿手工编辑。
#![allow(dead_code, unused_variables, unused_mut, unused_imports)]
"""


def extract_pure_logic(source: str) -> str:
    """截取两个标记之间的内容。"""
    start = source.find(BEGIN)
    end = source.find(END)
    if start == -1 or end == -1:
        raise SystemExit(f"在 {LIB} 中找不到 PURE-LOGIC 标记")
    if end < start:
        raise SystemExit("PURE-LOGIC 标记顺序颠倒")
    return source[start + len(BEGIN) : end]


def main() -> int:
    source = LIB.read_text(encoding="utf-8")
    body = extract_pure_logic(source)
    tests = TESTS.read_text(encoding="utf-8")

    with tempfile.TemporaryDirectory(prefix="sweeperboom-logic-") as tmp:
        tmpdir = pathlib.Path(tmp)
        harness = tmpdir / "logic_harness.rs"
        binary = tmpdir / ("logic_harness.exe" if sys.platform == "win32" else "logic_harness")
        harness.write_text(PREAMBLE + body + "\n" + tests, encoding="utf-8")

        print(f"抽出 {len(body.splitlines())} 行纯逻辑，编译测试二进制…")
        build = subprocess.run(
            ["rustc", "--edition", "2024", "--test", "-O", str(harness), "-o", str(binary)],
            capture_output=True,
            text=True,
        )
        if build.returncode != 0:
            print(build.stdout)
            print(build.stderr, file=sys.stderr)
            return build.returncode

        run = subprocess.run([str(binary)], capture_output=True, text=True)
        print(run.stdout, end="")
        if run.stderr:
            print(run.stderr, end="", file=sys.stderr)
        return run.returncode


if __name__ == "__main__":
    raise SystemExit(main())
