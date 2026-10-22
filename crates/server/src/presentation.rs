//! How the composition is presented after an edit, following azooKey-Desktop
//! (`SegmentsManager.getCurrentMarkedText` and its `requestRichCandidates`
//! use).

/// The edit that produced the composition being presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionEdit {
    /// Characters were typed or the cursor moved.
    Typed,
    /// Characters were deleted (Backspace).
    Deleted,
    /// The user asked for conversion (Space) or committed a prefix.
    ConversionRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    /// Show the typed hiragana as is.
    Hiragana,
    /// Show the converter's first candidate. Rich candidates take longer to
    /// generate, so they are only requested for the candidate list.
    Converted { rich_candidates: bool },
}

pub fn presentation(edit: CompositionEdit, live_conversion: bool, hiragana: &str) -> Presentation {
    match edit {
        CompositionEdit::ConversionRequested => Presentation::Converted {
            rich_candidates: true,
        },
        // after a deletion the hiragana is always shown
        CompositionEdit::Deleted => Presentation::Hiragana,
        // a single character is not live-converted
        CompositionEdit::Typed if live_conversion && hiragana.chars().count() > 1 => {
            Presentation::Converted {
                rich_candidates: false,
            }
        }
        CompositionEdit::Typed => Presentation::Hiragana,
    }
}

#[cfg(test)]
mod tests {
    use super::{presentation, CompositionEdit::*, Presentation::*};

    #[test]
    fn single_character_is_not_live_converted() {
        assert_eq!(presentation(Typed, true, "と"), Hiragana);
        assert_eq!(
            presentation(Typed, true, "とと"),
            Converted {
                rich_candidates: false
            }
        );
    }

    #[test]
    fn deletion_shows_hiragana_even_with_live_conversion() {
        assert_eq!(presentation(Deleted, true, "きょうは"), Hiragana);
    }

    #[test]
    fn live_conversion_off_converts_only_on_request() {
        assert_eq!(presentation(Typed, false, "きょうは"), Hiragana);
        assert_eq!(
            presentation(ConversionRequested, false, "と"),
            Converted {
                rich_candidates: true
            }
        );
    }
}
