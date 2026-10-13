//! Turns a project's long description (Markdown mixed with HTML) into simple blocks
//! the UI can lay out: headings, paragraphs, list items, images and code.

/// One piece of a description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(String),
    Paragraph(String),
    Bullet(String),
    /// Image URL.
    Image(String),
    Code(String),
    Rule,
}

pub fn parse(body: &str) -> Vec<Block> {
    let text = flatten_html(body);
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut code: Option<Vec<&str>> = None;

    let flush = |paragraph: &mut Vec<String>, blocks: &mut Vec<Block>| {
        let joined = paragraph.join(" ");
        paragraph.clear();
        push_text(blocks, &joined, Block::Paragraph);
    };

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(lines) = &mut code {
            if trimmed.starts_with("```") {
                blocks.push(Block::Code(lines.join("\n")));
                code = None;
            } else {
                lines.push(line);
            }
            continue;
        }
        if trimmed.starts_with("```") {
            flush(&mut paragraph, &mut blocks);
            code = Some(Vec::new());
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
            continue;
        }
        if is_rule(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        if trimmed.starts_with('#') {
            flush(&mut paragraph, &mut blocks);
            push_text(&mut blocks, trimmed.trim_start_matches('#'), Block::Heading);
            continue;
        }
        if let Some(item) = bullet(trimmed) {
            flush(&mut paragraph, &mut blocks);
            push_text(&mut blocks, item, Block::Bullet);
            continue;
        }
        // Reference link targets: `[1]: https://...`.
        if trimmed.starts_with('[') && trimmed.contains("]: ") {
            continue;
        }
        // Setext headings ("Title" over "=====") and table separators.
        if trimmed.chars().all(|c| matches!(c, '=' | '-' | '|' | ':' | ' ')) {
            if trimmed.starts_with('=') && !paragraph.is_empty() {
                let title = paragraph.join(" ");
                paragraph.clear();
                push_text(&mut blocks, &title, Block::Heading);
            }
            continue;
        }
        let quote = trimmed.trim_start_matches('>').trim();
        let row = quote.trim_matches('|').replace(" | ", " · ");
        paragraph.push(row);
    }
    if let Some(lines) = code {
        blocks.push(Block::Code(lines.join("\n")));
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

/// Adds `text` as a `make` block, with any images in it pulled out as their own blocks.
fn push_text(blocks: &mut Vec<Block>, text: &str, make: fn(String) -> Block) {
    let (text, images) = inline(text);
    for url in images {
        if !url.is_empty() && !is_badge(&url) {
            blocks.push(Block::Image(url));
        }
    }
    if !text.is_empty() {
        blocks.push(make(text));
    }
}

/// Badges (shields.io and the like) are noise outside a web page.
fn is_badge(url: &str) -> bool {
    let url = url.to_lowercase();
    ["shields.io", "badge", "cf.way2muchnoise", "img.shields", ".svg"].iter().any(|b| url.contains(b))
}

fn is_rule(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3 && ["-", "*", "_"].iter().any(|c| compact.chars().all(|x| x.to_string() == *c))
}

fn bullet(line: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest.trim_start_matches("[ ] ").trim_start_matches("[x] "));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        return line[digits..].strip_prefix(". ").or_else(|| line[digits..].strip_prefix(") "));
    }
    None
}

/// Plain text of one Markdown line, plus the URLs of the images in it.
fn inline(text: &str) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut images = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // ![alt](url)
        if c == '!' && chars.get(i + 1) == Some(&'[') {
            if let Some((_, url, end)) = link_at(&chars, i + 1) {
                images.push(url);
                i = end;
                continue;
            }
        }
        // [text](url), where text may itself be an image
        if c == '[' {
            if let Some((label, _, end)) = link_at(&chars, i) {
                let (label, inner) = inline(&label);
                images.extend(inner);
                out.push_str(&label);
                i = end;
                continue;
            }
        }
        // Emphasis marks; underscores inside words (snake_case) stay.
        let word = |j: Option<&char>| j.is_some_and(|c| c.is_alphanumeric());
        if matches!(c, '*' | '`' | '~') || (c == '_' && !(word(i.checked_sub(1).and_then(|j| chars.get(j))) && word(chars.get(i + 1)))) {
            i += 1;
            continue;
        }
        if c == '\\' && i + 1 < chars.len() {
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    (out.split_whitespace().collect::<Vec<_>>().join(" "), images)
}

/// Reads `[label](url)` starting at `start` (the `[`): label, URL and the index after `)`.
fn link_at(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let mut depth = 0;
    let mut close = None;
    for (j, &c) in chars.iter().enumerate().skip(start) {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(j);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let label: String = chars[start + 1..close].iter().collect();
    // Reference links: `[label][1]`.
    if chars.get(close + 1) == Some(&'[') {
        let end = (close + 2..chars.len()).find(|&j| chars[j] == ']')?;
        return Some((label, String::new(), end + 1));
    }
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = (close + 2..chars.len()).find(|&j| chars[j] == ')')?;
    let target: String = chars[close + 2..end].iter().collect();
    // `(url "title")`
    let url = target.split_whitespace().next().unwrap_or_default().to_string();
    Some((label, url, end + 1))
}

/// Rewrites the HTML in a description as Markdown-ish lines and drops the remaining tags.
fn flatten_html(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        if let Some(comment) = after.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        // A lone `<` (as in "MC <1.16") is text, not a tag.
        if !after[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '/') {
            out.push('<');
            rest = &after[1..];
            continue;
        }
        let Some(close) = after.find('>') else {
            out.push_str(after);
            rest = "";
            break;
        };
        let tag = &after[1..close];
        rest = &after[close + 1..];
        let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
        let closing = tag.starts_with('/');
        match name.as_str() {
            "img" => {
                if let Some(src) = attribute(tag, "src") {
                    out.push_str(&format!("\n\n![]({src})\n\n"));
                }
            }
            "br" => out.push('\n'),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => out.push_str(if closing { "\n\n" } else { "\n\n## " }),
            "li" if !closing => out.push_str("\n- "),
            "p" | "div" | "center" | "ul" | "ol" | "li" | "table" | "tr" | "details" | "summary" | "blockquote" => out.push_str("\n\n"),
            "hr" => out.push_str("\n\n---\n\n"),
            "pre" => out.push_str("\n```\n"),
            "td" | "th" => out.push_str(" | "),
            _ => {}
        }
    }
    out.push_str(rest);
    decode_entities(&out)
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(name) {
        let at = from + pos;
        from = at + name.len();
        // Must be a whole attribute name, e.g. not `data-src`.
        if at > 0 && !lower.as_bytes()[at - 1].is_ascii_whitespace() {
            continue;
        }
        let value = tag[from..].trim_start().strip_prefix('=')?.trim_start();
        let quote = value.chars().next()?;
        return if quote == '"' || quote == '\'' {
            value[1..].split(quote).next().map(str::to_string)
        } else {
            value.split_whitespace().next().map(str::to_string)
        };
    }
    None
}

/// Decodes numeric (`&#233;`, `&#xE9;`) and common named (`&eacute;`, `&mdash;`) entities;
/// unknown ones are left as they are.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let decoded = rest[1..]
            .find(';')
            .filter(|&end| end <= 10)
            .and_then(|end| entity(&rest[1..=end]).map(|c| (c, end + 2)));
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The character for an entity name without `&` and `;`.
fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code);
    }
    let named = match name {
        "nbsp" => ' ',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "amp" => '&',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "times" => '×',
        "deg" => '°',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "euro" => '€',
        "pound" => '£',
        "szlig" => 'ß',
        "ccedil" => 'ç',
        "Ccedil" => 'Ç',
        "ntilde" => 'ñ',
        "Ntilde" => 'Ñ',
        _ => return accented(name),
    };
    Some(named)
}

/// `eacute`, `Ouml`, `agrave`, ... for the common vowels.
fn accented(name: &str) -> Option<char> {
    let mut chars = name.chars();
    let base = chars.next()?;
    let table = match chars.as_str() {
        "acute" => "aáeéiíoóuúyýAÁEÉIÍOÓUÚYÝ",
        "grave" => "aàeèiìoòuùAÀEÈIÌOÒUÙ",
        "circ" => "aâeêiîoôuûAÂEÊIÎOÔUÛ",
        "uml" => "aäeëiïoöuüyÿAÄEËIÏOÖUÜ",
        "tilde" => "aãoõAÃOÕ",
        _ => return None,
    };
    let mut pairs = table.chars();
    while let (Some(plain), Some(marked)) = (pairs.next(), pairs.next()) {
        if plain == base {
            return Some(marked);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_entities() {
        assert_eq!(decode_entities("Pok&eacute;mon &amp; &#233;&#xE9; &mdash; a&b &bogus; &"), "Pokémon & éé — a&b &bogus; &");
    }

    #[test]
    fn parses_markdown() {
        let blocks = parse(
            "# Sodium\n\nA **fast** rendering mod.\nSee [the wiki](https://x) for `help`.\n\n- one\n* two\n1. three\n\n---\n\n```\nlet x = 1;\n```\n![shot](https://cdn/x.png)",
        );
        assert_eq!(
            blocks,
            [
                Block::Heading("Sodium".into()),
                Block::Paragraph("A fast rendering mod. See the wiki for help.".into()),
                Block::Bullet("one".into()),
                Block::Bullet("two".into()),
                Block::Bullet("three".into()),
                Block::Rule,
                Block::Code("let x = 1;".into()),
                Block::Image("https://cdn/x.png".into()),
            ]
        );
    }

    #[test]
    fn flattens_html_and_drops_badges() {
        let blocks = parse(
            "<center><h2>Features</h2><img src=\"https://cdn/a.webp\" width=500><br>Fast &amp; light</center>\
             <!-- hidden --><ul><li>Item</li></ul>[![badge](https://img.shields.io/x.svg)](https://x)",
        );
        assert_eq!(
            blocks,
            [
                Block::Heading("Features".into()),
                Block::Image("https://cdn/a.webp".into()),
                Block::Paragraph("Fast & light".into()),
                Block::Bullet("Item".into()),
            ]
        );
    }

    #[test]
    fn keeps_text_that_looks_like_markup() {
        let blocks = parse("Lithium (for MC <1.16) in your _mods folder_, see [the wiki][1] and my_mod_id.\n\n[1]: https://x");
        assert_eq!(blocks, [Block::Paragraph("Lithium (for MC <1.16) in your mods folder, see the wiki and my_mod_id.".into())]);
    }
}
