import EfficientNGram
import Foundation
import Testing

// Zenzai's personalization term is alpha * (personal - base) log probability
// from two EfficientNGram models. Without SwiftyMarisa the converter compiles
// a mock whose initializer loads nothing and which returns 1/6000 for every
// token, so the term is always zero. This trains a model the way the tuner
// does and checks that loading it really predicts from it.
@Test func trainedModelPredictsItsTrainingText() throws {
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("azookey-personalization-\(UUID().uuidString)", isDirectory: true)
    defer { try? FileManager.default.removeItem(at: directory) }

    trainNGram(
        lines: Array(repeating: "これは日本語です", count: 3),
        n: 5,
        baseFilePattern: "lm",
        outputDir: directory.path
    )
    // The files the server requires before it enables personalization.
    for suffix in ["_c_abc.marisa", "_r_xbx.marisa", "_u_abx.marisa", "_u_xbc.marisa"] {
        let path = directory.appendingPathComponent("lm" + suffix).path
        #expect(FileManager.default.fileExists(atPath: path), "\(path) was not written")
    }

    let model = EfficientNGram(
        baseFilename: directory.appendingPathComponent("lm").path,
        n: 5,
        d: 0.75,
        tokenizer: ZenzTokenizer()
    )
    // "これは日本語です" is [268, 262, 253, 304, 358, 698, 246, 255]; after
    // "これは" the model has only ever seen "日" (304).
    let probabilities = model.bulkPredict([268, 262, 253])
    let best = try #require(probabilities.indices.max { probabilities[$0] < probabilities[$1] })
    #expect(best == 304)
    #expect(probabilities[304] > 0.5)
    // The same query must give the same answer: keys and queries are byte
    // arrays without a terminator, and reading past them made results vary.
    #expect(model.bulkPredict([268, 262, 253]) == probabilities)
}
