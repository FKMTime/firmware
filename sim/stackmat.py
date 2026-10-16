#!/usr/bin/env python3
"""Fake stackmat on esp-emu's UART1 bridge (sim/run.sh starts it on 127.0.0.1:5556).

Commands on stdin: start | stop | reset | off (unplug) | on (plug in) | quit
"""
import socket
import sys
import threading
import time

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 5556


def frame(state: bytes, ms: int) -> bytes:
    ms = min(ms, 9 * 60_000 + 59_999)
    digits = f"{ms // 60_000}{ms % 60_000 // 1000:02}{ms % 1000:03}".encode()
    checksum = 64 + sum(d - ord("0") for d in digits)
    return state + digits + bytes([checksum]) + b"\n\r"


assert frame(b" ", 61_234) == b" 101234" + bytes([64 + 11]) + b"\n\r"

state, started, shown, plugged = b"I", 0.0, 0, True


def commands():
    global state, started, shown, plugged
    for line in sys.stdin:
        cmd = line.strip()
        if cmd == "start":
            state, started = b" ", time.monotonic()
        elif cmd == "stop" and state == b" ":
            state, shown = b"S", int((time.monotonic() - started) * 1000)
        elif cmd == "reset":
            state, shown = b"I", 0
        elif cmd in ("on", "off"):
            plugged = cmd == "on"
        elif cmd == "quit":
            break
        else:
            print("start | stop | reset | off | on | quit")
        print(f"stackmat: {state.decode()!r} plugged={plugged}")
    sys.exit(0)


sock = socket.create_connection(("127.0.0.1", PORT))
threading.Thread(target=commands, daemon=True).start()
print("stackmat connected:", "start | stop | reset | off | on | quit")
while True:
    if plugged:
        ms = int((time.monotonic() - started) * 1000) if state == b" " else shown
        try:
            sock.sendall(frame(state, ms))
        except OSError:
            sys.exit("stackmat: emulator closed the connection")
    time.sleep(10 * 10 / 1200)  # one 10-byte frame at 1200 baud
