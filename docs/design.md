# fesReview design

## Agreed scope

Mac-first Rust/egui app for reviewing generated children's animation. Minimal persistent text; fesTerm styling reference. Video dominates, compact transport, collapsible editable notes. Pause, step to the exact frame, dictate, save/resume. Export timestamped JSON, retain original audio, optionally include screenshots. Implement MCP after the review workflow works; defer Windows.

## Boundaries

- `src/model.rs`: portable review data, source identity, lock, recovery and atomic persistence.
- `src/ui.rs`: egui presentation and recording lifecycle; no AVFoundation calls.
- `src/backend.rs`: typed events over a private child-process stdin/stdout transport. No listener, ports or network authentication surface.
- `native/main.swift`: AVFoundation indexing/playback/capture; speech adapters. Player and recorder state live on the main thread. File indexing occurs off-thread. Transcription is asynchronous and queued.

The Rust UI creates and persists the pending note before starting the recorder. The helper receives the visible frame index, resolves the exact image and timestamp, writes any screenshot, then starts audio and acknowledges that anchor. Rust saves the acknowledged anchor. On release, the helper closes the WAV, emits `recorded`, then starts transcription. Playback may resume before transcription completes. Transcript edits are protected from late automatic results.

The rational video presentation timestamp is authoritative. Frame index is zero-based presentation order from a full compressed-sample index. Neither wall time, recording duration, nor nominal FPS determines the note position. This first version accepts ordinary single-video-track files; multiview/composite/live/DRM media are outside scope.

Autosave uses a same-directory temporary file, file sync, atomic rename, and directory sync on Unix. A review lock prevents multiple writers. Unsupported versions, malformed JSON, unsafe attachment paths and mismatched video digests fail without rewriting the manifest. Interrupted recordings/transcriptions remain visible for retry. Audio may need repair after a process or power failure during an active WAV recording; finalized recordings remain usable.

## Transcription

Prefer SpeechAnalyzer/SpeechTranscriber on macOS 26, with a downloadable system-managed model and current system locale. Earlier macOS uses SFSpeechRecognizer only after both `supportsOnDeviceRecognition` and `requiresOnDeviceRecognition` are true. Unsupported configurations report the failure and retain audio. No silent cloud fallback. Native adapter can be replaced for Windows or a local model later.

API references: [SpeechAnalyzer](https://developer.apple.com/documentation/speech/speechanalyzer), [Apple's introduction](https://developer.apple.com/videos/play/wwdc2025/277/), [on-device requirement](https://developer.apple.com/documentation/speech/sfspeechrecognitionrequest/requiresondevicerecognition). Theme reference: [fesTerm theme](https://github.com/fes/fesTerm/blob/main/crates/festerm-ui-egui/src/theme.rs).

## MCP follow-up

Expose the same UI application command dispatcher: open, play/pause, seek, frame step, current state, list/get/update notes, screenshot, export and next/previous note. Commands must control the visible player, with one owner for recording state. Put MCP in a separate adapter, not inside native media code. Record commands must retain visible recording indication. Decide supported transport, authentication and the ChatGPT-to-local-Mac connection separately; an MCP implementation alone does not provide that connection. No MCP server is shipped in this milestone.
