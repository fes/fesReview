#!/usr/bin/env python3
"""Exercise real AVFoundation decoding and VFR stepping without a microphone or UI."""
import base64
import json
import pathlib
import queue
import subprocess
import tempfile
import threading
import time

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
        send(command="play")
        for _ in range(25):
            send(command="poll")
            if receive("frame")["playing"]:
                break
            time.sleep(.02)
        else:
            raise AssertionError("Playback did not start")
        send(command="step", delta=1)
        stepped = receive("frame")
        assert not stepped["playing"], stepped
        # Return to the exact first frame before the burst.
        send(command="frame", frame_index=0)
        first = frame(0, 0)
        # Commands sent without waiting must accumulate, even across pending seeks.
        for _ in range(3):
            send(command="step", delta=1)
        while True:
            event = receive("frame")
            assert not event["playing"], event
            if event["anchor"]["frame_index"] == 3:
                break
        send(command="frame", frame_index=0)
        frame(0, 0)
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
        send(command="frame", frame_index=2)
        displayed = frame(2, .3)
        shot = pathlib.Path(directory) / "shot.png"
        # A pending step must not change which already displayed frame is captured.
        send(command="step", delta=1)
        send(command="screenshot", id="note", frame_index=2, path=str(shot))
        saw_shot = saw_step = False
        while not (saw_shot and saw_step):
            event = messages.get(timeout=30)
            assert event["event"] != "failed", event
            if event["event"] == "screenshot":
                assert event["id"] == "note"
                saw_shot = True
            elif event["event"] == "frame" and event["anchor"]["frame_index"] == 3:
                saw_step = True
        assert shot.read_bytes() == base64.b64decode(displayed)
        send(command="frame", frame_index=9999)
        frame(4, .8)
        send(command="frame", frame_index=0)
        frame(0, 0)
        print("PASS: decode, VFR stepping, playback pause, burst steps, visible-frame screenshot, bounds")
    finally:
        proc.stdin.close()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
