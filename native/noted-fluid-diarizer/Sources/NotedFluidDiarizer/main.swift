import FluidAudio
import Darwin
import Foundation

private struct PreparedOutput: Encodable {
    let ready: Bool
}

private struct SegmentOutput: Encodable {
    let speakerId: String
    let startMs: Int64
    let endMs: Int64
    let quality: Float
}

private struct DiarizationOutput: Encodable {
    let segments: [SegmentOutput]
    let speakers: [String: [Float]]
}

private enum HelperError: LocalizedError {
    case usage(String)

    var errorDescription: String? {
        switch self {
        case .usage(let message): message
        }
    }
}

@main
private struct NotedFluidDiarizer {
    static func main() async {
        do {
            try await run()
        } catch {
            FileHandle.standardError.write(
                Data("noted-fluid-diarizer: \(error.localizedDescription)\n".utf8)
            )
            exit(1)
        }
    }

    private static func run() async throws {
        let arguments = Array(CommandLine.arguments.dropFirst())
        guard let command = arguments.first else {
            throw HelperError.usage(usage)
        }
        let modelsDirectory = try requiredValue("--models-dir", in: arguments)
        let modelsURL = URL(fileURLWithPath: modelsDirectory, isDirectory: true)

        switch command {
        case "prepare":
            let manager = OfflineDiarizerManager()
            try await manager.prepareModels(directory: modelsURL)
            try writeJSON(PreparedOutput(ready: true))

        case "diarize":
            let audioPath = try requiredValue("--audio", in: arguments)
            var config = OfflineDiarizerConfig(clusteringThreshold: 0.7)
            if let rawCount = optionalValue("--num-speakers", in: arguments),
               let count = Int(rawCount), count > 0 {
                config = config.withSpeakers(exactly: count)
            }
            let manager = OfflineDiarizerManager(config: config)
            try await manager.prepareModels(directory: modelsURL)
            let result = try await manager.process(URL(fileURLWithPath: audioPath))
            let segments = result.segments.map { segment in
                SegmentOutput(
                    speakerId: segment.speakerId,
                    startMs: Int64((Double(segment.startTimeSeconds) * 1_000).rounded()),
                    endMs: Int64((Double(segment.endTimeSeconds) * 1_000).rounded()),
                    quality: segment.qualityScore
                )
            }
            try writeJSON(
                DiarizationOutput(
                    segments: segments,
                    speakers: result.speakerDatabase ?? [:]
                )
            )

        default:
            throw HelperError.usage(usage)
        }
    }

    private static func requiredValue(_ flag: String, in arguments: [String]) throws -> String {
        guard let value = optionalValue(flag, in: arguments), !value.isEmpty else {
            throw HelperError.usage("Missing \(flag).\n\(usage)")
        }
        return value
    }

    private static func optionalValue(_ flag: String, in arguments: [String]) -> String? {
        guard let index = arguments.firstIndex(of: flag), arguments.indices.contains(index + 1)
        else { return nil }
        return arguments[index + 1]
    }

    private static func writeJSON<T: Encodable>(_ value: T) throws {
        var data = try JSONEncoder().encode(value)
        data.append(0x0A)
        FileHandle.standardOutput.write(data)
    }

    private static let usage = """
    Usage:
      noted-fluid-diarizer prepare --models-dir <path>
      noted-fluid-diarizer diarize --models-dir <path> --audio <wav> [--num-speakers <count>]
    """
}
