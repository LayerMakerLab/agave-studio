// agave-vision: finds where the subject is in a video, for Agave Edit's "Follow" reframe.
// Usage: agave-vision <video> [samples-per-second]
// Prints JSON: {"points":[[t, cx, cy, kind], ...]} with cx, cy in 0...1 from the top left of the
// picture and kind 2 = face, 1 = person, 0 = salient object, -1 = nothing found. Objects come from
// objectness saliency (tight around things), then attention saliency (looser) as a fallback.
// Everything runs on this Mac with Apple's Vision framework; nothing leaves the machine.

import AVFoundation
import Foundation
import Vision

func fail(_ msg: String) -> Never {
    FileHandle.standardError.write((msg + "\n").data(using: .utf8)!)
    exit(1)
}

let args = CommandLine.arguments
guard args.count >= 2 else { fail("usage: agave-vision <video> [samples-per-second]") }
let rate = min(max(args.count > 2 ? Double(args[2]) ?? 4 : 4, 0.5), 10)
let asset = AVURLAsset(url: URL(fileURLWithPath: args[1]))
let duration: Double
do {
    duration = try await asset.load(.duration).seconds
} catch {
    fail("could not read the video: \(error.localizedDescription)")
}
guard duration.isFinite, duration > 0 else { fail("the video has no length") }

let generator = AVAssetImageGenerator(asset: asset)
generator.appliesPreferredTrackTransform = true
generator.maximumSize = CGSize(width: 640, height: 640)
let tolerance = CMTime.zero // exact frames, so a shot change is seen at its real time
generator.requestedTimeToleranceBefore = tolerance
generator.requestedTimeToleranceAfter = tolerance

/// The centre of the largest box, flipped so y runs from the top.
func centre(_ boxes: [CGRect]) -> (Double, Double)? {
    guard let b = boxes.max(by: { $0.width * $0.height < $1.width * $1.height }) else { return nil }
    return (Double(b.midX), 1 - Double(b.midY))
}

var points: [[Double]] = []
var t = 0.0
while t < duration {
    let time = CMTime(seconds: t, preferredTimescale: 600)
    if let image = try? await generator.image(at: time).image {
        let faces = VNDetectFaceRectanglesRequest()
        let people = VNDetectHumanRectanglesRequest()
        people.upperBodyOnly = false
        let objects = VNGenerateObjectnessBasedSaliencyImageRequest()
        let saliency = VNGenerateAttentionBasedSaliencyImageRequest()
        let handler = VNImageRequestHandler(cgImage: image, options: [:])
        try? handler.perform([faces, people, objects, saliency])
        if let c = centre((faces.results ?? []).filter { $0.confidence > 0.5 }.map(\.boundingBox)) {
            points.append([t, c.0, c.1, 2])
        } else if let c = centre((people.results ?? []).filter { $0.confidence > 0.5 }.map(\.boundingBox)) {
            points.append([t, c.0, c.1, 1])
        } else if let c = centre((objects.results ?? []).flatMap { $0.salientObjects ?? [] }.map(\.boundingBox)) {
            points.append([t, c.0, c.1, 0])
        } else if let c = centre((saliency.results ?? []).flatMap { $0.salientObjects ?? [] }.map(\.boundingBox)) {
            points.append([t, c.0, c.1, 0])
        } else {
            points.append([t, 0.5, 0.5, -1])
        }
    }
    t += 1 / rate
}

let out = try JSONSerialization.data(withJSONObject: ["points": points.map { p in p.map { ($0 * 1000).rounded() / 1000 } }])
FileHandle.standardOutput.write(out)
