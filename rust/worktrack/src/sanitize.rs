//! Strip ANSI escapes and control characters from external text (spec 11.3).

/// Remove ANSI escape sequences and control characters except `\n`. Tabs become a space.
pub fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                // CSI: ESC [ ... final byte 0x40-0x7E; OSC: ESC ] ... BEL or ESC \ ; other: ESC + 1 char
                match chars.peek() {
                    Some('[') => {
                        chars.next();
                        for n in chars.by_ref() {
                            if ('\u{40}'..='\u{7e}').contains(&n) {
                                break;
                            }
                        }
                    }
                    Some(']') | Some('P') | Some('_') | Some('^') => {
                        chars.next();
                        while let Some(n) = chars.next() {
                            if n == '\u{7}' {
                                break;
                            }
                            if n == '\u{1b}' && chars.peek() == Some(&'\\') {
                                chars.next();
                                break;
                            }
                        }
                    }
                    Some(_) => {
                        chars.next();
                    }
                    None => {}
                }
            }
            '\n' => out.push('\n'),
            '\t' => out.push(' '),
            c if c.is_control() => {}
            // C1 controls are covered by is_control; also drop bidi overrides.
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {}
            c => out.push(c),
        }
    }
    out
}

/// `clean` and also flatten newlines to spaces, for single-line cells.
pub fn clean_line(s: &str) -> String {
    clean(s).replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_osc_and_controls() {
        assert_eq!(clean("a\u{1b}[31mred\u{1b}[0m b"), "ared b");
        assert_eq!(clean("x\u{1b}]0;title\u{7}y"), "xy");
        assert_eq!(clean("x\u{1b}]8;;http://e\u{1b}\\y"), "xy");
        assert_eq!(clean("a\rb\u{8}c\nd"), "abc\nd");
        assert_eq!(clean_line("a\nb"), "a b");
    }
}
