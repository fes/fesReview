#!/usr/bin/env python3
"""Exercise real AVFoundation decoding and VFR stepping without a microphone or UI."""
import base64
import json
import pathlib
import queue
import subprocess
import tempfile
import threading

root = pathlib.Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory() as directory:
    movie = pathlib.Path(directory) / "variable.mov"
    subprocess.run(["xcrun", "swift", str(root / "scripts/fixture.swift"), str(movie)], check=True)
    proc = subprocess.Popen([str(root / "dist/fesReview.app/Contents/MacOS/fesreview-native")], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    messages = queue.Queue()
    def read():
        for line in proc.stdout:
            messages.put(json.loads(line))
    threading.Thread(target=read, daemon=True).start()
    def send(**command):
        proc.stdin.write(json.dumps(command) + "\n")
        proc.stdin.flush()
    def receive(kind):
        while True:
            event = messages.get(timeout=30)
            assert event["event"] != "failed", event
            if event["event"] == kind:
                return event
    def frame(index, seconds):
        event = receive("frame")
        assert event["anchor"]["frame_index"] == index, event["anchor"]
        t = event["anchor"]
        assert abs(t["value"] / t["timescale"] - seconds) < .002, t
        assert base64.b64decode(event["png"]).startswith(b"\x89PNG\r\n\x1a\n")
        return event["png"]
    try:
        send(command="open", path=str(movie))
        opened = receive("opened")
        assert opened["frames"] == 5, opened
        first = frame(0, 0)
        send(command="step", delta=1)
        second = frame(1, .1)
        assert first != second
        send(command="step", delta=1)
        frame(2, .3)
        send(command="step", delta=-1)
        assert frame(1, .1) == second
        send(command="seek", seconds=.35)
        frame(2, .3)
        send(command="jump", seconds=.5)
        frame(4, .8)
        send(command="jump", seconds=-.5)
        frame(2, .3)
        shot = pathlib.Path(directory) / "shot.png"
        send(command="screenshot", id="note", frame_index=2, path=str(shot))
        assert receive("screenshot")["id"] == "note"
        assert shot.read_bytes().startswith(b"\x89PNG\r\n\x1a\n")
        send(command="frame", frame_index=9999)
        frame(4, .8)
        send(command="frame", frame_index=0)
        frame(0, 0)
        print("PASS: real video decode, distinct pixels, VFR next/previous, seek, frame bounds")
    finally:
        proc.stdin.close()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
