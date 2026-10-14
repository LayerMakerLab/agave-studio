// agave-speech <model dir> <audio file>...
// Loads Parakeet TDT 0.6B v3 from <model dir> (never downloads) and writes one JSON line per file:
// {"file":…,"duration":…,"words":[[t0,t1,"text"],…]} with times in seconds from the start of that file.
// Loading the model is the slow part, so the engine passes every stretch of speech in one call.
import FluidAudio
import Foundation

struct Line: Encodable {
    let file: String
    let duration: Double
    let words: [Word]
}

struct Word: Encodable {
    let t0: Double
    let t1: Double
    let text: String
    func encode(to encoder: Encoder) throws {
        var c = encoder.unkeyedContainer()
        try c.encode((t0 * 1000).rounded() / 1000)
        try c.encode((t1 * 1000).rounded() / 1000)
        try c.encode(text)
    }
}

/// Tokens that start with a space begin a new word.
func words(_ tokens: [TokenTiming]) -> [Word] {
    var out: [Word] = []
    var text = ""
    var t0 = 0.0
    var t1 = 0.0
    for tok in tokens {
        if tok.token.hasPrefix(" ") || text.isEmpty {
            if !text.isEmpty { out.append(Word(t0: t0, t1: t1, text: text)) }
            text = tok.token.trimmingCharacters(in: .whitespacesAndNewlines)
            t0 = tok.startTime
        } else {
            text += tok.token
        }
        t1 = tok.endTime
    }
    if !text.isEmpty { out.append(Word(t0: t0, t1: t1, text: text)) }
    return out.filter { !$0.text.isEmpty }
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

let args = Array(CommandLine.arguments.dropFirst())
guard args.count >= 2 else { fail("usage: agave-speech <model dir> <audio file>...") }

do {
    let version = AsrModelVersion.v3
    let models = try AsrModels.loadLocal(from: URL(fileURLWithPath: args[0]), version: version)
    let manager = AsrManager(config: ASRConfig(
        tdtConfig: TdtConfig(blankId: version.blankId), encoderHiddenSize: version.encoderHiddenSize))
    try await manager.loadModels(models)
    let encoder = JSONEncoder()
    for path in args.dropFirst() {
        var state = TdtDecoderState.make(decoderLayers: await manager.decoderLayerCount)
        let result = try await manager.transcribe(URL(fileURLWithPath: path), decoderState: &state)
        let line = Line(file: path, duration: result.duration, words: words(result.tokenTimings ?? []))
        FileHandle.standardOutput.write(try encoder.encode(line) + Data("\n".utf8))
    }
} catch {
    fail("agave-speech: \(error)")
}
