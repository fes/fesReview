import Foundation
import AVFoundation
import CoreVideo
let url = URL(fileURLWithPath: CommandLine.arguments[1])
let writer = try AVAssetWriter(outputURL: url, fileType: .mov)
let input = AVAssetWriterInput(mediaType: .video, outputSettings: [AVVideoCodecKey: AVVideoCodecType.h264, AVVideoWidthKey: 160, AVVideoHeightKey: 90])
let adapter = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: input, sourcePixelBufferAttributes: [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32ARGB, kCVPixelBufferWidthKey as String: 160, kCVPixelBufferHeightKey as String: 90])
writer.add(input)
precondition(writer.startWriting())
writer.startSession(atSourceTime: .zero)
for (index, value) in [0, 100, 300, 400, 800].enumerated() {
    while !input.isReadyForMoreMediaData { Thread.sleep(forTimeInterval: 0.01) }
    var pixel: CVPixelBuffer?
    precondition(CVPixelBufferPoolCreatePixelBuffer(nil, adapter.pixelBufferPool!, &pixel) == kCVReturnSuccess)
    CVPixelBufferLockBaseAddress(pixel!, [])
    let bytes = CVPixelBufferGetBaseAddress(pixel!)!.assumingMemoryBound(to: UInt8.self)
    let stride = CVPixelBufferGetBytesPerRow(pixel!)
    for y in 0..<90 { for x in 0..<160 {
        let p = y * stride + x * 4
        bytes[p] = 255; bytes[p+1] = UInt8(index * 50); bytes[p+2] = UInt8(x); bytes[p+3] = UInt8(y)
    } }
    CVPixelBufferUnlockBaseAddress(pixel!, [])
    precondition(adapter.append(pixel!, withPresentationTime: CMTime(value: Int64(value), timescale: 1000)))
}
writer.endSession(atSourceTime: CMTime(value: 900, timescale: 1000))
input.markAsFinished()
let done = DispatchSemaphore(value: 0)
writer.finishWriting { done.signal() }
done.wait()
precondition(writer.status == .completed, "\(String(describing: writer.error))")
