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
    let Some(&line_start) = starts.get(position.line as usize) else { return text.len() };
    let line_end = starts.get(position.line as usize + 1).map(|&e| e - 1).unwrap_or(text.len());
    let line = &text[line_start..line_end.min(text.len())];

    let mut units = 0u32;
    for (byte_index, unit_count) in line.char_indices().map(|(i, c)| (i, c.len_utf16() as u32)) {
        if units >= position.character {
            return line_start + byte_index;
        }
        units += unit_count;
    }
    line_start + line.len()
}
