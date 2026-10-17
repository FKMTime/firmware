#!/usr/bin/env python3
"""The sim device in a browser: OLED, buttons, cards, stackmat, battery, Wi-Fi/server/power.

    python3 sim/web.py [--mc ~/projects/micro-connector] [--no-server] [--port 8000]

Builds the sim image, starts esp-emu, the fake stackmat and (unless --no-server) a local
micro-connector, then serves http://127.0.0.1:8000. Stdlib only: events go out as SSE,
commands come in as POSTs.
"""
import argparse
import json
import re
import signal
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import monkey

JOB = 9  # monkey jobs use 0..; keep ports apart so a campaign can run next to this
ANSI = re.compile(r"\x1b\[[0-9;]*m")


class Hub:
    """Collects events for the page: OLED frames, log lines, state."""

    def __init__(self):
        self.events, self.cond = [], threading.Condition()
        self.oled = None

    def push(self, kind: str, data):
        with self.cond:
            self.events.append((kind, data))
            if kind == "oled":
                self.oled = data
            self.cond.notify_all()

    def watch(self, emu: monkey.Emu):
        """Turns emulator output into events (OLED frames as `@oled <hex>`, page-major like the
        SSD1309 GRAM: byte = 8 vertical pixels)."""
        seen = 0
        while True:
            with emu.lock:
                new = [l for _, l in emu.lines[seen:]]
                seen += len(new)
            for line in new:
                if line.startswith("@oled "):
                    if len(line) == 6 + 2048:
                        self.push("oled", line[6:])
                elif line.startswith("@state "):
                    self.push("state", dict(kv.split("=", 1) for kv in line[7:].split() if "=" in kv))
                else:
                    self.push("log", ANSI.sub("", line))
            time.sleep(0.01)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--mc", default="~/projects/micro-connector")
    p.add_argument("--no-server", action="store_true")
    p.add_argument("--port", type=int, default=8000)
    args = p.parse_args()

    out = monkey.ROOT / "sim/out/web"
    out.mkdir(parents=True, exist_ok=True)
    srv_port = monkey.SRV_PORT + JOB
    print("building the sim image…")
    bin_, elf = monkey.build(srv_port)

    server = None
    if not args.no_server:
        server = monkey.Server(Path(args.mc).expanduser(), srv_port, out)
        server.start()
    base = 20000 + JOB * 10
    ports = dict(http=base, uart1=base + 1, control=base + 2)
    stackmat = monkey.Stackmat(ports["uart1"])
    emu = monkey.Emu(bin_, elf, ports, out / "firmware.log")
    hub = Hub()
    threading.Thread(target=hub.watch, args=(emu,), daemon=True).start()

    def poll_state():
        time.sleep(1)
        emu.send("oled raw")
        while True:
            emu.send("state")
            time.sleep(1)

    threading.Thread(target=poll_state, daemon=True).start()

    def command(target: str, arg: str):
        if target == "emu":
            emu.send(arg)
        elif target == "stk":
            stackmat.cmd(arg)
        elif target == "power":
            emu.power_cycle()
        elif target == "server" and server:
            server.stop() if arg == "stop" else server.start()

    page = (monkey.ROOT / "sim/web.html").read_bytes()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            if self.path == "/":
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.end_headers()
                self.wfile.write(page)
            elif self.path == "/events":
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-cache")
                self.end_headers()
                with hub.cond:
                    cursor = len(hub.events)
                    first = [("oled", hub.oled)] if hub.oled else []
                    # recent log so a reload isn't empty
                    first += [e for e in hub.events[-300:] if e[0] == "log"]
                try:
                    for kind, data in first:
                        self.wfile.write(f"event: {kind}\ndata: {json.dumps(data)}\n\n".encode())
                    while True:
                        with hub.cond:
                            hub.cond.wait_for(lambda: len(hub.events) > cursor, timeout=15)
                            batch, cursor = hub.events[cursor:], len(hub.events)
                        msg = "".join(f"event: {k}\ndata: {json.dumps(d)}\n\n" for k, d in batch)
                        self.wfile.write((msg or ": ping\n\n").encode())
                        self.wfile.flush()
                except OSError:
                    pass
            else:
                self.send_error(404)

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            try:
                req = json.loads(body)
                command(str(req["target"]), str(req.get("arg", "")))
                self.send_response(204)
            except (ValueError, KeyError, OSError):
                self.send_response(400)
            self.end_headers()

    def on_term(*_):
        raise KeyboardInterrupt  # same cleanup as Ctrl-C

    signal.signal(signal.SIGTERM, on_term)
    http = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    http.daemon_threads = True
    print(f"device on http://127.0.0.1:{args.port}  (firmware log: {out / 'firmware.log'})")
    try:
        http.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        stackmat.alive = False
        emu.stop()
        if server:
            server.stop()


if __name__ == "__main__":
    main()
