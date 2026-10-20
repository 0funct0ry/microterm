#!/usr/bin/env python3
"""Probe a terminal's VT behaviour and compare terminals (microterm vs Ghostty, ...).

Run the SAME command inside each terminal you want to compare (it talks to /dev/tty, so it
must run directly in that terminal, not through a pipe or an IDE console):

    scripts/vt_probe.py run --name microterm
    scripts/vt_probe.py run --name ghostty

Each run feeds every case of crates/mt_term/tests/vt_conformance.toml (the Ghostty VT
reference work list) to the terminal and records what can be observed from the program side:

  * the bytes the terminal writes back (replies to OSC 4/5/10-19/21 queries, DSR, ...),
  * the cursor position afterwards (CPR),
  * the DEC private mode state (DECRQM) and cursor style / scroll region / SGR (DECRQSS),
  * the terminal's identity (DA1, DA2, XTVERSION).

It cannot read the screen, so grid contents are not compared; use the manual list printed by
`run` for the few visual checks. Results go to vt-probe-out/<name>.json. Then:

    scripts/vt_probe.py compare vt-probe-out/microterm.json vt-probe-out/ghostty.json
    scripts/vt_probe.py check   vt-probe-out/microterm.json     # against the TOML expectations

`compare` writes vt-probe-out/compare-<a>-vs-<b>.md and prints a summary; send that file
for analysis. Nothing here changes any setting; each case starts with RIS (ESC c), and RIS is
sent again at the end. OSC 9 cases raise a few desktop notifications; skip them with
`--skip OSC\\ 9`.
"""

import argparse
import json
import os
import re
import select
import sys
import termios
import time
import tty

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TOML = os.path.join(ROOT, "crates/mt_term/tests/vt_conformance.toml")
OUT_DIR = os.path.join(ROOT, "vt-probe-out")

CPR = re.compile(rb"\x1b\[(\d+);(\d+)R")
DECRQM = re.compile(rb"\x1b\[\?(\d+);(\d)\$y")
DECRQSS = re.compile(rb"\x1bP([01])\$r(.*?)\x1b\\", re.S)
MODES = [1, 6, 7, 12, 25, 47, 69, 1000, 1002, 1003, 1004, 1005, 1006, 1007, 1015, 1016,
         1047, 1049, 2004, 2026, 2027]
DECRQSS_QUERIES = {"cursor_style": b" q", "scroll_region": b"r", "sgr": b"m"}
HEX_COLOUR = re.compile(r"rgb:([0-9a-fA-F]+)/([0-9a-fA-F]+)/([0-9a-fA-F]+)")

# Cases a terminal could prompt or hang on: skipped by default.
DEFAULT_SKIP = [("OSC 52", lambda c: ";?" in c["input"])]


# --------------------------------------------------------------------------- data

def load_cases(path=TOML):
    """Minimal reader for vt_conformance.toml (the generator writes `key = <JSON value>`)."""
    rows, row, case = [], None, None
    for line in open(path, encoding="utf-8"):
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        if s == "[[row]]":
            row = {"case": []}
            rows.append(row)
            case = None
        elif s == "[[row.case]]":
            case = {}
            row["case"].append(case)
        else:
            key, _, value = s.partition(" = ")
            (case if case is not None else row)[key.strip()] = json.loads(value)
    return rows


# --------------------------------------------------------------------------- tty

class Tty:
    def __init__(self):
        self.fd = os.open("/dev/tty", os.O_RDWR | os.O_NOCTTY)
        self.saved = termios.tcgetattr(self.fd)
        tty.setraw(self.fd)

    def restore(self):
        termios.tcsetattr(self.fd, termios.TCSADRAIN, self.saved)

    def write(self, data):
        if isinstance(data, str):
            data = data.encode("utf-8", "surrogateescape")
        while data:
            n = os.write(self.fd, data)
            data = data[n:]

    def read_for(self, seconds):
        buf = b""
        end = time.time() + seconds
        while True:
            left = end - time.time()
            if left <= 0:
                return buf
            if select.select([self.fd], [], [], left)[0]:
                chunk = os.read(self.fd, 65536)
                if not chunk:
                    return buf
                buf += chunk

    def read_until(self, done, timeout):
        """Reads until `done(buf)` is true; returns (buf, finished)."""
        buf = b""
        end = time.time() + timeout
        while True:
            if done(buf):
                return buf, True
            left = end - time.time()
            if left <= 0:
                return buf, False
            if select.select([self.fd], [], [], left)[0]:
                chunk = os.read(self.fd, 65536)
                if not chunk:
                    return buf, False
                buf += chunk


def exchange(t, payload, expected_cprs, timeout):
    """Sends `payload` + a CPR barrier and returns (reply bytes before the barrier, cursor,
    finished). `expected_cprs` counts CPR replies the payload itself asks for."""
    t.write(payload + "\x1b[6n")
    need = expected_cprs + 1

    def done(buf):
        return len(CPR.findall(buf)) >= need

    buf, ok = t.read_until(done, timeout)
    matches = list(CPR.finditer(buf))
    if not matches or len(matches) < need:
        return buf, None, False
    barrier = matches[need - 1]
    # Anything after the barrier is stray; keep it with the reply so it shows up in diffs.
    reply = buf[: barrier.start()] + buf[barrier.end():]
    cursor = [int(barrier.group(1)) - 1, int(barrier.group(2)) - 1]
    return reply, cursor, True


def show(b):
    return b.decode("latin-1").encode("unicode_escape").decode("ascii")


# --------------------------------------------------------------------------- run

def identity(t, timeout):
    out = {}
    for name, q in (("da1", "\x1b[c"), ("da2", "\x1b[>c"), ("xtversion", "\x1b[>0q")):
        reply, _, _ = exchange(t, q, 0, timeout)
        out[name] = show(reply)
    return out


def probe_state(t, timeout):
    q = "".join(f"\x1b[?{m}$p" for m in MODES)
    q += "".join(f"\x1bP$q{v.decode()}\x1b\\" for v in DECRQSS_QUERIES.values())
    reply, _, ok = exchange(t, q, 0, timeout)
    modes = {int(m): int(s) for m, s in ((a.decode(), b.decode()) for a, b in DECRQM.findall(reply))}
    ss = {}
    found = DECRQSS.findall(reply)
    for (valid, body), name in zip(found, DECRQSS_QUERIES):
        ss[name] = body.decode("latin-1") if valid == b"1" else None
    return modes, ss, ok


def skipped(row, case, skips):
    for code, pred in DEFAULT_SKIP:
        if row["code"] == code and pred(case):
            return True
    return any(row["code"] == s or row["code"].startswith(s) for s in skips)


def cmd_run(args):
    rows = load_cases()
    if not sys.stdin.isatty() and not os.path.exists("/dev/tty"):
        sys.exit("vt_probe: needs a terminal (/dev/tty)")
    name = args.name or os.environ.get("TERM_PROGRAM") or os.environ.get("TERM", "terminal")
    t = Tty()
    results = []
    try:
        meta = {
            "name": name,
            "term": os.environ.get("TERM"),
            "term_program": os.environ.get("TERM_PROGRAM"),
            "term_program_version": os.environ.get("TERM_PROGRAM_VERSION"),
            "colorterm": os.environ.get("COLORTERM"),
            "time": time.strftime("%Y-%m-%dT%H:%M:%S"),
            "timeout": args.timeout,
        }
        t.write("\x1bc")
        t.read_for(0.2)
        meta.update(identity(t, args.timeout))
        cases = [(r, i, c) for r in rows for i, c in enumerate(r["case"])]
        only = args.only
        for n, (row, i, case) in enumerate(cases):
            if only and not any(row["code"].startswith(o) for o in only):
                continue
            if skipped(row, case, args.skip):
                results.append({"row": row["code"], "name": row["name"], "idx": i,
                                "input": case["input"], "skipped": True})
                continue
            t.write("\x1bc")
            t.read_for(args.settle)
            expected_cprs = case["input"].count("\x1b[6n") + case["input"].count("\x1b[?6n")
            reply, cursor, ok = exchange(t, case["input"], expected_cprs, args.timeout)
            modes, ss, ok2 = probe_state(t, args.timeout)
            results.append({
                "row": row["code"], "name": row["name"], "idx": i, "input": case["input"],
                "reply": show(reply), "cursor": cursor, "modes": modes, "decrqss": ss,
                "timed_out": not (ok and ok2),
            })
            t.write(f"\x1b[2J\x1b[H probing {n + 1}/{len(cases)} {row['code']}")
    finally:
        t.write("\x1bc")
        t.restore()
    os.makedirs(OUT_DIR, exist_ok=True)
    path = args.out or os.path.join(OUT_DIR, f"{name}.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump({"meta": meta, "results": results}, f, indent=1)
    done = [r for r in results if not r.get("skipped")]
    print(f"vt_probe: {len(done)} cases probed ({sum(1 for r in done if r['timed_out'])} timed out), "
          f"{len(results) - len(done)} skipped -> {path}")
    print(MANUAL)


MANUAL = """\
Visual checks the probe cannot do (compare by eye in both terminals):
  printf '\\e]5;0;rgb:ff/00/00\\e\\\\'; printf '\\e[1mbold\\e[0m\\n'; printf '\\e]105\\e\\\\'   # bold colour
  printf '\\e]17;rgb:00/80/00\\e\\\\'  then select text; printf '\\e]117\\e\\\\'             # selection bg
  printf '\\e]22;pointer\\e\\\\'  (hover the pane)   printf '\\e]22;\\e\\\\'                 # pointer shape
  printf '\\e]9;4;1;40\\e\\\\'  (tab progress)       printf '\\e]9;build done\\e\\\\'        # progress / notification
  printf '\\e[>1s' then Shift-click in htop                                               # XTSHIFTESCAPE
  printf '\\e#8'; clear                                                                    # DECALN fill
"""


# --------------------------------------------------------------------------- compare

def shape(s):
    """Replies with colour values compared by structure: `rgb:e0e0/6c6c/7575` -> `rgb:4/4/4`."""
    return HEX_COLOUR.sub(lambda m: "rgb:%d/%d/%d" % tuple(len(g) for g in m.groups()), s or "")


def key(r):
    return (r["row"], r["idx"])


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def diff_fields(a, b):
    out = []
    for f in ("reply", "cursor", "modes", "decrqss"):
        if a.get(f) != b.get(f):
            out.append(f)
    return out


def cmd_compare(args):
    A, B = load(args.a), load(args.b)
    na, nb = A["meta"]["name"], B["meta"]["name"]
    ra, rb = {key(r): r for r in A["results"]}, {key(r): r for r in B["results"]}
    keys = [k for k in ra if k in rb]
    same, value_only, differ, skipped = [], [], [], []
    for k in keys:
        a, b = ra[k], rb[k]
        if a.get("skipped") or b.get("skipped"):
            skipped.append(k)
            continue
        fields = diff_fields(a, b)
        if not fields:
            same.append(k)
        elif fields == ["reply"] and shape(a["reply"]) == shape(b["reply"]):
            value_only.append(k)
        else:
            differ.append((k, fields))
    md = [f"# VT probe: {na} vs {nb}", "",
          f"| | {na} | {nb} |", "|---|---|---|"]
    for f in ("term", "term_program", "term_program_version", "da1", "da2", "xtversion", "time"):
        md.append(f"| {f} | `{A['meta'].get(f)}` | `{B['meta'].get(f)}` |")
    md += ["", f"**{len(same)} identical, {len(value_only)} differ only in colour values "
               f"(theme palettes), {len(differ)} differ, {len(skipped)} skipped.**", ""]
    if value_only:
        md += ["## Colour values only (same structure, different palette)", ""]
        md += [f"- `{k[0]}` case {k[1]}: {na} `{ra[k]['reply']}` / {nb} `{rb[k]['reply']}`"
               for k in value_only]
        md.append("")
    if differ:
        md += ["## Differences", ""]
        for k, fields in differ:
            a, b = ra[k], rb[k]
            md.append(f"### {k[0]} ({a['name']}) case {k[1]}: {', '.join(fields)}")
            md.append(f"input: `{show(a['input'].encode())}`")
            for f in fields:
                if f == "modes":
                    ma, mb = a["modes"], b["modes"]
                    for m in sorted(set(ma) | set(mb), key=int):
                        if ma.get(m) != mb.get(m):
                            md.append(f"- mode {m}: {na}={ma.get(m)} {nb}={mb.get(m)}")
                else:
                    md.append(f"- {f}: {na}=`{a.get(f)}` {nb}=`{b.get(f)}`")
            md.append("")
    # Mode support baseline independent of cases.
    base_a = next((r for r in ra.values() if not r.get("skipped")), None)
    base_b = next((r for r in rb.values() if not r.get("skipped")), None)
    if base_a and base_b:
        md += ["## Mode support after RIS (DECRQM: 0 unknown, 1 set, 2 reset, 3/4 permanent)", "",
               f"| mode | {na} | {nb} |", "|---|---|---|"]
        for m in MODES:
            va, vb = base_a["modes"].get(str(m)), base_b["modes"].get(str(m))
            flag = "" if va == vb else " **<-**"
            md.append(f"| {m} | {va} | {vb} |{flag}")
        md.append("")
    out = args.out or os.path.join(OUT_DIR, f"compare-{na}-vs-{nb}.md")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        f.write("\n".join(md))
    print(f"{na} vs {nb}: {len(same)} identical, {len(value_only)} colour-value only, "
          f"{len(differ)} differ, {len(skipped)} skipped -> {out}")
    for k, fields in differ[:15]:
        print(f"  differs: {k[0]} case {k[1]} ({', '.join(fields)})")
    if len(differ) > 15:
        print(f"  ... and {len(differ) - 15} more in the report")


# --------------------------------------------------------------------------- check

SHAPE_OF_STYLE = {0: "block", 1: "block", 2: "block", 3: "underline", 4: "underline",
                  5: "beam", 6: "beam"}
MODE_OF = {"app_cursor": 1, "alt_screen": 1049, "bracketed_paste": 2004}


def cmd_check(args):
    """Compares one run with the expectations in the TOML (written against One Dark)."""
    data = load(args.file)
    rows = {r["code"]: r for r in load_cases()}
    bad = ok = 0
    for r in data["results"]:
        if r.get("skipped"):
            continue
        case = rows[r["row"]]["case"][r["idx"]]
        problems = []
        if "reply" in case:
            want = show(case["reply"].replace(
                "{version}", data["meta"].get("term_program_version") or "?").encode())
            if r["reply"] != want:
                kind = "colour value" if shape(r["reply"]) == shape(want) else "reply"
                problems.append(f"{kind}: want `{want}` got `{r['reply']}`")
        if "cursor" in case and r["cursor"] != case["cursor"]:
            problems.append(f"cursor: want {case['cursor']} got {r['cursor']}")
        if "cursor_shape" in case:
            style = (r["decrqss"] or {}).get("cursor_style")
            got = SHAPE_OF_STYLE.get(int(style.split()[0])) if style else None
            if got != case["cursor_shape"]:
                problems.append(f"cursor shape: want {case['cursor_shape']} got {got} ({style})")
        for want_on, names in ((True, case.get("modes_on", [])), (False, case.get("modes_off", []))):
            for n in names:
                m = MODE_OF.get(n)
                if m is None:
                    continue
                v = r["modes"].get(str(m))
                if v not in (None, 0) and (v in (1, 3)) != want_on:
                    problems.append(f"mode {n}({m}): want {'on' if want_on else 'off'} got {v}")
        if problems:
            bad += 1
            print(f"{r['row']} case {r['idx']} ({r['name']}):")
            for p in problems:
                print(f"  - {p}")
        else:
            ok += 1
    print(f"{data['meta']['name']}: {ok} cases match the expectations, {bad} do not "
          "(colour-value mismatches are palette differences, not bugs)")


# --------------------------------------------------------------------------- main

def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run", help="probe the terminal this is run in")
    r.add_argument("--name", help="label for this terminal (default $TERM_PROGRAM)")
    r.add_argument("--out", help="output file (default vt-probe-out/<name>.json)")
    r.add_argument("--timeout", type=float, default=1.0, help="seconds to wait per exchange")
    r.add_argument("--settle", type=float, default=0.02, help="seconds to wait after RIS")
    r.add_argument("--only", action="append", default=[], metavar="CODE",
                   help="only rows whose code starts with this (repeatable), e.g. 'OSC 4'")
    r.add_argument("--skip", action="append", default=[], metavar="CODE",
                   help="skip rows whose code starts with this (repeatable), e.g. 'OSC 9'")
    r.set_defaults(fn=cmd_run)
    c = sub.add_parser("compare", help="diff two runs and write a markdown report")
    c.add_argument("a")
    c.add_argument("b")
    c.add_argument("--out")
    c.set_defaults(fn=cmd_compare)
    k = sub.add_parser("check", help="compare one run with the TOML expectations")
    k.add_argument("file")
    k.set_defaults(fn=cmd_check)
    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
