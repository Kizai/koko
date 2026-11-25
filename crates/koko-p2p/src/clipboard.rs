//! Streams clipboard files and folders over one QUIC stream, at network speed and any size.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use koko_proto::{
    CLIPBOARD_ERROR, CLIPBOARD_MANIFEST, ClipboardEntry, ClipboardManifest, MAX_REMOTE_REPLY,
    RemoteCommand, RemoteReply,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::remote::RemoteClient;

const BUFFER: usize = 256 * 1024;

/// Lists files and folders to send, parent first, and the source path of every file entry in
/// the same order. Symbolic links and special files inside folders are left out; a selected
/// link or special file is refused. Top-level names that repeat get a numbered suffix.
pub fn collect(paths: &[PathBuf]) -> io::Result<(Vec<ClipboardEntry>, Vec<PathBuf>)> {
    let mut entries = Vec::new();
    let mut sources = Vec::new();
    let mut names = BTreeSet::new();
    for path in paths {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_file() && !metadata.is_dir() {
            return Err(unsupported("only regular files and folders can be copied"));
        }
        let base = utf8_name(path)?;
        let mut name = base.clone();
        let mut copy = 2;
        while !names.insert(name.clone()) {
            name = numbered(&base, copy);
            copy += 1;
        }
        walk(path, &name, &metadata, &mut entries, &mut sources)?;
    }
    Ok((entries, sources))
}

fn walk(
    path: &Path,
    name: &str,
    metadata: &std::fs::Metadata,
    entries: &mut Vec<ClipboardEntry>,
    sources: &mut Vec<PathBuf>,
) -> io::Result<()> {
    if metadata.is_file() {
        // Opening now surfaces a missing permission before any bytes are sent, so the person
        // sees the reason instead of a broken transfer.
        std::fs::File::open(path)?;
        entries.push(ClipboardEntry {
            path: name.to_owned(),
            size: metadata.len(),
            dir: false,
        });
        sources.push(path.to_owned());
        return Ok(());
    }
    entries.push(ClipboardEntry {
        path: name.to_owned(),
        size: 0,
        dir: true,
    });
    let mut children: Vec<_> = std::fs::read_dir(path)?.collect::<Result<_, _>>()?;
    children.sort_by_key(|child| child.file_name());
    for child in children {
        let metadata = std::fs::symlink_metadata(child.path())?;
        if !metadata.is_file() && !metadata.is_dir() {
            continue;
        }
        let child_name = format!("{name}/{}", utf8_name(&child.path())?);
        walk(&child.path(), &child_name, &metadata, entries, sources)?;
    }
    Ok(())
}

fn utf8_name(path: &Path) -> io::Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| unsupported("file names must be valid UTF-8"))
}

/// `Report.pdf` becomes `Report 2.pdf`; a folder or extensionless name gets ` 2` at the end.
fn numbered(name: &str, copy: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!("{stem} {copy}.{extension}"),
        _ => format!("{name} {copy}"),
    }
}

fn unsupported(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, message)
}

/// Writes every file's bytes in entry order. A file whose size changed since it was listed fails
/// the transfer rather than sending a truncated or padded copy.
pub async fn send_files(
    entries: &[ClipboardEntry],
    sources: &[PathBuf],
    output: &mut (impl AsyncWrite + Unpin),
    mut progress: impl FnMut(u64),
) -> io::Result<()> {
    let files = entries.iter().filter(|entry| !entry.dir);
    let mut sent = 0;
    let mut buffer = vec![0; BUFFER];
    for (entry, source) in files.zip(sources) {
        let mut file = tokio::fs::File::open(source).await?.take(entry.size);
        let mut remaining = entry.size;
        while remaining > 0 {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                return Err(changed());
            }
            output.write_all(&buffer[..read]).await?;
            remaining -= read as u64;
            sent += read as u64;
            progress(sent);
        }
        if file.into_inner().read(&mut buffer[..1]).await? != 0 {
            return Err(changed());
        }
    }
    output.flush().await
}

fn changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "a file changed while it was being copied",
    )
}

/// Validates all paths before recreating entries under `folder`, which must be new and empty.
/// Reads exactly the listed bytes, requires the end of the stream, and returns the top-level items.
pub async fn receive_files(
    entries: &[ClipboardEntry],
    input: &mut (impl AsyncRead + Unpin),
    folder: &Path,
    mut progress: impl FnMut(u64),
) -> io::Result<Vec<PathBuf>> {
    let paths = entries
        .iter()
        .map(|entry| receive_path(folder, &entry.path, cfg!(windows)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut top = Vec::new();
    let mut received = 0;
    let mut buffer = vec![0; BUFFER];
    for (entry, path) in entries.iter().zip(paths) {
        let first = folder.join(entry.path.split('/').next().unwrap_or_default());
        if !top.contains(&first) {
            top.push(first);
        }
        if entry.dir {
            tokio::fs::create_dir_all(&path).await?;
            continue;
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await?;
        let mut remaining = entry.size;
        while remaining > 0 {
            let want = buffer
                .len()
                .min(usize::try_from(remaining).unwrap_or(usize::MAX));
            let read = input.read(&mut buffer[..want]).await?;
            if read == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            file.write_all(&buffer[..read]).await?;
            remaining -= read as u64;
            received += read as u64;
            progress(received);
        }
        file.flush().await?;
    }
    if input.read(&mut buffer[..1]).await? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "more data than the file list describes",
        ));
    }
    Ok(top)
}

#[derive(Debug)]
struct UnsafeClipboardPath;

impl std::fmt::Display for UnsafeClipboardPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("unsafe clipboard path")
    }
}

impl std::error::Error for UnsafeClipboardPath {}

fn receive_path(
    folder: &Path,
    relative: &str,
    windows: bool,
) -> Result<PathBuf, UnsafeClipboardPath> {
    if !koko_proto::safe_relative_path(relative) {
        return Err(UnsafeClipboardPath);
    }
    let mut path = folder.to_owned();
    for part in relative.split('/') {
        if windows && !safe_windows_name(part) {
            return Err(UnsafeClipboardPath);
        }
        path.push(part);
    }
    // The caller provides a newly created, empty directory. Check the joined path
    // before using it for any filesystem operation.
    if !path.starts_with(folder) {
        return Err(UnsafeClipboardPath);
    }
    Ok(path)
}

fn safe_windows_name(name: &str) -> bool {
    if name.ends_with(['.', ' '])
        || name.chars().any(|ch| {
            ch.is_control() || matches!(ch, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
    {
        return false;
    }
    // Windows matches device names before the first dot, ignoring spaces before it.
    let stem = name
        .split_once('.')
        .map_or(name, |(stem, _)| stem)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return false;
    }
    // Windows also reserves COM and LPT device names with these superscript digits.
    !matches!(
        stem.strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT")),
        Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
    )
}

/// The error code a person sees for a failed transfer.
pub fn error_code(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::PermissionDenied => "clipboard_file_permission",
        io::ErrorKind::Unsupported => "clipboard_unsupported",
        io::ErrorKind::InvalidData
            if error
                .get_ref()
                .is_some_and(|cause| cause.is::<UnsafeClipboardPath>()) =>
        {
            "clipboard_invalid"
        }
        _ => "clipboard_failed",
    }
}

/// Writes one framed record: big-endian length of kind plus payload, the kind, the payload.
pub async fn write_record(
    output: &mut (impl AsyncWrite + Unpin),
    kind: u8,
    payload: &[u8],
) -> io::Result<()> {
    let length = u32::try_from(payload.len() + 1)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "record too large"))?;
    output.write_all(&length.to_be_bytes()).await?;
    output.write_all(&[kind]).await?;
    output.write_all(payload).await
}

/// Reads one framed record of at most `limit` bytes.
pub async fn read_record(
    input: &mut (impl AsyncRead + Unpin),
    limit: usize,
) -> io::Result<(u8, Vec<u8>)> {
    let mut length = [0; 4];
    input.read_exact(&mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    if !(1..=limit).contains(&length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad record length",
        ));
    }
    let mut record = vec![0; length];
    input.read_exact(&mut record).await?;
    let kind = record.remove(0);
    Ok((kind, record))
}

/// What a pull delivered: text, or the top-level files and folders written under the folder.
pub struct Received {
    pub text: Option<String>,
    pub paths: Vec<PathBuf>,
}

/// Receives a `clipboard_pull` or `outbox_pull` stream into `folder`, which must be new and
/// empty. Errors are the codes people see, such as `clipboard_file_permission`, or the
/// connection's own message.
pub async fn pull(
    client: &RemoteClient,
    command: &RemoteCommand,
    folder: &Path,
    mut progress: impl FnMut(u64, u64),
) -> Result<Received, String> {
    let (mut send, mut recv) = client
        .open_stream(command)
        .await
        .map_err(|error| format!("{error:?}"))?;
    send.finish().map_err(|error| format!("{error:?}"))?;
    let (kind, payload) = read_record(&mut recv, MAX_REMOTE_REPLY)
        .await
        .map_err(|error| format!("{error:?}"))?;
    if kind == CLIPBOARD_ERROR {
        return Err(String::from_utf8_lossy(&payload).into_owned());
    }
    let manifest = (kind == CLIPBOARD_MANIFEST)
        .then(|| serde_json::from_slice::<ClipboardManifest>(&payload).ok())
        .flatten()
        .filter(ClipboardManifest::valid)
        .ok_or("clipboard_invalid")?;
    let total = manifest.total_size();
    progress(0, total);
    let paths = receive_files(&manifest.entries, &mut recv, folder, |done| {
        progress(done, total)
    })
    .await
    .map_err(|error| error_code(&error).to_owned())?;
    Ok(Received {
        text: manifest.text,
        paths,
    })
}

/// Pastes text, or files and folders, onto the host.
pub async fn push(
    client: &RemoteClient,
    transfer: &str,
    text: Option<String>,
    paths: Vec<PathBuf>,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    let (entries, sources) = tokio::task::spawn_blocking(move || collect(&paths))
        .await
        .map_err(|_| "clipboard_failed")?
        .map_err(|error| error_code(&error).to_owned())?;
    let manifest = ClipboardManifest { text, entries };
    if !manifest.valid() {
        return Err("clipboard_unsupported".into());
    }
    let total = manifest.total_size();
    let command = RemoteCommand::ClipboardPush {
        transfer: transfer.to_owned(),
        manifest: manifest.clone(),
    };
    let (mut send, mut recv) = client
        .open_stream(&command)
        .await
        .map_err(|error| format!("{error:?}"))?;
    progress(0, total);
    let sent = send_files(&manifest.entries, &sources, &mut send, |done| {
        progress(done, total)
    })
    .await;
    if sent.is_ok() {
        send.finish().map_err(|error| format!("{error:?}"))?;
    }
    // When the host refuses early, for example without a folder to write to, its reply explains
    // why the upload was cut off, so read it before reporting the write error.
    let reply = recv.read_to_end(MAX_REMOTE_REPLY).await;
    match reply.ok().map(|reply| serde_json::from_slice(&reply)) {
        Some(Ok(RemoteReply::Error { code })) => Err(code),
        Some(Ok(RemoteReply::Ok)) if sent.is_ok() => Ok(()),
        _ => match sent {
            Err(error) => Err(error_code(&error).to_owned()),
            Ok(()) => Err("clipboard_invalid".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_rejects_forbidden_characters_in_path_segments() {
        let folder = Path::new("received");
        for name in [
            r"one\two.txt",
            "file:stream",
            "C:file.txt",
            "quote\".txt",
            "left<.txt",
            "right>.txt",
            "pipe|.txt",
            "question?.txt",
            "star*.txt",
            "nul\0.txt",
            "control\u{1f}.txt",
            "delete\u{7f}.txt",
        ] {
            let path = format!("folder/{name}");
            assert!(receive_path(folder, &path, true).is_err(), "{path:?}");
        }
    }

    #[test]
    fn windows_rejects_reserved_device_names_with_or_without_extensions() {
        let folder = Path::new("received");
        let numbered = ["cOm", "LpT"].into_iter().flat_map(|prefix| {
            ('1'..='9')
                .chain(['¹', '²', '³'])
                .map(move |digit| format!("{prefix}{digit}"))
        });
        for device in ["conIN$", "ConOUT$", "coN", "pRn", "auX", "NuL"]
            .map(str::to_owned)
            .into_iter()
            .chain(numbered)
        {
            for path in [
                format!("{device}.tar.gz"),
                format!("folder/{device}/file.txt"),
            ] {
                assert!(receive_path(folder, &path, true).is_err(), "{path:?}");
            }
        }
    }

    #[test]
    fn windows_rejects_reserved_device_stems_with_spaces_before_an_extension() {
        let folder = Path::new("received");
        assert!(receive_path(folder, "NUL .txt", true).is_err());
    }

    #[test]
    fn windows_rejects_path_segments_ending_in_a_dot_or_space() {
        let folder = Path::new("received");
        for path in [
            "folder./file.txt",
            "folder /file.txt",
            "folder/file.",
            "folder/file ",
        ] {
            assert!(receive_path(folder, path, true).is_err(), "{path:?}");
        }
    }

    #[test]
    fn windows_accepts_ordinary_names_under_the_destination_folder() {
        let folder = Path::new("received");
        let path = receive_path(folder, "AUXiliary/COM10/.报告 LPT0.txt", true).unwrap();
        assert_eq!(
            path,
            folder
                .join("AUXiliary")
                .join("COM10")
                .join(".报告 LPT0.txt")
        );
    }

    #[tokio::test]
    async fn rejects_an_unsafe_path_before_receiving_any_entries() -> io::Result<()> {
        let invalid_paths = ["folder/../invalid.txt"];
        #[cfg(windows)]
        let invalid_paths = invalid_paths
            .into_iter()
            .chain(["folder/cOm1.txt/child.txt"]);

        for path in invalid_paths {
            let entries = [
                ClipboardEntry {
                    path: "first.txt".into(),
                    size: 5,
                    dir: false,
                },
                ClipboardEntry {
                    path: path.into(),
                    size: 0,
                    dir: false,
                },
            ];
            let target = tempfile::tempdir()?;
            let mut input = b"hello".as_slice();
            let error = receive_files(&entries, &mut input, target.path(), |_| {})
                .await
                .unwrap_err();
            assert!(target.path().read_dir()?.next().is_none(), "{path}");
            assert_eq!(input, b"hello", "{path}");
            assert_eq!(error_code(&error), "clipboard_invalid", "{path}");
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn receives_macos_names_that_are_reserved_on_windows() -> io::Result<()> {
        let names = [r"one\two.txt", "one:two.txt", "NUL.txt"];
        let entries = names.map(|name| ClipboardEntry {
            path: name.into(),
            size: 1,
            dir: false,
        });
        let target = tempfile::tempdir()?;
        let paths = receive_files(&entries, &mut b"abc".as_slice(), target.path(), |_| {}).await?;
        assert_eq!(paths, names.map(|name| target.path().join(name)));
        for (path, byte) in paths.iter().zip(b"abc") {
            assert_eq!(std::fs::read(path)?, [*byte]);
        }
        Ok(())
    }

    #[tokio::test]
    async fn copies_folders_and_files_byte_for_byte() -> io::Result<()> {
        let source = tempfile::tempdir()?;
        let album = source.path().join("Album");
        std::fs::create_dir_all(album.join("Empty"))?;
        std::fs::write(album.join("a.bin"), vec![7; 700_000])?;
        std::fs::write(source.path().join("Notes.txt"), b"hello")?;
        let other = tempfile::tempdir()?;
        std::fs::write(other.path().join("Notes.txt"), b"second")?;

        let (entries, sources) = collect(&[
            album,
            source.path().join("Notes.txt"),
            other.path().join("Notes.txt"),
        ])?;
        let paths: Vec<_> = entries.iter().map(|entry| entry.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Album",
                "Album/Empty",
                "Album/a.bin",
                "Notes.txt",
                "Notes 2.txt"
            ]
        );

        let mut wire = Vec::new();
        send_files(&entries, &sources, &mut wire, |_| {}).await?;
        assert_eq!(wire.len(), 700_000 + 5 + 6);

        let target = tempfile::tempdir()?;
        let mut last = 0;
        let top = receive_files(&entries, &mut wire.as_slice(), target.path(), |done| {
            last = done
        })
        .await?;
        assert_eq!(last, wire.len() as u64);
        assert_eq!(top.len(), 3);
        assert_eq!(
            std::fs::read(target.path().join("Album/a.bin"))?,
            vec![7; 700_000]
        );
        assert!(target.path().join("Album/Empty").is_dir());
        assert_eq!(std::fs::read(target.path().join("Notes 2.txt"))?, b"second");

        // A stream with extra or missing bytes does not match its list and is refused.
        let again = tempfile::tempdir()?;
        let mut longer = wire.clone();
        longer.push(0);
        assert!(
            receive_files(&entries, &mut longer.as_slice(), again.path(), |_| {})
                .await
                .is_err()
        );
        let short = tempfile::tempdir()?;
        let truncated = &wire[..wire.len() - 1];
        let error = receive_files(&entries, &mut &truncated[..], short.path(), |_| {})
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn collect_skips_symlinks_inside_folders() -> io::Result<()> {
        let source = tempfile::tempdir()?;
        let album = source.path().join("Album");
        std::fs::create_dir(&album)?;
        let file = album.join("file.txt");
        std::fs::write(&file, b"hello")?;
        let link = album.join("link.txt");

        #[cfg(unix)]
        std::os::unix::fs::symlink(&file, &link)?;

        #[cfg(windows)]
        {
            const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;
            match std::os::windows::fs::symlink_file(&file, &link) {
                Ok(()) => {}
                Err(error) if error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => {
                    eprintln!(
                        "Skipping Windows symlink test because symlink creation requires Developer Mode or SeCreateSymbolicLinkPrivilege"
                    );
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        }

        let (entries, sources) = collect(&[album])?;
        let paths: Vec<_> = entries.iter().map(|entry| entry.path.as_str()).collect();
        assert_eq!(paths, ["Album", "Album/file.txt"]);
        assert_eq!(sources, [file]);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_files_report_a_permission_problem() -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let folder = tempfile::tempdir()?;
        let locked = folder.path().join("locked");
        std::fs::create_dir(&locked)?;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))?;
        let error = collect(std::slice::from_ref(&locked)).unwrap_err();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700))?;
        assert_eq!(error_code(&error), "clipboard_file_permission");
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_files_report_a_permission_problem() -> io::Result<()> {
        let folder = tempfile::tempdir()?;
        let locked = folder.path().join("locked.txt");
        std::fs::write(&locked, b"hello")?;

        // Denying only read-data to Everyone keeps metadata access and deletion available.
        let output = std::process::Command::new("icacls.exe")
            .arg(&locked)
            .args(["/deny", "*S-1-1-0:(RD)"])
            .output()?;
        assert!(
            output.status.success(),
            "icacls failed ({}): {}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let error = collect(&[locked]).unwrap_err();
        assert_eq!(error_code(&error), "clipboard_file_permission");
        folder.close()
    }
}
