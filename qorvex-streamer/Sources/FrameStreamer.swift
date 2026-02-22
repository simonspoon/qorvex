// FrameStreamer.swift
// Captures frames from a Simulator window via ScreenCaptureKit,
// encodes them as JPEG, and writes them to a SocketWriter.

import Foundation
import ScreenCaptureKit
import CoreMedia
import ImageIO
import CoreGraphics

final class FrameStreamer: NSObject, SCStreamOutput, SCStreamDelegate {
    private let window: SCWindow
    private let display: SCDisplay
    private let fps: Int
    private let quality: CGFloat
    private let socketWriter: SocketWriter
    private var stream: SCStream?
    private var isWriting = false // Backpressure flag
    private let writeQueue = DispatchQueue(label: "com.qorvex.streamer.write")

    init(window: SCWindow, display: SCDisplay, fps: Int, quality: Int, socketWriter: SocketWriter) {
        self.window = window
        self.display = display
        self.fps = fps
        self.quality = CGFloat(quality) / 100.0
        self.socketWriter = socketWriter
        super.init()
    }

    func start() async throws {
        let config = SCStreamConfiguration()

        // Size from the window frame at native resolution.
        let scale = CGFloat(display.width) / CGFloat(display.frame.width)
        config.width = Int(window.frame.width * scale)
        config.height = Int(window.frame.height * scale)
        config.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps))
        config.pixelFormat = kCVPixelFormatType_32BGRA
        config.showsCursor = false

        let filter = SCContentFilter(desktopIndependentWindow: window)

        let stream = SCStream(filter: filter, configuration: config, delegate: self)
        self.stream = stream

        try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: writeQueue)
        try await stream.startCapture()
    }

    func stop() {
        stream?.stopCapture { error in
            if let error = error {
                NSLog("[qorvex-streamer] Error stopping capture: %@", "\(error)")
            }
        }
        stream = nil
    }

    // MARK: - SCStreamOutput

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen else { return }

        // Backpressure: skip if previous write hasn't completed.
        if isWriting { return }

        guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        guard let jpegData = encodeJPEG(pixelBuffer: pixelBuffer) else { return }

        isWriting = true
        socketWriter.writeFrame(jpegData) { [weak self] in
            self?.isWriting = false
        }
    }

    // MARK: - SCStreamDelegate

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        NSLog("[qorvex-streamer] Stream stopped with error: %@", "\(error)")
        socketWriter.close()
        exit(1)
    }

    // MARK: - JPEG encoding

    private func encodeJPEG(pixelBuffer: CVPixelBuffer) -> Data? {
        CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }

        let width = CVPixelBufferGetWidth(pixelBuffer)
        let height = CVPixelBufferGetHeight(pixelBuffer)
        let bytesPerRow = CVPixelBufferGetBytesPerRow(pixelBuffer)
        guard let baseAddress = CVPixelBufferGetBaseAddress(pixelBuffer) else { return nil }

        let colorSpace = CGColorSpaceCreateDeviceRGB()
        guard let context = CGContext(
            data: baseAddress,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: bytesPerRow,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.noneSkipFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
        ) else { return nil }

        guard let cgImage = context.makeImage() else { return nil }

        let mutableData = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(
            mutableData as CFMutableData,
            "public.jpeg" as CFString,
            1,
            nil
        ) else { return nil }

        let options: [CFString: Any] = [
            kCGImageDestinationLossyCompressionQuality: quality
        ]
        CGImageDestinationAddImage(destination, cgImage, options as CFDictionary)

        guard CGImageDestinationFinalize(destination) else { return nil }

        return mutableData as Data
    }
}
