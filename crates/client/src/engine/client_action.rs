use super::input_mode::InputMode;

#[derive(Debug, PartialEq)]
pub enum ClientAction {
    StartComposition,
    EndComposition,
    CancelComposition,

    AppendText(String),
    RequestConversion,
    RemoveText,
    ShrinkText(String),
    CommitCandidate,
    LoadPredictions,
    AcceptPrediction,
    DismissPredictions,
    MagicConvert,

    SetTextWithType(SetTextType),

    SetSelection(SetSelectionType),

    SetIMEMode(InputMode),
}

#[derive(Debug, PartialEq)]
pub enum SetSelectionType {
    Up,
    Down,
    Number(i32),
}

#[derive(Debug, PartialEq)]
pub enum SetTextType {
    Hiragana,     // F6
    Katakana,     // F7
    HalfKatakana, // F8
    FullLatin,    // F9
    HalfLatin,    // F10
}
