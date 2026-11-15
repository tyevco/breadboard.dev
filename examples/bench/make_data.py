#!/usr/bin/env python3
"""The data of examples/bench's benches (milestone B1, ADR-010 D3), written
deterministically (integer arithmetic, fixed decimal text).

    python3 examples/bench/make_data.py              # rewrite the committed files
    python3 examples/bench/make_data.py --check      # require them byte for byte
    python3 examples/bench/make_data.py --long DIR   # write DIR/long.csv (not committed)

Committed (data/ and bench/):
- data/ain.csv: 601 points, a 0-2 V triangle of period 20 ms with a 0.5 V
  step at 150 ms, over 300 ms (the playback of bench/stimulus.toml), as
  canonical CSV (t in seconds, values in volts);
- bench/playback-pwl.toml: the same 601 points as an inline [[pwl]] (ADR-010
  G2: a playback against the same data as a short [[pwl]]);
- data/burst.csv: a current profile, 20 mA bursts of 5 ms every 40 ms from
  30 ms (hold).
Generated for the playback cost gate (plan B1: 1 s at 100 ksps):
- long.csv: 100 001 points every 10 us over 1 s, the triangle of ain.csv.
"""
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent


def fs_text(fs):
    """Seconds as a plain decimal string from whole femtoseconds."""
    s = f"{fs // 10**15}.{fs % 10**15:015d}".rstrip("0").rstrip(".")
    return s or "0"


def uv_text(uv):
    s = f"{uv // 10**6}.{uv % 10**6:06d}".rstrip("0").rstrip(".")
    return s or "0"


def triangle_uv(t_fs, step=True):
    """0..2 V triangle, period 20 ms; +0.5 V from 150 ms (the step)."""
    period = 20 * 10**12
    ph = t_fs % period
    half = period // 2
    uv = (2 * 10**6 * ph) // half if ph < half else (2 * 10**6 * (period - ph)) // half
    if step and t_fs >= 150 * 10**12:
        uv += 500_000
    return uv


def ain_points():
    return [(k * 500 * 10**9, triangle_uv(k * 500 * 10**9)) for k in range(601)]


def csv(name, points, value=uv_text):
    return f"t,{name}\n" + "".join(f"{fs_text(t)},{value(v)}\n" for t, v in points)


def burst_points():
    pts = [(0, 0)]
    for k in range(7):
        t0 = (30 + 40 * k) * 10**12
        pts.append((t0, 20_000))  # uA
        pts.append((t0 + 5 * 10**12, 0))
    return pts


def amps_text(ua):
    return uv_text(ua)  # micro-units in, base units out


PWL_HEAD = """\
# examples/bench/bench/playback-pwl.toml: data/ain.csv as an inline [[pwl]] (written by
# make_data.py). ADR-010 gate G2: a playback (bench/playback-small.toml) against the same data as a
# short [[pwl]] (at most 1 000 points). Everything else as bench/playback-small.toml.
schema = "bb-bench/1"

[[supply]]
net = "+3V3"
volts = "3.3V"
rs = "1"

[[supply]]
net = "VSW"
volts = "3.3V"
rs = "10m"

[[pwl]]
net = "AIN"
points = [
"""

PWL_TAIL = """]

[options]
set = "phase0"
"""


def pwl_bench():
    rows = "".join(f'  ["{fs_text(t)}", "{uv_text(v)}V"],\n' for t, v in ain_points())
    return PWL_HEAD + rows + PWL_TAIL


def files():
    return {
        HERE / "data" / "ain.csv": csv("AIN", ain_points()),
        HERE / "data" / "burst.csv": csv("I", burst_points(), amps_text),
        HERE / "bench" / "playback-pwl.toml": pwl_bench(),
    }


def main():
    args = sys.argv[1:]
    if args[:1] == ["--long"]:
        out = pathlib.Path(args[1]) / "long.csv"
        pts = [(k * 10 * 10**9, triangle_uv(k * 10 * 10**9, step=False)) for k in range(100_001)]
        out.write_text(csv("AIN", pts))
        print(f"wrote {out} ({len(pts)} points)")
        return
    check = "--check" in args
    bad = 0
    for path, text in files().items():
        if check:
            if not path.is_file() or path.read_text() != text:
                print(f"make_data: {path.relative_to(HERE)} is not what make_data.py writes")
                bad += 1
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
            print(f"wrote {path.relative_to(HERE)}")
    if bad:
        sys.exit(1)


if __name__ == "__main__":
    main()
