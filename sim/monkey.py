#!/usr/bin/env python3
"""Seeded monkey tester: abuses the real firmware in esp-emu and checks it never crashes,
reboots, hangs or softlocks, and never sends inconsistent solves to the server.

  python3 sim/monkey.py --seeds 1-20 --jobs 4 --minutes 4     # fuzz
  python3 sim/monkey.py --replay 7                            # rerun one seed (same actions)
  python3 sim/monkey.py --replay-file sim/out/7/actions.json --shrink
  python3 sim/monkey.py --hw v3 --seeds 1-6                   # v3 hardware (character LCD)

Needs: esp-emu on PATH (or ~/.local/bin), a built micro-connector (--mc, default
~/projects/micro-connector), the sim patches (see sim/README.md). Failures go to sim/out/<seed>/.
"""
import argparse
import json
import os
import random
import re
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

from bus import Emu  # esp-emu + the bus tap and device models

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "sim" / "out"

COMPETITOR, JUDGE, MULTI = 69420, 3004425529, 1264825046  # micro-connector tests.json cards
KNOWN_CARDS = {COMPETITOR, JUDGE, MULTI}
# real thresholds (src/buttons.rs) and their neighbours
HOLDS = [5, 20, 29, 31, 80, 150, 400, 990, 1010, 1500, 2990, 3010, 4500, 9990, 10010, 12500]
SLOW = 3  # emulated time runs up to ~3x slower than real time with several instances
SRV_PORT = 18100  # per-job server port base, away from the default 8080 other sims use
HEAP_LIMIT = 0.92  # of the heap size printed by the firmware
# v3: the emulated LCD may differ from what the firmware wrote only this long (emulated ms)
LCD_STALE_MS = 4000
IDLE = dict(scene="WaitingForCompetitor", comp="-1", time="-1", menu="None", cfg="-1", err="0")


# ── action generation (pure: same seed ⇒ same list) ─────────────────────────────────────────

def generate(seed: int, count: int, hw: str = "v4") -> list:
    """Weighted random actions, biased towards abuse. Each action: [delay_ms, kind, *args].
    v3 adds LCD faults (lost E edge, lost write, module brownout) the display must heal from."""
    r = random.Random(seed)
    acts = []

    def add(delay, *a):
        acts.append([delay, *a])

    weights = {
        "btn": 26, "mash": 8, "multi": 5, "card": 14, "spam": 6,
        "stk": 14, "stk_chaos": 5, "bat": 2, "wait": 8, "flow": 10,
        "server": 1, "power": 1, "menu": 3, "wifi": 2, "outage": 1,
    }
    if hw == "v3":
        weights["lcd"] = 2
    kinds, w = zip(*weights.items())
    while len(acts) < count:
        kind = r.choices(kinds, w)[0]
        gap = r.choice([0, 0, 10, 40, 150, 400, 900, 2000])
        if kind == "btn":
            add(gap, "btn", r.randint(1, 4), r.choice(HOLDS))
        elif kind == "mash":
            for _ in range(r.randint(3, 12)):
                add(r.choice([0, 5, 20, 60]), "btn", r.randint(1, 4), r.choice([5, 20, 40, 120]))
        elif kind == "multi":
            hold = r.choice(HOLDS)
            for b in r.sample([1, 2, 3, 4], r.randint(2, 4)):
                add(0, "btn", b, hold)
            add(gap, "wait", 0)
        elif kind == "card":
            add(gap, "card", r.choice([COMPETITOR, COMPETITOR, JUDGE, MULTI, r.randint(1, 2**32 - 1)]))
        elif kind == "spam":
            card = r.choice([COMPETITOR, JUDGE, MULTI])
            for _ in range(r.randint(2, 6)):
                add(r.choice([0, 30, 200]), "card", card if r.random() < 0.7 else r.choice(list(KNOWN_CARDS)))
        elif kind == "stk":
            add(gap, "stk", r.choice(["start", "stop", "reset", "off", "on", "start", "stop"]))
        elif kind == "stk_chaos":
            for _ in range(r.randint(2, 6)):
                add(r.choice([0, 30, 120, 600]), "stk", r.choice(["start", "stop", "reset", "garbage", "off", "on"]))
        elif kind == "bat":
            add(gap, "bat", r.randint(0, 100))
        elif kind == "wait":
            add(r.choice([500, 1500, 3000, 6000]), "wait", 0)
        elif kind == "flow":
            # a plausible solve with random deviations, so deeper states get reached
            card = r.choice([COMPETITOR, COMPETITOR, MULTI])
            add(gap, "card", card)
            if card == MULTI:
                add(1500, "btn", r.choice([1, 4]), 150)
                add(300, "btn", 3, 150)
            add(1500, "btn", 1, 150)
            add(r.choice([500, 2000, 9000, 16000]), "stk", "start")
            add(r.choice([300, 1200, 4000]), "stk", "stop")
            if r.random() < 0.3:
                add(500, "btn", 4, r.choice([150, 1100]))
            add(800, "btn", 3, 150)
            add(800, "card", r.choice([JUDGE, JUDGE, COMPETITOR, MULTI]))
            add(800, "card", r.choice([card, card, JUDGE]))
        elif kind == "menu":
            add(gap, "btn", 3, 10_400)  # config menu
            for _ in range(r.randint(0, 8)):
                add(r.choice([100, 400]), "btn", r.choice([1, 4]), 120)
            if r.random() < 0.8:
                add(300, "btn", 3, 150)
        elif kind == "server":
            add(gap, "server", r.randint(1500, 9000))
        elif kind == "power":
            add(gap, "power", 0)
        elif kind == "wifi":
            if r.random() < 0.5:  # one outage, up to minutes
                add(gap, "wifi", r.choice([1500, 5000, 20000, 60000, 150000]))
            else:  # flapping link
                for _ in range(r.randint(3, 10)):
                    add(r.choice([0, 100, 500, 3000]), "wifi", r.choice([100, 400, 1500, 3000]))
        elif kind == "outage":  # server gone for minutes
            add(gap, "server", r.choice([30_000, 90_000, 180_000]))
        elif kind == "lcd":
            add(gap, "emu", "lcd " + r.choice(["glitch", "drop", "reset"]))
    return acts[:count]


def check_invariants(s: dict):
    """State combinations the UI must never show, whatever the input."""
    bad = []
    if s.get("menu") != "None" and s.get("cfg") != "-1":
        bad.append("a menu and the config menu at once")
    if s.get("scene") == "Finished" and s.get("time") == "-1":
        bad.append("Finished without a time")
    # a DNF from inspection is confirmed right away, also before the competitor is known
    if s.get("conf") == "1" and s.get("scene") != "Finished" and s.get("pen") != "-1":
        bad.append("confirmed time outside Finished")
    if s.get("judge") not in ("-1", None) and s.get("judge") == s.get("comp"):
        bad.append("judge is the competitor")
    if bad:
        raise Failure("contradicting state (" + ", ".join(bad) + f"): {s}")


# ── processes ───────────────────────────────────────────────────────────────────────────────

class Server:
    """micro-connector (dev, plain ws) + fake FKMTime. Attempts and incidents survive
    connector restarts; delegate decisions use `ctl`, requests and answers go to srv/e2e.jsonl.
    """

    def __init__(self, mc: Path, port: int, workdir: Path, fresh=False, extra=None):
        self.mc, self.port, self.dir = mc, port, workdir
        self.e2e, self.backend = None, None
        # unix socket paths are limited to 104 bytes on macOS (SUN_LEN): keep them short
        self.sock, self.ctl_path = f"/tmp/fkm-monkey-{port}.sock", f"/tmp/fkm-monkey-{port}.ctl"
        # appended across restarts: solve consistency is checked over the whole run
        self.log_file = open(workdir / "server.log", "a", buffering=1)
        if fresh:
            self.log_file.write("\n=== fresh server after reset settings ===\n")
        tests = json.load(open(mc / "tests.json"))
        tests.update(extra or {})
        (workdir / "srv").mkdir(exist_ok=True)
        json.dump(tests, open(workdir / "srv" / "tests.json", "w"))
        if fresh:  # a new FKMTime: its attempts start over
            (workdir / "srv" / "e2e.jsonl").unlink(missing_ok=True)

    def start(self):
        srv = self.dir / "srv"
        (srv / "fw").mkdir(exist_ok=True)
        (srv / "logs").mkdir(exist_ok=True)
        env = dict(os.environ, SOCKET_PATH=self.sock)
        if self.e2e is None:
            os.unlink(self.sock) if os.path.exists(self.sock) else None
            # Older connector builds select fake FKMTime with this flag; newer ones always use it.
            env.update(E2E_SIM="1", E2E_CONTROL=self.ctl_path, E2E_LOG=str(srv / "e2e.jsonl"))
            self.e2e = subprocess.Popen([self.mc / "target/debug/e2e"], cwd=srv, env=env,
                                        stdout=self.log_file, stderr=self.log_file)
            time.sleep(0.8)
        env.update(DEV="1", NO_TLS="1", NO_BT="1", NO_MDNS="1", PORT=str(self.port),
                   FIRMWARE_DIR=str(srv / "fw"), DEVICE_LOGS=str(srv / "logs"),
                   RUST_LOG="none,backend=trace")
        self.backend = subprocess.Popen([self.mc / "target/debug/backend"], cwd=srv, env=env,
                                        stdout=self.log_file, stderr=subprocess.STDOUT)

    @staticmethod
    def _end(p):
        if p is None:
            return
        p.terminate()
        try:
            p.wait(5)
        except subprocess.TimeoutExpired:
            p.kill()

    def stop(self):
        """Everything (also FKMTime)."""
        self._end(self.backend)
        self._end(self.e2e)
        self.backend = self.e2e = None

    def restart(self, down_s: float):
        """The connector goes away for `down_s`; fake FKMTime stays up."""
        self._end(self.backend)
        time.sleep(down_s)
        self.start()

    def ctl(self, **cmd) -> dict:
        with socket.socket(socket.AF_UNIX) as s:
            s.settimeout(5)
            s.connect(self.ctl_path)
            s.sendall((json.dumps(cmd) + "\n").encode())
            buf = b""
            while not buf.endswith(b"\n"):
                chunk = s.recv(65536)
                if not chunk:
                    break
                buf += chunk
        return json.loads(buf)

    def attempts(self) -> list:
        return self.ctl(cmd="attempts")["attempts"]

    def log(self) -> list:
        try:
            text = (self.dir / "srv" / "e2e.jsonl").read_text(errors="replace")
        except FileNotFoundError:
            return []
        return [json.loads(l) for l in text.splitlines() if l.strip()]

    def solves(self) -> list:
        """Solve entries the server received: (time, penalty, competitor, session, group, delegate)."""
        text = (self.dir / "server.log").read_text(errors="replace")
        text = re.sub(r"\x1b\[[0-9;]*m", "", text)
        pat = r"Solve: (\d+) \((-?\d+)\) (\d+) \S+ \d+ (\S+) (\w+) (\S+)"
        return [(int(m[1]), int(m[2]), int(m[3]), m[4], m[6], m[5] == "true")
                for m in re.finditer(pat, text)]


class Stackmat:
    """Fake gen4 stackmat on esp-emu's UART1 bridge (frames as in sim/stackmat.py)."""

    def __init__(self, port: int):
        self.port, self.state, self.started, self.shown, self.plugged = port, b"I", 0.0, 0, True
        self.garbage, self.stopped_times, self.lock = 0, [], threading.Lock()
        self.longest_run = 0.0  # s; a button (or unplug) can stop a solve before the timer does
        self.alive = True
        threading.Thread(target=self.run, daemon=True).start()

    @staticmethod
    def frame(state: bytes, ms: int) -> bytes:
        ms = min(ms, 9 * 60_000 + 59_999)
        digits = f"{ms // 60_000}{ms % 60_000 // 1000:02}{ms % 1000:03}".encode()
        return state + digits + bytes([64 + sum(d - 48 for d in digits)]) + b"\n\r"

    def cmd(self, c: str):
        with self.lock:
            if c == "start" and self.state != b" ":
                self.state, self.started = b" ", time.monotonic()
            elif c == "stop" and self.state == b" ":
                self.state, self.shown = b"S", int((time.monotonic() - self.started) * 1000)
                self.stopped_times.append(self.shown)
            elif c == "reset":
                if self.state == b" ":
                    self.longest_run = max(self.longest_run, time.monotonic() - self.started)
                self.state, self.shown = b"I", 0
            elif c in ("on", "off"):
                self.plugged = c == "on"
            elif c == "garbage":
                self.garbage += 1

    def run(self):
        sock = None
        while self.alive:
            if sock is None:
                try:
                    sock = socket.create_connection(("127.0.0.1", self.port), timeout=1)
                except OSError:
                    time.sleep(0.5)
                    continue
            with self.lock:
                ms = int((time.monotonic() - self.started) * 1000) if self.state == b" " else self.shown
                data = self.frame(self.state, ms) if self.plugged else b""
                if self.garbage:
                    data = bytes(random.getrandbits(8) for _ in range(23)) + data
                    self.garbage -= 1
            try:
                if data:
                    sock.sendall(data)
            except OSError:
                sock = None
            time.sleep(10 * 10 / 1200)


# ── one run ─────────────────────────────────────────────────────────────────────────────────

class Failure(Exception):
    pass


class Run:
    def __init__(self, seed, actions, job, args, image):
        self.seed, self.actions, self.args = seed, actions, args
        self.dir = out_dir(seed, args.hw)
        if self.dir.exists():
            shutil.rmtree(self.dir)
        self.dir.mkdir(parents=True)
        job += args.job_base
        base = 20000 + job * 10
        self.ports = dict(http=base, uart1=base + 1, control=base + 2, tap=base + 3)
        self.server_extra = getattr(args, "server_extra", None)
        self.server = Server(Path(args.mc).expanduser(), SRV_PORT + job, self.dir,
                             extra=self.server_extra)
        self.image = image
        self.executed = []
        self.expected_boots = 1
        self.settings_resets = 0
        self.reseed_after = 0
        self.heap_peak = 0

    # helpers
    def state(self, timeout=4.0) -> dict:
        t0 = time.monotonic()
        self.emu.send("state")
        while time.monotonic() - t0 < timeout:
            for line in self.emu.since(t0):
                if line.startswith("@state "):
                    st = dict(kv.split("=", 1) for kv in line[7:].split() if "=" in kv)
                    check_invariants(st)
                    if int(st.get("lcd", 0)) > LCD_STALE_MS:
                        self.emu.send("screen")
                        raise Failure(f"LCD shows other text than the firmware wrote for {st['lcd']} ms")
                    return st
            time.sleep(0.05)
        raise Failure(f"no answer to `state` within {timeout}s (hang?)")

    def check_alive(self):
        if self.emu.proc.poll() is not None:
            raise Failure(f"emulator exited ({self.emu.proc.returncode}): crash/panic stop")
        text = self.emu.since(0)
        boots = sum(1 for l in text if "INFO - Hardware Rev:" in l)
        # "Reset settings" in the config menu legitimately wipes NVS and reboots; the device then
        # registers again with a new sign key, so the server starts fresh too. Its Wi-Fi comes
        # back the way the host seeded it: written again after that reboot, then one more.
        resets = sum(1 for l in text if "Reset settings: rebooting" in l)
        if resets > self.settings_resets:
            self.settings_resets = resets
            self.expected_boots += 2
            self.server.stop()
            self.server = Server(self.server.mc, self.server.port, self.dir, fresh=True,
                                 extra=self.server_extra)
            self.server.start()
            self.reseed_after = self.expected_boots - 1
        if self.reseed_after and boots >= self.reseed_after:
            self.reseed_after = 0
            self.emu.reseed(self.image[0].with_suffix(".nvs"))
        if boots > self.expected_boots:
            raise Failure(f"unexpected reboot (#{boots})")
        for l in text[-400:]:
            if re.search(r"panicked|PANIC|Guru Meditation|Exception was unhandled", l):
                raise Failure(f"panic: {l}")
            m = re.search(r"Heap: used (\d+) max (\d+) of (\d+)", l)
            if m:
                self.heap_peak = max(self.heap_peak, int(m[2]))
                if int(m[2]) > HEAP_LIMIT * int(m[3]):
                    raise Failure(f"heap peak {m[2]}/{m[3]}")

    def wait_for(self, pred, timeout, what):
        timeout *= SLOW  # timeouts are in emulated seconds; emulation can lag real time
        t0 = time.monotonic()
        last = {}
        while time.monotonic() - t0 < timeout:
            self.check_alive()
            last = self.state()
            if pred(last):
                return last
            time.sleep(0.5)
        raise Failure(f"timeout waiting for {what}; last state {last}")

    def press(self, button, ms):
        """A deliberate press: waits until the firmware saw it released (emulated time can run
        several times slower than real time when instances share the CPU)."""
        self.emu.send(f"btn {button} {ms}")
        time.sleep(0.15)
        self.wait_for(lambda s: s.get("held") == "0", ms / 1000 * 6 + 5, f"btn {button} released")

    def boot(self):
        self.wait_for(lambda s: s.get("srv") == "Some(true)" and s.get("scene") == "WaitingForCompetitor",
                      180, "boot to idle with server")

    def do(self, act):
        delay, kind, *a = act
        time.sleep(delay / 1000)
        self.executed.append([round(time.monotonic() - self.t0, 3), *act])
        if kind == "btn":
            self.emu.send(f"btn {a[0]} {a[1]}")
        elif kind == "card":
            self.emu.send(f"card {a[0]}")
        elif kind == "stk":
            self.stackmat.cmd(a[0])
        elif kind == "bat":
            self.emu.send(f"bat {a[0]}")
        elif kind == "emu":  # a raw sim command (e.g. `lcd glitch`)
            self.emu.send(a[0])
        elif kind == "wait":
            pass
        elif kind == "expect":  # scripted scenarios only: state field reaches a value (in a[2] s)
            self.wait_for(lambda s: s.get(a[0]) == str(a[1]), a[2] if len(a) > 2 else 15, f"{a[0]}={a[1]}")
        elif kind in ("resolve", "delete"):  # sim e2e: the delegate decides the open incident
            open_ = [x for x in self.server.attempts() if x["status"] == "unresolved"]
            if not open_:
                raise Failure(f"{kind}: no open incident on the server")
            extra = a[0] if a else {}
            r = self.server.ctl(cmd=kind, sessionId=open_[-1]["sessionId"], **extra)
            if not r.get("ok"):
                raise Failure(f"{kind}: {r}")
        elif kind == "drop_reply":  # sim e2e: the next attempt is entered, its answer lost
            self.server.ctl(cmd="drop_reply", espId=int(self.state()["id"], 16))
        elif kind == "expect_db":  # sim e2e: the attempts of a[0] (competitor) match a[1]
            def db_matches():
                rows = [x for x in self.server.attempts() if x["competitorId"] == a[0]]
                return len(rows) == len(a[1]) and all(
                    all(r.get(k) == v for k, v in want.items()) for r, want in zip(rows, a[1]))
            t0 = time.monotonic()
            while not db_matches():
                if time.monotonic() - t0 > 15:
                    rows = [x for x in self.server.attempts() if x["competitorId"] == a[0]]
                    raise Failure(f"server attempts of {a[0]}: {rows}, expected {a[1]}")
                time.sleep(0.5)
        elif kind == "wifi":  # radio torn down for a[0] ms
            self.emu.send("wifi off")
            time.sleep(a[0] / 1000)
            self.emu.send("wifi on")
        elif kind == "server":
            self.server.restart(a[0] / 1000)
        elif kind == "power":
            before = self.state(timeout=6).get("time")
            self.expected_boots += 1
            self.emu.power_cycle()
            time.sleep(2)
            # the timer still shows its last time after a reboot: that must not become a solve
            if before == "-1":
                # frames of a plugged timer arrive long before the server is up
                self.wait_for(lambda s: s.get("srv") == "Some(true)", 180, "server after reboot")
                if (after := self.state(timeout=6).get("time")) != "-1":
                    raise Failure(f"a solve ({after} ms) appeared after a reboot")
        # a button press lasts `ms`; don't send the next action for the same press into it
        if kind == "btn" and a[1] > 3000:
            time.sleep(a[1] / 1000)

    def recover(self):
        """What a user who knows the device does to get back to idle. Fails ⇒ softlock."""
        self.stackmat.cmd("on")
        self.stackmat.cmd("stop")
        self.stackmat.cmd("reset")
        t0 = time.monotonic()
        # a long press from the abuse phase may still be held (emulated time lags real time)
        self.wait_for(lambda s: s.get("held") == "0", 20, "all buttons released")
        while time.monotonic() - t0 < 60:
            self.check_alive()
            s = self.state()
            if all(s.get(k) == v for k, v in IDLE.items()) and s.get("msg") == "0":
                # a card lookup sent during the abuse phase may still answer: idle must hold
                time.sleep(3)
                if all(self.state().get(k) == v for k, v in IDLE.items()):
                    return s
                continue
            # reconnecting / timer replugged above: the device must come back by itself
            # (while it shows the disconnected screen it ignores input by design)
            if s.get("srv") != "Some(true)" or s.get("stk") != "Some(true)":
                time.sleep(1)
                continue
            if s.get("err") == "1":
                self.press(3, 120)  # dismiss the error
            elif s.get("menu") == "Some(ErrorLog)":
                sel, count = s.get("el", "0/1").split("/")
                # an open entry closes on submit; in the list, Exit is the last item
                at_exit = s.get("eo") == "1" or int(sel) == int(count) - 1
                self.press(3 if at_exit else 4, 120)
            elif s.get("menu") != "None":
                self.press(3, 120)  # BT display, signing, buzzer volume: submit leaves
            elif s.get("cfg") != "-1":
                self.press(1, 120)  # walk the config menu up to its last item (Exit)
                time.sleep(0.4)
                if self.state().get("cfg") == str(self.args.cfg_exit):
                    self.press(3, 120)
            elif s.get("msg") == "1":
                pass  # messages time out by themselves (≤ 8 s)
            else:
                self.press(3, 3400)  # reset competitor
            time.sleep(0.8)
        raise Failure(f"SOFTLOCK: no way back to idle within 60s; last state {s}")

    def probe(self):
        """Recovery + a full valid solve that the server must receive exactly once."""
        self.recover()
        self.wait_for(lambda s: s.get("stk") == "Some(true)", 10, "stackmat connected")
        before = len(self.server.solves())
        self.emu.send(f"card {COMPETITOR}")
        self.wait_for(lambda s: s.get("scene") == "CompetitorInfo", 20, "competitor info")
        self.press(1, 120)
        self.wait_for(lambda s: s.get("scene") == "Inspection", 5, "inspection")
        time.sleep(1.0)
        self.stackmat.cmd("start")
        self.wait_for(lambda s: s.get("scene") == "Timer", 5, "timer running")
        time.sleep(1.2)
        self.stackmat.cmd("stop")
        shown = self.stackmat.shown
        s = self.wait_for(lambda s: s.get("scene") == "Finished", 5, "finished")
        if abs(int(s["time"]) - shown) > 0:
            raise Failure(f"solve time {s['time']} != stackmat {shown}")
        self.press(3, 120)
        self.wait_for(lambda s: s.get("conf") == "1", 5, "time confirmed")
        self.emu.send(f"card {JUDGE}")
        self.wait_for(lambda s: s.get("judge") == str(JUDGE), 20, "judge scanned")
        self.emu.send(f"card {COMPETITOR}")
        self.wait_for(lambda s: s.get("scene") == "WaitingForCompetitor" and s.get("comp") == "-1",
                      25, "solve accepted (back to idle)")
        new = self.server.solves()[before:]
        mine = [x for x in new if x[2] == COMPETITOR and x[0] == shown]
        if len(mine) != 1:
            raise Failure(f"server got {len(mine)} copies of the probe solve ({shown} ms): {new}")
        self.stackmat.cmd("reset")

    def check_errors(self, s):
        """Crash markers in the error log: stacktrace (S), repeated crashes (90), watchdog (92)."""
        codes = [c for c in s.get("errs", "").split(",") if c]
        bad = [c for c in codes if c in ("S", "90", "92")]
        if bad:
            raise Failure(f"crash recorded in the error log: {codes}")

    def check_server(self):
        """Solves the device sent while being abused must still be consistent."""
        by_session = {}
        # a delegate's decision may change the time the device then sends
        decided = {x[3] for x in self.server.solves() if x[5]}
        for t, pen, comp, session, group, delegate in self.server.solves():
            if comp not in KNOWN_CARDS:
                raise Failure(f"solve for unknown competitor {comp}")
            # a delegate call carries whatever time there is (0 before the solve)
            st = self.stackmat
            running = time.monotonic() - st.started if st.state == b" " else 0
            # emulated time runs slower than real time: a button stop is within the real run
            button_stop = t <= max(st.longest_run, running, *[x / 1000 for x in st.stopped_times], 0) * 1000
            if not delegate and t not in st.stopped_times and pen != -1 and not button_stop \
                    and session not in decided:
                raise Failure(f"solve time {t} never shown by the stackmat {self.stackmat.stopped_times[-8:]}")
            if delegate:
                continue  # a delegate call is a request; the solve submitted later supersedes it
            prev = by_session.setdefault(session, (t, pen, comp, group))
            if prev != (t, pen, comp, group):
                raise Failure(f"session {session} sent twice with different data: {prev} vs {(t, pen, comp, group)}")

    def execute(self, deadline):
        self.server.start()
        self.emu = Emu(*self.image, self.ports, self.dir / "firmware.log", hw=self.args.hw)
        self.stackmat = Stackmat(self.ports["uart1"])
        self.t0 = time.monotonic()
        try:
            self.boot()
            self.probe()
            for i, act in enumerate(self.actions):
                if not self.args.replay_file and time.monotonic() > deadline:
                    break
                self.do(act)
                if i % 10 == 0:
                    self.check_alive()
                    self.state(timeout=6)
                if (i + 1) % self.args.chunk == 0:
                    self.probe()
                    self.check_server()
            self.probe()
            self.check_server()
            return None
        except Failure as f:
            return str(f)
        finally:
            self.stackmat.alive = False
            self.emu.stop()
            self.server.stop()
            with (self.dir / "actions.json").open("w") as out:
                json.dump({"seed": self.seed, "actions": self.actions, "executed": self.executed},
                          out, indent=0)


# ── driver ──────────────────────────────────────────────────────────────────────────────────

def out_dir(seed, hw: str) -> Path:
    """Logs of one run: sim/out/<seed> (v4), sim/out/v3-<seed>."""
    return OUT / (f"v3-{seed}" if hw == "v3" else str(seed))


def target(hw: str) -> Path:
    return ROOT / ("target/sim-v3" if hw == "v3" else "target/sim")


def build(port: int, hw: str = "v4") -> tuple:
    env = dict(os.environ, SIM_BUILD_ONLY="1", SIM_WS_URL=f"ws://192.168.4.1:{port}", SIM_HW=hw)
    subprocess.run([str(ROOT / "sim/run.sh")], cwd=ROOT, env=env, check=True,
                   stdout=subprocess.DEVNULL)
    t = target(hw)
    bin_, elf = t / f"monkey-{port}.bin", t / f"monkey-{port}.elf"
    shutil.copy(t / "sim.bin", bin_)
    shutil.copy(t / "sim.nvs", bin_.with_suffix(".nvs"))  # the seeded Wi-Fi, for `Reset settings`
    shutil.copy(t / "riscv32imc-unknown-none-elf/release/fkm-firmware", elf)
    return bin_, elf


def run_one(seed, actions, job, args, image):
    deadline = time.monotonic() + args.minutes * 60
    err = Run(seed, actions, job, args, image).execute(deadline)
    return seed, err


def shrink(seed, actions, args, image):
    """ddmin over the action list, bounded by --shrink-runs reruns."""
    budget, n = args.shrink_runs, 2
    while len(actions) >= 2 and budget > 0:
        chunk = max(1, len(actions) // n)
        reduced = False
        for i in range(0, len(actions), chunk):
            if budget <= 0:
                break
            candidate = actions[:i] + actions[i + chunk:]
            budget -= 1
            _, err = run_one(seed, candidate, 0, args, image)
            print(f"  shrink {len(candidate)} actions: {'FAIL' if err else 'ok'}")
            if err:
                actions, n, reduced = candidate, max(n - 1, 2), True
                break
        if not reduced:
            if n >= len(actions):
                break
            n = min(len(actions), n * 2)
    json.dump({"seed": seed, "actions": actions}, open(out_dir(seed, args.hw) / "minimal.json", "w"))
    return actions


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--seeds", default="1-4", help="e.g. 1-20 or 3,7,9")
    p.add_argument("--seed", type=int)
    p.add_argument("--replay", type=int)
    p.add_argument("--replay-file")
    p.add_argument("--jobs", type=int, default=1)
    p.add_argument("--minutes", type=float, default=4,
                   help="fuzz duration; replay files always run every step")
    p.add_argument("--actions", type=int, default=400)
    p.add_argument("--chunk", type=int, default=40, help="recovery probe every N actions")
    p.add_argument("--hw", choices=["v3", "v4"], default=os.environ.get("SIM_HW", "v4"))
    p.add_argument("--cfg-exit", type=int, help="index of Exit in the config menu (v3: 5, v4: 6)")
    p.add_argument("--mc", default="~/projects/micro-connector",
                   help="micro-connector checkout with its fake FKMTime binary built")
    p.add_argument("--sim-e2e", action="store_true",
                   help="compatibility option: e2e always plays FKMTime")
    p.add_argument("--shrink", action="store_true")
    p.add_argument("--shrink-runs", type=int, default=12)
    p.add_argument("--no-build", action="store_true")
    p.add_argument("--job-base", type=int, default=0,
                   help="first job number: emulator ports 20000+10*N.., server port 18100+N..")
    p.add_argument("--mode", choices=["fuzz", "competition"], default="fuzz")
    p.add_argument("--devices", type=int, default=1, help="competition: stations on one server")
    p.add_argument("--max-attempts", type=int, default=40, help="competition: attempts in the day")
    p.add_argument("--time-scale", type=float, default=0.5, help="competition: solve lengths")
    p.add_argument("--slip-rate", type=float, default=1.0, help="competition: slip rate factor")
    p.add_argument("--delegate-rate", type=float, default=1.0,
                   help="competition: delegate call rate factor (on top of --slip-rate)")
    args = p.parse_args()
    if args.replay_file:  # a scenario written for one hardware revision runs on it
        args.hw = json.load(open(args.replay_file)).get("hw", args.hw)
    if args.cfg_exit is None:
        args.cfg_exit = 5 if args.hw == "v3" else 6

    if args.mode == "competition" or (args.replay_file and "day" in json.load(open(args.replay_file))):
        import competition
        sys.exit(competition.main(args))

    if args.replay_file:
        d = json.load(open(args.replay_file))
        work = [(d["seed"], d["actions"])]
        args.server_extra = d.get("server")
        if any(str(a[1]).startswith("expect") for a in d["actions"]):
            args.chunk = 10**9  # scripted: no recovery probe between its steps
    else:
        if args.seed is not None or args.replay is not None:
            seeds = [args.seed if args.seed is not None else args.replay]
        else:
            seeds = []
            for part in args.seeds.split(","):
                lo, _, hi = part.partition("-")
                seeds += range(int(lo), int(hi or lo) + 1)
        work = [(s, generate(s, args.actions, args.hw)) for s in seeds]

    jobs = max(1, min(args.jobs, len(work)))
    images = {}
    for j in range(jobs):
        t = target(args.hw)
        port = SRV_PORT + args.job_base + j
        images[j] = ((t / f"monkey-{port}.bin", t / f"monkey-{port}.elf")
                     if args.no_build else build(port, args.hw))

    results, queue, lock = [], list(work), threading.Lock()

    def worker(j):
        while True:
            with lock:
                if not queue:
                    return
                seed, actions = queue.pop(0)
            seed, err = run_one(seed, actions, j, args, images[j])
            print(f"seed {seed}: {'FAIL: ' + err if err else 'ok'}", flush=True)
            with lock:
                results.append((seed, err, actions))

    threads = [threading.Thread(target=worker, args=(j,)) for j in range(jobs)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    failed = [(s, e, a) for s, e, a in results if e]
    print(f"\n{len(results) - len(failed)}/{len(results)} seeds clean")
    for seed, err, actions in failed:
        print(f"  seed {seed}: {err}  (logs: {out_dir(seed, args.hw)})")
        if args.shrink:
            minimal = shrink(seed, actions, args, images[0])
            print(f"  seed {seed}: minimal repro {len(minimal)} actions -> {out_dir(seed, args.hw)}/minimal.json")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    # self-check: generation is deterministic and covers misuse
    a, b = generate(5, 300), generate(5, 300)
    assert a == b and generate(6, 300) != a
    assert {x[1] for x in a} >= {"btn", "card", "stk"}
    assert "emu" in {x[1] for x in generate(5, 300, "v3")} and "emu" not in {x[1] for x in a}
    assert Stackmat.frame(b" ", 61_234) == b" 101234" + bytes([64 + 11]) + b"\n\r"
    main()
