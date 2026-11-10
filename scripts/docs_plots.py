#!/usr/bin/env python3
"""The docs site's waveform plots (site/public/plots), drawn from real `bb run` outputs.

    python3 scripts/docs_plots.py <runs> <svgdir>

<runs> is the output directory of scripts/docs_plots.sh (`make docs-plots`), which ran:

  <runs>/demo/rail, demo/rail-wrong   scripts/demo.sh (make demo): E1 on the cold bench, led_rail.elf
                                      and led_rail_wrong_order.elf, 120 ms, --probe net:VLED
                                      --probe net:LED_DIN
  <runs>/vsense/{sag,rec,s50,s50-none}  scripts/dms_vsense.sh run: the E2 cases of make dms-vsense at
                                      bb's defaults
  <runs>/spike                        the Phase 0 spike, make spike's arguments (SPIKE_ARGS)

Every figure is written twice, <name>.light.svg and <name>.dark.svg (the site shows one per theme).
Plain Python, no plotting library: the SVG text is a pure function of the run outputs (fixed
number formatting, no timestamps, no generated ids), so `make docs-plots-check` can regenerate the
figures and compare them byte for byte with the committed ones. Long traces are thinned with
Ramer-Douglas-Peucker in pixel space (0.15 px), which keeps every visible feature.

Colors follow the dataviz reference palette (categorical slots in fixed order, validated for both
modes; status critical only for the LSW-3 violation wash, always with a text label).
"""
import csv
import math
import json
import os
import sys
from xml.sax.saxutils import escape

FS_PER_MS = 1_000_000_000_000
FONT = "system-ui, -apple-system, &quot;Segoe UI&quot;, sans-serif"

THEMES = {
    "light": dict(surface="#fcfcfb", ink="#0b0b0b", ink2="#52514e", muted="#898781", grid="#e1e0d9",
                  axis="#c3c2b7", border="rgba(11,11,11,0.10)", critical="#d03b3b",
                  s=["#2a78d6", "#eb6834", "#1baf7a"]),
    "dark": dict(surface="#1a1a19", ink="#f0efec", ink2="#c3c2b7", muted="#898781", grid="#2c2c2a",
                 axis="#383835", border="rgba(255,255,255,0.10)", critical="#d03b3b",
                 s=["#3987e5", "#d95926", "#199e70"]),
}


def n(x, d=1):
    """Fixed-point text for a coordinate (never '-0.0')."""
    s = f"{x:.{d}f}"
    return s[1:] if s.startswith("-") and float(s) == 0 else s


# --- run outputs -------------------------------------------------------------------------------

def read_csv(path, cols):
    with open(path, newline="") as f:
        r = csv.reader(f)
        head = next(r)
        idx = [head.index(c) for c in cols]
        t, vals = [], [[] for _ in cols]
        for row in r:
            t.append(int(row[0]) / FS_PER_MS)
            for v, i in zip(vals, idx):
                v.append(float(row[i]))
    return t, dict(zip(cols, vals))


def records(path):
    with open(path) as f:
        return [json.loads(line) for line in f if '"type":"grant"' not in line]


def edges(recs, net):
    return [(r["t_fs"] / FS_PER_MS, r["level"]) for r in recs if r["type"] == "pin_edge" and r["net"] == net]


def readings(recs):
    out = []
    for r in recs:
        m = r.get("msg", "")
        if r["type"] == "log" and m.startswith("uart0: vsense: n="):
            f = dict(kv.split("=") for kv in m.split()[2:] if "=" in kv)
            out.append(dict(t=r["t_fs"] / FS_PER_MS, n=int(f["n"]), code=int(f["code"]),
                            est=int(f["vbat_est"]) / 1e6, leds=f["leds"]))
    return out


def samples(recs):
    out = []
    for r in recs:
        m = r.get("msg", "")
        if r["type"] == "log" and m.startswith("saadc sample: P0.02"):
            out.append((r["t_fs"], int(m.split(") ")[1].split(" uV")[0]), int(m.rsplit("code ", 1)[1])))
    return out


# --- drawing -----------------------------------------------------------------------------------

def rdp(pts, eps):
    """Ramer-Douglas-Peucker, iterative; pts in pixels."""
    if len(pts) < 3:
        return pts
    keep = [False] * len(pts)
    keep[0] = keep[-1] = True
    stack = [(0, len(pts) - 1)]
    while stack:
        a, b = stack.pop()
        (x1, y1), (x2, y2) = pts[a], pts[b]
        dx, dy = x2 - x1, y2 - y1
        L2 = dx * dx + dy * dy
        best, bi = -1.0, -1
        for i in range(a + 1, b):
            px, py = pts[i]
            if L2 == 0:
                d = (px - x1) ** 2 + (py - y1) ** 2
            else:
                c = abs(dy * px - dx * py + x2 * y1 - y2 * x1)
                d = c * c / L2
            if d > best:
                best, bi = d, i
        if bi > 0 and best > eps * eps:
            keep[bi] = True
            stack.append((a, bi))
            stack.append((bi, b))
    return [p for p, k in zip(pts, keep) if k]


def nice_ticks(lo, hi, want=5):
    span = hi - lo
    raw = span / want
    mag = 10 ** math.floor(math.log10(raw))
    step = next(m * mag for m in (1, 2, 2.5, 5, 10) if m * mag >= raw)
    first = math.ceil(lo / step - 1e-9) * step
    ticks, k = [], 0
    while first + k * step <= hi + 1e-9 * span:
        ticks.append(first + k * step)
        k += 1
    dec = max(0, -int(math.floor(math.log10(step) + 1e-9)))
    if step * 10 ** dec != round(step * 10 ** dec):
        dec += 1
    return ticks, dec


class Svg:
    def __init__(self, theme, width):
        self.th = THEMES[theme]
        self.w = width
        self.body = []
        self.clips = 0

    def add(self, s):
        self.body.append(s)

    def text(self, x, y, s, size=12, fill="ink2", anchor="start", weight=None, rotate=None):
        a = f' text-anchor="{anchor}"' if anchor != "start" else ""
        wt = f' font-weight="{weight}"' if weight else ""
        rot = f' transform="rotate({rotate} {n(x)} {n(y)})"' if rotate is not None else ""
        self.add(f'<text x="{n(x)}" y="{n(y)}" font-size="{size}" fill="{self.th[fill]}"{a}{wt}{rot}>'
                 f'{escape(s)}</text>')

    def line(self, x1, y1, x2, y2, stroke, width=1, extra=""):
        self.add(f'<line x1="{n(x1)}" y1="{n(y1)}" x2="{n(x2)}" y2="{n(y2)}" stroke="{stroke}" '
                 f'stroke-width="{width}"{extra}/>')

    def legend(self, x, y, items):
        """items: (label, color, kind) with kind 'line' or 'dot' or 'tick'; returns the end x."""
        for label, color, kind in items:
            if kind == "line":
                self.line(x, y - 4, x + 18, y - 4, color, 2, ' stroke-linecap="round"')
                tx = x + 24
            elif kind == "dot":
                self.add(f'<circle cx="{n(x + 5)}" cy="{n(y - 4)}" r="4" fill="{color}"/>')
                tx = x + 14
            elif kind == "wash":
                self.add(f'<rect x="{n(x)}" y="{n(y - 10)}" width="14" height="12" rx="2" fill="{color}" '
                         f'fill-opacity="0.22"/>')
                tx = x + 20
            else:  # a thin reference line
                self.line(x, y - 4, x + 18, y - 4, color, 1)
                tx = x + 24
            self.text(tx, y, label, 12, "ink2")
            x = tx + 7.0 * len(label) + 18
        return x

    def render(self, height, title):
        th = self.th
        return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{self.w}" height="{n(height, 0)}" '
                f'viewBox="0 0 {self.w} {n(height, 0)}" font-family="{FONT}" role="img">\n'
                f'<title>{escape(title)}</title>\n'
                f'<rect x="0.5" y="0.5" width="{self.w - 1}" height="{n(height - 1, 0)}" rx="8" '
                f'fill="{th["surface"]}" stroke="{th["border"]}"/>\n'
                + "\n".join(self.body) + "\n</svg>\n")


class Panel:
    def __init__(self, svg, x, y, w, h, xr, yr, title=None, xlabel=None, ylabel=None, xticks=None,
                 yticks=None):
        self.s, self.x, self.y, self.w, self.h = svg, x, y, w, h
        self.xr, self.yr = xr, yr
        th = svg.th
        svg.clips += 1
        self.clip = f"p{svg.clips}"
        svg.add(f'<clipPath id="{self.clip}"><rect x="{n(x)}" y="{n(y)}" width="{n(w)}" height="{n(h)}"/></clipPath>')
        if title:
            svg.text(x, y - 10, title, 13, "ink", weight="600")
        yt, yd = yticks if yticks else nice_ticks(*yr, 4)
        for v in yt:
            py = self.py(v)
            svg.line(x, py, x + w, py, th["grid"], 1)
            svg.text(x - 8, py + 4, f"{v:.{yd}f}", 11, "muted", "end")
        xt, xd = xticks if xticks else nice_ticks(*xr, 6)
        svg.line(x, y + h, x + w, y + h, th["axis"], 1)
        for v in xt:
            px = self.px(v)
            svg.line(px, y + h, px, y + h + 4, th["axis"], 1)
            svg.text(px, y + h + 17, f"{v:.{xd}f}", 11, "muted", "middle")
        if xlabel:
            svg.text(x + w / 2, y + h + 34, xlabel, 12, "ink2", "middle")
        if ylabel:
            svg.text(x - 46, y + h / 2, ylabel, 12, "ink2", "middle", rotate=-90)

    def px(self, v):
        return self.x + (v - self.xr[0]) / (self.xr[1] - self.xr[0]) * self.w

    def py(self, v):
        return self.y + self.h - (v - self.yr[0]) / (self.yr[1] - self.yr[0]) * self.h

    def series(self, t, v, color, width=2, eps=0.15):
        pts = [(self.px(a), self.py(b)) for a, b in zip(t, v) if self.xr[0] <= a <= self.xr[1]]
        pts = rdp(pts, eps)
        d = "M" + " L".join(f"{n(px)} {n(py)}" for px, py in pts)
        self.s.add(f'<path d="{d}" fill="none" stroke="{color}" stroke-width="{width}" '
                   f'stroke-linejoin="round" stroke-linecap="round" clip-path="url(#{self.clip})"/>')

    def vmark(self, t, label, anchor="start", row=0):
        """An event: a hairline over the plot, its label at the top."""
        th = self.s.th
        px = self.px(t)
        self.s.line(px, self.y, px, self.y + self.h, th["muted"], 1)
        if anchor == "auto":
            anchor = "end" if px > self.x + self.w - 8 * len(label) - 8 else "start"
        dx = 4 if anchor == "start" else -4
        self.s.text(px + dx, self.y + 11 + 13 * row, label, 11, "ink2", anchor)

    def hline(self, v, label, color=None, anchor="end", above=True):
        th = self.s.th
        py = self.py(v)
        self.s.line(self.x, py, self.x + self.w, py, color or th["muted"], 1)
        tx = self.x + self.w - 4 if anchor == "end" else self.x + 4
        self.s.text(tx, py - 5 if above else py + 13, label, 11, "ink2", anchor)

    def dot(self, t, v, color, r=4):
        self.s.add(f'<circle cx="{n(self.px(t))}" cy="{n(self.py(v))}" r="{r}" fill="{color}" '
                   f'stroke="{self.s.th["surface"]}" stroke-width="2"/>')

    def label(self, t, v, s, dy=-8, anchor="start", fill="ink2", dx=0):
        self.s.text(self.px(t) + dx, self.py(v) + dy, s, 12, fill, anchor)

    def bracket_h(self, t1, t2, v, label):
        th = self.s.th
        x1, x2, py = self.px(t1), self.px(t2), self.py(v)
        self.s.line(x1, py, x2, py, th["ink2"], 1)
        self.s.line(x1, py - 4, x1, py + 4, th["ink2"], 1)
        self.s.line(x2, py - 4, x2, py + 4, th["ink2"], 1)
        self.s.text((x1 + x2) / 2, py - 6, label, 11, "ink2", "middle")

    def bracket_v(self, t, v1, v2, label, anchor="start"):
        th = self.s.th
        px, y1, y2 = self.px(t), self.py(v1), self.py(v2)
        self.s.line(px, y1, px, y2, th["ink"], 1)
        self.s.line(px - 4, y1, px + 4, y1, th["ink"], 1)
        self.s.line(px - 4, y2, px + 4, y2, th["ink"], 1)
        dx = 7 if anchor == "start" else -7
        self.s.text(px + dx, (y1 + y2) / 2 + 4, label, 12, "ink", anchor, weight="600")


def at(t, v, x):
    """The sample at or just before time x."""
    lo, hi = 0, len(t) - 1
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if t[mid] <= x:
            lo = mid
        else:
            hi = mid - 1
    return v[lo]


# --- figures -----------------------------------------------------------------------------------

DIN = "/LED ring/LED_DIN"
W, PL, PR = 760, 72, 24


def fig_e1(theme, runs):
    ok_t, ok = read_csv(f"{runs}/demo/rail/nets.csv", ["VLED", DIN])
    wr_t, wr = read_csv(f"{runs}/demo/rail-wrong/nets.csv", ["VLED", DIN])
    ok_r, wr_r = records(f"{runs}/demo/rail/events.jsonl"), records(f"{runs}/demo/rail-wrong/events.jsonl")
    s = Svg(theme, W)
    c = s.th["s"]
    s.legend(PL, 26, [("VLED (LED rail)", c[0], "line"), ("LED_DIN (first LED's data input)", c[1], "line"),
                      ("LED_PWR_EN edge (P1.04)", s.th["muted"], "ref")])
    pw = W - PL - PR
    ph = 150
    y0 = 70
    panels = []
    for i, (name, t, v, recs) in enumerate([
            ("Correct order: led_rail.elf (all four checks pass)", ok_t, ok, ok_r),
            ("Wrong order: led_rail_wrong_order.elf (LSW-3 fails)", wr_t, wr, wr_r)]):
        p = Panel(s, PL, y0 + i * (ph + 64), pw, ph, (0, 120), (0, 3.5), title=name,
                  xlabel="Time (ms)" if i == 1 else None, ylabel="Voltage (V)",
                  yticks=([0, 1, 2, 3], 0))
        en = edges(recs, "U10.P1.04")
        din = edges(recs, "U10.P0.06")
        for k, (te, lvl) in enumerate(en):
            p.vmark(te, ("EN ↑" if lvl else "EN ↓"), "auto")
        p.series(t, v["VLED"], c[0])
        p.series(t, v[DIN], c[1])
        # The firmware's wait: P0.06 rises 30 ms after LED_PWR_EN.
        t_en, t_din = en[0][0], din[0][0]
        p.bracket_h(t_en, t_din, 1.6, f"P0.06 ↑ {t_din - t_en:.2f} ms after EN ↑")
        p.label(16, 2.97, "VLED", -8, "middle")
        p.label(16, 0, "LED_DIN", -8, "middle")
        panels.append((p, t, v, recs))
    # The LSW-3 maximum of the wrong order, from the run itself.
    p, t, v, recs = panels[1]
    diff = [a - b for a, b in zip(v[DIN], v["VLED"])]
    k = max(range(len(diff)), key=lambda i: diff[i])
    tk = t[k]
    p.dot(tk, v[DIN][k], c[1])
    p.dot(tk, v["VLED"][k], c[0])
    p.bracket_v(tk + 1.2, v["VLED"][k], v[DIN][k], f"{diff[k]:.2f} V > 0.7 V at {tk:.2f} ms")
    # Zoom: the wrong order's switch-off, with the limit line VLED + 0.7 V.
    en_off = [te for te, lvl in edges(recs, "U10.P1.04") if not lvl][0]
    din_off = [te for te, lvl in edges(recs, "U10.P0.06") if not lvl][0]
    z0, z1 = 33, 48
    zy = y0 + 2 * (ph + 64) + 14
    p = Panel(s, PL, zy, pw, 190, (z0, z1), (1.0, 4.0),
              title="Wrong order, zoomed: LED_PWR_EN off while the data line is still high",
              xlabel="Time (ms)", ylabel="Voltage (V)", yticks=([1.0, 2.0, 3.0, 4.0], 1))
    lim = [a + 0.7 for a in v["VLED"]]
    # The violation wash: where LED_DIN is above VLED + 0.7 V.
    idx = [i for i in range(len(t)) if z0 <= t[i] <= z1 and v[DIN][i] > lim[i]]
    if idx:
        top = [(p.px(t[i]), p.py(v[DIN][i])) for i in idx]
        bot = [(p.px(t[i]), p.py(lim[i])) for i in reversed(idx)]
        pts = rdp(top, 0.15) + rdp(bot, 0.15)
        s.add(f'<path d="M{" L".join(f"{n(a)} {n(b)}" for a, b in pts)} Z" fill="{s.th["critical"]}" '
              f'fill-opacity="0.22" clip-path="url(#{p.clip})"/>')
    p.vmark(en_off, f"EN ↓ {en_off:.2f} ms", "start")
    p.vmark(din_off, f"P0.06 ↓ {din_off:.2f} ms", "end")
    p.series(t, lim, s.th["ink2"], 1)
    p.series(t, v["VLED"], c[0])
    p.series(t, v[DIN], c[1])
    p.dot(tk, v[DIN][k], c[1])
    p.label(z0 + 0.4, at(t, lim, z0 + 0.4), "VLED + 0.7 V (the LSW-3 limit)", 15)
    p.label(z0 + 0.4, at(t, v[DIN], z0 + 0.4), "LED_DIN", 18)
    p.label(z0 + 0.4, at(t, v["VLED"], z0 + 0.4), "VLED", 18)
    first = idx[0] if idx else k
    p.label(t[first] + 1.0, at(t, v[DIN], t[first] + 1.0), "LSW-3 violated (shaded): LED_DIN above VLED + 0.7 V",
            -12, "start", "ink")
    p.bracket_v(tk - 0.15, v["VLED"][k], v[DIN][k], f"max {diff[k]:.3f} V", "end")
    h = zy + 190 + 46
    return s.render(h, "E1: VLED and LED_DIN, correct-order and wrong-order firmware, cold bench")


def fig_e2(theme, runs, case):
    t, v = read_csv(f"{runs}/vsense/{case}/nets.csv", ["VSYS", "VLED"])
    recs = records(f"{runs}/vsense/{case}/events.jsonl")
    rd = readings(recs)
    s = Svg(theme, W)
    c = s.th["s"]
    s.legend(PL, 26, [("VSYS", c[0], "line"), ("VBAT_est (firmware reading, UART)", c[1], "dot"),
                      ("VLED (LED rail)", c[2], "line")])
    pw, x1 = W - PL - PR, t[-1]
    span = (0, round(x1 / 1000, 1))
    ts = [a / 1000 for a in t]
    lo = min(min(v["VSYS"]), min(r["est"] for r in rd))
    hi = max(v["VSYS"])
    yr = (round(lo - 0.03, 2), round(hi + 0.025, 2))
    yt = nice_ticks(yr[0], yr[1], 4)
    p = Panel(s, PL, 70, pw, 220, span, yr, title="Battery side: VSYS and the firmware's estimate",
              ylabel="Voltage (V)", yticks=yt)
    p.hline(3.53, "shed below 3.53 V", above=False, anchor="start")
    p.hline(3.58, "restore at ≥ 3.58 V", anchor="start")
    en = edges(recs, "U10.P1.04")
    for te, lvl in en[1:]:
        p.vmark(te / 1000, f"LED_PWR_EN {'↑' if lvl else '↓'} {te:.1f} ms", "end")
    p.series(ts, v["VSYS"], c[0])
    for r in rd:
        p.dot(r["t"] / 1000, r["est"], c[1])
    for r in rd:
        prev = [q for q in rd if q["n"] == r["n"] - 1]
        if prev and prev[0]["leds"] != r["leds"]:
            p.label(r["t"] / 1000, r["est"], f"reading {r['n']}: {r['est']:.4f} V, LEDs {r['leds']}",
                    18 if r["leds"] == "off" else 30, "end", dx=-8)
    p.label(0.15, at(ts, v["VSYS"], 0.15), "VSYS", -8)
    q = Panel(s, PL, 340, pw, 90, span, (0, 4), title="LED rail", xlabel="Time (s)",
              ylabel="VLED (V)", yticks=([0, 2, 4], 0))
    for te, lvl in en[1:]:
        q.vmark(te / 1000, "", "end")
    q.series(ts, v["VLED"], c[2])
    return s.render(340 + 90 + 46, f"E2 {case}: VSYS, VBAT_est readings and VLED")


def fig_e2_sampling(theme, runs):
    ld = samples(records(f"{runs}/vsense/s50/events.jsonl"))
    un = samples(records(f"{runs}/vsense/s50-none/events.jsonl"))
    rd = readings(records(f"{runs}/vsense/s50/events.jsonl"))
    assert len(ld) == len(un) == 8 * len(rd) and [a[0] for a in ld] == [b[0] for b in un]
    err = [(a[1] - b[1], a[2]) for a, b in zip(ld, un)]
    pad = un[0][1]
    code0 = un[0][2]
    lsb = 3_600_000 / 4096
    boundary = (code0 + 0.5) * lsb - pad
    s = Svg(theme, W)
    c = s.th["s"]
    last = err[-8:]
    m_last = sum(e for e, _ in last) / 8
    s.legend(PL, 26, [(f"sample, code {code0}", c[0], "dot"), (f"sample, code {code0 + 1}", c[1], "dot"),
                      (f"8-sample mean (last {m_last:.1f} µV)", s.th["ink"], "line")])
    s.legend(PL, 46, [(f"code {code0}/{code0 + 1} boundary, {boundary:.0f} µV above the unloaded pad",
                       s.th["ink2"], "ref")])
    pw = W - PL - PR
    nb = len(rd)
    p = Panel(s, PL, 96, pw, 220, (-0.6, nb - 0.4), (0, 1000), title=None,
              xlabel="Reading n (one 8× burst every 50 ms)", ylabel="Sampling error (µV)",
              xticks=(list(range(nb)), 0), yticks=([0, 250, 500, 750, 1000], 0))
    p.hline(boundary, "", color=s.th["ink2"])
    for b in range(nb):
        burst = err[8 * b:8 * b + 8]
        for j, (e, code) in enumerate(burst):
            p.dot(b + (j - 3.5) * 0.075, e, c[0] if code == code0 else c[1], 3.5)
        m = sum(e for e, _ in burst) / 8
        s.line(p.px(b - 0.36), p.py(m), p.px(b + 0.36), p.py(m), s.th["ink"], 2, ' stroke-linecap="round"')
        s.text(p.px(b), p.y - 8, str(rd[b]["code"]), 11, "ink2", "middle")
    s.text(p.x - 8, p.y - 8, "code", 11, "muted", "end")
    return s.render(96 + 220 + 46, "E2: per-acquisition sampling error on VSYS_SENSE, 50 ms readings")


def fig_spike(theme, runs):
    t, v = read_csv(f"{runs}/spike/nets.csv", ["PA5", "LED_A"])
    s = Svg(theme, W)
    c = s.th["s"]
    s.legend(PL, 26, [("PA5 (STM32F103 pin)", c[0], "line"), ("LED_A (LED anode)", c[1], "line")])
    pw = W - PL - PR
    p = Panel(s, PL, 56, pw, 170, (0, 500), (0, 3.5), xlabel="Time (ms)", ylabel="Voltage (V)",
              yticks=([0, 1, 2, 3], 0))
    p.series(t, v["PA5"], c[0])
    p.series(t, v["LED_A"], c[1])
    p.label(75, at(t, v["PA5"], 75), "PA5", -8, "middle")
    p.label(75, at(t, v["LED_A"], 75), "LED_A", 18, "middle")
    return s.render(56 + 170 + 46, "Phase 0 spike: PA5 and LED_A over 500 ms")


FIGURES = {
    "e1-rail": fig_e1,
    "e2-sag": lambda th, runs: fig_e2(th, runs, "sag"),
    "e2-recover": lambda th, runs: fig_e2(th, runs, "rec"),
    "e2-sampling": fig_e2_sampling,
    "spike": fig_spike,
}


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    runs, out = sys.argv[1], sys.argv[2]
    os.makedirs(out, exist_ok=True)
    for name, fn in FIGURES.items():
        for theme in THEMES:
            path = f"{out}/{name}.{theme}.svg"
            with open(path, "w", encoding="utf-8", newline="\n") as f:
                f.write(fn(theme, runs))
            print(f"docs-plots: {path} ({os.path.getsize(path)} bytes)")


if __name__ == "__main__":
    main()
