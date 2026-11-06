import Foundation
import AVFoundation
import AppKit
import CoreImage
import Speech

// A private, local JSON-lines pipe. All player/recorder state is main-thread owned.
func emit(_ object: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: object) else { return }
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data([10]))
}
func failure(_ operation: String, _ message: String, id: String? = nil) {
    var event: [String: Any] = ["event": "failed", "operation": operation, "message": message]
    if let id { event["id"] = id }
    emit(event)
}
struct ReviewError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
    init(_ message: String) { self.message = message }
}

final class MediaService: NSObject, AVAudioRecorderDelegate {
    var player: AVPlayer?
    var asset: AVURLAsset?
    var output: AVPlayerItemVideoOutput?
    var generator: AVAssetImageGenerator?
    var times: [CMTime] = []
    var selected = 0
    var transform = CGAffineTransform.identity
    var lastImage: CGImage?
    var recorder: AVAudioRecorder?
    var recordingID: String?
    var recordingURL: URL?
    var busy = false
    var seekGeneration = 0
    var pendingTarget: Int?
    var transcriptionQueue: [(String, URL)] = []
    var transcribing = false
    let context = CIContext()

    func anchor(_ index: Int) -> [String: Any] {
        let time = times[index]
        return ["value": time.value, "timescale": time.timescale, "frame_index": index]
    }
    func index(at time: CMTime) -> Int {
        var low = 0, high = times.count
        while low < high {
            let mid = (low + high) / 2
            if CMTimeCompare(times[mid], time) <= 0 { low = mid + 1 } else { high = mid }
        }
        return max(0, low - 1)
    }
    func imageData(_ image: CGImage, preview: Bool) -> Data? {
        var image = image
        if preview && image.width > 1280 {
            let scale = 1280.0 / Double(image.width)
            let ci = CIImage(cgImage: image).transformed(by: CGAffineTransform(scaleX: scale, y: scale))
            if let scaled = context.createCGImage(ci, from: ci.extent) { image = scaled }
        }
        return NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])
    }
    func sendFrame(_ image: CGImage? = nil) {
        guard !times.isEmpty else { return }
        emit(["event": "frame", "anchor": anchor(selected), "png": image.flatMap { imageData($0, preview: true) }?.base64EncodedString() ?? "", "playing": (player?.rate ?? 0) != 0])
    }
    func exactFrame(_ requested: Int) throws -> CGImage {
        guard let generator, !times.isEmpty else { throw ReviewError("Open a video first.") }
        let requested = min(max(requested, 0), times.count - 1)
        var actual = CMTime.zero
        let image = try generator.copyCGImage(at: times[requested], actualTime: &actual)
        selected = index(at: actual)
        lastImage = image
        return image
    }
    func seekFrame(_ requested: Int) {
        guard recordingID == nil, !busy, !times.isEmpty else { return }
        player?.pause()
        let target = min(max(requested, 0), times.count - 1)
        pendingTarget = target
        seekGeneration += 1
        let generation = seekGeneration
        player?.seek(to: times[target], toleranceBefore: .zero, toleranceAfter: .zero) { [weak self] finished in
            DispatchQueue.main.async {
                guard let self, finished, self.seekGeneration == generation else { return }
                self.pendingTarget = nil
                do { self.sendFrame(try self.exactFrame(target)) }
                catch { failure("seek", error.localizedDescription) }
            }
        }
    }
    func open(_ path: String) {
        guard recordingID == nil, !busy else { return }
        busy = true
        seekGeneration += 1; pendingTarget = nil
        player?.pause()
        player = nil; output = nil; times = []; lastImage = nil
        let url = URL(fileURLWithPath: path)
        // Build a presentation-time index, rather than multiplying seconds by nominal FPS.
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let asset = AVURLAsset(url: url)
                guard let track = asset.tracks(withMediaType: .video).first else { throw ReviewError("The file has no video track.") }
                let reader = try AVAssetReader(asset: asset)
                let samples = AVAssetReaderTrackOutput(track: track, outputSettings: nil)
                samples.alwaysCopiesSampleData = false
                guard reader.canAdd(samples) else { throw ReviewError("Cannot read this video format.") }
                reader.add(samples)
                guard reader.startReading() else { throw reader.error ?? ReviewError("Cannot index video.") }
                var times: [CMTime] = []
                while let sample = samples.copyNextSampleBuffer() {
                    for sampleIndex in 0..<CMSampleBufferGetNumSamples(sample) {
                        var timing = CMSampleTimingInfo()
                        guard CMSampleBufferGetSampleTimingInfo(sample, at: sampleIndex, timingInfoOut: &timing) == noErr else {
                            throw ReviewError("Cannot read per-frame video timing.")
                        }
                        let time = timing.presentationTimeStamp
                        if time.isNumeric && time >= .zero { times.append(time) }
                    }
                }
                guard reader.status == .completed else { throw reader.error ?? ReviewError("Video indexing was interrupted.") }
                times.sort { CMTimeCompare($0, $1) < 0 }
                guard !times.isEmpty, asset.duration.seconds.isFinite, asset.duration.seconds > 0 else { throw ReviewError("Video has no readable frames or duration.") }
                let indexed = times
                let transform = track.preferredTransform
                DispatchQueue.main.async {
                    self.asset = asset; self.times = indexed; self.transform = transform; self.selected = 0
                    let item = AVPlayerItem(asset: asset)
                    let output = AVPlayerItemVideoOutput(pixelBufferAttributes: [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA])
                    item.add(output)
                    self.output = output; self.player = AVPlayer(playerItem: item)
                    let generator = AVAssetImageGenerator(asset: asset)
                    generator.appliesPreferredTrackTransform = true
                    generator.requestedTimeToleranceBefore = .zero; generator.requestedTimeToleranceAfter = .zero
                    self.generator = generator; self.busy = false
                    emit(["event": "opened", "duration": asset.duration.seconds, "frames": indexed.count])
                    do { self.sendFrame(try self.exactFrame(0)) }
                    catch { failure("open", error.localizedDescription) }
                }
            } catch {
                DispatchQueue.main.async { self.busy = false; failure("open", error.localizedDescription) }
            }
        }
    }
    func poll() {
        guard let player, let output, !times.isEmpty else { return }
        let time = player.currentTime()
        if !busy, recordingID == nil, pendingTarget == nil, output.hasNewPixelBuffer(forItemTime: time) {
            var displayTime = CMTime.zero
            if let pixels = output.copyPixelBuffer(forItemTime: time, itemTimeForDisplay: &displayTime) {
                let ci = CIImage(cvPixelBuffer: pixels).transformed(by: transform)
                if let image = context.createCGImage(ci, from: ci.extent) {
                    selected = index(at: displayTime); lastImage = image; sendFrame(image); return
                }
            }
        }
        sendFrame()
    }
    func record(id: String, frame: Int, audio: String, screenshot: String?) {
        guard recordingID == nil, !busy, !times.isEmpty else { failure("record", "Video is not ready to record a note.", id: id); return }
        player?.pause(); seekGeneration += 1; pendingTarget = nil
        busy = true
        // Anchor is the frame the UI displayed when Record was pressed, not permission-dialog time.
        let target = min(max(frame, 0), times.count - 1)
        AVCaptureDevice.requestAccess(for: .audio) { allowed in
            DispatchQueue.main.async {
                guard allowed else { self.busy = false; failure("record", "Microphone access is disabled. Enable fesReview in System Settings → Privacy & Security → Microphone.", id: id); return }
                do {
                    let image = try self.exactFrame(target)
                    self.player?.seek(to: self.times[self.selected], toleranceBefore: .zero, toleranceAfter: .zero)
                    var captured = false
                    if let screenshot {
                        guard let data = self.imageData(image, preview: false) else { throw ReviewError("Cannot encode the selected frame.") }
                        try data.write(to: URL(fileURLWithPath: screenshot), options: .atomic)
                        captured = true
                    }
                    let url = URL(fileURLWithPath: audio)
                    let recorder = try AVAudioRecorder(url: url, settings: [
                        AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: 44100,
                        AVNumberOfChannelsKey: 1, AVLinearPCMBitDepthKey: 16,
                        AVLinearPCMIsFloatKey: false, AVLinearPCMIsBigEndianKey: false
                    ])
                    recorder.delegate = self
                    guard recorder.prepareToRecord(), recorder.record() else { throw ReviewError("Cannot start the microphone recorder.") }
                    self.recorder = recorder; self.recordingID = id; self.recordingURL = url; self.busy = false
                    self.sendFrame(image)
                    emit(["event": "recording", "id": id, "anchor": self.anchor(self.selected), "screenshot": captured])
                } catch { self.busy = false; failure("record", error.localizedDescription, id: id) }
            }
        }
    }
    func stopRecording() {
        guard let id = recordingID, let url = recordingURL else { return }
        recorder?.stop(); recorder = nil; recordingID = nil; recordingURL = nil
        // Finalize and sync the WAV before notifying Rust or beginning transcription.
        do {
            let file = try FileHandle(forWritingTo: url)
            try file.synchronize()
            try file.close()
        } catch { failure("record", "Could not sync recording: \(error.localizedDescription)", id: id); return }
        emit(["event": "recorded", "id": id])
        enqueueTranscription(id: id, url: url)
    }
    func audioRecorderEncodeErrorDidOccur(_ recorder: AVAudioRecorder, error: Error?) {
        DispatchQueue.main.async { guard self.recorder === recorder else {return}; self.recordingFailed(error?.localizedDescription ?? "Audio recording failed.") }
    }
    func audioRecorderDidFinishRecording(_ recorder: AVAudioRecorder, successfully flag: Bool) {
        if !flag { DispatchQueue.main.async { guard self.recorder === recorder else {return}; self.recordingFailed("Audio recording ended unexpectedly; any recorded audio is retained.") } }
    }
    func recordingFailed(_ message: String) {
        guard let id = recordingID else { return }
        recordingID = nil; recordingURL = nil
        recorder?.stop(); recorder = nil
        failure("record", message, id: id)
    }
    func enqueueTranscription(id: String, url: URL) {
        guard !transcriptionQueue.contains(where: { $0.0 == id }) else { return }
        transcriptionQueue.append((id, url)); runNextTranscription()
    }
    func runNextTranscription() {
        guard !transcribing, !transcriptionQueue.isEmpty else { return }
        transcribing = true
        let (id, url) = transcriptionQueue.removeFirst()
        Task { @MainActor in
            do {
                let text: String
                if #available(macOS 26.0, *) {
                    text = try await transcribeModern(url, id: id)
                } else {
                    text = try await transcribeLegacy(url)
                }
                emit(["event": "transcript", "id": id, "text": text])
            } catch { failure("transcribe", error.localizedDescription, id: id) }
            self.transcribing = false; self.runNextTranscription()
        }
    }
    func handle(_ command: [String: Any]) {
        let name = command["command"] as? String ?? ""
        switch name {
        case "open": if let path = command["path"] as? String { open(path) }
        case "poll": poll()
        case "pause": player?.pause(); sendFrame()
        case "play":
            if recordingID == nil && !busy, let player {
                if let asset, player.currentTime().seconds >= asset.duration.seconds - 0.001 {
                    player.seek(to: .zero, toleranceBefore: .zero, toleranceAfter: .zero) { finished in
                        if finished { DispatchQueue.main.async { player.play() } }
                    }
                } else { player.play() }
            }
        case "seek":
            if let seconds = command["seconds"] as? Double, seconds.isFinite { seekFrame(index(at: CMTime(seconds: seconds, preferredTimescale: 600000))) }
        case "frame": if let index = command["frame_index"] as? Int { seekFrame(index) }
        case "step": if let delta = command["delta"] as? Int { seekFrame((pendingTarget ?? selected) + max(-1, min(1, delta))) }
        case "record":
            if let id = command["id"] as? String, let frame = command["frame_index"] as? Int, let audio = command["audio"] as? String {
                record(id: id, frame: frame, audio: audio, screenshot: command["screenshot"] as? String)
            }
        case "stop_record": stopRecording()
        case "transcribe":
            if let id = command["id"] as? String, let audio = command["audio"] as? String { enqueueTranscription(id: id, url: URL(fileURLWithPath: audio)) }
        default: failure("protocol", "Unknown media command: \(name)")
        }
    }
}

@available(macOS 26.0, *)
func transcribeModern(_ url: URL, id: String) async throws -> String {
    guard SpeechTranscriber.isAvailable else { throw ReviewError("On-device SpeechTranscriber is unavailable on this Mac. Audio is saved.") }
    guard let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current) else { throw ReviewError("On-device transcription does not support the current system language. Audio is saved.") }
    let module = SpeechTranscriber(locale: locale, transcriptionOptions: [], reportingOptions: [], attributeOptions: [])
    if let download = try await AssetInventory.assetInstallationRequest(supporting: [module]) {
        await MainActor.run { emit(["event": "transcription_status", "id": id, "message": "Downloading speech model…"]) }
        try await download.downloadAndInstall()
    }
    let analyzer = SpeechAnalyzer(modules: [module])
    let results = Task { () throws -> String in
        var text = ""
        for try await result in module.results { text += String(result.text.characters) }
        return text
    }
    do {
        let file = try AVAudioFile(forReading: url)
        if let last = try await analyzer.analyzeSequence(from: file) {
            try await analyzer.finalizeAndFinish(through: last)
        } else { await analyzer.cancelAndFinishNow() }
        return try await results.value
    } catch {
        results.cancel(); await analyzer.cancelAndFinishNow(); throw error
    }
}

// Older macOS: explicitly refuse server recognition. Unsupported locales retain audio for retry/edit.
func transcribeLegacy(_ url: URL) async throws -> String {
    let authorized = await withCheckedContinuation { (continuation: CheckedContinuation<Bool, Never>) in
        SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0 == .authorized) }
    }
    guard authorized else { throw ReviewError("Speech recognition permission denied. Audio is saved.") }
    guard let recognizer = SFSpeechRecognizer(locale: Locale.current), recognizer.isAvailable, recognizer.supportsOnDeviceRecognition else {
        throw ReviewError("On-device speech is unavailable for this language. Enable Dictation for it in System Settings, then retry. Audio is saved.")
    }
    let request = SFSpeechURLRecognitionRequest(url: url)
    request.requiresOnDeviceRecognition = true; request.shouldReportPartialResults = false
    return try await withCheckedThrowingContinuation { continuation in
        let completion = LegacyCompletion(continuation)
        completion.task = recognizer.recognitionTask(with: request) { result, error in
            DispatchQueue.main.async {
                if let error { completion.finish(.failure(error)) }
                else if let result, result.isFinal { completion.finish(.success(result.bestTranscription.formattedString)) }
            }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 120) {
            completion.finish(.failure(ReviewError("Transcription timed out. Audio is saved; retry when ready.")))
        }
    }
}
final class LegacyCompletion {
    var continuation: CheckedContinuation<String, Error>?
    var task: SFSpeechRecognitionTask?
    init(_ continuation: CheckedContinuation<String, Error>) { self.continuation = continuation }
    func finish(_ result: Result<String, Error>) {
        guard let continuation else { return }
        self.continuation = nil
        continuation.resume(with: result)
        task?.cancel(); task = nil
    }
}

let service = MediaService()
// NSApplication establishes the native application context for TCC dialogs.
let application = NSApplication.shared
application.setActivationPolicy(.prohibited)
DispatchQueue.global(qos: .userInitiated).async {
    while let line = readLine() {
        guard let data = line.data(using: .utf8), let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { continue }
        DispatchQueue.main.async { service.handle(object) }
    }
    DispatchQueue.main.async { service.recorder?.stop(); exit(0) }
}
application.run()
