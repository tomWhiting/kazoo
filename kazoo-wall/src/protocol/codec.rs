//! Reading and writing protocol lines, with the [`MAX_LINE`] cap.

use std::io::{self, BufRead, Write};

use serde::Serialize;

use super::MAX_LINE;

/// What reading a line found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineRead {
    /// A whole line, without its newline (a last line with no newline at
    /// the end of the stream counts too).
    Line(Vec<u8>),
    /// The stream ended cleanly between lines.
    End,
    /// The line grew past the cap before its newline arrived. The rest of
    /// it has not been read: the connection should be closed.
    TooLong,
}

/// Read one line of at most `max` bytes (newline excluded).
///
/// # Errors
///
/// Fails on an I/O error from the reader. A read interrupted by a signal is
/// retried.
pub fn read_line_capped(reader: &mut impl BufRead, max: usize) -> io::Result<LineRead> {
    let mut line = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        if available.is_empty() {
            return Ok(if line.is_empty() {
                LineRead::End
            } else {
                LineRead::Line(line)
            });
        }
        let (taken, done) = available
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or((available.len(), false), |at| (at, true));
        if line.len() + taken > max {
            return Ok(LineRead::TooLong);
        }
        line.extend_from_slice(&available[..taken]);
        reader.consume(if done { taken + 1 } else { taken });
        if done {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(LineRead::Line(line));
        }
    }
}

/// Read one protocol line (at most [`MAX_LINE`] bytes).
///
/// # Errors
///
/// As [`read_line_capped`].
pub fn read_line(reader: &mut impl BufRead) -> io::Result<LineRead> {
    read_line_capped(reader, MAX_LINE)
}

/// Encode `value` as one line of JSON, newline included.
///
/// # Errors
///
/// Fails if `value` cannot be encoded, or encodes longer than
/// [`MAX_LINE`].
pub fn encode_line(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    if line.len() > MAX_LINE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "a line of {} bytes is over the {MAX_LINE}-byte cap",
                line.len()
            ),
        ));
    }
    line.push(b'\n');
    Ok(line)
}

/// Write `value` as one line of JSON and flush.
///
/// # Errors
///
/// As [`encode_line`], or an I/O error from the writer.
pub fn write_line(writer: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let line = encode_line(value)?;
    writer.write_all(&line)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};

    use super::*;

    fn lines(input: &[u8], max: usize, capacity: usize) -> Vec<LineRead> {
        let mut reader = BufReader::with_capacity(capacity, Cursor::new(input.to_vec()));
        let mut out = Vec::new();
        loop {
            let read = read_line_capped(&mut reader, max).unwrap();
            let stop = matches!(read, LineRead::End | LineRead::TooLong);
            out.push(read);
            if stop {
                return out;
            }
        }
    }

    #[test]
    fn lines_split_on_newlines_whatever_the_buffer() {
        for capacity in [1, 3, 64] {
            assert_eq!(
                lines(b"ab\ncd\r\n\nef", 10, capacity),
                vec![
                    LineRead::Line(b"ab".to_vec()),
                    LineRead::Line(b"cd".to_vec()),
                    LineRead::Line(Vec::new()),
                    LineRead::Line(b"ef".to_vec()),
                    LineRead::End,
                ]
            );
        }
    }

    #[test]
    fn a_line_at_the_cap_passes_and_one_over_does_not() {
        assert_eq!(
            lines(b"12345\n", 5, 2),
            vec![LineRead::Line(b"12345".to_vec()), LineRead::End]
        );
        assert_eq!(lines(b"123456\n", 5, 2), vec![LineRead::TooLong]);
        assert_eq!(lines(b"123456", 5, 64), vec![LineRead::TooLong]);
    }

    #[test]
    fn oversized_protocol_lines_are_caught() {
        let mut big = vec![b'x'; MAX_LINE + 1];
        big.push(b'\n');
        let mut reader = BufReader::new(Cursor::new(big));
        assert_eq!(read_line(&mut reader).unwrap(), LineRead::TooLong);
        let too_big = "y".repeat(MAX_LINE);
        assert!(encode_line(&too_big).is_err());
    }

    #[test]
    fn written_lines_end_in_one_newline() {
        let mut out = Vec::new();
        write_line(&mut out, &serde_json::json!({"id": 1})).unwrap();
        assert_eq!(out, b"{\"id\":1}\n");
    }
}
