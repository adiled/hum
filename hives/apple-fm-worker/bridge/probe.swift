import Foundation
import FoundationModels

@main
struct Probe {
    static func main() async {
        let session = LanguageModelSession(model: .default)
        do {
            let stream = try await session.streamResponse(to: "Say: streaming works")
            var count = 0
            for try await snapshot in stream {
                let text = snapshot.content as String
                if !text.isEmpty {
                    print("[snapshot \(count)] \(text)")
                    count += 1
                }
            }
            print("[finished snapshots=\(count)]")
        } catch {
            print("[error] \(error)")
        }
    }
}
