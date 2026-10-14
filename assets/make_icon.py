#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成 SweeperBoom 插件图标（icon.svg 与 icon-<n>.png）。

只依赖标准库：自带的迷你光栅器把图形画在 3 倍超采样画布上再降采样，
用 zlib 手写 PNG，因此不依赖 rsvg-convert / Pillow / cairosvg。

配色取自 src/lib.rs 的 `number_color()`，保证图标与聊天栏里的棋盘观感一致。

用法：
    python assets/make_icon.py
"""

from __future__ import annotations

import pathlib
import struct
import zlib

HERE = pathlib.Path(__file__).resolve().parent

# --- Minecraft 命名颜色，与 src/lib.rs::number_color 一一对应 ---
NUMBER = {
    1: "#5555FF",
    2: "#55FF55",
    3: "#FF5555",
    4: "#FF55FF",
    5: "#FFAA00",
    6: "#55FFFF",
    7: "#FFFFFF",
    8: "#AAAAAA",
}
FLAG = "#FF5555"
BOARD_BG = "#1B1B21"
BOARD_EDGE = "#3A3A44"
HIDDEN = "#4A4A55"
HIDDEN_TOP = "#6E6E7C"
REVEALED = "#26262E"
PUMPKIN = "#FF8A3D"

# --- 5x7 点阵数字 ---
FONT = {
    "1": ["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."],
    "2": [".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"],
    "3": ["####.", "....#", "....#", ".###.", "....#", "....#", "####."],
    "4": ["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."],
    "5": ["#####", "#....", "####.", "....#", "....#", "#...#", ".###."],
    "6": ["..##.", ".#...", "#....", "####.", "#...#", "#...#", ".###."],
    "7": ["#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#..."],
    "8": [".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."],
}

# --- 5x5 盘面：H 隐藏 / F 插旗 / 数字 ---
BOARD = [
    "HHHHH",
    "H112H",
    "H1F2H",
    "H112H",
    "HHHHH",
]

SS = 3  # 超采样倍数
SIZES = [64, 128, 256, 512]


# ---------------------------------------------------------------------------
# 迷你光栅器
# ---------------------------------------------------------------------------


class Canvas:
    def __init__(self, size: int) -> None:
        self.size = size
        self.px = bytearray(size * size * 4)

    def blend(self, x: int, y: int, color: tuple[int, int, int], alpha: float = 1.0) -> None:
        if x < 0 or y < 0 or x >= self.size or y >= self.size or alpha <= 0.0:
            return
        i = (y * self.size + x) * 4
        buf = self.px
        sa = min(1.0, alpha)
        da = buf[i + 3] / 255.0
        out_a = sa + da * (1.0 - sa)
        if out_a <= 0.0:
            return
        for c in range(3):
            src = color[c]
            dst = buf[i + c]
            buf[i + c] = int(round((src * sa + dst * da * (1.0 - sa)) / out_a))
        buf[i + 3] = int(round(out_a * 255))

    def rounded_rect(self, x0, y0, x1, y1, radius, color, alpha=1.0) -> None:
        r = max(0.0, min(radius, (x1 - x0) / 2.0, (y1 - y0) / 2.0))
        for y in range(int(y0), int(y1) + 1):
            cy = min(max(y + 0.5, y0 + r), y1 - r)
            for x in range(int(x0), int(x1) + 1):
                cx = min(max(x + 0.5, x0 + r), x1 - r)
                if (x + 0.5 - cx) ** 2 + (y + 0.5 - cy) ** 2 <= r * r + 1e-6:
                    self.blend(x, y, color, alpha)

    def circle(self, cx, cy, radius, color, alpha=1.0) -> None:
        x0, x1 = int(cx - radius) - 1, int(cx + radius) + 1
        y0, y1 = int(cy - radius) - 1, int(cy + radius) + 1
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                if (x + 0.5 - cx) ** 2 + (y + 0.5 - cy) ** 2 <= radius * radius:
                    self.blend(x, y, color, alpha)

    def triangle(self, p0, p1, p2, color, alpha=1.0) -> None:
        xs = [p0[0], p1[0], p2[0]]
        ys = [p0[1], p1[1], p2[1]]
        for y in range(int(min(ys)), int(max(ys)) + 1):
            for x in range(int(min(xs)), int(max(xs)) + 1):
                px, py = x + 0.5, y + 0.5
                d = []
                neg = 0
                for (ax, ay), (bx, by) in ((p0, p1), (p1, p2), (p2, p0)):
                    cross = (bx - ax) * (py - ay) - (by - ay) * (px - ax)
                    d.append(cross)
                    if cross < 0:
                        neg += 1
                if neg in (0, 3):
                    self.blend(x, y, color, alpha)


def rgb(hex_color: str) -> tuple[int, int, int]:
    h = hex_color.lstrip("#")
    return tuple(int(h[i : i + 2], 16) for i in (0, 2, 4))  # type: ignore[return-value]


def render(size: int) -> bytes:
    """渲染一张 size×size 的 RGBA 图像。"""
    big = size * SS
    canvas = Canvas(big)
    s = big / 512.0

    def u(v: float) -> float:
        return v * s

    # 背板
    canvas.rounded_rect(u(8), u(8), u(504), u(504), u(72), rgb(BOARD_BG))
    canvas.rounded_rect(u(8), u(8), u(504), u(504), u(72), rgb(PUMPKIN), 0.0)

    # 网格
    cols = len(BOARD[0])
    rows = len(BOARD)
    grid = u(336)
    gap = u(18)
    cell = (grid - gap * (cols - 1)) / cols
    left = u(88)
    top = u(112)

    for ry, row in enumerate(BOARD):
        for rx, ch in enumerate(row):
            x0 = left + rx * (cell + gap)
            y0 = top + ry * (cell + gap)
            x1, y1 = x0 + cell, y0 + cell

            if ch == "H":
                canvas.rounded_rect(x0, y0, x1, y1, u(14), rgb(HIDDEN))
                canvas.rounded_rect(x0, y0, x1, y0 + u(10), u(14), rgb(HIDDEN_TOP))
                canvas.rounded_rect(x0, y0, x1, y1, u(14), rgb(BOARD_EDGE), 0.0)
            else:
                canvas.rounded_rect(x0, y0, x1, y1, u(14), rgb(REVEALED))

            if ch == "F":
                # 旗杆 + 旗面
                pole_x = x0 + cell * 0.44
                canvas.rounded_rect(pole_x, y0 + cell * 0.22, pole_x + u(9), y0 + cell * 0.78, u(4), rgb("#D8D8E0"))
                canvas.triangle(
                    (pole_x + u(9), y0 + cell * 0.24),
                    (pole_x + u(9), y0 + cell * 0.5),
                    (x0 + cell * 0.78, y0 + cell * 0.37),
                    rgb(FLAG),
                )
                canvas.rounded_rect(
                    x0 + cell * 0.3, y0 + cell * 0.76, x0 + cell * 0.7, y0 + cell * 0.84, u(4), rgb("#D8D8E0")
                )
            elif ch.isdigit():
                draw_digit(canvas, ch, x0, y0, cell, u)

    return downsample(canvas, SS)


def draw_digit(canvas: Canvas, ch: str, x0: float, y0: float, cell: float, u) -> None:
    glyph = FONT[ch]
    gw, gh = len(glyph[0]), len(glyph)
    dot = cell * 0.13
    total_w = gw * dot
    total_h = gh * dot
    ox = x0 + (cell - total_w) / 2
    oy = y0 + (cell - total_h) / 2
    color = rgb(NUMBER[int(ch)])
    for ry, line in enumerate(glyph):
        for rx, mark in enumerate(line):
            if mark != "#":
                continue
            canvas.rounded_rect(
                ox + rx * dot,
                oy + ry * dot,
                ox + (rx + 1) * dot,
                oy + (ry + 1) * dot,
                dot * 0.28,
                color,
            )
    _ = u


def downsample(canvas: Canvas, factor: int) -> bytes:
    size = canvas.size // factor
    out = bytearray(size * size * 4)
    src = canvas.px
    n = factor * factor
    for y in range(size):
        for x in range(size):
            r = g = b = a = 0
            for dy in range(factor):
                base = ((y * factor + dy) * canvas.size + x * factor) * 4
                for dx in range(factor):
                    i = base + dx * 4
                    r += src[i]
                    g += src[i + 1]
                    b += src[i + 2]
                    a += src[i + 3]
            o = (y * size + x) * 4
            out[o] = r // n
            out[o + 1] = g // n
            out[o + 2] = b // n
            out[o + 3] = a // n
    return bytes(out)


def write_png(path: pathlib.Path, size: int, pixels: bytes) -> None:
    raw = bytearray()
    stride = size * 4
    for y in range(size):
        raw.append(0)  # filter: none
        raw += pixels[y * stride : (y + 1) * stride]

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", ihdr)
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    path.write_bytes(png)


SVG = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
  <title>sweeperboom</title>
  <rect x="8" y="8" width="496" height="496" rx="72" fill="{board_bg}"/>
{grid}
</svg>
"""


def svg_cell(rx: int, ry: int, cell: float, gap: float, ch: str) -> str:
    x = 88 + rx * (cell + gap)
    y = 112 + ry * (cell + gap)
    parts = []
    if ch == "H":
        parts.append(f'  <rect x="{x:.1f}" y="{y:.1f}" width="{cell:.1f}" height="{cell:.1f}" rx="14" fill="{HIDDEN}"/>')
        parts.append(
            f'  <rect x="{x:.1f}" y="{y:.1f}" width="{cell:.1f}" height="10" rx="5" fill="{HIDDEN_TOP}"/>'
        )
    else:
        parts.append(
            f'  <rect x="{x:.1f}" y="{y:.1f}" width="{cell:.1f}" height="{cell:.1f}" rx="14" fill="{REVEALED}"/>'
        )
        if ch == "F":
            pole = x + cell * 0.44
            parts.append(
                f'  <rect x="{pole:.1f}" y="{y + cell * 0.22:.1f}" width="9" height="{cell * 0.56:.1f}" rx="4" fill="#D8D8E0"/>'
            )
            parts.append(
                f'  <polygon points="{pole + 9:.1f},{y + cell * 0.24:.1f} {pole + 9:.1f},{y + cell * 0.50:.1f} '
                f'{x + cell * 0.78:.1f},{y + cell * 0.37:.1f}" fill="{FLAG}"/>'
            )
            parts.append(
                f'  <rect x="{x + cell * 0.3:.1f}" y="{y + cell * 0.76:.1f}" width="{cell * 0.4:.1f}" height="8" rx="4" fill="#D8D8E0"/>'
            )
        elif ch.isdigit():
            parts.append(
                f'  <text x="{x + cell / 2:.1f}" y="{y + cell * 0.68:.1f}" fill="{NUMBER[int(ch)]}" '
                f'font-family="monospace" font-size="{cell * 0.62:.1f}" font-weight="bold" '
                f'text-anchor="middle">{ch}</text>'
            )
    return "\n".join(parts)


def main() -> None:
    cols, rows = len(BOARD[0]), len(BOARD)
    gap = 18.0
    cell = (336.0 - gap * (cols - 1)) / cols
    grid = "\n".join(svg_cell(rx, ry, cell, gap, ch) for ry, row in enumerate(BOARD) for rx, ch in enumerate(row))

    svg_path = HERE / "icon.svg"
    svg_path.write_text(
        SVG.format(board_bg=BOARD_BG, grid=grid),
        encoding="utf-8",
    )
    print(f"已写出 {svg_path}")

    for size in SIZES:
        path = HERE / f"icon-{size}.png"
        write_png(path, size, render(size))
        print(f"已写出 {path} ({path.stat().st_size} 字节)")


if __name__ == "__main__":
    main()
