struct TagDef {
    name: &'static str,
    ansi_on: &'static str,
    ansi_off: &'static str,
    html_class: &'static str,
}

const TAGS: &[TagDef] = &[
    TagDef { name: "b",       ansi_on: "\x1b[1m",  ansi_off: "\x1b[22m", html_class: "b" },
    TagDef { name: "dim",     ansi_on: "\x1b[2m",  ansi_off: "\x1b[22m", html_class: "dim" },
    TagDef { name: "i",       ansi_on: "\x1b[3m",  ansi_off: "\x1b[23m", html_class: "i" },
    TagDef { name: "u",       ansi_on: "\x1b[4m",  ansi_off: "\x1b[24m", html_class: "u" },
    TagDef { name: "red",     ansi_on: "\x1b[31m", ansi_off: "\x1b[39m", html_class: "c-red" },
    TagDef { name: "green",   ansi_on: "\x1b[32m", ansi_off: "\x1b[39m", html_class: "c-green" },
    TagDef { name: "yellow",  ansi_on: "\x1b[33m", ansi_off: "\x1b[39m", html_class: "c-yellow" },
    TagDef { name: "blue",    ansi_on: "\x1b[34m", ansi_off: "\x1b[39m", html_class: "c-blue" },
    TagDef { name: "magenta", ansi_on: "\x1b[35m", ansi_off: "\x1b[39m", html_class: "c-magenta" },
    TagDef { name: "cyan",    ansi_on: "\x1b[36m", ansi_off: "\x1b[39m", html_class: "c-cyan" },
    TagDef { name: "white",   ansi_on: "\x1b[37m", ansi_off: "\x1b[39m", html_class: "c-white" },
];

fn find_tag(name: &str) -> Option<&'static TagDef> {
    TAGS.iter().find(|t| t.name == name)
}

/// Convert BBCode-style markup to ANSI escape sequences for telnet.
/// Raw ANSI codes pass through unchanged.
pub fn to_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices();

    while let Some((i, ch)) = chars.next() {
        if ch != '[' {
            out.push(ch);
            continue;
        }
        if let Some(end) = text[i + 1..].find(']') {
            let tag = &text[i + 1..i + 1 + end];
            if tag == "/" {
                out.push_str("\x1b[0m");
                advance(&mut chars, end + 1);
                continue;
            }
            if let Some(name) = tag.strip_prefix('/') {
                if let Some(def) = find_tag(name) {
                    out.push_str(def.ansi_off);
                    advance(&mut chars, end + 1);
                    continue;
                }
            }
            if let Some(def) = find_tag(tag) {
                out.push_str(def.ansi_on);
                advance(&mut chars, end + 1);
                continue;
            }
            if tag.starts_with("cmd=") {
                out.push_str("\x1b[4m");
                advance(&mut chars, end + 1);
                continue;
            }
            if tag == "/cmd" {
                out.push_str("\x1b[24m");
                advance(&mut chars, end + 1);
                continue;
            }
        }
        out.push('[');
    }

    out
}

/// Convert BBCode-style markup to HTML spans for the web client.
/// Strips telnet IAC sequences and converts raw ANSI escape sequences
/// that softcode may produce.
pub fn to_html(text: &str) -> String {
    let clean = strip_iac(text);
    let ansi_converted = ansi_to_bbcode(&clean);
    bbcode_to_html(&ansi_converted)
}

fn strip_iac(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch == '\u{FFFD}' {
            continue;
        }
        if ch as u32 == 0xFF {
            continue;
        }
        out.push(ch);
    }
    out
}

fn advance(chars: &mut std::str::CharIndices, n: usize) {
    for _ in 0..n {
        chars.next();
    }
}

const ANSI_COLORS: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

fn ansi_to_bbcode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let parts: Vec<&str> = text.split('\x1b').collect();
    out.push_str(parts[0]);

    for part in &parts[1..] {
        if let Some(rest) = part.strip_prefix('[') {
            if let Some(m_end) = rest.find('m') {
                let codes_str = &rest[..m_end];
                let after = &rest[m_end + 1..];
                let codes: Vec<u16> = codes_str
                    .split(';')
                    .filter_map(|s| s.parse().ok())
                    .collect();

                for &c in &codes {
                    match c {
                        0 => out.push_str("[/]"),
                        1 => out.push_str("[b]"),
                        2 => out.push_str("[dim]"),
                        3 => out.push_str("[i]"),
                        4 => out.push_str("[u]"),
                        22 => out.push_str("[/b]"),
                        23 => out.push_str("[/i]"),
                        24 => out.push_str("[/u]"),
                        30..=37 => {
                            let name = ANSI_COLORS[(c - 30) as usize];
                            out.push_str(&format!("[{name}]"));
                        }
                        39 => out.push_str("[/]"),
                        90..=97 => {
                            let name = ANSI_COLORS[(c - 90) as usize];
                            out.push_str(&format!("[{name}]"));
                        }
                        _ => {}
                    }
                }
                out.push_str(after);
                continue;
            }
        }
        out.push('\x1b');
        out.push_str(part);
    }

    out
}

fn bbcode_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut open: u32 = 0;
    let mut chars = text.char_indices();

    while let Some((i, ch)) = chars.next() {
        if ch == '[' {
            if let Some(end) = text[i + 1..].find(']') {
                let tag = &text[i + 1..i + 1 + end];
                if tag == "/" {
                    for _ in 0..open {
                        out.push_str("</span>");
                    }
                    open = 0;
                    advance(&mut chars, end + 1);
                    continue;
                }
                if let Some(name) = tag.strip_prefix('/') {
                    if find_tag(name).is_some() && open > 0 {
                        out.push_str("</span>");
                        open -= 1;
                        advance(&mut chars, end + 1);
                        continue;
                    }
                }
                if let Some(def) = find_tag(tag) {
                    out.push_str("<span class=\"");
                    out.push_str(def.html_class);
                    out.push_str("\">");
                    open += 1;
                    advance(&mut chars, end + 1);
                    continue;
                }
                if let Some(cmd) = tag.strip_prefix("cmd=") {
                    out.push_str("<span class=\"cmd\" data-cmd=\"");
                    for c in cmd.chars() {
                        match c {
                            '"' => out.push_str("&quot;"),
                            '&' => out.push_str("&amp;"),
                            '<' => out.push_str("&lt;"),
                            _ => out.push(c),
                        }
                    }
                    out.push_str("\">");
                    open += 1;
                    advance(&mut chars, end + 1);
                    continue;
                }
                if tag == "/cmd" && open > 0 {
                    out.push_str("</span>");
                    open -= 1;
                    advance(&mut chars, end + 1);
                    continue;
                }
            }
            out.push_str("&#91;");
        } else {
            match ch {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                _ => out.push(ch),
            }
        }
    }

    for _ in 0..open {
        out.push_str("</span>");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbcode_to_ansi_roundtrip() {
        let input = "[b][cyan]Hello[/]";
        let ansi = to_ansi(input);
        assert_eq!(ansi, "\x1b[1m\x1b[36mHello\x1b[0m");
    }

    #[test]
    fn bbcode_to_html_basic() {
        let input = "[b][cyan]Hello[/]";
        let html = to_html(input);
        assert_eq!(html, "<span class=\"b\"><span class=\"c-cyan\">Hello</span></span>");
    }

    #[test]
    fn raw_ansi_to_html() {
        let input = "\x1b[1m\x1b[36mHello\x1b[0m";
        let html = to_html(input);
        assert_eq!(html, "<span class=\"b\"><span class=\"c-cyan\">Hello</span></span>");
    }

    #[test]
    fn html_escapes_content() {
        let input = "[red]<script>alert(1)</script>[/]";
        let html = to_html(input);
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn unknown_brackets_pass_through() {
        let ansi = to_ansi("score [10/20]");
        assert_eq!(ansi, "score [10/20]");
    }

    #[test]
    fn mixed_bbcode_and_ansi() {
        let input = "[b]Hello\x1b[31m world[/]";
        let html = to_html(input);
        assert!(html.contains("class=\"b\""));
        assert!(html.contains("class=\"c-red\""));
    }
}
