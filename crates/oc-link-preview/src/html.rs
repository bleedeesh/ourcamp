//! What a page says about itself: the `<title>` and `<meta>` tags of its head. Not an HTML
//! parser: it walks the tags and stops at the body, which is all a preview needs. Bytes from
//! the internet can be anything, so it never panics and never looks back.

/// What a page says about itself; empty strings where it says nothing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PageInfo {
    /// Its title, cleaned.
    pub title: String,
    /// Its description, cleaned.
    pub description: String,
    /// As written in the page: possibly relative.
    pub image: String,
}

/// Each field by the tags that may fill it, the best first.
const TITLE: [&str; 2] = ["og:title", "twitter:title"];
const DESCRIPTION: [&str; 3] = ["og:description", "twitter:description", "description"];
const IMAGE: [&str; 4] = [
    "og:image:secure_url",
    "og:image",
    "twitter:image",
    "twitter:image:src",
];

/// Reads a page, given what its server said about the encoding (`Content-Type`'s `charset`).
#[must_use]
pub fn read_page(bytes: &[u8], declared_charset: Option<&str>) -> PageInfo {
    // Tags are ASCII in every encoding handled here, so bytes are read as Latin-1 first to find
    // the charset, without losing anything: each byte is one `char`.
    let raw: String = bytes.iter().map(|&b| char::from(b)).collect();
    let tags = head_tags(&raw);
    let charset = declared_charset
        .map(str::to_owned)
        .or_else(|| tags.charset.clone());
    let decode = |text: &str| decode(text, charset.as_deref());

    let best = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| {
                tags.meta
                    .iter()
                    .find(|(key, value)| key == name && !value.trim().is_empty())
                    .map(|(_, value)| value.as_str())
            })
            .map_or_else(String::new, |value| clean(&decode(value)))
    };

    let mut title = best(&TITLE);
    if title.is_empty() {
        title = clean(&decode(&tags.title));
    }
    PageInfo {
        title,
        description: best(&DESCRIPTION),
        image: best(&IMAGE),
    }
}

/// The charset in a `Content-Type` value.
#[must_use]
pub fn charset_of(content_type: &str) -> Option<&str> {
    content_type.split(';').skip(1).find_map(|part| {
        let (key, value) = part.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"'))
    })
}

#[derive(Debug, Default)]
struct HeadTags {
    title: String,
    /// `property` or `name`, lowercased, and `content`, all as raw as the page wrote them.
    meta: Vec<(String, String)>,
    charset: Option<String>,
}

fn head_tags(page: &str) -> HeadTags {
    let mut found = HeadTags::default();
    let mut rest = page;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        let name_end = rest[1..]
            .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
            .map_or(rest.len(), |at| at + 1);
        let name = rest[1..name_end].to_ascii_lowercase();
        let Some(tag_end) = end_of_tag(&rest[name_end..]).map(|at| at + name_end) else {
            break;
        };
        let attributes = &rest[name_end..tag_end];
        let after = &rest[(tag_end + 1).min(rest.len())..];
        match name.as_str() {
            "body" | "/head" => break,
            "meta" => read_meta(attributes, &mut found),
            "title" => {
                let (text, next) = raw_text(after, "title");
                if found.title.is_empty() {
                    text.clone_into(&mut found.title);
                }
                rest = next;
                continue;
            }
            // Their insides can hold anything, tags included.
            "script" | "style" | "noscript" | "textarea" => {
                rest = raw_text(after, &name).1;
                continue;
            }
            _ => {}
        }
        rest = after;
    }
    found
}

/// Where a tag's `>` is, past quoted values.
fn end_of_tag(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (at, c) in tag.char_indices() {
        match (quote, c) {
            (None, '>') => return Some(at),
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if q == c => quote = None,
            _ => {}
        }
    }
    None
}

/// The text up to `</name`, and what follows that closing tag.
fn raw_text<'a>(after: &'a str, name: &str) -> (&'a str, &'a str) {
    let closing = format!("</{name}");
    let Some(at) = find_ignoring_case(after, &closing) else {
        return (after, "");
    };
    let rest = &after[at..];
    (
        &after[..at],
        rest.find('>').map_or("", |end| &rest[end + 1..]),
    )
}

fn find_ignoring_case(text: &str, needle: &str) -> Option<usize> {
    text.as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn read_meta(attributes: &str, found: &mut HeadTags) {
    let (mut key, mut content, mut http_equiv) = (None, None, false);
    for (name, value) in parse_attributes(attributes) {
        match name.as_str() {
            "property" | "name" => key = Some(value.trim().to_ascii_lowercase()),
            "content" => content = Some(value),
            "http-equiv" => http_equiv = value.trim().eq_ignore_ascii_case("content-type"),
            "charset" if found.charset.is_none() => found.charset = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    if http_equiv && found.charset.is_none() {
        found.charset = content.as_deref().and_then(charset_of).map(str::to_owned);
    }
    if let (Some(key), Some(content)) = (key, content) {
        found.meta.push((key, content));
    }
}

/// `name="value" name='value' name=value name`, names lowercased.
fn parse_attributes(mut text: &str) -> Vec<(String, String)> {
    let mut attributes = Vec::new();
    loop {
        text = text.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        if text.is_empty() {
            return attributes;
        }
        let name_end = text
            .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '/')
            .unwrap_or(text.len());
        let name = text[..name_end].to_ascii_lowercase();
        text = text[name_end..].trim_start_matches(|c: char| c.is_ascii_whitespace());
        let Some(value_start) = text.strip_prefix('=') else {
            attributes.push((name, String::new()));
            continue;
        };
        text = value_start.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let value = if let Some(quote @ ('"' | '\'')) = text.chars().next() {
            let inner = &text[1..];
            let end = inner.find(quote).unwrap_or(inner.len());
            text = inner.get(end + 1..).unwrap_or("");
            &inner[..end]
        } else {
            let end = text
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(text.len());
            let value = &text[..end];
            text = &text[end..];
            value
        };
        attributes.push((name, value.to_owned()));
    }
}

/// Text read as Latin-1, as the page meant it. An encoding not handled here gives nothing: a
/// card without a title is better than one in mojibake.
fn decode(latin1: &str, charset: Option<&str>) -> String {
    let bytes = latin1.chars().map(|c| c as u32 as u8);
    match charset.map(str::to_ascii_lowercase).as_deref() {
        None | Some("utf-8" | "utf8") => String::from_utf8(bytes.collect()).unwrap_or_default(),
        Some("windows-1251" | "cp1251" | "cp-1251" | "x-cp1251") => {
            bytes.map(windows_1251).collect()
        }
        Some("koi8-r" | "koi8r" | "koi8-u") => bytes.map(koi8_r).collect(),
        Some("iso-8859-1" | "latin1" | "windows-1252" | "us-ascii" | "ascii") => latin1.to_owned(),
        Some(_) => String::new(),
    }
}

/// Windows-1251: old Russian sites still use it. The letters run in order, the rest is a table.
fn windows_1251(byte: u8) -> char {
    const HIGH: [char; 64] = [
        'Ђ', 'Ѓ', '‚', 'ѓ', '„', '…', '†', '‡', '€', '‰', 'Љ', '‹', 'Њ', 'Ќ', 'Ћ', 'Џ', //
        'ђ', '‘', '’', '“', '”', '•', '–', '—', ' ', '™', 'љ', '›', 'њ', 'ќ', 'ћ', 'џ', //
        '\u{A0}', 'Ў', 'ў', 'Ј', '¤', 'Ґ', '¦', '§', 'Ё', '©', 'Є', '«', '¬', '\u{AD}', '®',
        'Ї', //
        '°', '±', 'І', 'і', 'ґ', 'µ', '¶', '·', 'ё', '№', 'є', '»', 'ј', 'Ѕ', 'ѕ', 'ї',
    ];
    match byte {
        0..=0x7F => char::from(byte),
        0x80..=0xBF => HIGH[usize::from(byte - 0x80)],
        0xC0..=0xFF => char::from_u32(0x410 + u32::from(byte - 0xC0)).unwrap_or(' '),
    }
}

/// KOI8-R: the oldest Russian sites. Letters only; the table's drawing symbols become spaces.
fn koi8_r(byte: u8) -> char {
    const LETTERS: [char; 64] = [
        'ю', 'а', 'б', 'ц', 'д', 'е', 'ф', 'г', 'х', 'и', 'й', 'к', 'л', 'м', 'н', 'о', //
        'п', 'я', 'р', 'с', 'т', 'у', 'ж', 'в', 'ь', 'ы', 'з', 'ш', 'э', 'щ', 'ч', 'ъ', //
        'Ю', 'А', 'Б', 'Ц', 'Д', 'Е', 'Ф', 'Г', 'Х', 'И', 'Й', 'К', 'Л', 'М', 'Н', 'О', //
        'П', 'Я', 'Р', 'С', 'Т', 'У', 'Ж', 'В', 'Ь', 'Ы', 'З', 'Ш', 'Э', 'Щ', 'Ч', 'Ъ',
    ];
    match byte {
        0..=0x7F => char::from(byte),
        0xA3 => 'ё',
        0xB3 => 'Ё',
        0x80..=0xBF => ' ',
        0xC0..=0xFF => LETTERS[usize::from(byte - 0xC0)],
    }
}

/// Entities undone, whitespace collapsed.
fn clean(text: &str) -> String {
    let decoded = decode_entities(text);
    let mut clean = String::with_capacity(decoded.len());
    for word in decoded.split_whitespace() {
        if !clean.is_empty() {
            clean.push(' ');
        }
        clean.push_str(word);
    }
    clean.retain(|c| !c.is_control());
    clean
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest[1..].find(';').filter(|&end| end <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..=end];
        if let Some(c) = entity(name) {
            out.push(c);
        } else {
            out.push_str(&rest[..end + 2]);
        }
        rest = &rest[end + 2..];
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{A0}',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(page: &str) -> PageInfo {
        read_page(page.as_bytes(), None)
    }

    #[test]
    fn open_graph_comes_first_then_the_rest() {
        let page = r#"<!doctype html><html><head>
            <title>Запасной</title>
            <meta name="description" content="Простое описание">
            <meta name="twitter:title" content="Для твиттера">
            <meta property="og:title" content="Главный заголовок">
            <meta property="og:image" content="https://cdn.example/a.gif">
            <meta property="og:description" content="Описание карточки" />
            </head><body></body></html>"#;
        assert_eq!(
            read(page),
            PageInfo {
                title: "Главный заголовок".into(),
                description: "Описание карточки".into(),
                image: "https://cdn.example/a.gif".into(),
            }
        );
        assert_eq!(
            read("<title>Только он</title><meta name=description content=Слово>"),
            PageInfo {
                title: "Только он".into(),
                description: "Слово".into(),
                image: String::new(),
            }
        );
    }

    #[test]
    fn attributes_come_in_any_order_and_quoting() {
        let page = "<META CONTENT='Заголовок' PROPERTY='og:title'><meta content=\"a > b\" name=description><meta property=og:image content=/p.png>";
        let info = read(page);
        assert_eq!(info.title, "Заголовок");
        assert_eq!(info.description, "a > b", "a bracket inside quotes");
        assert_eq!(info.image, "/p.png");
    }

    #[test]
    fn entities_and_whitespace_are_undone() {
        assert_eq!(
            read("<title>\n  Кот &amp; пёс\t&#8212; &#x41;&quot;  &unknown; &amp\n</title>").title,
            "Кот & пёс — A\" &unknown; &amp"
        );
    }

    #[test]
    fn tags_inside_scripts_comments_and_the_body_do_not_count() {
        let page = r#"<head>
            <!-- <title>Из комментария</title> -->
            <script>document.write('<meta property="og:title" content="Из скрипта">')</script>
            <title>Настоящий</title>
            </head><body><meta property="og:image" content="/из-тела.png"></body>"#;
        let info = read(page);
        assert_eq!(info.title, "Настоящий");
        assert_eq!(info.image, "");
    }

    #[test]
    fn the_encoding_comes_from_the_server_or_the_page() {
        // «Привет» in Windows-1251.
        let hi = [0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2];
        let page = |head: &[u8]| [head, b"<title>", &hi, b"</title>"].concat();
        assert_eq!(
            read_page(&page(b""), Some("windows-1251")).title,
            "Привет",
            "the server's word"
        );
        assert_eq!(
            read_page(&page(b"<meta charset=\"cp1251\">"), None).title,
            "Привет"
        );
        assert_eq!(
            read_page(
                &page(b"<meta http-equiv=Content-Type content='text/html; charset=windows-1251'>"),
                None
            )
            .title,
            "Привет"
        );
        // «Привет» in KOI8-R.
        assert_eq!(
            read_page(b"<title>\xF0\xD2\xC9\xD7\xC5\xD4</title>", Some("koi8-r")).title,
            "Привет"
        );
        assert_eq!(
            read_page(&page(b""), None).title,
            "",
            "not UTF-8 and nothing said: no mojibake"
        );
        assert_eq!(
            read_page(&page(b""), Some("shift_jis")).title,
            "",
            "an encoding not handled"
        );
    }

    #[test]
    fn charsets_are_found_in_content_types() {
        assert_eq!(charset_of("text/html; charset=UTF-8"), Some("UTF-8"));
        assert_eq!(charset_of("text/html;charset=\"cp1251\""), Some("cp1251"));
        assert_eq!(charset_of("text/html"), None);
    }

    #[test]
    fn broken_pages_never_panic() {
        for page in [
            "",
            "<",
            "<<<<",
            "<meta",
            "<meta content=",
            "<meta content='",
            "<title>",
            "<title",
            "<!--",
            "<script>",
            "</",
            "<meta property=og:title content=\"&#xFFFFFFFF; &#; &;\">",
            "&&&&<title>&",
            "<щ щ=щ>",
        ] {
            let _ = read(page);
        }
        let _ = read_page(&[0xFF, b'<', 0xC0, b'>', 0xE2, 0x82], None);
    }
}
