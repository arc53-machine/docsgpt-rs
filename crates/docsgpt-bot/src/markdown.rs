//! Platform-neutral Markdown helpers: image extraction, fence repair, and
//! splitting long answers into messages without breaking code blocks.
//!
//! Each bot still renders Markdown into its platform's format; these helpers
//! work on the Markdown DocsGPT returns (or on any block-structured text).

use std::sync::LazyLock;

use regex::Regex;

use crate::util::truncate_chars;

static IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"!\[[^\]]*\]\(\s*<?([^)\s>]+)>?(?:\s+"[^"]*")?\s*\)"#).expect("valid regex"));

/// Remove Markdown images. Returns the text without them (lines left empty
/// are collapsed) and the `http(s)` image URLs in order, without duplicates.
pub fn strip_images(md: &str) -> (String, Vec<String>) {
    let mut urls = Vec::new();
    let replaced = IMAGE.replace_all(md, |caps: &regex::Captures| {
        let url = caps[1].to_string();
        if (url.starts_with("http://") || url.starts_with("https://")) && !urls.contains(&url) {
            urls.push(url);
        }
        String::new()
    });
    let mut out = String::with_capacity(replaced.len());
    let mut prev_blank = false;
    for line in replaced.lines() {
        let blank = line.trim().is_empty();
        if blank && prev_blank {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        prev_blank = blank;
    }
    (out.trim().to_string(), urls)
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    line.len() - t.len() <= 3 && (t.starts_with("```") || t.starts_with("~~~"))
}

/// Close an unterminated code fence, so a partial answer still renders.
pub fn close_open_fence(md: &str) -> String {
    if md.lines().filter(|l| is_fence(l)).count() % 2 == 1 {
        format!("{md}\n```")
    } else {
        md.to_string()
    }
}

/// Split Markdown into blocks at blank lines, keeping each fenced code block
/// (blank lines included) in one piece.
pub fn markdown_blocks(md: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in md.lines() {
        if is_fence(line) {
            if !in_fence && !cur.is_empty() {
                blocks.push(cur.join("\n"));
                cur.clear();
            }
            in_fence = !in_fence;
            cur.push(line);
            if !in_fence {
                blocks.push(cur.join("\n"));
                cur.clear();
            }
            continue;
        }
        if !in_fence && line.trim().is_empty() {
            if !cur.is_empty() {
                blocks.push(cur.join("\n"));
                cur.clear();
            }
            continue;
        }
        cur.push(line);
    }
    if !cur.is_empty() {
        blocks.push(cur.join("\n"));
    }
    blocks
}

/// Split Markdown into messages of at most `limit` characters, breaking at
/// block boundaries first and re-fencing code blocks that must be split.
pub fn split_markdown(md: &str, limit: usize) -> Vec<String> {
    pack_blocks(&markdown_blocks(md), limit)
}

/// Shorten Markdown to at most `limit` characters, ending with `…` when cut.
/// Cuts at a block boundary when it can and never leaves a code fence open.
pub fn clamp_markdown(md: &str, limit: usize) -> String {
    if md.chars().count() <= limit {
        return md.to_string();
    }
    let room = limit.saturating_sub(3).max(1);
    let first = split_markdown(md, room).into_iter().next().unwrap_or_default();
    format!("{first}\n\n…")
}

/// Join blocks into messages of at most `limit` characters (blocks separated
/// by a blank line), splitting blocks that are too long on their own.
pub fn pack_blocks(blocks: &[String], limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0;
    for block in blocks {
        let pieces = if block.chars().count() > limit {
            split_block(block, limit)
        } else {
            vec![block.clone()]
        };
        for piece in pieces {
            let len = piece.chars().count();
            let sep = if cur.is_empty() { 0 } else { 2 };
            if cur_len + sep + len > limit && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            if !cur.is_empty() {
                cur.push_str("\n\n");
                cur_len += 2;
            }
            cur.push_str(&piece);
            cur_len += len;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Split one block to fit `limit`. A fenced code block becomes several fenced
/// code blocks with the same opening line.
pub fn split_block(block: &str, limit: usize) -> Vec<String> {
    if block.lines().next().is_some_and(is_fence) {
        let first_nl = block.find('\n').unwrap_or(block.len());
        let fence = block[..first_nl].trim_start();
        let closing = if fence.starts_with('~') { "~~~" } else { "```" };
        let mut body = block[first_nl..].trim_start_matches('\n');
        if let Some(last) = body.lines().last()
            && is_fence(last)
        {
            body = body[..body.len() - last.len()].trim_end_matches('\n');
        }
        let overhead = fence.chars().count() + closing.len() + 2;
        return split_plain(body, limit.saturating_sub(overhead).max(16))
            .into_iter()
            .map(|c| format!("{fence}\n{c}\n{closing}"))
            .collect();
    }
    split_plain(block, limit)
}

/// Split text into pieces of at most `limit` characters, cutting at a newline,
/// else a space, else anywhere. Empty pieces are dropped.
pub fn split_plain(text: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(1);
    let mut chunks = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while !rest.is_empty() {
        if rest.len() <= limit {
            chunks.push(rest.iter().collect::<String>());
            break;
        }
        let window = &rest[..limit];
        let mut cut = window.iter().rposition(|c| *c == '\n');
        if cut.is_none_or(|c| c < limit / 4) {
            cut = window.iter().rposition(|c| *c == ' ').or(cut);
        }
        let cut = cut.filter(|c| *c > 0).unwrap_or(limit);
        chunks.push(rest[..cut].iter().collect::<String>().trim_end().to_string());
        let skip = if rest[cut] == '\n' || rest[cut] == ' ' {
            cut + 1
        } else {
            cut
        };
        rest.drain(..skip);
    }
    chunks.into_iter().filter(|c| !c.trim().is_empty()).collect()
}

/// Render table rows as aligned monospace text (cells capped at 40 characters),
/// with a separator under the first row.
pub fn render_table_monospace(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return String::new();
    }
    let mut widths = vec![0usize; cols];
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count().min(40));
        }
    }
    let mut out = String::new();
    for (ri, r) in rows.iter().enumerate() {
        let mut line = String::new();
        for (i, width) in widths.iter().enumerate() {
            let cell: String = r.get(i).map(String::as_str).unwrap_or("").chars().take(40).collect();
            let pad = width.saturating_sub(cell.chars().count());
            line.push_str(&cell);
            line.push_str(&" ".repeat(pad));
            if i + 1 < cols {
                line.push_str(" | ");
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
        if ri == 0 && rows.len() > 1 {
            let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
            out.push_str(&sep.join("-|-"));
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// A source title safe to put inside link text: brackets removed, whitespace
/// collapsed, at most 80 characters, `"Source"` when empty.
pub fn clean_title(t: &str) -> String {
    let t: String = t.chars().map(|c| if c == '[' || c == ']' { ' ' } else { c }).collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(if t.is_empty() { "Source" } else { &t }, 80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_images() {
        let (text, urls) = strip_images("Look:\n\n![cat](https://x.test/a.png)\n\nDone.");
        assert_eq!(text, "Look:\n\nDone.");
        assert_eq!(urls, vec!["https://x.test/a.png"]);
        let (t2, u2) = strip_images("![a](https://x/1.png \"cap\") and ![b](https://x/2.png) ![c](https://x/1.png)");
        assert_eq!(t2, "and");
        assert_eq!(u2.len(), 2);
        let (t3, u3) = strip_images("![local](data/x.png) kept");
        assert_eq!((t3.as_str(), u3.len()), ("kept", 0));
    }

    #[test]
    fn closes_fence() {
        assert_eq!(close_open_fence("```py\nx"), "```py\nx\n```");
        assert_eq!(close_open_fence("```py\nx\n```"), "```py\nx\n```");
        assert_eq!(close_open_fence("text with ``` inline"), "text with ``` inline");
    }

    #[test]
    fn blocks_keep_code_whole() {
        let md = "Intro\nline two\n\n```py\na = 1\n\nb = 2\n```\nafter\n\n\n- x\n- y";
        assert_eq!(
            markdown_blocks(md),
            vec!["Intro\nline two", "```py\na = 1\n\nb = 2\n```", "after", "- x\n- y"]
        );
    }

    #[test]
    fn packs_and_splits() {
        let blocks = vec!["a".repeat(10), "b".repeat(10), "c".repeat(30)];
        let msgs = pack_blocks(&blocks, 25);
        assert_eq!(msgs[0], format!("{}\n\n{}", "a".repeat(10), "b".repeat(10)));
        assert_eq!(msgs.len(), 3);
        assert!(msgs.iter().all(|m| m.chars().count() <= 25));

        let code = format!(
            "```py\n{}\n```",
            (0..50).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")
        );
        let pieces = split_block(&code, 120);
        assert!(pieces.len() > 1);
        for p in &pieces {
            assert!(p.starts_with("```py\n") && p.ends_with("\n```"), "{p}");
            assert!(p.chars().count() <= 120);
        }
        let joined: Vec<String> = pieces
            .iter()
            .flat_map(|p| p.lines().filter(|l| l.starts_with("line")).map(String::from))
            .collect();
        assert_eq!(joined.len(), 50);
    }

    #[test]
    fn split_plain_prefers_newlines() {
        assert_eq!(
            split_plain("one two three\nfour five six\nseven", 16),
            vec!["one two three", "four five six", "seven"]
        );
        assert_eq!(split_plain(&"x".repeat(50), 20).len(), 3);
        assert_eq!(split_plain("héllo wörld ✓✓", 6), vec!["héllo", "wörld", "✓✓"]);
    }

    #[test]
    fn split_markdown_never_breaks_a_fence() {
        let md = format!(
            "Intro paragraph.\n\n```\n{}\n```\n\nOutro.",
            (0..40).map(|i| format!("row {i}")).collect::<Vec<_>>().join("\n")
        );
        for msg in split_markdown(&md, 100) {
            assert!(msg.chars().count() <= 100, "{msg}");
            assert_eq!(msg.lines().filter(|l| is_fence(l)).count() % 2, 0, "unbalanced: {msg}");
        }
    }

    #[test]
    fn clamp_is_fence_safe() {
        let md = format!("{}\n\n{}", "a".repeat(10), "b".repeat(10));
        assert_eq!(clamp_markdown(&md, 18), format!("{}\n\n…", "a".repeat(10)));
        assert_eq!(clamp_markdown("short", 18), "short");
        // The Telegram bot cut this inside the fence.
        let code = format!("```\n{}\n```", "x\n".repeat(40));
        let c = clamp_markdown(&code, 50);
        assert!(c.chars().count() <= 50, "{}", c.chars().count());
        assert!(c.starts_with("```\n") && c.ends_with("```\n\n…"), "{c}");
    }

    #[test]
    fn table_and_titles() {
        let rows = vec![
            vec!["Cat".to_string(), "Trait".to_string()],
            vec!["Tom".into(), "Curious".into()],
        ];
        assert_eq!(
            render_table_monospace(&rows),
            "Cat | Trait\n----|--------\nTom | Curious"
        );
        assert_eq!(render_table_monospace(&[]), "");
        assert_eq!(clean_title(" Doc [1]\n v2 "), "Doc 1 v2");
        assert_eq!(clean_title(""), "Source");
        assert_eq!(clean_title(&"t".repeat(100)).chars().count(), 80);
    }
}
