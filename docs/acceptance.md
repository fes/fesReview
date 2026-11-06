# Mac acceptance pass

Automated checks cannot grant microphone consent or judge the actual video/audio experience. Run the app bundle on the intended Mac before calling this a release.

- Open a normal animation MP4 with sound. Check aspect ratio, rotated footage, audio/video sync, resize, end of playback and replay.
- Pause and step forward/backward; compare with burned-in frame numbers on 24, 29.97, 30, 60 FPS and variable-frame-rate sources. Rapid repeated arrows should advance repeatedly. Compare saved PNG with the frame selected at Record.
- Drag timeline, then click a note timestamp. Check exact displayed frame.
- Press R, speak, press R again. Check microphone prompt on first run, visible recording state, that playback stays paused and the transcript arrives. Test stopping while permission is still pending. Click Dictate/Stop as an alternative. Press S during and after dictation; check the PNG attaches to the right note.
- Click a transport button or the timeline, then confirm Space, arrows, ⇧arrows, R and S still work.
- Deny microphone access, then enable it in Settings and retry. No stranded recording state.
- Download the current-locale speech model on macOS 26. Disconnect networking after installation and transcribe. Test unsupported locale/device and speech permission denial on older macOS; audio must remain and cloud fallback must not occur.
- Edit transcript before automatic completion: it must keep the edit. Retry a failed note and verify its video anchor is unchanged.
- Close/reopen during transcription; retry from WAV. Close while recording; the WAV must finalize before exit. Force quit during recording and inspect recovery.
- Make destination unwritable / simulate full disk. Existing JSON must remain readable; save error stays actionable and closing must not silently discard edited notes.
- Launch a second instance on the same review; reject concurrent writing. Replace source video bytes; reject old review rather than attaching notes to changed footage.
- Share the review folder: JSON paths resolve to WAV/PNG attachments and timestamps are understandable without the app.

Follow-ups after acceptance: more efficient shared-memory/GPU preview transport, optional waveform/audio replay, packaging/signing/notarization, MCP adapter, Windows media/speech backend.
