//! Byte-offset <-> LSP `Position` conversion. LSP characters are UTF-16
//! code units, not bytes and not Unicode scalars, so both directions have
//! to walk the text rather than just adding/subtracting.

use tower_lsp::lsp_types::Position;

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    let mut offset = 0usize;
    for line in text.split('\n') {
        offset += line.len() + 1;
        starts.push(offset);
    }
    starts
}

pub fn offset_to_position(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let starts = line_starts(text);
    let line = starts.partition_point(|start| *start <= offset).saturating_sub(1);
    let line_start = starts[line];
    let character = text[line_start..offset].encode_utf16().count() as u32;
    Position { line: line as u32, character }
}

pub fn position_to_offset(text: &str, position: Position) -> usize {
    let starts = line_starts(text);
    // `line_starts` pushes one entry per real line plus a final phantom one
    // past the end of `text` (from the `+1` it always adds, even for a
    // trailing segment with no real newline after it) — every file ending
    // in `\n` has one, and editors report that as a real, empty last line,
    // so a cursor/hover there is a completely ordinary position, not a
    // client bug. Clamping both ends here is what keeps that in bounds.
    let Some(&line_start) = starts.get(position.line as usize) else { return text.len() };
    let line_start = line_start.min(text.len());
    let line_end = starts
        .get(position.line as usize + 1)
        .map(|&next| next.saturating_sub(1))
        .unwrap_or(text.len())
        .clamp(line_start, text.len());
    let line = &text[line_start..line_end];

    let mut units = 0u32;
    for (byte_index, unit_count) in line.char_indices().map(|(i, c)| (i, c.len_utf16() as u32)) {
        if units >= position.character {
            return line_start + byte_index;
        }
        units += unit_count;
    }
    line_start + line.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The regression this guards: `text.split('\n')` on a trailing-newline
    // file yields a final empty segment, so `line_starts` includes an entry
    // one past `text.len()` — and editors genuinely send positions on that
    // phantom last line (it's how a cursor sitting after a file's final
    // newline is represented), so this must not panic.
    #[test]
    fn position_on_trailing_phantom_line_does_not_panic() {
        let text = "a\nb\n";
        // Line 2 is the real (empty) last line VS Code shows after the
        // trailing newline; line 3 is one further still — e.g. a cursor or
        // hover parked at end-of-document, which editors represent as
        // `{line: lineCount, character: 0}`. Both used to panic (line 3
        // did, via a `line_start > line_end` slice); both must resolve to
        // end-of-text now.
        assert_eq!(position_to_offset(text, Position { line: 2, character: 0 }), text.len());
        assert_eq!(position_to_offset(text, Position { line: 3, character: 0 }), text.len());
        assert_eq!(position_to_offset(text, Position { line: 5, character: 3 }), text.len());
    }

    #[test]
    fn position_round_trips_through_offset() {
        let text = "abc\ndef\nghi";
        for offset in 0..=text.len() {
            if !text.is_char_boundary(offset) { continue }
            let position = offset_to_position(text, offset);
            assert_eq!(position_to_offset(text, position), offset);
        }
    }
}
