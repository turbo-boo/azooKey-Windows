import Testing
@testable import azookey_server

@Test func dictionaryReadingNormalizesHiraganaOnly() {
    #expect(normalizeDictionaryReading("あずーきーABC123") == "アズーキーABC123")
}

@Test @MainActor func predictionLimitIsClamped() {
    let previous = config["predictionMaxCandidates"]
    defer {
        if let previous {
            config["predictionMaxCandidates"] = previous
        } else {
            config.removeValue(forKey: "predictionMaxCandidates")
        }
    }

    config["predictionMaxCandidates"] = 0
    #expect(predictionLimit() == 1)

    config["predictionMaxCandidates"] = 7
    #expect(predictionLimit() == 7)

    config["predictionMaxCandidates"] = 100
    #expect(predictionLimit() == 10)
}


@Test func candidateDisplayTextPreservesMixedRomanSurface() {
    let candidate = Candidate(
        text: "Shift",
        value: -10,
        correspondingCount: 5,
        lastMid: MIDData.一般.mid,
        data: [
            DicdataElement(
                word: "Shift",
                ruby: "shift",
                cid: CIDData.固有名詞.cid,
                mid: MIDData.一般.mid,
                value: -10
            )
        ]
    )

    #expect(candidateDisplayText(candidate) == "Shift")
}
