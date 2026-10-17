"""Competition day for sim/monkey.py (`--mode competition`): clumsy but well-meaning competitors
and judges run a seeded competition on one or more stations (each its own esp-emu, all on one
micro-connector), and the server must get exactly the intended results.

Every human action is a planned step with a defined outcome. The oracle models the firmware rules
(src/buttons.rs, src/rfid.rs, src/stackmat.rs) independently and checks the device state after each
step. micro-connector's e2e plays FKMTime (`sim-e2e` branch, Server(sim=True)): a delegate answers
each call with a seeded decision, cutoffs end rounds early, answers get lost. The oracle keeps the
attempts FKMTime should hold (`Ledger`) and compares them with e2e's after each attempt and at the
end: station, competitor, judge, group, attempt number, extra, status, value, penalty, replacement
and inspection; every session's sends must carry the same data.
"""
import contextlib
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
DELEGATE_TIMEOUT = 5 * 60_000  # src/consts.rs: the device waits this long for the delegate
# what the delegate does with a call (FKMTime incident page), relative weights
DECISIONS = {
    "accept": 3,     # resolved, time stands; the cards confirm it
    "change": 3,     # resolved with another time / penalty; the cards confirm it
    "no_cards": 1.5,  # resolved, "do not require cards": the device resets
    "extra": 2,      # extra attempt given: the device resets, the competitor solves again
    "delete": 1,     # incident deleted: the device resets, the attempt is redone
    "late": 0.4,     # answers only after the device gave up (5 min): the device ignores it
}
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
    "delegate": 0.03,              # judge calls the delegate (3.5 s hold); x --delegate-rate
    "lost_reply": 0.02,            # the answer to the solve gets lost: error, resend refused
    "wifi_blip": 0.02,             # radio off 1-4 s on this station only
    "abandon": 0.01,               # attempt reset before inspection, redone right away
    "over_limit": 0.10,            # (limited events) solve runs past the time limit: DNF
    "plus2": 0.05,                 # legit judge decisions
    "dnf": 0.02,
}


# ── generation (pure: same seed and devices ⇒ same day) ────────────────────────────────────

def generate_day(seed: int, devices: int, max_attempts: int, slip_rate: float = 1.0,
                 time_scale: float = 0.5, delegate_rate: float = 1.0) -> dict:
    r = random.Random(seed * 1000 + devices)
    uids = set()

    def uid():
        while True:
            u = r.randint(10_000, 2**32 - 1)
            if u not in uids and u not in M.KNOWN_CARDS:
                uids.add(u)
                return u

    events = [EVENTS[0]] + r.sample(EVENTS[1:4], r.randint(1, 2)) + [EVENTS[4]]
    # FKMTime's rounds (centiseconds): one short event may have a cutoff after 2 attempts
    round_info = {f"{e[0]}-r1": dict(attempts=e[4], limit=e[3] // 10 if e[3] else None, cutoff=None)
              for e in events}
    if r.random() < 0.6:
        e = events[1]
        round_info[f"{e[0]}-r1"]["cutoff"] = dict(attempts=2, value=int(e[5] * time_scale * 85))
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
                                        len(comp_groups[c]) > 1, slip_rate, others, staff, comps,
                                        delegate_rate))
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
                rounds=round_info, schedule=schedule, attempts=total)


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


def timing(r, insp, limit, mean, sd, scale):
    """Inspection (ms, None without) and solve (s) of one attempt."""
    x = r.random()
    insp_ms = None
    if insp:  # firmware thresholds are never scaled; targets keep ≥ 0.6 s away from them
        insp_ms = (r.randint(5000, 12000) if x < 0.90 else
                   r.randint(15600, 16400) if x < 0.97 else r.randint(17800, 19000))
    solve = max(0.8, r.gauss(mean, sd)) * scale
    if limit:
        solve = min(solve, limit / 1000 - 3)
    return insp_ms, round(solve, 2)


def attempt_spec(r, comp, judge, gid, k, insp, limit, mean, sd, scale, multi, rate, others, staff,
                 comps, delegate_rate=1.0):
    def slip(name):
        return r.random() < SLIPS[name] * rate * (delegate_rate if name == "delegate" else 1)

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
        # mostly after the solve (a judging question); sometimes before it (puzzle, scramble)
        at = "before_inspection" if r.random() < 0.15 else "after_stop"
        opts = DECISIONS if at == "after_stop" else {"extra": 1, "delete": 1}
        insp_r, solve_r = timing(r, insp, limit, mean, sd, scale)
        sl["delegate"] = dict(at=at, decision=r.choices(list(opts), list(opts.values()))[0],
                              think=round(r.choice([1, 3, 8, 20, 45]) * r.uniform(0.8, 1.2), 1),
                              delta=r.randint(-150, 150), pen=r.choice([0, 0, 2, -1]),
                              redo=dict(insp=insp_r, solve=solve_r))
    if slip("lost_reply"):
        sl["lost_reply"] = True
    if slip("wrong_judge_card"):
        sl["wrong_judge_card"] = r.choice([x for x in comps + staff if x not in (comp, judge)])
    if slip("wrong_button"):
        sl["wrong_button"] = r.choice([2, 1])
    if slip("wifi_blip"):
        sl["wifi_blip"] = dict(at=r.choice(["before_scan", "after_stop"]), ms=r.randint(1000, 4000))
    decision = "dnf" if slip("dnf") else "plus2" if slip("plus2") else None
    if decision:
        sl["judge_" + decision] = True

    insp_ms, solve = timing(r, insp, limit, mean, sd, scale)
    return dict(comp=comp, judge=judge, group=gid, attempt=k, insp=insp_ms, solve=solve,
                gap=round(r.choice([0.5, 1, 2, 3, 5]) * r.uniform(0.8, 1.2), 2), slips=sl)


# ── what FKMTime should hold ──────────────────────────────────────────────────────────────────

def round_of(group: str) -> str:
    return group.split("-g")[0]


def requests(log: list) -> list:
    """EnterAttempt requests in e2e's log, with their answer (None if lost)."""
    out = []
    for e in log:
        req = e.get("req") or {}
        if req.get("type") != "EnterAttempt":
            continue
        d = req["data"]
        resp = e.get("resp") or {}
        out.append(dict(time=d["valueMs"], pen=d["penalty"], esp=d["espId"], judge=int(d["judgeId"] or 0),
                        comp=int(d["competitorId"]), delegate=d["isDelegate"], session=d["sessionId"],
                        insp=d["inspectionTime"], group=d["groupId"],
                        answer=(resp.get("data") or {}).get("message")))
    return out


class Ledger:
    """The attempts FKMTime should hold (its rules: numbering, extra attempts replacing the
    original, time limit DNF, cutoff and finished rounds), built from what the people did."""

    def __init__(self, day):
        self.rows, self.day, self.lock = [], day, threading.Lock()
        self.rounds = day.get("rounds", {})  # days saved before rounds existed: unlimited

    def mine(self, comp, rnd):
        return [x for x in self.rows if x["comp"] == comp and x["round"] == rnd]

    def enter(self, esp, comp, judge, group, ms, pen, insp, status="standard"):
        with self.lock:
            rnd = round_of(group)
            mine = self.mine(comp, rnd)
            orig = next((x for x in mine if x["status"] == "extra_given" and x["replaced_by"] is None), None)
            if orig:
                number = sum(x["extra"] for x in mine) + 1
                orig["replaced_by"] = number
            else:
                number = max([x["number"] for x in mine if not x["extra"]], default=0) + 1
            limit = (self.rounds.get(rnd) or {}).get("limit")
            row = dict(esp=esp, comp=comp, judge=judge, group=group, round=rnd, number=number,
                       extra=orig is not None, status=status, value=ms // 10,
                       pen=-1 if limit and ms // 10 >= limit else pen, insp=insp, replaced_by=None)
            self.rows.append(row)
            return row

    def remove(self, row):
        with self.lock:
            self.rows.remove(row)

    def finished(self, comp, rnd):
        info = self.rounds.get(rnd) or {}
        if not info.get("attempts"):
            return False
        done = [x for x in self.mine(comp, rnd) if x["status"] != "extra_given"]
        cut = info.get("cutoff")
        passed = (not cut or len(done) < cut["attempts"] or
                  any(x["pen"] != -1 and x["value"] + x["pen"] * 100 < cut["value"] for x in done))
        return len(done) >= (info["attempts"] if passed else cut["attempts"])

    def open_groups(self, comp):
        """The competitor's groups whose round isn't over (FKMTime offers only those)."""
        mine = self.day["cards"][str(comp)]["groups"]
        return [g["groupId"] for g in self.day["groups"]
                if g["groupId"] in mine and not self.finished(comp, round_of(g["groupId"]))]

    def rejects(self, comp):
        """FKMTime answers this competitor's card with "No attempts left" (and a reset)."""
        return bool(self.day["cards"][str(comp)]["groups"]) and not self.open_groups(comp)


def row_key(x):
    return (x["esp"], x["comp"], x["judge"], x["group"], x["number"], x["extra"], x["status"],
            x["value"], x["pen"], x["replaced_by"])


def db_key(x):
    return (x["espId"], x["competitorId"], x["judgeId"], x["groupId"], x["attemptNumber"], x["extra"],
            x["status"], x["value"], x["penalty"], x["replacedBy"])


def compare(want_rows, db_rows, what):
    errs = []
    want, got = Counter(map(row_key, want_rows)), Counter(map(db_key, db_rows))
    for k in (want - got).elements():
        errs.append(f"{what}: intended attempt missing on the server: {k}")
    for k in (got - want).elements():
        errs.append(f"{what}: unintended attempt on the server: {k}")
    for x in want_rows:
        for d in db_rows:
            if db_key(d) == row_key(x) and x["insp"] is not None:
                if x["insp"] and abs(d["inspectionTime"] - x["insp"]) > 900:
                    errs.append(f"{what}: inspection {d['inspectionTime']} ms, intended ~{x['insp']}")
                if not x["insp"] and d["inspectionTime"] != 0:
                    errs.append(f"{what}: inspection {d['inspectionTime']} ms sent for an attempt without inspection")
    return errs


def same_sessions(reqs):
    """A session's (non-delegate) sends must all carry the same solve."""
    errs, seen = [], {}
    for e in reqs:
        if e["delegate"]:
            continue
        key = (e["comp"], e["time"], e["pen"], e["judge"], e["group"])
        if seen.setdefault(e["session"], key) != key:
            errs.append(f"session {e['session']} sent twice with different data: {seen[e['session']]} vs {key}")
    return errs


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
        self.esp, self.ledger = None, None
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
            lost = []  # the press may have hit a short disconnect that is over by now

            def seen(s):
                if s.get("srv") != "Some(true)" or s.get("stk") != "Some(true)":
                    lost.append(s)
                return pred(s)
            s = self.poll(seen, 2500)
            if s:
                return s
            s = self.st()
            if lost or s.get("srv") != "Some(true)" or s.get("stk") != "Some(true)":
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
        """One attempt as planned; returns the spec of a redo the delegate asked for, or None."""
        C, J, G, sl = sp["comp"], sp["judge"], sp["group"], sp["slips"]
        group = next(g for g in day["groups"] if g["groupId"] == G)
        led = self.ledger
        mine = led.open_groups(C)  # FKMTime only offers groups whose round isn't over
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
            if day["cards"][str(W)]["canCompete"] and led.rejects(W):
                self.rejected_scan(W)
            elif day["cards"][str(W)]["canCompete"]:
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

        dl = sl.get("delegate")
        if dl and dl["at"] == "before_inspection":
            out = self.delegate(dl, rec, C, G, 0, 0, 0)
            if out and out["kind"] == "redo":
                return out["redo"]

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

        out = None
        if dl and dl["at"] == "after_stop":
            out = self.delegate(dl, rec, C, G, exp_time, pen, insp_expect)
            if out and out["kind"] in ("redo", "idle"):
                rec.update(time=exp_time, pen=pen, insp=insp_expect)
                self.stk("reset")
                return out.get("redo")
            if out and out["kind"] == "cards":  # the delegate's decision, confirmed
                exp_time, pen, conf = out["time"], out["pen"], True

        def pen_step(b, ms, newpen):
            nonlocal pen
            if conf:
                newpen = pen
            self.press_check(b, ms, lambda s: norm_pen(s) == newpen, f"penalty {pen} → {newpen}")
            pen = newpen

        if not out:
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

        if self.st().get("conf") != "1":
            hold = sl.get("submit_long", 120) if not out else 120
            self.press_check(3, hold, lambda s: s.get("conf") == "1", "time confirmed")
            if "double_submit" in sl and not out:
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

        if out and out["kind"] == "late":
            # the incident is still open on the server: it refuses the attempt's send
            self.connected()
            self.tap(C)
            self.poll(lambda s: s.get("err") == "1", 16000, "resend of an open incident refused")
            self.dismiss(self.st())
            self.to_idle()
            self.late_answer(dl, out, rec)
            rec.update(time=exp_time, pen=pen, insp=insp_expect)
            self.stk("reset")
            return None

        row = out["row"] if out else None
        self.submit(C, J, G, rec, row, lost="lost_reply" in sl,
                    entry=dict(ms=exp_time, pen=pen, insp=insp_expect))
        rec.update(time=exp_time, pen=pen, insp=insp_expect)
        self.stk("reset")
        return None

    def submit(self, C, J, G, rec, row, lost, entry):
        """The competitor's confirming tap sends the attempt (`row`: the resolved incident it
        confirms). Returns once the server holds it and the device is idle again."""
        led, rnd = self.ledger, round_of(G)

        def on_server():
            db = [x for x in self.server.attempts() if x["competitorId"] == C and x["roundId"] == rnd]
            if row:
                return any(x["status"] == "resolved" and x["judgeId"] == J and x["espId"] == self.esp
                           for x in db)
            return len(db) > len(led.mine(C, rnd))

        with self.restart_lock if lost else contextlib.nullcontext():
            if lost:
                self.connected()
                self.server.ctl(cmd="drop_reply", espId=self.esp)
                rec["lost_reply"] = True
            for _ in range(4):
                self.connected()
                self.tap(C)
                s = self.poll(lambda s: self.idle(s) or s.get("err") == "1", 16000)
                if s and s.get("err") != "1":
                    break
                if on_server():
                    # entered, but the answer got lost: a retap is refused ("already entered")
                    self.dismiss(self.st())
                    self.tap(C)
                    self.poll(lambda s: s.get("err") == "1", 16000, "resend refused (already entered)")
                    self.dismiss(self.st())
                    self.to_idle()
                    break
                self.disturbed = True  # lookup lost to a restart: tap again
                self.dismiss(self.st())
            else:
                raise Mismatch(f"solve not sent after 4 taps: {brief(self.st())}")
        if not on_server():
            raise Mismatch("the attempt is not on the server")
        if row:
            row["judge"] = J
        else:
            led.enter(self.esp, C, J, G, entry["ms"], entry["pen"], entry["insp"])

    def rejected_scan(self, C):
        """FKMTime answers "No attempts left" and makes the device reset."""
        for _ in range(4):
            self.connected()
            self.tap(C)
            if self.poll(lambda s: s.get("err") == "1" and self.idle(s), 14000):
                break
            self.dismiss(self.st())
        else:
            raise Mismatch(f"finished competitor {C} was not refused: {brief(self.st())}")
        self.dismiss(self.st())
        self.poll(lambda s: self.idle(s) and s.get("err") == "0", 5000, "idle after the refusal")

    def delegate(self, dl, rec, C, G, ms, pen, insp):
        """The judge calls the delegate (3.5 s hold); the device waits, the delegate decides after
        thinking. Returns None (call ignored) or the outcome: cards (decision shown, confirmed),
        idle (resolved without cards), redo (extra attempt / deleted), late (device gave up)."""
        led = self.ledger
        with self.restart_lock:  # no server restart between the call and its incident
            self.connected()
            mark = len(self.emu.since(0))
            known = {x["sessionId"] for x in self.server.attempts()}
            self.btn(2, DELEGATE_HOLD + 500)
            if any("Server not trusted" in l for l in self.emu.since(0)[mark:]):
                rec["delegates"].append(dict(decision="ignored (server not trusted yet)"))
                return None
            inc = None
            for _ in range(30):
                inc = next((x for x in self.server.attempts() if x["sessionId"] not in known
                            and x["espId"] == self.esp and x["status"] == "unresolved"), None)
                if inc:
                    break
                time.sleep(0.5)
            if not inc or inc["competitorId"] != C:
                raise Mismatch(f"delegate call not on the server: {inc}")
            row = led.enter(self.esp, C, None, G, ms, pen, insp, status="unresolved")
            s = self.st()
            if s.get("dhold") != "3":
                raise Mismatch(f"'waiting for delegate' gone while the call is pending: {brief(s)}")
        d = dict(decision=dl["decision"], at=dl["at"], session=inc["sessionId"])
        rec["delegates"].append(d)
        if dl["decision"] == "late":
            # the device gives up after DELEGATE_TIMEOUT; the solve stays as it was
            s = self.poll(lambda s: s.get("dhold") == "-1", DELEGATE_TIMEOUT + 30_000, "device stops waiting")
            if s.get("scene") != "Finished" or int(s["time"]) != ms:
                raise Mismatch(f"the solve changed when the device stopped waiting: {brief(s)}")
            return dict(kind="late", row=row, session=inc["sessionId"])

        time.sleep(dl["think"])
        decision = dl["decision"]
        value, newpen = row["value"], row["pen"]
        if decision == "change":
            value, newpen = max(1, value + dl["delta"]), dl["pen"]
        with self.restart_lock:  # the decision reaches the device
            self.connected()
            if decision == "delete":
                r = self.server.ctl(cmd="delete", sessionId=inc["sessionId"])
            else:
                status = "extra_given" if decision == "extra" else "resolved"
                r = self.server.ctl(cmd="resolve", sessionId=inc["sessionId"], status=status, value=value,
                                    penalty=newpen, noCards=decision == "no_cards")
            if not r.get("ok") or not r.get("delivered"):
                raise Mismatch(f"delegate decision not delivered: {r}")
            if decision == "delete":
                led.remove(row)
            else:
                row.update(status=status, value=value, pen=newpen)
            if decision in ("accept", "change"):
                self.poll(lambda s: s.get("scene") == "Finished" and s.get("conf") == "1" and s.get("dhold") == "-1"
                          and int(s["time"]) == value * 10 and norm_pen(s) == newpen, 10000,
                          "delegate's decision on the device")
                return dict(kind="cards", time=value * 10, pen=newpen, row=row)
            self.poll(lambda s: self.idle(s) and s.get("time") == "-1" and s.get("dhold") == "-1", 10000,
                      "back to idle after the decision")
        if decision == "no_cards":
            return dict(kind="idle")
        redo = dict(rec["spec"], insp=dl["redo"]["insp"], solve=dl["redo"]["solve"], slips={},
                    redo=decision)
        return dict(kind="redo", redo=redo)

    def late_answer(self, dl, out, rec):
        """The delegate answers after the device gave up: the device must ignore it."""
        row = out["row"]
        value, newpen = max(1, row["value"] + dl["delta"]), dl["pen"]
        with self.restart_lock:
            self.connected()
            r = self.server.ctl(cmd="resolve", sessionId=out["session"], status="resolved", value=value,
                                penalty=newpen)
            if not r.get("ok"):
                raise Mismatch(f"late decision refused: {r}")
            row.update(status="resolved", value=value, pen=newpen)
            time.sleep(3)
            s = self.st()
            if not self.idle(s) or s.get("time") != "-1":
                raise Mismatch(f"a late delegate answer changed the device: {brief(s)}")

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

class Day:
    def __init__(self, seed, day, job, args, image):
        self.seed, self.day, self.args = seed, day, args
        self.dir = M.OUT / f"comp-d{day['devices']}" / str(seed)
        if self.dir.exists():
            shutil.rmtree(self.dir)
        self.dir.mkdir(parents=True)
        self.server = M.Server(Path(args.mc).expanduser(), M.SRV_PORT + args.job_base + job, self.dir,
                               sim=True, extra=dict(groups=day["groups"], cards=day["cards"],
                                                    rounds=day.get("rounds", {})))
        self.ledger, self.refused = Ledger(day), set()
        self.job, self.image = job, image
        self.records, self.mismatches, self.lock = [], [], threading.Lock()
        self.restart_lock = threading.Lock()
        self.restarts, self.timers, self.heap = 0, [], 0

    def restart_server(self, down, stations):
        with self.restart_lock:
            for st in stations:
                st.disturbed = True
            self.server.restart(down)
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
        led, C, rnd = self.ledger, sp["comp"], round_of(sp["group"])
        if led.finished(C, rnd):
            # the round ended early (cutoff): staff tell them, but some come once anyway
            if (C, rnd) in self.refused or not led.rejects(C):
                st.note("skip (round over)", C, sp["group"], sp["attempt"])
                return
            self.refused.add((C, rnd))
            st.note("refused", C, sp["group"], sp["attempt"])
            rec = dict(station=st.n, esp=st.esp, comp=C, group=sp["group"], attempt=sp["attempt"],
                       slips=["refused (no attempts left)"], delegates=[])
            try:
                st.rejected_scan(C)
                errs = []
            except Mismatch as m:
                errs = [str(m)]
            self.record(st, sp, rec, errs)
            return
        while sp:
            rec = dict(station=st.n, esp=st.esp, comp=C, judge=sp["judge"], group=sp["group"],
                       attempt=sp["attempt"], slips=sorted(sp["slips"]) + ([f"redo ({sp['redo']})"] if
                       sp.get("redo") else []), delegates=[], abandoned=0, time=None, pen=None, insp=None,
                       spec=sp)
            mark = len(self.server.log())
            st.note("attempt", C, sp["group"], sp["attempt"])
            redo = None
            try:
                redo = st.attempt(sp, self.day, rec)
                time.sleep(1.0)
                errs = self.check(st, C, rnd, mark)
            except Mismatch as m:
                errs = [str(m)]
            self.record(st, sp, rec, errs)
            if errs:
                st.stackmat.cmd("stop")
                st.recover()
                rec["recovered"] = True
                redo = None
                with led.lock:  # carry on from what the server holds (the mismatch is reported)
                    led.rows = [x for x in led.rows if not (x["comp"] == C and x["round"] == rnd)] + [
                        dict(esp=x["espId"], comp=C, judge=x["judgeId"], group=x["groupId"], round=rnd,
                             number=x["attemptNumber"], extra=x["extra"], status=x["status"], value=x["value"],
                             pen=x["penalty"], insp=None, replaced_by=x["replacedBy"])
                        for x in self.server.attempts() if x["competitorId"] == C and x["roundId"] == rnd]
            st.check_alive()
            sp = redo

    def check(self, st, C, rnd, mark):
        """What the server holds for this competitor's round, and this attempt's sends."""
        db = [x for x in self.server.attempts() if x["competitorId"] == C and x["roundId"] == rnd]
        errs = compare(self.ledger.mine(C, rnd), db, "attempt")
        return errs + same_sessions([e for e in requests(self.server.log()[mark:]) if e["esp"] == st.esp])

    def record(self, st, sp, rec, errs):
        rec["ok"] = not errs
        rec.pop("spec", None)
        with self.lock:
            self.records.append(rec)
            for e in errs:
                self.mismatches.append(f"station {st.n} {sp['comp']} {sp['group']} #{sp['attempt']}: {e}")
                print(f"  [{self.seed}] MISMATCH station {st.n} {sp['comp']} {sp['group']} #{sp['attempt']}: {e}",
                      flush=True)

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
                st.esp, st.ledger = int(st.st()["id"], 16), self.ledger
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
        """The server holds exactly the intended attempts; every session's sends agree."""
        esps = {st.esp for st in stations}
        db = self.server.attempts()
        for x in db:
            if x["espId"] not in esps:
                self.mismatches.append(f"final: attempt from unknown device {x['espId']}: {x}")
        self.mismatches += compare(self.ledger.rows, db, "final")
        self.mismatches += same_sessions(requests(self.server.log()))

    def save(self, stations):
        json.dump(dict(seed=self.seed, mode="competition", day=self.day, intended=self.records,
                       mismatches=self.mismatches,
                       executed=sorted(sum((st.executed for st in stations), []))),
                  open(self.dir / "day.json", "w"), indent=0)

    def summary(self, failures):
        slips = Counter(s for r in self.records for s in r["slips"])
        decisions = Counter(d["decision"] for r in self.records for d in r["delegates"])
        return dict(seed=self.seed, attempts=len(self.records), decisions=dict(decisions),
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
        work = [(s, generate_day(s, args.devices, args.max_attempts, args.slip_rate, args.time_scale,
                                 args.delegate_rate)) for s in seeds]
    jobs = max(1, min(args.jobs, len(work)))
    images = {}
    for j in range(jobs):
        port = M.SRV_PORT + args.job_base + j
        t = M.ROOT / "target/sim"
        images[j] = ((t / f"monkey-{port}.bin", t / f"monkey-{port}.elf") if args.no_build
                     else M.build(port))
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
        print(f"  delegate: {', '.join(f'{k} {v}' for k, v in sorted(s['decisions'].items())) or '-'}")
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
    # ledger: numbering, extra replacing the original, limit DNF, cutoff ends the round
    day = dict(rounds={"x-r1": dict(attempts=3, limit=500, cutoff=dict(attempts=1, value=100))},
               cards={"1": dict(groups=["x-r1-g1"])}, groups=[dict(groupId="x-r1-g1")])
    led = Ledger(day)
    first = led.enter(5, 1, 3, "x-r1-g1", 1200, 0, 8000, status="unresolved")
    assert led.open_groups(1) == [] and led.rejects(1)  # 12.00 misses the 1.00 cutoff
    first["status"] = "extra_given"
    assert not led.finished(1, "x-r1")
    extra = led.enter(5, 1, 3, "x-r1-g1", 6000, 2, 8000)
    assert (extra["number"], extra["extra"], extra["pen"], first["replaced_by"]) == (1, True, -1, 1)
    db = [dict(espId=5, competitorId=1, judgeId=3, groupId="x-r1-g1", attemptNumber=n, extra=x, status=st,
               value=v, penalty=p, replacedBy=rb, inspectionTime=8100) for n, x, st, v, p, rb in
          [(1, False, "extra_given", 120, 0, 1), (1, True, "standard", 600, -1, None)]]
    assert compare(led.rows, db, "t") == [] and compare(led.rows, db[:1], "t")
    assert compare(led.rows, [dict(db[0], inspectionTime=0), db[1]], "t")
    e = dict(comp=1, time=900, pen=2, judge=3, group="g", delegate=False, session="s")
    assert same_sessions([e, e, dict(e, delegate=True, time=0)]) == [] and same_sessions([e, dict(e, pen=0)])
    assert press_penalty(16) == 0 and press_penalty(-1) == 0 and hold_penalty(2) == -1
    print("ok")
