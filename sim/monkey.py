#!/usr/bin/env python3
"""Seeded monkey tester: abuses the real firmware in esp-emu and checks it never crashes,
reboots, hangs or softlocks, and never sends inconsistent solves to the server.

  python3 sim/monkey.py --seeds 1-20 --jobs 4 --minutes 4     # fuzz
  python3 sim/monkey.py --replay 7                            # rerun one seed (same actions)
  python3 sim/monkey.py --replay-file sim/out/7/actions.json --shrink
  python3 sim/monkey.py --seed 7 --emit-hil /tmp/tests.json   # scenario for micro-connector's HIL

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

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "sim" / "out"
EMU = shutil.which("esp-emu") or str(Path.home() / ".local/bin/esp-emu")

COMPETITOR, JUDGE, MULTI = 69420, 3004425529, 1264825046  # micro-connector tests.json cards
KNOWN_CARDS = {COMPETITOR, JUDGE, MULTI}
# real thresholds (src/buttons.rs) and their neighbours
HOLDS = [5, 20, 29, 31, 80, 150, 400, 990, 1010, 1500, 2990, 3010, 4500, 9990, 10010, 12500]
BUTTON_NAMES = {1: "inspect", 2: "delegate", 3: "submit", 4: "penalty"}
SLOW = 3  # emulated time runs up to ~3x slower than real time with several instances
HEAP_LIMIT = 0.92  # of the heap size printed by the firmware
IDLE = dict(scene="WaitingForCompetitor", comp="-1", menu="None", cfg="-1", err="0")


# ── action generation (pure: same seed ⇒ same list) ─────────────────────────────────────────

def generate(seed: int, count: int) -> list:
    """Weighted random actions, biased towards abuse. Each action: [delay_ms, kind, *args]."""
    r = random.Random(seed)
    acts = []

    def add(delay, *a):
        acts.append([delay, *a])

    weights = {
        "btn": 26, "mash": 8, "multi": 5, "card": 14, "spam": 6,
        "stk": 14, "stk_chaos": 5, "bat": 2, "wait": 8, "flow": 10,
        "server": 1, "power": 1, "menu": 3,
    }
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
    return acts[:count]


def check_invariants(s: dict):
    """State combinations the UI must never show, whatever the input."""
    bad = []
    if s.get("menu") != "None" and s.get("cfg") != "-1":
        bad.append("a menu and the config menu at once")
    if s.get("scene") == "Finished" and s.get("time") == "-1":
        bad.append("Finished without a time")
    if s.get("conf") == "1" and s.get("scene") != "Finished":
        bad.append("confirmed time outside Finished")
    if s.get("judge") not in ("-1", None) and s.get("judge") == s.get("comp"):
        bad.append("judge is the competitor")
    if bad:
        raise Failure("contradicting state (" + ", ".join(bad) + f"): {s}")


# ── processes ───────────────────────────────────────────────────────────────────────────────

class Server:
    """micro-connector (dev, plain ws) + its e2e backend answering card lookups."""

    def __init__(self, mc: Path, port: int, workdir: Path, fresh=False):
        self.mc, self.port, self.dir = mc, port, workdir
        self.procs = []
        # appended across restarts: solve consistency is checked over the whole run
        self.log = open(workdir / "server.log", "a", buffering=1)
        if fresh:
            self.log.write("\n=== fresh server after reset settings ===\n")
        tests = json.load(open(mc / "tests.json"))
        tests["tests"] = [{"name": "idle (monkey)", "steps": [{"type": "Sleep", "data": 86_400_000}]}]
        (workdir / "srv").mkdir(exist_ok=True)
        json.dump(tests, open(workdir / "srv" / "tests.json", "w"))

    def start(self):
        srv = self.dir / "srv"
        (srv / "fw").mkdir(exist_ok=True)
        (srv / "logs").mkdir(exist_ok=True)
        # unix socket paths are limited to 104 bytes on macOS (SUN_LEN): keep it short
        sock = f"/tmp/fkm-monkey-{self.port}.sock"
        env = dict(os.environ, SOCKET_PATH=sock)
        try:
            os.unlink(sock)
        except FileNotFoundError:
            pass
        self.procs.append(subprocess.Popen([self.mc / "target/debug/e2e"], cwd=srv, env=env,
                                           stdout=self.log, stderr=self.log))
        time.sleep(0.8)
        env.update(DEV="1", NO_TLS="1", NO_BT="1", NO_MDNS="1", PORT=str(self.port),
                   FIRMWARE_DIR=str(srv / "fw"), DEVICE_LOGS=str(srv / "logs"),
                   RUST_LOG="none,backend=trace")
        self.procs.append(subprocess.Popen([self.mc / "target/debug/backend"], cwd=srv, env=env,
                                           stdout=self.log, stderr=subprocess.STDOUT))

    def stop(self):
        for p in self.procs:
            p.terminate()
        for p in self.procs:
            try:
                p.wait(5)
            except subprocess.TimeoutExpired:
                p.kill()
        self.procs = []

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


class Emu:
    def __init__(self, bin_: Path, elf: Path, ports: dict, log: Path):
        self.log = open(log, "w", buffering=1)
        self.lines, self.lock = [], threading.Lock()
        self.proc = subprocess.Popen(
            [EMU, "--chip", "esp32c3", "--firmware", str(bin_), "--elf", str(elf),
             "--net", f"user,hostfwd=tcp:127.0.0.1:{ports['http']}-:80",
             "--uart1-tcp", f"127.0.0.1:{ports['uart1']}",
             "--control-tcp", f"127.0.0.1:{ports['control']}"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, bufsize=0)
        self.control = ports["control"]
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        buf = b""
        while True:
            chunk = self.proc.stdout.read(4096)
            if not chunk:
                break
            buf += chunk
            *lines, buf = buf.split(b"\n")
            now = time.monotonic()
            for raw in lines:
                line = raw.decode(errors="replace").rstrip("\r")
                self.log.write(f"{now:.3f} {line}\n")
                with self.lock:
                    self.lines.append((now, line))

    def send(self, line: str):
        try:
            self.proc.stdin.write((line + "\n").encode())
            self.proc.stdin.flush()
        except OSError:
            pass

    def since(self, t0: float) -> list:
        with self.lock:
            return [l for t, l in self.lines if t >= t0]

    def power_cycle(self):
        with socket.create_connection(("127.0.0.1", self.control), timeout=3) as s:
            s.sendall(b"reset\n")

    def stop(self):
        self.proc.send_signal(signal.SIGINT)
        try:
            self.proc.wait(5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


# ── one run ─────────────────────────────────────────────────────────────────────────────────

class Failure(Exception):
    pass


class Run:
    def __init__(self, seed, actions, job, args, image):
        self.seed, self.actions, self.args = seed, actions, args
        self.dir = OUT / str(seed)
        if self.dir.exists():
            shutil.rmtree(self.dir)
        self.dir.mkdir(parents=True)
        base = 20000 + job * 10
        self.ports = dict(http=base, uart1=base + 1, control=base + 2)
        self.server = Server(Path(args.mc).expanduser(), 8080 + job, self.dir)
        self.image = image
        self.executed = []
        self.expected_boots = 1
        self.settings_resets = 0
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
                    return st
            time.sleep(0.05)
        raise Failure(f"no answer to `state` within {timeout}s (hang?)")

    def check_alive(self):
        if self.emu.proc.poll() is not None:
            raise Failure(f"emulator exited ({self.emu.proc.returncode}): crash/panic stop")
        text = self.emu.since(0)
        boots = sum(1 for l in text if l.startswith("ESP-ROM:"))
        # "Reset settings" in the config menu legitimately wipes NVS and reboots; the device then
        # registers again with a new sign key, so the server starts fresh too
        resets = sum(1 for l in text if "Reset settings: rebooting" in l)
        if resets > self.settings_resets:
            self.settings_resets = resets
            self.expected_boots += 1
            self.server.stop()
            self.server = Server(self.server.mc, self.server.port, self.dir, fresh=True)
            self.server.start()
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
        elif kind == "wait":
            pass
        elif kind == "server":
            self.server.stop()
            time.sleep(a[0] / 1000)
            self.server.start()
        elif kind == "power":
            self.expected_boots += 1
            self.emu.power_cycle()
            time.sleep(2)
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
                return s
            if s.get("srv") != "Some(true)":
                time.sleep(1)  # reconnecting: the device must come back by itself
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
        for t, pen, comp, session, group, delegate in self.server.solves():
            if comp not in KNOWN_CARDS:
                raise Failure(f"solve for unknown competitor {comp}")
            # a delegate call carries whatever time there is (0 before the solve)
            if not delegate and t not in self.stackmat.stopped_times and pen != -1:
                raise Failure(f"solve time {t} never shown by the stackmat {self.stackmat.stopped_times[-8:]}")
            prev = by_session.setdefault(session, (t, pen, comp, group))
            if prev != (t, pen, comp, group):
                raise Failure(f"session {session} sent twice with different data: {prev} vs {(t, pen, comp, group)}")

    def execute(self, deadline):
        self.server.start()
        self.emu = Emu(*self.image, self.ports, self.dir / "firmware.log")
        self.stackmat = Stackmat(self.ports["uart1"])
        self.t0 = time.monotonic()
        try:
            self.boot()
            self.probe()
            for i, act in enumerate(self.actions):
                if time.monotonic() > deadline:
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
            json.dump({"seed": self.seed, "actions": self.actions, "executed": self.executed},
                      open(self.dir / "actions.json", "w"), indent=0)


# ── driver ──────────────────────────────────────────────────────────────────────────────────

def build(port: int) -> tuple:
    env = dict(os.environ, SIM_BUILD_ONLY="1", SIM_WS_URL=f"ws://192.168.4.1:{port}")
    subprocess.run([str(ROOT / "sim/run.sh")], cwd=ROOT, env=env, check=True,
                   stdout=subprocess.DEVNULL)
    t = ROOT / "target/sim"
    bin_, elf = t / f"monkey-{port}.bin", t / f"monkey-{port}.elf"
    shutil.copy(t / "sim.bin", bin_)
    shutil.copy(t / "riscv32imc-unknown-none-elf/release/fkm-firmware", elf)
    return bin_, elf


def emit_hil(actions, path):
    """Exports what micro-connector's HIL format can express (single buttons, cards, sleeps,
    one random solve time); misuse it can't express is dropped and listed."""
    steps, dropped = [], set()
    for delay, kind, *a in actions:
        if delay:
            steps.append({"type": "Sleep", "data": delay})
        if kind == "btn":
            steps.append({"type": "Button", "data": {"name": BUTTON_NAMES[a[0]], "time": a[1]}})
        elif kind == "card":
            steps.append({"type": "ScanCard", "data": a[0]})
        elif kind == "stk" and a[0] == "stop":
            steps.append({"type": "SolveTime"})
        elif kind == "wait":
            pass
        else:
            dropped.add(f"{kind} {a[0] if a else ''}".strip())
    json.dump({"name": "monkey", "sleepBetween": 0, "steps": steps}, open(path, "w"), indent=1)
    if dropped:
        print("not expressible in tests.json (dropped):", ", ".join(sorted(dropped)))


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
    json.dump({"seed": seed, "actions": actions}, open(OUT / str(seed) / "minimal.json", "w"))
    return actions


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--seeds", default="1-4", help="e.g. 1-20 or 3,7,9")
    p.add_argument("--seed", type=int)
    p.add_argument("--replay", type=int)
    p.add_argument("--replay-file")
    p.add_argument("--jobs", type=int, default=1)
    p.add_argument("--minutes", type=float, default=4)
    p.add_argument("--actions", type=int, default=400)
    p.add_argument("--chunk", type=int, default=40, help="recovery probe every N actions")
    p.add_argument("--cfg-exit", type=int, default=6, help="index of Exit in the config menu (v4)")
    p.add_argument("--mc", default="~/projects/micro-connector")
    p.add_argument("--emit-hil")
    p.add_argument("--shrink", action="store_true")
    p.add_argument("--shrink-runs", type=int, default=12)
    p.add_argument("--no-build", action="store_true")
    args = p.parse_args()

    if args.replay_file:
        d = json.load(open(args.replay_file))
        work = [(d["seed"], d["actions"])]
    else:
        if args.seed is not None or args.replay is not None:
            seeds = [args.seed if args.seed is not None else args.replay]
        else:
            seeds = []
            for part in args.seeds.split(","):
                lo, _, hi = part.partition("-")
                seeds += range(int(lo), int(hi or lo) + 1)
        work = [(s, generate(s, args.actions)) for s in seeds]

    if args.emit_hil:
        emit_hil(work[0][1], args.emit_hil)
        return

    jobs = max(1, min(args.jobs, len(work)))
    images = {}
    for j in range(jobs):
        t = ROOT / "target/sim"
        images[j] = ((t / f"monkey-{8080 + j}.bin", t / f"monkey-{8080 + j}.elf")
                     if args.no_build else build(8080 + j))

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
        print(f"  seed {seed}: {err}  (logs: sim/out/{seed}/)")
        if args.shrink:
            minimal = shrink(seed, actions, args, images[0])
            print(f"  seed {seed}: minimal repro {len(minimal)} actions -> sim/out/{seed}/minimal.json")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    # self-check: generation is deterministic and covers misuse
    a, b = generate(5, 300), generate(5, 300)
    assert a == b and generate(6, 300) != a
    assert {x[1] for x in a} >= {"btn", "card", "stk"}
    assert Stackmat.frame(b" ", 61_234) == b" 101234" + bytes([64 + 11]) + b"\n\r"
    main()
