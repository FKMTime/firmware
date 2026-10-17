"""Competition day for sim/monkey.py (`--mode competition`): clumsy but well-meaning competitors
and judges run a seeded competition on one or more stations (each its own esp-emu, all on one
micro-connector), and the server must get exactly the intended results.

Every human action is a planned step with a defined outcome. The oracle models the firmware rules
(src/buttons.rs, src/rfid.rs, src/stackmat.rs) independently, checks the device state after each
step, and after each attempt checks the server's EnterAttempt log (srv/logs/unix.log): exactly one
solve (per session) with the right station, competitor, time, penalty, judge, group and inspection.
"""
import json
import random
import re
import shutil
import subprocess
import threading
import time
from collections import Counter
from pathlib import Path

import monkey as M

PLUS2_MS, DNF_MS = 15000, 17000  # src/consts.rs INSPECTION_TIME_*
RESET_HOLD, DNF_HOLD, INSPECT_STOP_HOLD, DELEGATE_HOLD = 3000, 1000, 1000, 3000  # src/buttons.rs
DELEGATE_TIMEOUT = 5 * 60_000  # src/consts.rs: a pending delegate call blocks new ones this long
EVENTS = [  # id, name, inspection, time limit (ms), attempts, solve mean/sd (s, before --time-scale)
    ("333", "3x3x3 Cube", True, None, 5, 13.0, 3.0),
    ("222", "2x2x2 Cube", True, None, 5, 6.0, 2.0),
    ("pyram", "Pyraminx", True, None, 5, 7.0, 2.5),
    ("skewb", "Skewb", True, None, 5, 8.0, 2.5),
    ("333bf", "3x3x3 Blindfolded", False, 25_000, 3, 14.0, 3.0),
]
# per attempt (the realistic base rates; --slip-rate multiplies them)
SLIPS = {
    "judge_card_first": 0.03,      # judge's card on the reader before the competitor's
    "other_card_first": 0.03,      # someone else's card first, judge resets (submit hold)
    "scan_twice": 0.04,            # competitor taps twice
    "forgot_inspection": 0.03,     # starts the timer without inspection
    "inspect_twice": 0.04,         # second inspect press during inspection: no effect
    "inspection_restart": 0.02,    # judge cancels inspection (inspect hold) and starts it again
    "fumble_before_scan": 0.03,    # plays with the timer (start, stop?, reset) before scanning
    "timer_reset_running": 0.01,   # resets a running timer after scanning: DNF (0)
    "stackmat_reset_early": 0.05,  # judge resets the timer before confirming: no effect
    "plus2_fix": 0.03,             # applies +2 by mistake, then corrects (wrap-around / DNF hold)
    "wrong_judge_card": 0.03,      # another person's card as judge, then the right judge
    "competitor_before_judge": 0.04,  # competitor scans before the judge ("cards the same")
    "double_submit": 0.04,
    "submit_long": 0.04,           # 1.3-2.6 s (reset is at 3 s)
    "wrong_button": 0.04,          # short delegate / inspect press while finished: no effect
    "group_scroll": 0.3,           # multi-group competitors: scroll past their group and back
    "delegate": 0.03,              # judge calls the delegate (3.5 s hold)
    "wifi_blip": 0.02,             # radio off 1-4 s on this station only
    "abandon": 0.01,               # attempt reset before inspection, redone right away
    "over_limit": 0.10,            # (limited events) solve runs past the time limit: DNF
    "plus2": 0.05,                 # legit judge decisions
    "dnf": 0.02,
}


# ── generation (pure: same seed and devices ⇒ same day) ────────────────────────────────────

def generate_day(seed: int, devices: int, max_attempts: int, slip_rate: float = 1.0,
                 time_scale: float = 0.5) -> dict:
    r = random.Random(seed * 1000 + devices)
    uids = set()

    def uid():
        while True:
            u = r.randint(10_000, 2**32 - 1)
            if u not in uids and u not in M.KNOWN_CARDS:
                uids.add(u)
                return u

    events = [EVENTS[0]] + r.sample(EVENTS[1:4], r.randint(1, 2)) + [EVENTS[4]]
    n = r.randint(20, 40)
    comps = [uid() for _ in range(n)]
    regs = {c: [e[0] for e in events if r.random() < (0.9 if e[0] == "333" else 0.35)] or ["333"]
            for c in comps}
    groups, comp_groups, schedule_groups = [], {c: [] for c in comps}, []
    for ev, name, insp, limit, attempts, mean, sd in events:
        people = [c for c in comps if ev in regs[c]]
        if not people:
            continue
        k = 2 if len(people) > 14 else 1
        for g in range(k):
            gid = f"{ev}-r1-g{g + 1}"
            members = people[g::k]
            groups.append(dict(groupId=gid, name=f"{name} R1", useInspection=insp,
                               secondaryText=f"Group {g + 1}", limit=limit))
            for c in members:
                comp_groups[c].append(gid)
            schedule_groups.append((gid, members, insp, limit, attempts, mean, sd))
    staff = [uid() for _ in range(r.randint(3, 5))]
    cards = {}
    for i, c in enumerate(comps):
        cards[str(c)] = dict(registrantId=i + 1, name=f"Competitor {i + 1}", wcaId=f"2026COMP{i + 1:02}",
                             canCompete=True, groups=comp_groups[c])
    for i, s in enumerate(staff):
        cards[str(s)] = dict(registrantId=100 + i, name=f"Staff {i + 1}", wcaId="", canCompete=False,
                             groups=[])

    # the whole competition is registered; the day plays the first group of each event, each
    # cut to a share of --max-attempts (whole attempts of the first few competitors)
    played = [g for i, g in enumerate(schedule_groups)
              if g[0].endswith("-g1") or i == len(schedule_groups) - 1][:4]
    budget = max(1, max_attempts // len(played))
    schedule, total = [], 0
    for gid, members, insp, limit, attempts, mean, sd in played:
        judges = staff + [c for c in comps if c not in members]
        r.shuffle(judges)
        members = members[:max(devices, -(-budget // attempts))]
        rounds = plan_rounds(r, members, attempts, devices)
        specs = []
        jrot, in_group = 0, 0
        for rd in rounds:
            row = []
            busy = {x[0] for x in rd if x}
            for c, k in [x or (None, None) for x in rd]:
                if c is None or in_group >= budget or total >= max_attempts:
                    row.append(None)
                    continue
                total += 1
                in_group += 1
                judge = judges[jrot % len(judges)]
                jrot += 1
                others = [x for x in members if x not in busy]
                row.append(attempt_spec(r, c, judge, gid, k, insp, limit, mean, sd, time_scale,
                                        len(comp_groups[c]) > 1, slip_rate, others, staff, comps))
            if any(row):
                specs.append(row)
        restart = None
        if specs and r.random() < 0.35:
            restart = dict(round=r.randrange(len(specs)), delay=round(r.uniform(2, 25), 1),
                           down=round(r.uniform(1.5, 6), 1))
        schedule.append(dict(group=gid, rounds=specs, restart=restart,
                             sync=[r.random() < 0.3 for _ in specs],
                             break_s=round(r.choice([5, 10, 20, 40, 90]) * min(1.0, time_scale * 2), 1)))
    # battery drains over the day
    i = 0
    for g in schedule:
        for row in g["rounds"]:
            for sp in row:
                if sp:
                    sp["bat"] = max(5, 100 - int(85 * i / max(1, total)) - r.randint(0, 2))
            i += 1 if any(row) else 0
    return dict(seed=seed, devices=devices, time_scale=time_scale, groups=groups, cards=cards,
                schedule=schedule, attempts=total)


def plan_rounds(r, members, attempts, devices):
    """Rounds of up to `devices` distinct competitors: (competitor, attempt no). Competitors
    usually stay at their station for the next attempt; sometimes they move."""
    left = {c: attempts for c in members}
    queue = list(members)
    seats = [None] * devices
    rounds = []
    while any(left.values()):
        row = [None] * devices
        used = set()
        for s in range(devices):
            c = seats[s]
            if c and left[c] and c not in used and r.random() < 0.85:
                row[s] = c
                used.add(c)
        for s in range(devices):
            if row[s] is None:
                cand = next((c for c in queue if left[c] and c not in used), None)
                if cand:
                    row[s] = cand
                    used.add(cand)
                    queue.remove(cand)
                    queue.append(cand)
        if devices > 1 and r.random() < 0.15:
            r.shuffle(row)  # people moved to other stations
        seats = row
        out = []
        for c in row:
            if c:
                out.append((c, attempts - left[c] + 1))
                left[c] -= 1
            else:
                out.append(None)
        rounds.append(out)
    return rounds


def attempt_spec(r, comp, judge, gid, k, insp, limit, mean, sd, scale, multi, rate, others, staff,
                 comps):
    def slip(name):
        return r.random() < SLIPS[name] * rate

    sl = {}
    first = None
    if slip("judge_card_first"):
        first = judge
    elif slip("other_card_first"):
        first = r.choice([x for x in others if x != comp] or staff)
    if first:
        sl["wrong_first"] = first
    for name in ("scan_twice", "fumble_before_scan", "stackmat_reset_early", "double_submit",
                 "submit_long", "competitor_before_judge", "abandon"):
        if slip(name):
            sl[name] = True
    if "scan_twice" in sl:
        sl["scan_twice"] = r.choice([0.2, 1.5])
    if "fumble_before_scan" in sl:
        sl["fumble_before_scan"] = r.random() < 0.5  # with a stop before the reset
    if "submit_long" in sl:
        sl["submit_long"] = r.randint(1300, 2600)
    if multi and slip("group_scroll"):
        sl["group_scroll"] = True
    if slip("timer_reset_running"):
        sl["timer_reset_running"] = True
    elif insp and slip("forgot_inspection"):
        sl["forgot_inspection"] = True
    elif insp:
        if slip("inspect_twice"):
            sl["inspect_twice"] = True
        if slip("inspection_restart"):
            sl["inspection_restart"] = True
    if limit and "timer_reset_running" not in sl and slip("over_limit"):
        sl["over_limit"] = True
    if slip("plus2_fix"):
        sl["plus2_fix"] = r.choice(["wrap", "dnf_hold"])
    if slip("delegate"):
        sl["delegate"] = True
    if slip("wrong_judge_card"):
        sl["wrong_judge_card"] = r.choice([x for x in comps + staff if x not in (comp, judge)])
    if slip("wrong_button"):
        sl["wrong_button"] = r.choice([2, 1])
    if slip("wifi_blip"):
        sl["wifi_blip"] = dict(at=r.choice(["before_scan", "after_stop"]), ms=r.randint(1000, 4000))
    decision = "dnf" if slip("dnf") else "plus2" if slip("plus2") else None
    if decision:
        sl["judge_" + decision] = True

    x = r.random()
    insp_ms = None
    if insp:  # firmware thresholds are never scaled; targets keep ≥ 0.6 s away from them
        insp_ms = (r.randint(5000, 12000) if x < 0.90 else
                   r.randint(15600, 16400) if x < 0.97 else r.randint(17800, 19000))
    solve = max(0.8, r.gauss(mean, sd)) * scale
    if limit:
        solve = min(solve, limit / 1000 - 3)
    return dict(comp=comp, judge=judge, group=gid, attempt=k, insp=insp_ms, solve=round(solve, 2),
                gap=round(r.choice([0.5, 1, 2, 3, 5]) * r.uniform(0.8, 1.2), 2), slips=sl)


# ── server log ───────────────────────────────────────────────────────────────────────────────

ENTRY = re.compile(
    r'EnterAttempt \{ value: \d+, value_ms: (\d+), penalty: (-?\d+), solved_at: "[^"]*", '
    r'esp_id: (\d+), judge_id: "(\d*)", competitor_id: "(\d+)", is_delegate: (\w+), '
    r'session_id: "([^"]*)", inspection_time: (-?\d+), group_id: "([^"]*)"')


def entries(run_dir: Path) -> list:
    """Every EnterAttempt the backend forwarded (appended across server restarts)."""
    try:
        text = (run_dir / "srv" / "logs" / "unix.log").read_text(errors="replace")
    except FileNotFoundError:
        return []
    return [dict(time=int(m[1]), pen=int(m[2]), esp=int(m[3]), judge=int(m[4] or 0), comp=int(m[5]),
                 delegate=m[6] == "true", session=m[7], insp=int(m[8]), group=m[9])
            for m in re.finditer(ENTRY, text)]


# ── penalty model (independent of the firmware code, from its documented rules) ─────────────

def press_penalty(p):  # 0 → +2 → … → +16 → 0, DNF → 0
    return 0 if p >= 16 or p == -1 else p + 2


def hold_penalty(p):  # DNF toggle
    return 0 if p == -1 else -1


class Mismatch(Exception):
    pass


# ── one station ──────────────────────────────────────────────────────────────────────────────

class Station(M.Run):
    def __init__(self, n, run_dir, server, image, ports, args, restart_lock):
        self.n, self.dir, self.server, self.args, self.ports = n, run_dir, server, args, ports
        self.expected_boots, self.settings_resets, self.heap_peak = 1, 0, 0
        self.executed, self.t0 = [], time.monotonic()
        self.restart_lock = restart_lock
        efuse = run_dir / f"dev{n}.efuse"
        subprocess.run(["python3", str(M.ROOT / "sim/make-efuse.py"), "--chip", "esp32c3",
                        "--chip-rev", "1.3", "--mac", f"24:0A:C4:00:00:{n + 1:02X}", "-o", str(efuse)],
                       check=True, stdout=subprocess.DEVNULL)
        bin_ = run_dir / f"dev{n}.bin"
        shutil.copy(image[0], bin_)
        self.emu = M.Emu(bin_, image[1], ports, run_dir / f"firmware{n}.log", efuse)
        self.stackmat = M.Stackmat(ports["uart1"])
        self.esp = None
        self.delegate_until = -10**9  # emulated ms until which a pending delegate blocks new calls
        self.disturbed = False  # a Wi-Fi blip / server restart touched the current attempt

    def note(self, *what):
        self.executed.append([round(time.monotonic() - self.t0, 3), self.n, *what])

    def st(self):
        return self.state(timeout=10)

    def poll(self, pred, emu_ms, what=None):
        """State once `pred` holds within `emu_ms` of emulated time (None if not, or Failure if
        `what` is given). A real-time cap catches a hung emulator."""
        s = self.st()
        end, cap = int(s["up"]) + emu_ms, time.monotonic() + emu_ms / 1000 * 8 + 15
        while True:
            if pred(s):
                return s
            if int(s["up"]) >= end or time.monotonic() > cap:
                if what:
                    raise Mismatch(f"{what}: not reached; state {brief(s)}")
                return None
            time.sleep(0.15)
            self.check_alive()
            s = self.st()

    def connected(self):
        self.wait_for(lambda s: s.get("srv") == "Some(true)" and s.get("stk") == "Some(true)",
                      120, "server and stackmat connected")

    def btn(self, b, ms):
        self.note("btn", b, ms)
        self.press(b, ms)
        time.sleep(0.1)

    def tap(self, uid):
        self.note("card", uid)
        self.emu.send(f"card {uid}")

    def stk(self, c):
        self.note("stk", c)
        self.stackmat.cmd(c)

    def dismiss(self, s):
        """An error screen (e.g. a lookup lost to a server restart) is dismissed with submit."""
        if s.get("err") == "1":
            self.btn(3, 120)
            time.sleep(0.5)

    def scan(self, uid, pred, what, wait=14000, tries=4):
        """Tap a card until the screen shows it worked (no beep / an error ⇒ tap again). `wait`
        outlasts a request timeout (10 s), so a retap never queues behind a pending answer."""
        for _ in range(tries):
            self.connected()
            self.tap(uid)
            s = self.poll(pred, wait)
            if s:
                return s
            s = self.st()
            self.disturbed = True
            self.dismiss(s)
        raise Mismatch(f"{what}: card {uid} had no effect after {tries} taps; state {brief(self.st())}")

    def press_check(self, b, ms, pred, what, tries=3):
        for _ in range(tries):
            self.connected()
            self.btn(b, ms)
            s = self.poll(pred, 2500)
            if s:
                return s
            s = self.st()
            if s.get("srv") != "Some(true)" or s.get("stk") != "Some(true)":
                self.disturbed = True  # the disconnected screen ignores buttons: press again
                continue
            if s.get("err") != "1":
                raise Mismatch(f"{what}: btn {b} {ms} ms gave {brief(s)}")
            self.dismiss(s)
        raise Mismatch(f"{what}: btn {b} ignored {tries}x")

    def idle(self, s):
        return s.get("scene") == "WaitingForCompetitor" and s.get("comp") == "-1"

    def to_idle(self):
        """Reset competitor (submit hold) and wait until idle."""
        self.btn(3, RESET_HOLD + 400)
        self.poll(self.idle, 8000, "idle after the reset hold")

    def wifi_blip(self, ms):
        self.disturbed = True
        self.note("wifi", ms)
        self.emu.send("wifi off")
        time.sleep(ms / 1000)
        self.emu.send("wifi on")

    # ── one attempt ─────────────────────────────────────────────────────────────────────────

    def attempt(self, sp, day, rec):
        C, J, G, sl = sp["comp"], sp["judge"], sp["group"], sp["slips"]
        group = next(g for g in day["groups"] if g["groupId"] == G)
        mine = [g["groupId"] for g in day["groups"] if g["groupId"] in day["cards"][str(C)]["groups"]]
        self.disturbed = False
        self.emu.send(f"bat {sp['bat']}")
        s = self.poll(lambda s: self.idle(s) and s.get("time") == "-1", 12000)
        if not s:
            raise Mismatch(f"not idle before the attempt: {brief(self.st())}")
        blip = sl.get("wifi_blip")
        if blip and blip["at"] == "before_scan":
            self.wifi_blip(blip["ms"])

        if "fumble_before_scan" in sl:
            self.connected()
            self.stk("start")
            time.sleep(0.7)
            if sl["fumble_before_scan"]:
                self.stk("stop")
                time.sleep(0.6)
            self.stk("reset")
            self.poll(lambda s: self.idle(s) and s.get("time") == "-1" and s.get("timer") == "Some(0)",
                      5000, "fumbled timer cleared")

        if "wrong_first" in sl:
            W = sl["wrong_first"]
            if day["cards"][str(W)]["canCompete"]:
                self.scan(W, lambda s: s.get("comp") == str(W), "wrong card shows that person")
                self.to_idle()
            else:
                self.connected()
                self.tap(W)
                time.sleep(2)
                if not self.idle(self.st()):
                    raise Mismatch(f"staff card {W} changed the idle screen: {brief(self.st())}")

        def scan_competitor():
            return self.scan(C, lambda s: s.get("comp") == str(C) and s.get("scene") in
                             ("CompetitorInfo", "GroupSelect"), "competitor scanned")

        def select_group():
            if len(mine) < 2:
                return self.poll(lambda s: s.get("group") == G, 3000, "single group selected")
            self.poll(lambda s: s.get("scene") == "GroupSelect" and s.get("groups") == str(len(mine)),
                      3000, "group select with the competitor's groups")
            k, t, sel = len(mine), mine.index(G), 0
            steps = [4] * t
            if "group_scroll" in sl:
                steps = [4] * (t + 1) + [1]
            for b in steps:
                sel = (sel + 1) % k if b == 4 else (sel - 1) % k
                self.press_check(b, 120, lambda s, sel=sel: s.get("sel") == str(sel), "group cursor")
            return self.press_check(3, 120, lambda s: s.get("scene") == "CompetitorInfo" and s.get("group") == G,
                                    "group chosen")

        s = scan_competitor()
        if "scan_twice" in sl:
            time.sleep(sl["scan_twice"])
            self.tap(C)
            time.sleep(1.5)
            s2 = self.st()
            if s2.get("comp") != str(C) or s2.get("scene") != s.get("scene"):
                raise Mismatch(f"second tap of the competitor changed the screen: {brief(s2)}")
        select_group()
        if "abandon" in sl:
            self.to_idle()
            rec["abandoned"] += 1
            scan_competitor()
            select_group()

        conf, insp_expect, limit = False, None, group["limit"]
        if "timer_reset_running" in sl:
            self.connected()
            self.stk("start")
            self.poll(lambda s: s.get("scene") == "Timer", 5000, "fumble starts the timer")
            time.sleep(0.6)
            self.stk("reset")
            s = self.poll(lambda s: s.get("scene") == "Finished", 5000, "reset running timer ⇒ DNF")
            exp_time, pen, conf, insp_expect = 0, -1, True, 0
        else:
            target = sp["insp"]
            if group["useInspection"] and "forgot_inspection" not in sl:
                t0 = self.start_inspection()
                if "inspect_twice" in sl:
                    time.sleep(1.5)
                    self.press_check(1, 120, lambda s: s.get("scene") == "Inspection", "2nd inspect press")
                if "inspection_restart" in sl:
                    time.sleep(1.5)
                    self.press_check(1, INSPECT_STOP_HOLD + 300, lambda s: s.get("scene") == "CompetitorInfo",
                                     "inspection cancelled")
                    t0 = self.start_inspection()
                self.poll(lambda s: False, max(0, t0 + target - int(self.st()["up"])))
                insp_expect = target
            else:
                self.connected()
                time.sleep(1)
                insp_expect = 0
                target = 0
            self.stk("start")
            self.poll(lambda s: s.get("scene") == "Timer", 5000, "timer running")
            if "over_limit" in sl:
                time.sleep(limit / 1000 + 1.5)
            else:
                time.sleep(sp["solve"])
            self.stk("stop")
            shown = self.stackmat.shown
            if "over_limit" in sl:
                exp_time, pen = limit, -1
            else:
                exp_time = shown
                pen = -1 if target >= DNF_MS else 2 if target >= PLUS2_MS else 0
            s = self.poll(lambda s: s.get("scene") == "Finished", 5000, "finished")
        rec["solve_ms"] = exp_time
        if int(s["time"]) != exp_time or norm_pen(s) != pen:
            raise Mismatch(f"after the stop: time {s['time']} pen {s['pen']}, expected {exp_time} ({pen})")

        if "stackmat_reset_early" in sl:
            self.stk("reset")
            time.sleep(1)
            s = self.st()
            if s.get("scene") != "Finished" or int(s["time"]) != exp_time:
                raise Mismatch(f"timer reset before confirming changed the solve: {brief(s)}")
        if blip and blip["at"] == "after_stop":
            self.wifi_blip(blip["ms"])

        if "delegate" in sl:
            self.connected()
            up = int(self.st()["up"])
            # a call within the pending window is ignored; near its end, either is fine
            exp = "no" if up < self.delegate_until - 15000 else "maybe" if up < self.delegate_until + 15000 else "yes"
            log_mark = len(self.emu.since(0))
            self.btn(2, DELEGATE_HOLD + 500)
            if any("Server not trusted" in l for l in self.emu.since(0)[log_mark:]):
                exp = "no"  # just reconnected, trust handshake not done yet
            if exp != "no":
                self.delegate_until = up + DELEGATE_TIMEOUT
            if self.disturbed and exp == "yes":
                exp = "maybe"
            rec["delegates"].append(dict(expect=exp, time=exp_time, pen=pen))

        def pen_step(b, ms, newpen):
            nonlocal pen
            if conf:
                newpen = pen
            self.press_check(b, ms, lambda s: norm_pen(s) == newpen, f"penalty {pen} → {newpen}")
            pen = newpen

        if "plus2_fix" in sl and not conf:
            p0 = pen
            pen_step(4, 120, press_penalty(pen))
            if sl["plus2_fix"] == "dnf_hold" and p0 == 0:
                pen_step(4, DNF_HOLD + 200, hold_penalty(pen))
                pen_step(4, DNF_HOLD + 200, hold_penalty(pen))
            elif p0 == -1:
                pen_step(4, DNF_HOLD + 200, hold_penalty(pen))
            else:  # wrap-around back to where it was
                while pen != p0:
                    pen_step(4, 120, press_penalty(pen))
        if "judge_plus2" in sl:
            pen_step(4, 120, press_penalty(pen))
        if "judge_dnf" in sl:
            pen_step(4, DNF_HOLD + 200, hold_penalty(pen))
        if "wrong_button" in sl:
            before = self.st()
            self.btn(sl["wrong_button"], 300)
            s = self.st()
            if any(s.get(k) != before.get(k) for k in ("scene", "time", "pen", "conf", "comp")):
                raise Mismatch(f"wrong button {sl['wrong_button']} changed the solve: {brief(s)}")

        hold = sl.get("submit_long", 120)
        self.press_check(3, hold, lambda s: s.get("conf") == "1", "time confirmed")
        if "double_submit" in sl:
            time.sleep(0.2)
            self.btn(3, 120)
        s = self.st()
        if int(s["time"]) != exp_time or norm_pen(s) != pen or s.get("scene") != "Finished":
            raise Mismatch(f"after confirming: {brief(s)}, expected {exp_time} ({pen})")

        if "competitor_before_judge" in sl:
            self.scan(C, lambda s: s.get("msg") == "1", "competitor first: 'cards the same' message")
        if "wrong_judge_card" in sl:
            X = sl["wrong_judge_card"]
            self.scan(X, lambda s: s.get("judge") == str(X), "wrong judge card taken as judge")
        self.scan(J, lambda s: s.get("judge") == str(J), "judge scanned", wait=16000)
        self.scan(C, self.idle, "solve sent (back to idle)", wait=16000)
        rec.update(time=exp_time, pen=pen, insp=insp_expect)
        self.stk("reset")

    def start_inspection(self):
        """Inspect press; returns the emulated ms it started at (just before the press)."""
        for _ in range(3):
            self.connected()
            s = self.st()
            if s.get("timer") not in ("Some(0)", "None"):
                raise Mismatch(f"stackmat not at 0 before inspection: {brief(s)}")
            t0 = int(s["up"]) + 30
            self.note("btn", 1, 120)
            self.emu.send("btn 1 120")
            if self.poll(lambda s: s.get("scene") == "Inspection", 2500):
                return t0
            self.dismiss(self.st())
        raise Mismatch(f"inspection did not start: {brief(self.st())}")


def norm_pen(s):
    p = int(s.get("pen", "-99"))
    return 0 if p == -99 else p


def brief(s):
    keys = ("scene", "comp", "judge", "time", "pen", "conf", "group", "groups", "sel", "err", "msg", "srv")
    return " ".join(f"{k}={s.get(k)}" for k in keys)


# ── a whole day ──────────────────────────────────────────────────────────────────────────────

def check_attempt(st, rec, new):
    """New log entries of this station since the attempt began vs. the intended result."""
    errs = []
    sessions = {}
    for e in new:
        if e["delegate"]:
            continue
        key = (e["comp"], e["time"], e["pen"], e["judge"], e["group"])
        if sessions.setdefault(e["session"], key) != key:
            errs.append(f"session {e['session']} sent twice with different data")
    got = list(sessions.values())
    want = (rec["comp"], rec["time"], rec["pen"], rec["judge"], rec["group"])
    if got != [want]:
        errs.append(f"server got {got}, intended {[want]}")
    for e in new:
        if not e["delegate"] and e["comp"] == rec["comp"]:
            if rec["insp"] and abs(e["insp"] - rec["insp"]) > 900:
                errs.append(f"inspection {e['insp']} ms, intended ~{rec['insp']}")
            if not rec["insp"] and e["insp"] != 0:
                errs.append(f"inspection {e['insp']} ms sent for an attempt without inspection")
    dels = [e for e in new if e["delegate"]]
    want_d = [d for d in rec["delegates"] if d["expect"] != "no"]
    sure = [d for d in want_d if d["expect"] == "yes"]
    if not len(sure) <= len(dels) <= len(want_d):
        errs.append(f"delegate calls: server got {len(dels)}, intended {rec['delegates']}")
    for e, d in zip(dels, want_d):
        if (e["comp"], e["group"], e["time"], e["pen"], e["judge"]) != (rec["comp"], rec["group"], d["time"], d["pen"], 0):
            errs.append(f"delegate call {e} != intended {d}")
    return errs


class Day:
    def __init__(self, seed, day, job, args, image):
        self.seed, self.day, self.args = seed, day, args
        self.dir = M.OUT / f"comp-d{day['devices']}" / str(seed)
        if self.dir.exists():
            shutil.rmtree(self.dir)
        self.dir.mkdir(parents=True)
        self.server = M.Server(Path(args.mc).expanduser(), M.SRV_PORT + args.job_base + job, self.dir)
        tests = json.load(open(self.dir / "srv" / "tests.json"))
        tests.update(groups=day["groups"], cards=day["cards"])
        json.dump(tests, open(self.dir / "srv" / "tests.json", "w"))
        self.job, self.image = job, image
        self.records, self.mismatches, self.lock = [], [], threading.Lock()
        self.restart_lock = threading.Lock()
        self.restarts, self.timers, self.heap = 0, [], 0

    def restart_server(self, down, stations):
        with self.restart_lock:
            for st in stations:
                st.disturbed = True
            self.server.stop()
            time.sleep(down)
            self.server.start()
            self.restarts += 1

    def run_station(self, st, stations, barrier, failures):
        try:
            for gi, g in enumerate(self.day["schedule"]):
                for ri, row in enumerate(g["rounds"]):
                    barrier.wait(timeout=3600)
                    if st.n == 0 and g["restart"] and g["restart"]["round"] == ri:
                        t = threading.Timer(g["restart"]["delay"], self.restart_server,
                                            (g["restart"]["down"], stations))
                        self.timers.append(t)
                        t.start()
                    sp = row[st.n] if st.n < len(row) else None
                    if not sp:
                        continue
                    if not g["sync"][ri]:
                        time.sleep(sp["gap"])
                    self.one(st, sp)
                barrier.wait(timeout=3600)
                if st.n == 0:
                    st.note("break", g["break_s"])
                time.sleep(g["break_s"])
        except threading.BrokenBarrierError:
            pass
        except M.Failure as f:
            failures.append(f"station {st.n}: {f}")
            barrier.abort()
        except Exception as e:  # harness bug: stop the day, keep the logs
            failures.append(f"station {st.n}: harness error {e!r}")
            barrier.abort()

    def one(self, st, sp):
        rec = dict(station=st.n, esp=st.esp, comp=sp["comp"], judge=sp["judge"], group=sp["group"],
                   attempt=sp["attempt"], slips=sorted(sp["slips"]), delegates=[], abandoned=0,
                   time=None, pen=None, insp=None)
        mark = len(entries(self.dir))
        st.note("attempt", sp["comp"], sp["group"], sp["attempt"])
        try:
            st.attempt(sp, self.day, rec)
            time.sleep(1.0)
            new = [e for e in entries(self.dir)[mark:] if e["esp"] == st.esp]
            errs = check_attempt(st, rec, new)
        except Mismatch as m:
            errs = [str(m)]
        rec["ok"] = not errs
        with self.lock:
            self.records.append(rec)
            for e in errs:
                self.mismatches.append(f"station {st.n} {sp['comp']} {sp['group']} #{sp['attempt']}: {e}")
                print(f"  [{self.seed}] MISMATCH station {st.n} {sp['comp']} {sp['group']} #{sp['attempt']}: {e}",
                      flush=True)
        if errs:
            st.stackmat.cmd("stop")
            st.recover()
            rec["recovered"] = True
        st.check_alive()

    def execute(self):
        n = self.day["devices"]
        self.server.start()
        stations, failures = [], []
        try:
            for i in range(n):
                base = 20000 + (self.args.job_base + self.job * n + i) * 10
                stations.append(Station(i, self.dir, self.server, self.image,
                                        dict(http=base, uart1=base + 1, control=base + 2), self.args,
                                        self.restart_lock))
            for st in stations:
                st.boot()
                st.esp = int(st.st()["id"], 16)
            if len({st.esp for st in stations}) != n:
                raise M.Failure(f"stations share a device id: {[st.esp for st in stations]}")
            barrier = threading.Barrier(n)
            threads = [threading.Thread(target=self.run_station, args=(st, stations, barrier, failures))
                       for st in stations]
            for t in threads:
                t.start()
            for t in threads:
                t.join()
            if not failures:
                for st in stations:
                    st.check_alive()
                    st.check_errors(st.st())
                self.final_check(stations)
        except M.Failure as f:
            failures.append(str(f))
        finally:
            for t in self.timers:
                t.cancel()
            for st in stations:
                self.heap = max(self.heap, st.heap_peak)
                st.stackmat.alive = False
                st.emu.stop()
            with self.restart_lock:
                self.server.stop()
            self.save(stations)
        return failures

    def final_check(self, stations):
        """The server's whole list (deduplicated by session) equals the intended list."""
        esps = {st.esp: st.n for st in stations}
        by_session = {}
        for e in entries(self.dir):
            if e["esp"] not in esps:
                self.mismatches.append(f"final: solve from unknown device {e['esp']}: {e}")
            if not e["delegate"]:
                by_session.setdefault(e["session"], (esps.get(e["esp"]), e["comp"], e["time"], e["pen"], e["judge"], e["group"]))
        got = Counter(by_session.values())
        want = Counter((r["station"], r["comp"], r["time"], r["pen"], r["judge"], r["group"])
                       for r in self.records if r.get("time") is not None)
        missing, extra = want - got, got - want
        for k in missing.elements():
            self.mismatches.append(f"final: intended solve missing on the server: {k}")
        for k in extra.elements():
            self.mismatches.append(f"final: unintended solve on the server: {k}")

    def save(self, stations):
        json.dump(dict(seed=self.seed, mode="competition", day=self.day, intended=self.records,
                       mismatches=self.mismatches,
                       executed=sorted(sum((st.executed for st in stations), []))),
                  open(self.dir / "day.json", "w"), indent=0)

    def summary(self, failures):
        slips = Counter(s for r in self.records for s in r["slips"])
        return dict(seed=self.seed, attempts=len(self.records),
                    ok=sum(1 for r in self.records if r["ok"]), slips=dict(slips),
                    mismatches=self.mismatches, failures=failures, restarts=self.restarts, heap=self.heap)


def run_day(seed, day, job, args, image):
    d = Day(seed, day, job, args, image)
    return d.summary(d.execute())


def main(args):
    if args.replay_file:
        data = json.load(open(args.replay_file))
        work = [(data["seed"], data["day"])]
    else:
        if args.seed is not None or args.replay is not None:
            seeds = [args.seed if args.seed is not None else args.replay]
        else:
            seeds = []
            for part in args.seeds.split(","):
                lo, _, hi = part.partition("-")
                seeds += range(int(lo), int(hi or lo) + 1)
        work = [(s, generate_day(s, args.devices, args.max_attempts, args.slip_rate, args.time_scale))
                for s in seeds]
    jobs = max(1, min(args.jobs, len(work)))
    images = {}
    for j in range(jobs):
        port = M.SRV_PORT + args.job_base + j
        t = M.target(args.hw)
        images[j] = ((t / f"monkey-{port}.bin", t / f"monkey-{port}.elf") if args.no_build
                     else M.build(port, args.hw))
    results, queue, lock = [], list(work), threading.Lock()

    def worker(j):
        while True:
            with lock:
                if not queue:
                    return
                seed, day = queue.pop(0)
            print(f"seed {seed}: {day['attempts']} attempts in {len(day['schedule'])} groups, "
                  f"{day['devices']} station(s)", flush=True)
            s = run_day(seed, day, j, args, images[j])
            print(f"seed {seed}: {'FAIL' if s['failures'] or s['mismatches'] else 'ok'} "
                  f"({s['ok']}/{s['attempts']} attempts clean)", flush=True)
            with lock:
                results.append(s)

    threads = [threading.Thread(target=worker, args=(j,)) for j in range(jobs)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    print()
    bad = 0
    for s in sorted(results, key=lambda s: s["seed"]):
        bad += bool(s["failures"] or s["mismatches"])
        print(f"seed {s['seed']}: {s['ok']}/{s['attempts']} attempts ok, {s['restarts']} server restarts, "
              f"heap peak {s['heap']}")
        print(f"  slips: {', '.join(f'{k} {v}' for k, v in sorted(s['slips'].items())) or '-'}")
        for m in s["mismatches"] + s["failures"]:
            print(f"  ! {m}")
    print(f"\n{len(results) - bad}/{len(results)} days clean (logs: sim/out/comp-d{work[0][1]['devices']}/<seed>/)")
    return 1 if bad else 0


if __name__ == "__main__":
    # self-check: generation is deterministic, distinct people, attempts capped
    a, b = generate_day(3, 2, 40), generate_day(3, 2, 40)
    assert a == b and generate_day(4, 2, 40) != a and a["attempts"] <= 40
    assert len(a["cards"]) == len(set(a["cards"]))
    for g in a["schedule"]:
        for row in g["rounds"]:
            people = [x["comp"] for x in row if x] + [x["judge"] for x in row if x]
            assert len(people) == len(set(people)), row
    rec = dict(comp=1, time=900, pen=2, judge=3, group="g", insp=16000, delegates=[])
    e = dict(comp=1, time=900, pen=2, judge=3, group="g", insp=16100, delegate=False, session="s")
    assert check_attempt(None, rec, [e, e]) == [] and check_attempt(None, dict(rec, judge=4), [e])
    assert check_attempt(None, rec, []) and check_attempt(None, rec, [e, dict(e, session="t")])
    assert press_penalty(16) == 0 and press_penalty(-1) == 0 and hold_penalty(2) == -1
    print("ok")
