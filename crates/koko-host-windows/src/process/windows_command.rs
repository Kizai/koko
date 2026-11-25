pub(super) fn append_argument(
    line: &mut Vec<u16>,
    argument: impl Iterator<Item = u16>,
) -> std::io::Result<()> {
    const QUOTE: u16 = b'"' as u16;
    const SLASH: u16 = b'\\' as u16;

    // CRT argument parsing treats backslashes specially only before a quote.
    line.push(QUOTE);
    let mut slashes = 0;
    for code in argument {
        match code {
            0 => return Err(std::io::ErrorKind::InvalidInput.into()),
            SLASH => slashes += 1,
            _ => {
                if code == QUOTE {
                    slashes = slashes * 2 + 1;
                }
                line.extend(std::iter::repeat_n(SLASH, slashes));
                line.push(code);
                slashes = 0;
            }
        }
    }
    line.extend(std::iter::repeat_n(SLASH, slashes * 2));
    line.push(QUOTE);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_empty_arguments_quotes_and_windows_paths() {
        let mut line = Vec::new();
        for arg in [
            r"C:\Program Files\KoKo\koko-daemon.exe",
            "",
            r#"A "name""#,
            r"C:\data\",
        ] {
            if !line.is_empty() {
                line.push(b' ' as u16);
            }
            append_argument(&mut line, arg.encode_utf16()).unwrap();
        }
        assert_eq!(
            String::from_utf16(&line).unwrap(),
            r#""C:\Program Files\KoKo\koko-daemon.exe" "" "A \"name\"" "C:\data\\""#,
        );
    }

    #[test]
    fn refuses_an_embedded_command_line_terminator() {
        let err = append_argument(&mut Vec::new(), "name\0other".encode_utf16()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
