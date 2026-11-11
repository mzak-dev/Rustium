//! JASC-PAL palette files: a text list of RGB triples.

use std::fmt::Write;

use crate::file_error::FileError;

/// Most colors a `.pal` file may hold.
pub const MAX_COLORS: usize = 256;

/// Reads the colors of a JASC-PAL file.
///
/// # Errors
/// Fails when the header is missing, a line is not three numbers `0..=255`, or fewer colors follow than the file
/// announces.
pub fn parse_pal(text: &str) -> Result<Vec<[u8; 3]>, FileError> {
    let fail = |reason: &str| Err(FileError::Palette(reason.to_owned()));
    // A leading byte order mark is common in files written by Windows tools.
    let mut lines = text.trim_start_matches('\u{feff}').lines().map(str::trim);
    if lines.next() != Some("JASC-PAL") {
        return fail("the first line must be JASC-PAL");
    }
    if lines.next() != Some("0100") {
        return fail("the second line must be 0100");
    }
    let Some(count) = lines.next().and_then(|line| line.parse::<usize>().ok()) else {
        return fail("the third line must be the number of colors");
    };
    if count == 0 || count > MAX_COLORS {
        return fail("the number of colors must be between 1 and 256");
    }
    let mut colors = Vec::with_capacity(count);
    for line in lines.filter(|line| !line.is_empty()).take(count) {
        let channels: Vec<u8> = line
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| FileError::Palette(format!("\"{line}\" is not a color")))?;
        let Ok(rgb) = <[u8; 3]>::try_from(channels.as_slice()) else {
            return fail("every color needs exactly three numbers");
        };
        colors.push(rgb);
    }
    if colors.len() != count {
        return fail("the file has fewer colors than it announces");
    }
    Ok(colors)
}

/// Writes colors as a JASC-PAL file (at most [`MAX_COLORS`]; extra colors are dropped).
pub fn write_pal(colors: &[[u8; 3]]) -> String {
    let colors = colors.get(..MAX_COLORS).unwrap_or(colors);
    let mut text = format!("JASC-PAL\r\n0100\r\n{}\r\n", colors.len());
    for [r, g, b] in colors {
        let _ = write!(text, "{r} {g} {b}\r\n");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_round_trip() {
        let colors = vec![[0, 0, 0], [255, 128, 7], [1, 2, 3]];
        assert_eq!(parse_pal(&write_pal(&colors)).unwrap(), colors);
    }

    #[test]
    fn a_file_with_unix_newlines_and_blank_lines_is_read() {
        let colors = parse_pal("JASC-PAL\n0100\n2\n10 20 30\n\n40 50 60\n").unwrap();
        assert_eq!(colors, vec![[10, 20, 30], [40, 50, 60]]);
    }

    #[test]
    fn broken_files_are_rejected() {
        for text in [
            "",
            "PAL\n0100\n1\n1 2 3",
            "JASC-PAL\n0100\n3\n1 2 3",
            "JASC-PAL\n0100\n1\n1 2 300",
            "JASC-PAL\n0100\n1\n1 2",
            "JASC-PAL\n0100\n0\n",
            "JASC-PAL\n0100\n999\n1 2 3",
        ] {
            assert!(parse_pal(text).is_err(), "{text:?}");
        }
    }
}
