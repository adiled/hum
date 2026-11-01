//! GSM 03.38, the 7-bit default alphabet: the charset `AT+CSCS="GSM"`
//! sets, one byte a character, on the port both ways.
//!
//! In text mode Ctrl-Z ends a text and ESC abandons it, so a reply never
//! carries them: Ξ, which shares Ctrl-Z's code, and every character of the
//! extension table, which is reached through ESC, go as their nearest in
//! the main table.

const BASIC: [char; 128] = [
    '@', '£', '$', '¥', 'è', 'é', 'ù', 'ì', 'ò', 'Ç', '\n', 'Ø', 'ø', '\r', 'Å', 'å',
    'Δ', '_', 'Φ', 'Γ', 'Λ', 'Ω', 'Π', 'Ψ', 'Σ', 'Θ', 'Ξ', '\u{1b}', 'Æ', 'æ', 'ß', 'É',
    ' ', '!', '"', '#', '¤', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/',
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?',
    '¡', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', 'Ä', 'Ö', 'Ñ', 'Ü', '§',
    '¿', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', 'ä', 'ö', 'ñ', 'ü', 'à',
];

const EXTENSION: [(u8, char); 10] = [
    (0x0A, '\u{0c}'), (0x14, '^'), (0x28, '{'), (0x29, '}'), (0x2F, '\\'),
    (0x3C, '['), (0x3D, '~'), (0x3E, ']'), (0x40, '|'), (0x65, '€'),
];

const CTRL_Z: u8 = 0x1A;
const ESC: u8 = 0x1B;
const MORE: &[u8] = b"...";

pub fn decode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut bytes = bytes.iter();
    while let Some(&b) = bytes.next() {
        let c = match b {
            ESC => match bytes.next() {
                Some(&e) => EXTENSION.iter().find(|(code, _)| *code == e).map_or(basic(e), |(_, c)| *c),
                None => ' ',
            },
            b => basic(b),
        };
        out.push(c);
    }
    out
}

fn basic(b: u8) -> char {
    BASIC.get(b as usize).copied().unwrap_or('?')
}

pub fn encode(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        match BASIC.iter().position(|b| *b == c) {
            Some(code) if code as u8 != CTRL_Z && code as u8 != ESC => out.push(code as u8),
            _ => out.extend(nearest(c).chars().filter_map(|n| BASIC.iter().position(|b| *b == n)).map(|code| code as u8)),
        }
    }
    out
}

fn nearest(c: char) -> &'static str {
    match c {
        '‘' | '’' | '‚' | '′' | '`' | '´' => "'",
        '“' | '”' | '„' | '″' | '«' | '»' => "\"",
        '‐' | '‑' | '–' | '—' | '−' | '~' => "-",
        '…' => "...",
        '•' | '·' => "*",
        '[' | '{' => "(",
        ']' | '}' => ")",
        '\\' | '|' => "/",
        '€' => "EUR",
        'Ξ' => "X",
        '\t' | '\u{0c}' | '\u{a0}' | '\u{2009}' | '\u{202f}' => " ",
        'á' | 'â' | 'ã' | 'ā' => "a",
        'ê' | 'ë' | 'ē' => "e",
        'í' | 'î' | 'ï' => "i",
        'ó' | 'ô' | 'õ' | 'ō' => "o",
        'ú' | 'û' => "u",
        'ç' => "c",
        'Á' | 'Â' | 'Ã' | 'À' => "A",
        'È' | 'Ê' | 'Ë' => "E",
        'Í' | 'Î' | 'Ï' | 'Ì' => "I",
        'Ó' | 'Ô' | 'Õ' | 'Ò' => "O",
        'Ú' | 'Û' | 'Ù' => "U",
        _ => "?",
    }
}

/// A reply as the modem takes it: in the GSM charset, at most `limit`
/// characters, and ending in `...` when it is cut and the limit leaves
/// room for more than that.
pub fn fit(reply: &str, limit: usize) -> Vec<u8> {
    let mut out = encode(reply);
    if out.len() > limit {
        if limit > MORE.len() {
            out.truncate(limit - MORE.len());
            out.extend_from_slice(MORE);
        } else {
            out.truncate(limit);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_table_goes_both_ways() {
        let all: String = BASIC.iter().filter(|c| !matches!(**c, 'Ξ' | '\u{1b}')).collect();
        assert_eq!(decode(&encode(&all)), all);
        assert_eq!(encode("@$_ ¡¿"), [0x00, 0x02, 0x11, 0x20, 0x40, 0x60]);
        assert_eq!(encode("Hi, 42!"), b"Hi, 42!");
    }

    #[test]
    fn what_the_charset_lacks_goes_as_its_nearest() {
        assert_eq!(decode(&encode("“Ok…” – [1] {x} ~ € 💐 日")), "\"Ok...\" - (1) (x) - EUR ? ?");
        assert_eq!(decode(&encode("ação, Ξ")), "acao, X");
    }

    #[test]
    fn a_reply_never_ends_or_abandons_a_text() {
        let every: String = (0..0x3000u32).filter_map(char::from_u32).collect();
        let out = encode(&every);
        assert!(!out.contains(&CTRL_Z) && !out.contains(&ESC));
        assert!(out.iter().all(|b| *b < 0x80));
    }

    #[test]
    fn the_extension_table_is_read() {
        assert_eq!(decode(&[0x1B, 0x65, b'5', 0x1B, 0x3C, b'a', 0x1B, 0x3E]), "€5[a]");
        assert_eq!(decode(&[0x1B, b'A']), "A");
    }

    #[test]
    fn a_cut_lands_on_a_character() {
        let reply = "é".repeat(800);
        assert_eq!(fit(&reply, 1500), [0x05; 800]);
        let fitted = fit(&"é".repeat(2000), 1500);
        assert_eq!(decode(&fitted), format!("{}...", "é".repeat(1497)));
        assert_eq!(decode(&fit("日本語です。", 5)), "??...");
    }

    #[test]
    fn a_low_limit_cuts_without_the_ellipsis() {
        assert_eq!(fit("hello", 0), b"");
        assert_eq!(fit("hello", 2), b"he");
        assert_eq!(fit("hello", 3), b"hel");
        assert_eq!(fit("hello", 4), b"h...");
        assert_eq!(fit("hello", 5), b"hello");
    }
}
