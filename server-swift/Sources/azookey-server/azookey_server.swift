import KanaKanjiConverterModule
import Foundation
import ffi

@MainActor let converter = KanaKanjiConverter()
@MainActor var composingText = ComposingText()

@MainActor var execURL = URL(filePath: "")
@MainActor var appDataURL = URL(filePath: "")
@MainActor var learningMemoryURL = URL(filePath: "")
@MainActor var lastCandidates: [Candidate] = []
@MainActor var lastCommittedCandidate: Candidate?
@MainActor var lastPredictionCandidates: [PostCompositionPredictionCandidate] = []
@MainActor var config: [String : Any] = [
    "enable": false,
    "profile": "",
    "learningMode": "inputAndOutput",
    "personalizationEnabled": false,
    "personalizationDirectory": "",
    "personalizationAlpha": 0.5,
    "predictionEnabled": false,
    "predictionMaxCandidates": 5,
]

@MainActor func getLearningType() -> LearningType {
    switch config["learningMode"] as? String {
    case "onlyOutput":
        return .onlyOutput
    case "nothing":
        return .nothing
    default:
        return .inputAndOutput
    }
}

@MainActor func getPersonalizationMode() -> ConvertRequestOptions.ZenzaiMode.PersonalizationMode? {
    guard config["personalizationEnabled"] as? Bool == true,
          let directory = config["personalizationDirectory"] as? String,
          !directory.isEmpty else {
        return nil
    }

    let requiredSuffixes = [
        "_c_abc.marisa",
        "_r_xbx.marisa",
        "_u_abx.marisa",
        "_u_xbc.marisa",
    ]
    let directoryURL = URL(filePath: directory)
    for prefix in ["original", "lm"] {
        for suffix in requiredSuffixes {
            let path = directoryURL.appendingPathComponent(prefix + suffix).path
            guard FileManager.default.fileExists(atPath: path) else {
                print("Personalization model missing: \(path)")
                return nil
            }
        }
    }

    let alphaValue = config["personalizationAlpha"]
    let alpha: Float
    if let number = alphaValue as? NSNumber {
        alpha = min(1, max(0, number.floatValue))
    } else {
        alpha = 0.5
    }

    return .init(
        baseNgramLanguageModel: directoryURL.appendingPathComponent("original").path,
        personalNgramLanguageModel: directoryURL.appendingPathComponent("lm").path,
        alpha: alpha
    )
}

@MainActor func getOptions(
    context: String = "",
    shouldResetMemory: Bool = false,
    requestRichCandidates: Bool = false
) -> ConvertRequestOptions {
    return ConvertRequestOptions(
        // As in azooKey-Desktop: prediction candidates mixed into the
        // conversion results would replace what was typed (と -> とか).
        requireJapanesePrediction: false,
        requireEnglishPrediction: false,
        keyboardLanguage: .ja_JP,
        learningType: getLearningType(),
        shouldResetMemory: shouldResetMemory,
        dictionaryResourceURL: execURL.appendingPathComponent("Dictionary"),
        memoryDirectoryURL: learningMemoryURL,
        sharedContainerURL: appDataURL,
        textReplacer: .init {
            return execURL.appendingPathComponent("EmojiDictionary").appendingPathComponent("emoji_all_E15.1.txt")
        },
        // zenzai
        zenzaiMode: config["enable"] as! Bool ? .on(
            weight: execURL.appendingPathComponent("zenz.gguf"),
            inferenceLimit: 1,
            requestRichCandidates: requestRichCandidates,
            personalizationMode: getPersonalizationMode(),
            versionDependentMode: .v3(
                .init(
                    profile: config["profile"] as! String,
                    leftSideContext: context
                )
            )
        ) : .off,
        preloadDictionary: true,
        metadata: .init(versionString: "Azookey for Windows")
    )
}

class SimpleComposingText {
    init(text: String, cursor: Int) {
        self.text = UnsafeMutablePointer<CChar>(mutating: text.utf8String)!
        self.cursor = cursor
    }

    var text: UnsafeMutablePointer<CChar>
    var cursor: Int
}

struct SComposingText {
    var text: UnsafeMutablePointer<CChar>
    var cursor: Int
}

func normalizeDictionaryReading(_ input: String) -> String {
    let result = input.utf16.map { scalar -> UInt16 in
        if 0x3041 <= scalar && scalar <= 0x3096 {
            return scalar + 96
        }
        return scalar
    }
    return String(utf16CodeUnits: result, count: result.count)
}

@MainActor func applyUserDictionary(_ json: [String: Any]) {
    let rawEntries = (json["dictionary"] as? [String: Any])?["entries"] as? [[String: Any]] ?? []
    let entries: [DicdataElement] = rawEntries.compactMap { item in
        guard let rawReading = item["reading"] as? String,
              let rawWord = item["word"] as? String else {
            return nil
        }

        let reading = rawReading.trimmingCharacters(in: .whitespacesAndNewlines)
        let word = rawWord.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !reading.isEmpty, !word.isEmpty else {
            return nil
        }

        return DicdataElement(
            word: word,
            ruby: normalizeDictionaryReading(reading),
            cid: CIDData.固有名詞.cid,
            mid: MIDData.一般.mid,
            value: -10
        )
    }

    converter.sendToDicdataStore(.importDynamicUserDict(entries))
}

func candidateDisplayText(_ candidate: Candidate) -> String {
    // ConversionResult.mainResults is already display-ready. Reconstructing the
    // surface from convertTarget and ruby breaks mixed Roman/Kana input because
    // roman2kana's intermediate target can differ from Candidate.text.
    candidate.text
}

@_silgen_name("LoadConfig")
@MainActor public func load_config() {
    if let appDataPath = ProcessInfo.processInfo.environment["APPDATA"] {
        let settingsPath = URL(filePath: appDataPath).appendingPathComponent("Azookey/settings.json")
        
        do {
            let data = try Data(contentsOf: settingsPath)
            if let json = try JSONSerialization.jsonObject(with: data) as? [String: Any] {
                if let zenzaiDict = json["zenzai"] as? [String: Any] {
                    if let enableValue = zenzaiDict["enable"] as? Bool {
                        config["enable"] = enableValue
                    }

                    if let profileValue = zenzaiDict["profile"] as? String {
                        config["profile"] = profileValue
                    }
                }

                if let learningDict = json["learning"] as? [String: Any],
                   let mode = learningDict["mode"] as? String {
                    config["learningMode"] = mode
                }

                if let personalization = json["personalization"] as? [String: Any] {
                    config["personalizationEnabled"] = personalization["enabled"] as? Bool ?? false
                    config["personalizationDirectory"] = personalization["tuner_directory"] as? String ?? ""
                    config["personalizationAlpha"] = personalization["alpha"] as? NSNumber ?? 0.5
                }

                if let prediction = json["prediction"] as? [String: Any] {
                    config["predictionEnabled"] = prediction["enabled"] as? Bool ?? false
                    config["predictionMaxCandidates"] = prediction["max_candidates"] as? NSNumber ?? 5
                }

                applyUserDictionary(json)
            }
        } catch {
            print("Failed to read settings: \(error)")
        }
    }
}

@_silgen_name("Initialize")
@MainActor public func initialize(
    path: UnsafePointer<CChar>,
    use_zenzai: Bool
) {
    let path = String(cString: path)
    execURL = URL(filePath: path)

    if let appDataPath = ProcessInfo.processInfo.environment["APPDATA"] {
        appDataURL = URL(filePath: appDataPath).appendingPathComponent("Azookey", isDirectory: true)
    } else {
        appDataURL = execURL.appendingPathComponent("UserData", isDirectory: true)
    }
    learningMemoryURL = appDataURL.appendingPathComponent("LearningMemory", isDirectory: true)

    do {
        try FileManager.default.createDirectory(
            at: learningMemoryURL,
            withIntermediateDirectories: true
        )
    } catch {
        print("Failed to create learning memory directory: \(error)")
    }

    load_config()

    composingText.insertAtCursorPosition("a", inputStyle: .roman2kana)
    converter.requestCandidates(composingText, options: getOptions())
    composingText = ComposingText()
}

@MainActor func predictionLimit() -> Int {
    let raw = (config["predictionMaxCandidates"] as? NSNumber)?.intValue ?? 5
    return min(10, max(1, raw))
}

@MainActor func clearPostCompositionPredictionState() {
    lastCommittedCandidate = nil
    lastPredictionCandidates = []
}

@MainActor func updatePostCompositionPredictions(candidate: Candidate) {
    guard config["predictionEnabled"] as? Bool == true else {
        clearPostCompositionPredictionState()
        return
    }

    lastCommittedCandidate = candidate
    let predictions = converter.requestPostCompositionPredictionCandidates(
        leftSideCandidate: candidate,
        options: getOptions()
    )
    lastPredictionCandidates = Array(predictions.prefix(predictionLimit()))
}

@_silgen_name("AppendText")
@MainActor public func append_text(
    input: UnsafePointer<CChar>,
    cursorPtr: UnsafeMutablePointer<Int32>
) -> UnsafeMutablePointer<CChar> {
    clearPostCompositionPredictionState()
    let inputString = String(cString: input)
    composingText.insertAtCursorPosition(inputString, inputStyle: .roman2kana)

    cursorPtr.pointee = Int32(composingText.convertTargetCursorPosition)    
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("RemoveText")
@MainActor public func remove_text(
    cursorPtr: UnsafeMutablePointer<Int32>
) -> UnsafeMutablePointer<CChar> {
    composingText.deleteBackwardFromCursorPosition(count: 1)

    cursorPtr.pointee = Int32(composingText.convertTargetCursorPosition)
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("MoveCursor")
@MainActor public func move_cursor(
    offset: Int32,
    cursorPtr: UnsafeMutablePointer<Int32>
) -> UnsafeMutablePointer<CChar> {
    let previousCursor = composingText.convertTargetCursorPosition
    let cursor = composingText.moveCursorFromCursorPosition(count: Int(offset))
    print("offset: \(offset), cursor: \(cursor)")

    cursorPtr.pointee = Int32(cursor)
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("ClearText")
@MainActor public func clear_text() {
    composingText = ComposingText()
    lastCandidates = []
}

@_silgen_name("CommitCandidate")
@MainActor public func commit_candidate(candidateIndex: Int32) {
    let index = Int(candidateIndex)
    guard lastCandidates.indices.contains(index) else {
        print("CommitCandidate: invalid candidate index \(candidateIndex)")
        return
    }

    let candidate = lastCandidates[index]
    converter.updateLearningData(candidate)

    var remainingText = composingText
    remainingText.prefixComplete(correspondingCount: candidate.correspondingCount)
    if remainingText.convertTarget.isEmpty {
        converter.stopComposition()
        updatePostCompositionPredictions(candidate: candidate)
    } else {
        clearPostCompositionPredictionState()
        converter.setCompletedData(candidate)
    }
}

@_silgen_name("FlushLearning")
@MainActor public func flush_learning() {
    converter.sendToDicdataStore(.closeKeyboard)
}

@_silgen_name("ResetLearning")
@MainActor public func reset_learning() {
    converter.sendToDicdataStore(
        .setRequestOptions(getOptions(shouldResetMemory: true))
    )
}

@_silgen_name("GetPostCompositionPredictions")
@MainActor public func get_post_composition_predictions(
    lengthPtr: UnsafeMutablePointer<Int32>
) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    var result: [FFICandidate] = []
    for (index, prediction) in lastPredictionCandidates.enumerated() {
        result.append(FFICandidate(
            text: strdup(prediction.text),
            subtext: strdup(""),
            hiragana: strdup(""),
            correspondingCount: 0,
            candidateIndex: Int32(index)
        ))
    }
    lengthPtr.pointee = Int32(result.count)
    return to_list_pointer(result)
}

@_silgen_name("CommitPrediction")
@MainActor public func commit_prediction(predictionIndex: Int32) {
    let index = Int(predictionIndex)
    guard let committedCandidate = lastCommittedCandidate,
          lastPredictionCandidates.indices.contains(index) else {
        clearPostCompositionPredictionState()
        return
    }

    let prediction = lastPredictionCandidates[index]
    if prediction.isTerminal {
        clearPostCompositionPredictionState()
        return
    }

    converter.updateLearningData(committedCandidate, with: prediction)
    let newCandidate = prediction.join(to: committedCandidate)
    updatePostCompositionPredictions(candidate: newCandidate)
}

@_silgen_name("ClearPostCompositionPredictions")
@MainActor public func clear_post_composition_predictions() {
    clearPostCompositionPredictionState()
}

func to_list_pointer(_ list: [FFICandidate]) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let pointer = UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?>.allocate(capacity: list.count)
    for (i, item) in list.enumerated() {
        pointer[i] = UnsafeMutablePointer<FFICandidate>.allocate(capacity: 1)
        pointer[i]?.pointee = item
    }
    return pointer
}

// Every string and candidate list returned across the FFI is owned by the
// caller and must be released with these functions.
@_silgen_name("FreeString")
public func free_string(ptr: UnsafeMutablePointer<CChar>?) {
    free(ptr)
}

@_silgen_name("FreeCandidates")
public func free_candidates(
    list: UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?>?,
    length: Int32
) {
    guard let list else { return }
    for i in 0..<Int(length) {
        guard let item = list[i] else { continue }
        free(item.pointee.text)
        free(item.pointee.subtext)
        free(item.pointee.hiragana)
        item.deallocate()
    }
    list.deallocate()
}

@_silgen_name("GetComposedText")
@MainActor public func get_composed_text(
    lengthPtr: UnsafeMutablePointer<Int32>,
    requestRichCandidates: Bool
) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let hiragana = composingText.convertTarget
    let contextString = (config["context"] as? String) ?? ""
    let options = getOptions(context: contextString, requestRichCandidates: requestRichCandidates)
    let converted = converter.requestCandidates(composingText, options: options)
    lastCandidates = converted.mainResults
    var result: [FFICandidate] = []

    for i in 0..<converted.mainResults.count {
        let candidate = converted.mainResults[i]

        let text = strdup(candidateDisplayText(candidate))
        let hiragana = strdup(hiragana)
        let correspondingCount = candidate.correspondingCount

        var afterComposingText = composingText
        afterComposingText.prefixComplete(correspondingCount: correspondingCount)
        let subtext = strdup(afterComposingText.convertTarget)

        result.append(FFICandidate(
            text: text,
            subtext: subtext,
            hiragana: hiragana,
            correspondingCount: Int32(correspondingCount),
            candidateIndex: Int32(i)
        ))        
    }

    lengthPtr.pointee = Int32(result.count)

    return to_list_pointer(result)
}

@_silgen_name("ShrinkText")
@MainActor public func shrink_text(
    offset: Int32
) -> UnsafeMutablePointer<CChar>  {
    var afterComposingText = composingText
    // prefixComplete clamps the upper bound only; a negative count traps.
    afterComposingText.prefixComplete(correspondingCount: max(0, Int(offset)))
    composingText = afterComposingText

    return _strdup(composingText.convertTarget)!
}

/// Input units in the composition. Candidates' correspondingCount and
/// ShrinkText count in these units, not in characters of convertTarget.
@_silgen_name("GetInputCount")
@MainActor public func get_input_count() -> Int32 {
    return Int32(composingText.input.count)
}

/// The input units themselves, one character each (GetInputCount of them).
/// After a deletion the converter may have rewritten romaji into kana
/// ("sha" -> "し"), so this is the only reliable record of the input.
@_silgen_name("GetInputText")
@MainActor public func get_input_text() -> UnsafeMutablePointer<CChar> {
    return _strdup(String(composingText.input.map(\.character)))!
}

@_silgen_name("SetContext")
@MainActor public func set_context(
    context: UnsafePointer<CChar>
) {
    let contextString = String(cString: context)
    config["context"] = contextString
}