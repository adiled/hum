// apple-fm's bridge, and the only code in hum that speaks to Apple.
//
// One run grows one bloom through Apple's on-device foundation model
// (SystemLanguageModel.default, through FoundationModels). It reads one
// JSON line on stdin:
//
//   {"instructions": "...", "turns": [{"prompt": "...", "response": "..."}],
//    "prompt": "...", "temperature": 0.7, "maximumResponseTokens": 256,
//    "sampling": {"mode": "greedy" | "top" | "nucleus", "k": 40, "p": 0.9, "seed": 7}}
//
// and writes NDJSON on stdout, each line as soon as it is known:
//
//   {"event": "delta", "text": "..."}          the words new since the last line
//   {"event": "finish", "inputTokens": n, "outputTokens": n}
//                                              (counts on macOS 26.4 and later)
//   {"event": "error", "code": "...", "message": "..."}
//
// `gen --probe` reads nothing and writes one line,
// {"event": "probe", "available": true|false, "reason": "..."}.
//
// It exits 0 after a finish or a probe, 1 after an error, 2 on a request it
// cannot read.

import Foundation
import FoundationModels

struct Turn: Decodable {
    let prompt: String
    let response: String
}

struct Sampling: Decodable {
    let mode: String
    let k: Int?
    let p: Double?
    let seed: UInt64?
}

struct Request: Decodable {
    let instructions: String?
    let turns: [Turn]?
    let prompt: String
    let temperature: Double?
    let maximumResponseTokens: Int?
    let sampling: Sampling?
}

struct Event: Encodable {
    let event: String
    var text: String? = nil
    var available: Bool? = nil
    var reason: String? = nil
    var code: String? = nil
    var message: String? = nil
    var inputTokens: Int? = nil
    var outputTokens: Int? = nil
}

@main
struct Gen {
    static func say(_ event: Event) {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.withoutEscapingSlashes]
        guard var line = try? encoder.encode(event) else { return }
        line.append(0x0A)
        FileHandle.standardOutput.write(line)
    }

    static func fail(_ code: String, _ message: String, exit status: Int32 = 1) -> Never {
        say(Event(event: "error", code: code, message: message))
        exit(status)
    }

    static func unavailable() -> String? {
        switch SystemLanguageModel.default.availability {
        case .available:
            return nil
        case .unavailable(.deviceNotEligible):
            return "device_not_eligible"
        case .unavailable(.appleIntelligenceNotEnabled):
            return "apple_intelligence_not_enabled"
        case .unavailable(.modelNotReady):
            return "model_not_ready"
        case .unavailable(let other):
            return String(describing: other)
        }
    }

    static func sampling(_ s: Sampling?) -> GenerationOptions.SamplingMode? {
        guard let s else { return nil }
        switch s.mode {
        case "greedy": return .greedy
        case "top": return .random(top: s.k ?? 50, seed: s.seed)
        case "nucleus": return .random(probabilityThreshold: s.p ?? 1.0, seed: s.seed)
        default: return nil
        }
    }

    static func code(_ error: LanguageModelSession.GenerationError) -> String {
        switch error {
        case .exceededContextWindowSize: return "exceeded_context_window"
        case .assetsUnavailable: return "assets_unavailable"
        case .guardrailViolation: return "guardrail_violation"
        case .unsupportedGuide: return "unsupported_guide"
        case .unsupportedLanguageOrLocale: return "unsupported_language"
        case .decodingFailure: return "decoding_failure"
        case .rateLimited: return "rate_limited"
        case .concurrentRequests: return "concurrent_requests"
        case .refusal: return "refusal"
        @unknown default: return "generation_failed"
        }
    }

    static func text(_ s: String) -> [Transcript.Segment] {
        [.text(Transcript.TextSegment(content: s))]
    }

    static func main() async {
        if CommandLine.arguments.dropFirst().contains("--probe") {
            let why = unavailable()
            say(Event(event: "probe", available: why == nil, reason: why))
            return
        }
        guard let line = readLine(strippingNewline: true),
              let request = try? JSONDecoder().decode(Request.self, from: Data(line.utf8))
        else {
            fail("bad_request", "the bridge reads one JSON request on stdin", exit: 2)
        }
        if let why = unavailable() {
            fail("unavailable", why)
        }

        var entries: [Transcript.Entry] = []
        if let instructions = request.instructions, !instructions.isEmpty {
            entries.append(.instructions(Transcript.Instructions(segments: text(instructions), toolDefinitions: [])))
        }
        for turn in request.turns ?? [] {
            entries.append(.prompt(Transcript.Prompt(segments: text(turn.prompt))))
            entries.append(.response(Transcript.Response(assetIDs: [], segments: text(turn.response))))
        }
        let session = LanguageModelSession(model: .default, transcript: Transcript(entries: entries))
        let options = GenerationOptions(
            sampling: sampling(request.sampling),
            temperature: request.temperature,
            maximumResponseTokens: request.maximumResponseTokens
        )

        var said = ""
        var sent = 0
        do {
            for try await snapshot in session.streamResponse(to: request.prompt, options: options) {
                said = snapshot.content
                let scalars = said.unicodeScalars
                let count = scalars.count
                if count > sent {
                    var delta = String.UnicodeScalarView()
                    delta.append(contentsOf: scalars.dropFirst(sent))
                    say(Event(event: "delta", text: String(delta)))
                    sent = count
                }
            }
        } catch let error as LanguageModelSession.GenerationError {
            fail(code(error), error.localizedDescription)
        } catch {
            fail("generation_failed", String(describing: error))
        }

        var finish = Event(event: "finish")
        if #available(macOS 26.4, *) {
            let asked = entries + [.prompt(Transcript.Prompt(segments: text(request.prompt)))]
            finish.inputTokens = try? await SystemLanguageModel.default.tokenCount(for: asked)
            if let counted = try? await SystemLanguageModel.default.tokenCount(for: said),
               let framing = try? await SystemLanguageModel.default.tokenCount(for: "") {
                finish.outputTokens = max(0, counted - framing)
            }
        }
        say(finish)
    }
}
