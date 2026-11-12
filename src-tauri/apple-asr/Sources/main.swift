// bifaria-asr: transcribes speech with Parakeet TDT 0.6B v3 or one of its re-trained siblings (Ultra,
// Redux, Phonon-2) on the Neural Engine (Core ML, through FluidAudio) for Bifaria. Bifaria keeps one
// worker running so the model stays loaded.
//
// Worker mode (no arguments): one JSON request per line on stdin, one JSON answer per line on
// stdout, in order.
//   {"op":"load","dir":"/path/to/parakeet-tdt-0.6b-v3-coreml","version":"v3","units":"ane"} → {"ok":true,"ms":…}
//     "version" is "v3" (the default), "ultra", "redux" or "phonon2"; "units" is where Core ML runs
//     it: "ane" (Neural Engine, the default), "gpu" or "cpu".
//   {"op":"transcribe","audio":"<base64 of 16 kHz mono float32 little-endian>"} → {"ok":true,"text":…,"ms":…}
// Failures are {"ok":false,"error":"not_loaded|bad_request|internal","message":…}.
//
// One-shot test: `bifaria-asr <model-dir> <16 kHz mono wav> [version [units]]` prints the text and timings.

import AVFoundation
import CoreML
import FluidAudio
import Foundation

func emit(_ object: [String: Any]) {
    guard var data = try? JSONSerialization.data(withJSONObject: object, options: [.withoutEscapingSlashes]) else {
        FileHandle.standardOutput.write(Data("{\"ok\":false,\"error\":\"internal\",\"message\":\"Could not encode the response\"}\n".utf8))
        return
    }
    data.append(0x0A)
    FileHandle.standardOutput.write(data)
}

func failure(_ code: String, _ message: String) -> [String: Any] {
    ["ok": false, "error": code, "message": message]
}

func ms(since start: Date) -> Int {
    Int(Date().timeIntervalSince(start) * 1000)
}

/// Float32 samples from little-endian bytes.
func floats(_ data: Data) -> [Float] {
    var out = [Float](repeating: 0, count: data.count / 4)
    _ = out.withUnsafeMutableBytes { data.copyBytes(to: $0) }
    return out
}

/// FluidAudio's model version for a "version" name; nil for an unknown one.
func modelVersion(_ name: String?) -> AsrModelVersion? {
    switch name ?? "v3" {
    case "v3": return .v3
    case "ultra": return .ultra
    case "redux": return .redux
    case "phonon2": return .phonon2
    default: return nil
    }
}

/// Core ML compute units for a "units" name; nil for an unknown one.
func computeUnits(_ name: String?) -> MLComputeUnits? {
    switch name ?? "ane" {
    case "ane": return .cpuAndNeuralEngine
    case "gpu": return .cpuAndGPU
    case "cpu": return .cpuOnly
    default: return nil
    }
}

actor Engine {
    private var asr: AsrManager?

    func load(dir: String, version: AsrModelVersion, units: MLComputeUnits) async throws -> Int {
        let start = Date()
        // A new model replaces the loaded one rather than sitting next to it.
        asr = nil
        let models = try AsrModels.loadLocal(
            from: URL(fileURLWithPath: dir), version: version,
            configuration: MLModelConfigurationUtils.defaultConfiguration(computeUnits: units))
        let asr = AsrManager(config: .default, models: models)
        // The first prediction compiles the model for its device; do it now, not on the user's first dictation.
        var state = TdtDecoderState.make()
        _ = try? await asr.transcribe([Float](repeating: 0, count: 16000), decoderState: &state)
        self.asr = asr
        return ms(since: start)
    }

    func transcribe(_ samples: [Float]) async throws -> String? {
        guard let asr else { return nil }
        var state = TdtDecoderState.make()
        return try await asr.transcribe(samples, decoderState: &state).text
    }
}

func readWav16k(_ path: String) throws -> [Float] {
    let file = try AVAudioFile(forReading: URL(fileURLWithPath: path))
    let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 16000, channels: 1, interleaved: false)!
    let converter = AVAudioConverter(from: file.processingFormat, to: format)!
    let input = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length))!
    try file.read(into: input)
    let capacity = AVAudioFrameCount(Double(file.length) * 16000 / file.processingFormat.sampleRate) + 1024
    let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity)!
    var fed = false
    var error: NSError?
    converter.convert(to: output, error: &error) { _, status in
        if fed { status.pointee = .endOfStream; return nil }
        fed = true
        status.pointee = .haveData
        return input
    }
    if let error { throw error }
    return Array(UnsafeBufferPointer(start: output.floatChannelData![0], count: Int(output.frameLength)))
}

let engine = Engine()
let args = CommandLine.arguments

if (3...5).contains(args.count) {
    guard let version = modelVersion(args.count >= 4 ? args[3] : nil),
        let units = computeUnits(args.count == 5 ? args[4] : nil)
    else {
        print("use: bifaria-asr <model-dir> <wav> [v3|ultra|redux|phonon2 [ane|gpu|cpu]]")
        exit(2)
    }
    let loadMs = try await engine.load(dir: args[1], version: version, units: units)
    let samples = try readWav16k(args[2])
    for run in 1...3 {
        let start = Date()
        let text = try await engine.transcribe(samples) ?? ""
        print("run \(run): \(ms(since: start)) ms for \(String(format: "%.1f", Double(samples.count) / 16000)) s (load \(loadMs) ms): \(text)")
    }
    exit(0)
}

while let line = readLine() {
    guard let request = (try? JSONSerialization.jsonObject(with: Data(line.utf8))) as? [String: Any],
        let op = request["op"] as? String
    else {
        emit(failure("bad_request", "Expected a JSON object with an \"op\""))
        continue
    }
    do {
        switch op {
        case "load":
            guard let dir = request["dir"] as? String else {
                emit(failure("bad_request", "\"load\" needs \"dir\""))
                continue
            }
            guard let version = modelVersion(request["version"] as? String) else {
                emit(failure("bad_request", "Unknown \"version\"; use v3, ultra, redux or phonon2"))
                continue
            }
            guard let units = computeUnits(request["units"] as? String) else {
                emit(failure("bad_request", "Unknown \"units\"; use ane, gpu or cpu"))
                continue
            }
            emit(["ok": true, "ms": try await engine.load(dir: dir, version: version, units: units)])
        case "transcribe":
            guard let audio = (request["audio"] as? String).flatMap({ Data(base64Encoded: $0) }) else {
                emit(failure("bad_request", "\"transcribe\" needs base64 \"audio\""))
                continue
            }
            let start = Date()
            guard let text = try await engine.transcribe(floats(audio)) else {
                emit(failure("not_loaded", "Send \"load\" first"))
                continue
            }
            emit(["ok": true, "text": text, "ms": ms(since: start)])
        case "ping":
            emit(["ok": true])
        default:
            emit(failure("bad_request", "Unknown op \(op)"))
        }
    } catch {
        emit(failure("internal", error.localizedDescription))
    }
}
