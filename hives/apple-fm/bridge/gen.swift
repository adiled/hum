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
//   {"event": "finish", "reason": "stop" | "max_tokens", "dropped": n,
//    "inputTokens": n, "outputTokens": n}      (reason and counts on macOS 26.4 and later)
//   {"event": "error", "code": "...", "message": "..."}
//
// The model's context is small (4096 tokens), so the oldest turns that do not
// fit are dropped and counted in the finish's `dropped`: the newest turns are
// kept that leave room for the reply, its `maximumResponseTokens` or 1024
// tokens, and a reply with no `maximumResponseTokens` is held to the room that
// is left, so it ends `max_tokens` rather than overflowing. Where Apple cannot
// count tokens, a transcript Apple refuses as too long is retried with the
// older half of its turns dropped.
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
    var dropped: Int? = nil
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

    static func exchange(_ turn: Turn) -> [Transcript.Entry] {
        [
            .prompt(Transcript.Prompt(segments: text(turn.prompt))),
            .response(Transcript.Response(assetIDs: [], segments: text(turn.response))),
        ]
    }

    static let reply = 1024

    @available(macOS 26.4, *)
    static func fit(_ turns: [Turn], around fixed: [Transcript.Entry], reserving: Int?) async -> (kept: [Turn], cap: Int)? {
        let model = SystemLanguageModel.default
        let context = model.contextSize
        let reserve = min(reserving ?? reply, context / 2)
        guard var used = try? await model.tokenCount(for: fixed) else { return nil }
        var kept = 0
        for turn in turns.reversed() {
            guard let counted = try? await model.tokenCount(for: exchange(turn)) else { return nil }
            let cost = max(0, counted - 1)
            if used + cost + reserve > context { break }
            used += cost
            kept += 1
        }
        return (Array(turns.suffix(kept)), max(1, context - used - 16))
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

        var head: [Transcript.Entry] = []
        if let instructions = request.instructions, !instructions.isEmpty {
            head.append(.instructions(Transcript.Instructions(segments: text(instructions), toolDefinitions: [])))
        }
        let asked: Transcript.Entry = .prompt(Transcript.Prompt(segments: text(request.prompt)))
        var turns = request.turns ?? []
        var dropped = 0
        var cap = request.maximumResponseTokens
        if #available(macOS 26.4, *), let fitted = await fit(turns, around: head + [asked], reserving: request.maximumResponseTokens) {
            dropped = turns.count - fitted.kept.count
            turns = fitted.kept
            if cap == nil { cap = fitted.cap }
        }
        let options = GenerationOptions(
            sampling: sampling(request.sampling),
            temperature: request.temperature,
            maximumResponseTokens: cap
        )

        var said = ""
        var sent = 0
        while true {
            let session = LanguageModelSession(model: .default, transcript: Transcript(entries: head + turns.flatMap(exchange)))
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
                break
            } catch LanguageModelSession.GenerationError.exceededContextWindowSize where sent == 0 && !turns.isEmpty {
                let older = (turns.count + 1) / 2
                turns.removeFirst(older)
                dropped += older
            } catch let error as LanguageModelSession.GenerationError {
                fail(code(error), error.localizedDescription)
            } catch {
                fail("generation_failed", String(describing: error))
            }
        }

        var finish = Event(event: "finish")
        if dropped > 0 {
            finish.dropped = dropped
        }
        if #available(macOS 26.4, *) {
            let model = SystemLanguageModel.default
            finish.inputTokens = try? await model.tokenCount(for: head + turns.flatMap(exchange) + [asked])
            if let counted = try? await model.tokenCount(for: said),
               let framing = try? await model.tokenCount(for: "") {
                finish.outputTokens = max(0, counted - framing)
            }
            if let out = finish.outputTokens, let cap {
                finish.reason = out >= cap ? "max_tokens" : "stop"
            }
        }
        say(finish)
    }
}
