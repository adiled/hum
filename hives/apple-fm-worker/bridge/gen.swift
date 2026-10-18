// apple-fm-worker bridge — the ONLY Apple-specific code in the tree.
// Wraps Apple's on-device FoundationModel (SystemLanguageModel, ~3B) and
// speaks NDJSON on stdout that the Rust worker parses into hum chunk/finish.
//
// stdin  line: {"system":"...","prompt":"..."}   (one request per run)
// stdout lines: {"event":"delta","text":"..."} / {"event":"finish"} / {"event":"error","message":"..."}
// {"probe":true} → emit availability and exit.

import Foundation
import FoundationModels

@main
struct Bridge {
    static func main() async {
        var system: String? = nil
        var prompt: String? = nil
        var probe = false
        while let raw = readLine() {
            guard let obj = parse(raw) else { continue }
            if let p = obj["probe"] as? Bool { probe = p; break }
            if let s = obj["system"] as? String { system = s }
            if let q = obj["prompt"] as? String { prompt = q; break }
        }

        if probe {
            print(#"{"event":"probe","available":\#(SystemLanguageModel.default.availability == .available)}"#)
            return
        }
        guard let prompt else {
            print(#"{"event":"error","message":"no prompt"}"#); return
        }

        let session: LanguageModelSession =
            system.map { LanguageModelSession(model: .default, instructions: Instructions($0)) }
                 ?? LanguageModelSession(model: .default)

        do {
            let stream = try await session.streamResponse(to: prompt)
            for try await snapshot in stream {
                print(encodeDelta(snapshot.content))
            }
            print(#"{"event":"finish"}"#)
        } catch {
            let msg = String(describing: error)
                .replacingOccurrences(of: "\\", with: "\\\\")
                .replacingOccurrences(of: "\"", with: "\\\"")
            print(#"{"event":"error","message":"\#(msg)"}"#)
        }
    }

    static func parse(_ s: String) -> [String: Any]? {
        guard let data = s.data(using: .utf8) else { return nil }
        return try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    }

    static func encodeDelta(_ text: String) -> String {
        let escaped = text
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
            .replacingOccurrences(of: "\n", with: "\\n")
            .replacingOccurrences(of: "\r", with: "\\r")
            .replacingOccurrences(of: "\t", with: "\\t")
        return #"{"event":"delta","text":"\#(escaped)"}"#
    }
}
