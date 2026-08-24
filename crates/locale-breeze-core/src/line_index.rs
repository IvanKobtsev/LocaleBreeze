#[derive(Clone, Debug)]
pub struct LineIndex {
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { line_starts }
    }

    pub fn offset(&self, text: &str, line: u32, utf16_column: u32) -> Option<usize> {
        let start = *self.line_starts.get(line as usize)?;
        let end = self
            .line_starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(text.len());
        let mut units = 0u32;
        for (byte, ch) in text[start..end].char_indices() {
            if units == utf16_column {
                return Some(start + byte);
            }
            units += ch.len_utf16() as u32;
            if units > utf16_column {
                return None;
            }
        }
        (units == utf16_column).then_some(end)
    }

    pub fn position(&self, text: &str, offset: usize) -> Option<(u32, u32)> {
        if offset > text.len() || !text.is_char_boundary(offset) {
            return None;
        }
        let line = self
            .line_starts
            .partition_point(|&x| x <= offset)
            .saturating_sub(1);
        let start = self.line_starts[line];
        let column = text[start..offset].encode_utf16().count() as u32;
        Some((line as u32, column))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handles_utf16() {
        let text = "a😀b\nç";
        let index = LineIndex::new(text);
        assert_eq!(index.position(text, "a😀".len()), Some((0, 3)));
        assert_eq!(index.offset(text, 0, 3), Some("a😀".len()));
    }
}
