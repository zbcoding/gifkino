#!/usr/bin/env python3
"""Regenerate the test GIF used for manual testing of the editor.

The fixture is committed (tests/fixtures/test-8-frames.gif), so this script
only needs rerunning after changing the recipe. Eight 160x120 frames at
100 ms, each showing its 1-based number on its own hue: moving, selecting,
delay-editing or drag-reordering frames is then visible at a glance, and a
misordered timeline reads instantly as "3 is where 2 was". Deterministic —
the same bytes on every run, so a regen shows up in git as either nothing
or a real change.

    scripts/make_test_gif.py           # rewrite tests/fixtures/test-8-frames.gif
    scripts/make_test_gif.py OUT       # write somewhere else
"""

import colorsys
import os
import sys

from PIL import Image, ImageDraw, ImageFont

FRAMES = 8
SIZE = (160, 120)
DELAY_MS = 100

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURE = os.path.join(ROOT, "tests", "fixtures", "test-8-frames.gif")


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else FIXTURE
    try:
        font = ImageFont.load_default(48)
    except TypeError:
        font = ImageFont.load_default()

    frames = []
    for i in range(FRAMES):
        hue = i / FRAMES
        r, g, b = (round(c * 255) for c in colorsys.hsv_to_rgb(hue, 0.55, 0.55))
        frame = Image.new("RGB", SIZE, (r, g, b))
        draw = ImageDraw.Draw(frame)
        draw.text(
            (SIZE[0] / 2, SIZE[1] / 2),
            str(i + 1),
            fill=(255, 255, 255),
            font=font,
            anchor="mm",
        )
        frames.append(frame)

    os.makedirs(os.path.dirname(out) or ".", exist_ok=True)
    frames[0].save(out, save_all=True, append_images=frames[1:], duration=DELAY_MS, loop=0)

    # Self-check: the bytes on disk must read back as what was intended.
    with Image.open(out) as check:
        assert check.n_frames == FRAMES, f"{check.n_frames} frames, wanted {FRAMES}"
        assert check.size == SIZE, f"{check.size}, wanted {SIZE}"
    print(f"{out}: {FRAMES} frames, {os.path.getsize(out)} bytes")


if __name__ == "__main__":
    main()
