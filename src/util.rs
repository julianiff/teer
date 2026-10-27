//! Small helpers shared across modules.

/// FNV-1a, 64 bit. Stable across Rust versions, unlike `DefaultHasher`,
/// which matters because hashes end up in file names and persisted reviews.
pub fn fnv64(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for part in parts {
        for b in *part {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        // Separator so ["ab", "c"] and ["a", "bc"] differ.
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn hash_hex(parts: &[&[u8]]) -> String {
    format!("{:016x}", fnv64(parts))
}

/// What a finished command printed: its status, stdout and stderr.
pub type Finished = (std::process::ExitStatus, Vec<u8>, Vec<u8>);

/// Run `cmd` with stdin closed and collect at most `cap` bytes of each
/// output stream, killing it once it runs longer than `limit`: `None` then.
pub fn output_within(
    cmd: &mut std::process::Command,
    limit: std::time::Duration,
    cap: u64,
) -> std::io::Result<Option<Finished>> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    fn drain(
        r: Option<impl Read + Send + 'static>,
        cap: u64,
    ) -> std::sync::mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(r) = r {
                let _ = r.take(cap).read_to_end(&mut buf);
            }
            let _ = tx.send(buf);
        });
        rx
    }
    let out_rx = drain(child.stdout.take(), cap);
    let err_rx = drain(child.stderr.take(), cap);
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait()? {
            // Helpers it started may keep the pipes open: don't wait for them forever.
            let grace = Duration::from_secs(5);
            let stdout = out_rx.recv_timeout(grace).unwrap_or_default();
            let stderr = err_rx.recv_timeout(grace).unwrap_or_default();
            return Ok(Some((status, stdout, stderr)));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffOp {
    Equal,
    Delete,
    Insert,
}

/// A line diff: (op, old line index, new line index). Indexes are 0-based
/// into the respective inputs; the unused side is the last valid position.
pub fn diff_lines<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<(DiffOp, usize, usize)> {
    let (n, m) = (old.len(), new.len());
    // Classic LCS table; units are small, and we cap the work for safety.
    if n * m > 4_000_000 {
        // A whole file can be large: match its unchanged start and end, and
        // diff only what lies between.
        let pre = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        let suf = old[pre..]
            .iter()
            .rev()
            .zip(new[pre..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        if pre + suf > 0 {
            let mut out: Vec<_> = (0..pre).map(|i| (DiffOp::Equal, i, i)).collect();
            let mid = diff_lines(&old[pre..n - suf], &new[pre..m - suf]);
            out.extend(mid.into_iter().map(|(op, i, j)| (op, i + pre, j + pre)));
            out.extend((0..suf).map(|k| (DiffOp::Equal, n - suf + k, m - suf + k)));
            return out;
        }
        if let Some(out) = anchored(old, new) {
            return out;
        }
        let mut out: Vec<_> = (0..n).map(|i| (DiffOp::Delete, i, 0)).collect();
        out.extend((0..m).map(|j| (DiffOp::Insert, n, j)));
        return out;
    }
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(n + m);
    while i < n || j < m {
        if i < n && j < m && old[i] == new[j] {
            out.push((DiffOp::Equal, i, j));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out.push((DiffOp::Delete, i, j));
            i += 1;
        } else {
            out.push((DiffOp::Insert, i, j));
            j += 1;
        }
    }
    out
}

/// Replace control characters (C0, C1, DEL, line and paragraph separators)
/// with a space, so untrusted text can't break a one-line row or move the
/// cursor when drawn.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() || c == '\u{2028}' || c == '\u{2029}' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// Clean text from an agent before it is stored or drawn: strip ANSI escape
/// sequences and control characters (keeping newlines and tabs), and cap
/// the length at 4000 characters.
pub fn sanitize_agent_text(s: &str) -> String {
    const MAX: usize = 4000;
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    let mut n = 0;
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                // CSI: parameters up to a final byte in @..~
                Some('[') => {
                    chars.next();
                    for d in chars.by_ref() {
                        if ('@'..='~').contains(&d) {
                            break;
                        }
                    }
                }
                // OSC / DCS / etc: up to BEL or ST (ESC \)
                Some(']' | 'P' | '_' | '^' | 'X') => {
                    chars.next();
                    while let Some(d) = chars.next() {
                        if d == '\u{7}' {
                            break;
                        }
                        if d == '\u{1b}' {
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
            continue;
        }
        let c = match c {
            '\r' => '\n',
            '\n' | '\t' => c,
            '\u{2028}' | '\u{2029}' => '\n',
            c if c.is_control() => continue,
            c => c,
        };
        if n == MAX {
            out.push('…');
            break;
        }
        out.push(c);
        n += 1;
    }
    out.trim().to_string()
}

/// Keep at most 5 plain-text sources: http(s) URLs or short citations
/// (no control characters, at most 300 characters).
pub fn clean_sources<'a>(items: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out = Vec::new();
    for raw in items {
        let t = sanitize_agent_text(raw);
        let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() {
            continue;
        }
        let lower = t.to_ascii_lowercase();
        let is_url = lower.starts_with("http://") || lower.starts_with("https://");
        if is_url && t.contains(' ') {
            continue;
        }
        if !is_url && (t.chars().count() > 120 || lower.contains("://")) {
            continue;
        }
        out.push(t.chars().take(300).collect());
        if out.len() == 5 {
            break;
        }
    }
    out
}

/// Truncate to `max` display columns, adding an ellipsis when cut. Control
/// characters become spaces; `max == 0` gives an empty string.
pub fn truncate(s: &str, max: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if max == 0 {
        return String::new();
    }
    let s = sanitize(s);
    let mut w = 0;
    let mut out = String::new();
    let total: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= max {
        return s;
    }
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        w += cw;
        out.push(c);
    }
    out.push('…');
    out
}

/// Truncate to exactly fit `w` display columns, then pad with spaces to `w`.
pub fn truncate_pad(s: &str, w: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    let mut out = truncate(s, w);
    let ow = out.width();
    out.push_str(&" ".repeat(w.saturating_sub(ow)));
    out
}

/// Word-wrap `text` to `width` display columns, keeping paragraph breaks.
/// Words wider than `width` are split. Control characters become spaces.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if width == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for para in text.split('\n') {
        let para = sanitize(para);
        let mut line = String::new();
        let mut lw = 0;
        for word in para.split_whitespace() {
            let mut word = word.to_string();
            let mut ww = word.width();
            if lw > 0 && lw + 1 + ww > width {
                out.push(std::mem::take(&mut line));
                lw = 0;
            }
            while ww > width {
                // Hard-split a word that can't fit on a line of its own.
                if lw > 0 {
                    out.push(std::mem::take(&mut line));
                    lw = 0;
                }
                let mut head = String::new();
                let mut hw = 0;
                let mut cut = word.len();
                for (i, c) in word.char_indices() {
                    let cw = c.width().unwrap_or(0);
                    if hw + cw > width {
                        cut = i;
                        break;
                    }
                    hw += cw;
                    head.push(c);
                }
                if head.is_empty() {
                    // A single glyph wider than the line: drop it.
                    let n = word.chars().next().map_or(0, char::len_utf8);
                    word.drain(..n);
                } else {
                    out.push(head);
                    word.drain(..cut);
                }
                ww = word.width();
            }
            if ww == 0 {
                continue;
            }
            if lw > 0 {
                line.push(' ');
                lw += 1;
            }
            line.push_str(&word);
            lw += ww;
        }
        out.push(line);
    }
    out
}

/// [`diff_lines`] for inputs too large for one table: lines that occur once
/// on each side, kept in order (the longest chain), pin the two together,
/// and only what lies between two of them is diffed. `None` without any.
fn anchored<'a>(old: &[&'a str], new: &[&'a str]) -> Option<Vec<(DiffOp, usize, usize)>> {
    // Per line: (times in old, where in old, times in new, where in new).
    let mut seen: std::collections::HashMap<&str, (usize, usize, usize, usize)> =
        std::collections::HashMap::new();
    for (i, l) in old.iter().enumerate() {
        let e = seen.entry(l).or_default();
        e.0 += 1;
        e.1 = i;
    }
    for (j, l) in new.iter().enumerate() {
        let e = seen.entry(l).or_default();
        e.2 += 1;
        e.3 = j;
    }
    let mut pairs: Vec<(usize, usize)> = seen
        .values()
        .filter(|e| e.0 == 1 && e.2 == 1)
        .map(|e| (e.1, e.3))
        .collect();
    pairs.sort_unstable();
    let chain = increasing(&pairs);
    if chain.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(old.len().max(new.len()));
    let (mut i0, mut j0) = (0, 0);
    for (i, j) in chain.into_iter().chain([(old.len(), new.len())]) {
        let gap = diff_lines(&old[i0..i], &new[j0..j]);
        out.extend(gap.into_iter().map(|(op, a, b)| (op, a + i0, b + j0)));
        if i < old.len() {
            out.push((DiffOp::Equal, i, j));
        }
        (i0, j0) = (i + 1, j + 1);
    }
    Some(out)
}

/// The longest run of `pairs` (sorted by their first) whose second also
/// rises.
fn increasing(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    // `tails[k]`: index of the pair ending the best run of length k + 1.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; pairs.len()];
    for (k, &(_, j)) in pairs.iter().enumerate() {
        let at = tails.partition_point(|&t| pairs[t].1 < j);
        if at > 0 {
            prev[k] = tails[at - 1];
        }
        if at == tails.len() {
            tails.push(k);
        } else {
            tails[at] = k;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut k = tails.last().copied().unwrap_or(usize::MAX);
    while k != usize::MAX {
        out.push(pairs[k]);
        k = prev[k];
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_large_file_diffs_only_its_changed_middle() {
        use super::{DiffOp, diff_lines};
        let old: Vec<String> = (0..3000).map(|i| format!("line {i}")).collect();
        let mut new = old.clone();
        new[1500] = "changed".into();
        let old: Vec<&str> = old.iter().map(String::as_str).collect();
        let new: Vec<&str> = new.iter().map(String::as_str).collect();
        let ops = diff_lines(&old, &new);
        let changed: Vec<_> = ops.iter().filter(|o| o.0 != DiffOp::Equal).collect();
        assert_eq!(
            changed,
            [&(DiffOp::Delete, 1500, 1500), &(DiffOp::Insert, 1501, 1500)]
        );
        assert_eq!(ops.len(), 3001);
    }

    #[test]
    fn agent_text_strips_escapes_and_controls() {
        use super::sanitize_agent_text as f;
        assert_eq!(f("\u{1b}[31mred\u{1b}[0m ok\u{7}\u{0}"), "red ok");
        assert_eq!(f("a\u{1b}]0;title\u{7}b"), "ab");
        assert_eq!(f("a\r\nb\tc"), "a\n\nb\tc");
        assert_eq!(f("trailing \u{1b}"), "trailing");
        let long = "x".repeat(5000);
        assert_eq!(f(&long).chars().count(), 4001);
    }

    #[test]
    fn sources_are_filtered_and_capped() {
        use super::clean_sources as f;
        let many: Vec<String> = (0..9).map(|i| format!("https://e.com/{i}")).collect();
        assert_eq!(f(many.iter().map(|s| s.as_str())).len(), 5);
        let got = f([
            "https://ok.dev/x",
            "javascript:alert(1)\u{1b}[2J",
            "RFC 9110 section 9.2",
            "ftp://nope",
            "https://has space.com",
            "",
        ]);
        assert_eq!(
            got,
            [
                "https://ok.dev/x",
                "javascript:alert(1)",
                "RFC 9110 section 9.2"
            ]
        );
        let long = format!("https://e.com/{}", "a".repeat(400));
        assert_eq!(f([long.as_str()])[0].chars().count(), 300);
    }

    use super::*;

    #[test]
    fn a_large_file_with_scattered_edits_marks_only_them() {
        // Too large for one table, with nothing to trim at either end.
        let old: Vec<String> = (0..3000).map(|i| format!("line {i}")).collect();
        let mut new = old.clone();
        new[0] = "first, edited".into();
        new[1500] = "middle, edited".into();
        new.insert(2200, "added".into());
        *new.last_mut().unwrap() = "last, edited".into();
        let (o, n): (Vec<&str>, Vec<&str>) = (
            old.iter().map(String::as_str).collect(),
            new.iter().map(String::as_str).collect(),
        );
        let ops = diff_lines(&o, &n);
        let inserted: Vec<usize> = ops
            .iter()
            .filter(|(op, _, _)| *op == DiffOp::Insert)
            .map(|&(_, _, j)| j)
            .collect();
        assert_eq!(inserted, [0, 1500, 2200, 3000]);
        assert_eq!(
            ops.iter().filter(|(op, ..)| *op == DiffOp::Delete).count(),
            3
        );
        // Every line of both sides is accounted for once.
        assert_eq!(
            ops.iter().filter(|(op, ..)| *op != DiffOp::Insert).count(),
            o.len()
        );
        assert_eq!(
            ops.iter().filter(|(op, ..)| *op != DiffOp::Delete).count(),
            n.len()
        );
    }

    #[test]
    fn diff_basic() {
        let ops: Vec<_> = diff_lines(&["a", "b", "c"], &["a", "x", "c"])
            .into_iter()
            .map(|o| o.0)
            .collect();
        assert_eq!(
            ops,
            vec![DiffOp::Equal, DiffOp::Delete, DiffOp::Insert, DiffOp::Equal]
        );
    }

    #[test]
    fn truncate_pad_is_display_width_aware() {
        use unicode_width::UnicodeWidthStr;
        assert_eq!(truncate_pad("ab", 4), "ab  ");
        assert_eq!(truncate_pad("abcdef", 4), "abc…");
        assert_eq!(truncate_pad("日本語", 5).width(), 5);
        assert_eq!(truncate("日本語", 5), "日本…");
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(hash_hex(&[b"teer"]), hash_hex(&[b"teer"]));
        assert_ne!(hash_hex(&[b"ab", b"c"]), hash_hex(&[b"a", b"bc"]));
    }

    #[test]
    fn truncate_and_pad_never_exceed_width_0_to_20() {
        use unicode_width::UnicodeWidthStr;
        assert_eq!(truncate("abc", 0), "");
        for w in 0..=20 {
            for s in [
                "",
                "abc",
                "a long ascii string",
                "日本語のテキスト",
                "a\nb\tc\0d",
            ] {
                let t = truncate(s, w);
                assert!(t.width() <= w, "{s:?} at {w}: {t:?}");
                assert_eq!(truncate_pad(s, w).width(), w, "{s:?} at {w}");
            }
        }
    }

    #[test]
    fn control_characters_become_spaces() {
        assert_eq!(truncate("a\nb\r\tc\0d\u{85}e", 20), "a b  c d e");
        assert_eq!(wrap("a\0b\u{9b}c", 20), vec!["a b c"]);
    }

    #[test]
    fn wrap_uses_display_width_and_respects_the_given_width() {
        use unicode_width::UnicodeWidthStr;
        let text = "日本語のテキストを折り返す 🙂🙂🙂🙂🙂🙂 mixed ascii words and 絵文字 here";
        for w in [1usize, 4, 10, 44, 46] {
            for l in wrap(text, w) {
                assert!(l.width() <= w, "{l:?} wider than {w}");
            }
        }
        assert!(wrap("x", 0).is_empty());
        let lines = wrap("one two three", 7);
        assert_eq!(lines, vec!["one two", "three"]);
        assert_eq!(wrap("a\n\nb", 5), vec!["a", "", "b"]);
    }
}
