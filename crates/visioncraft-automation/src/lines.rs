//! Reading request lines with a size cap, and the serve loop both protocols share.
//!
//! Ported from 3dcraft `crates/automation/src/lines.rs` (MIT).

use crate::MAX_REQUEST_BYTES;
use std::io::{self, BufRead, ErrorKind, Write};

/// One line read by [`read_line`].
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// The input ended before any byte of a new line.
    End,
    /// A line without its newline (and without a trailing `\r`).
    Bytes(Vec<u8>),
    /// A line longer than the cap; it was read up to its newline and dropped.
    TooLong,
}

/// Reads one line of at most `cap` bytes. A longer line is consumed and reported as
/// [`Line::TooLong`], so the next line is still read.
pub fn read_line(input: &mut impl BufRead, cap: usize) -> io::Result<Line> {
    let mut line = Vec::new();
    let (mut too_long, mut read_any) = (false, false);
    loop {
        let buffer = match input.fill_buf() {
            Ok(buffer) => buffer,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buffer.is_empty() {
            if !read_any {
                return Ok(Line::End);
            }
            break;
        }
        read_any = true;
        let newline = buffer.iter().position(|&byte| byte == b'\n');
        let chunk = &buffer[..newline.unwrap_or(buffer.len())];
        if !too_long {
            if line.len() + chunk.len() > cap {
                too_long = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(chunk);
            }
        }
        let used = newline.map_or(buffer.len(), |at| at + 1);
        input.consume(used);
        if newline.is_some() {
            break;
        }
    }
    if too_long {
        return Ok(Line::TooLong);
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(Line::Bytes(line))
}

pub(crate) fn too_long_message() -> String {
    format!(
        "the request line is longer than the limit of {MAX_REQUEST_BYTES} bytes; send smaller requests"
    )
}

/// Reads lines from `input` until it ends; each one goes to `handle` (`Err` for a line that is
/// too long or not UTF-8, with the message), and each reply is written and flushed.
pub(crate) fn serve(
    mut input: impl BufRead,
    mut output: impl Write,
    mut handle: impl FnMut(Result<&str, String>) -> Option<String>,
) -> io::Result<()> {
    loop {
        let reply = match read_line(&mut input, MAX_REQUEST_BYTES)? {
            Line::End => return Ok(()),
            Line::TooLong => handle(Err(too_long_message())),
            Line::Bytes(bytes) => match std::str::from_utf8(&bytes) {
                Ok(line) => handle(Ok(line)),
                Err(_) => handle(Err("the request line is not valid UTF-8".into())),
            },
        };
        if let Some(reply) = reply {
            output.write_all(reply.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    fn lines(input: &[u8], cap: usize, buffer: usize) -> Vec<Line> {
        let mut reader = BufReader::with_capacity(buffer, Cursor::new(input.to_vec()));
        let mut out = Vec::new();
        loop {
            match read_line(&mut reader, cap).unwrap() {
                Line::End => return out,
                line => out.push(line),
            }
        }
    }

    fn text(s: &str) -> Line {
        Line::Bytes(s.as_bytes().to_vec())
    }

    #[test]
    fn splits_lines_and_drops_over_long_ones() {
        for buffer in [1, 3, 64] {
            assert_eq!(
                lines(b"one\ntwo\r\n\nlast", 100, buffer),
                vec![text("one"), text("two"), text(""), text("last")]
            );
            assert_eq!(
                lines(b"12345\n123456\nok\n", 5, buffer),
                vec![text("12345"), Line::TooLong, text("ok")]
            );
        }
    }
}
