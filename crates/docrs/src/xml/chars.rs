//! XML 1.0 (Fifth Edition) character and name productions.

/// `Char ::= #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]`
pub fn is_xml_char(c: char) -> bool {
    matches!(c,
        '\u{9}' | '\u{A}' | '\u{D}'
        | '\u{20}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}'
        | '\u{10000}'..='\u{10FFFF}')
}

/// `S ::= (#x20 | #x9 | #xD | #xA)+`
pub fn is_xml_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

/// `NameStartChar` (including `:`).
pub fn is_name_start_char(c: char) -> bool {
    matches!(c,
        ':' | 'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}'
        | '\u{D8}'..='\u{F6}'
        | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}'
        | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}'
        | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}'
        | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

/// `NameChar` (including `:`).
pub fn is_name_char(c: char) -> bool {
    is_name_start_char(c)
        || matches!(c,
            '-' | '.' | '0'..='9' | '\u{B7}'
            | '\u{300}'..='\u{36F}'
            | '\u{203F}'..='\u{2040}')
}

/// `NCName`: a `Name` without colons.
pub fn is_ncname(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c != ':' && is_name_start_char(c) => {}
        _ => return false,
    }
    chars.all(|c| c != ':' && is_name_char(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ncname_rules() {
        assert!(is_ncname("rId1"));
        assert!(is_ncname("_x"));
        assert!(is_ncname("é"));
        assert!(!is_ncname(""));
        assert!(!is_ncname("1a"));
        assert!(!is_ncname("a:b"));
        assert!(!is_ncname("-a"));
        assert!(!is_ncname("a b"));
    }

    #[test]
    fn char_rules() {
        assert!(!is_xml_char('\u{0}'));
        assert!(!is_xml_char('\u{1}'));
        assert!(!is_xml_char('\u{FFFE}'));
        assert!(is_xml_char('\u{1F600}'));
        assert!(is_xml_char('\t'));
    }
}
