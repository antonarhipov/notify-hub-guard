use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Gsm7,
    Utf16,
}

#[derive(Debug, Serialize)]
pub struct Estimate {
    pub encoding: Encoding,
    pub units: usize,
    pub segments: usize,
    pub single_segment_limit: usize,
    pub multipart_segment_limit: usize,
}

fn gsm_width(character: char) -> Option<usize> {
    const BASIC: &str = "@£$¥èéùìòÇ\nØø\rÅåΔ_ΦΓΛΩΠΨΣΘΞÆæßÉ !\"#¤%&'()*+,-./0123456789:;<=>?¡ABCDEFGHIJKLMNOPQRSTUVWXYZÄÖÑÜ§¿abcdefghijklmnopqrstuvwxyzäöñüà";
    const EXTENDED: &str = "\u{000c}^{}\\[~]|€";
    if BASIC.contains(character) {
        Some(1)
    } else if EXTENDED.contains(character) {
        Some(2)
    } else {
        None
    }
}

/// Common SMS limits, assuming a six-byte concatenation header. No transliteration.
pub fn estimate(text: &str) -> Estimate {
    let encoding = if text.chars().all(|c| gsm_width(c).is_some()) {
        Encoding::Gsm7
    } else {
        Encoding::Utf16
    };
    let width = |c: char| match encoding {
        Encoding::Gsm7 => gsm_width(c).expect("GSM alphabet was checked"),
        Encoding::Utf16 => c.len_utf16(),
    };
    let units = text.chars().map(width).sum();
    let (single_segment_limit, multipart_segment_limit) = match encoding {
        Encoding::Gsm7 => (160, 153),
        Encoding::Utf16 => (70, 67),
    };
    let segments = if units == 0 {
        0
    } else if units <= single_segment_limit {
        1
    } else {
        // Never split a GSM escape pair or UTF-16 surrogate pair across segments.
        let (mut segments, mut used) = (1, 0);
        for character in text.chars() {
            let size = width(character);
            if used + size > multipart_segment_limit {
                segments += 1;
                used = 0;
            }
            used += size;
        }
        segments
    };
    Estimate {
        encoding,
        units,
        segments,
        single_segment_limit,
        multipart_segment_limit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gsm_boundaries() {
        for (length, segments) in [(0, 0), (1, 1), (160, 1), (161, 2), (306, 2), (307, 3)] {
            let result = estimate(&"a".repeat(length));
            assert_eq!(result.encoding, Encoding::Gsm7);
            assert_eq!(result.units, length);
            assert_eq!(result.segments, segments, "length {length}");
        }
    }

    #[test]
    fn extension_characters_cost_two_septets_and_cannot_be_split() {
        let result = estimate("^{}\\[~]|€\u{000c}");
        assert_eq!(result.encoding, Encoding::Gsm7);
        assert_eq!(result.units, 20);
        assert_eq!(estimate(&"€".repeat(80)).segments, 1);
        assert_eq!(estimate(&"€".repeat(81)).segments, 2);
        assert_eq!(estimate(&"€".repeat(153)).segments, 3);
    }

    #[test]
    fn unicode_boundaries_and_surrogate_pairs() {
        for (length, segments) in [(70, 1), (71, 2), (134, 2), (135, 3)] {
            let result = estimate(&"漢".repeat(length));
            assert_eq!(result.encoding, Encoding::Utf16);
            assert_eq!(result.segments, segments);
        }
        assert_eq!(estimate(&"😀".repeat(35)).segments, 1);
        assert_eq!(estimate(&"😀".repeat(36)).segments, 2);
        assert_eq!(estimate(&"😀".repeat(67)).segments, 3);
        assert_eq!(estimate("👩‍💻").units, 5);
    }

    #[test]
    fn one_emoji_changes_the_encoding_of_the_entire_message() {
        let plain = "a".repeat(80);
        assert_eq!(estimate(&plain).segments, 1);
        let result = estimate(&(plain + "😀"));
        assert_eq!(result.encoding, Encoding::Utf16);
        assert_eq!(result.units, 82);
        assert_eq!(result.segments, 2);
        assert_eq!(estimate("éÅΔ").encoding, Encoding::Gsm7);
        assert_eq!(estimate("e\u{301}").encoding, Encoding::Utf16);
    }
}
