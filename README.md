# fesReview

A small video review app: pause, find the frame, speak a note, continue.

Rust + egui, macOS first. Uses fesTerm's dark surfaces and cyan accent, a video-first window, one transport strip, and a collapsible notes panel. MCP and Windows support are later milestones.

## Build on a Mac

Requires Rust stable, Xcode 26 or newer (including its selected command-line tools), and macOS 14 or newer. macOS 26 uses Apple's SpeechAnalyzer; earlier systems use on-device-only SFSpeechRecognizer when supported by the system language. No API key is needed. Speech model installation may require an internet connection; feedback audio is not submitted to a cloud transcription provider.

```sh
git clone https://github.com/fes/fesReview.git
cd fesReview
./scripts/build-macos.sh
open dist/fesReview.app
```

The development bundle is ad-hoc signed, not notarized. Open the bundled app rather than `cargo run` so macOS sees the microphone/speech usage descriptions and the adjacent native helper. CI builds an Apple Silicon app archive; it is not a universal binary. Building locally targets the local Mac's architecture.

## Review

- **⌘O** opens an MP4/MOV/M4V file. Choose the parent folder for the review on first use.
- **Space** plays/pauses. **← / →** step one frame (actual video presentation timestamps); **⇧← / ⇧→** (or **⌃**) jump one second. The transport also has -1s, -1f, +1f, +1s buttons.
- **R** (or **● Dictate**) pauses and starts dictating a note; press it again to stop. Playback stays paused; press Space to continue. The first recording may show a macOS microphone permission dialog: wait for the red Recording indicator before speaking.
- **S** (or **Screenshot**) saves the full-resolution current frame. While dictating it attaches to that note; otherwise it attaches to a note already on this frame, or creates a screenshot-only note you can type into.
- Click a note's timestamp to return to its frame. Edit its transcript directly; **↻** retries failed/interrupted transcription from the retained WAV.
- The **⋯** menu lists the shortcuts and reveals the review folder.

Notes autosave to `video.ext.fesreview/review.json`, alongside `audio/*.wav` and optional full-resolution `frames/*.png`. Share the complete review folder with the animation workflow. The video itself is not copied. Reopening the same video with its review folder restores the notes; when the folder is elsewhere, choose that same parent again.

Each note includes a zero-based presentation-order frame index, an exact rational timestamp (`value / timescale`), convenient seconds, transcription state/text, and relative attachment paths. A SHA-256 digest binds the review to that exact source video. Variable-frame-rate footage is indexed rather than assigned an estimated `seconds × fps` frame number.

## Validation

```sh
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo fmt --check
./scripts/build-macos.sh
python3 scripts/smoke-macos.py
```

Rust tests cover recovery, source-version mismatches, concurrent writers, safe attachment paths, and failed-save preservation. The macOS smoke test creates a variable-frame-rate movie and checks real decoding, distinct frame pixels, next/previous stepping, seeking, and bounds. Microphone permissions, speech model downloads, transcription quality, playback sound, and interactive UI still need a hands-on Mac acceptance pass; see [the checklist](docs/acceptance.md).

Current limits: indexing reads the complete video before playback; preview frames cross the helper pipe as PNG at up to 30 Hz and 1280 px wide (full-resolution note screenshots are preserved). This favors a straightforward first implementation over high-resolution playback efficiency. Open another video after pending transcriptions finish; closing during transcription retains audio and marks the note recoverable on reopening.
