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

/// Recreates the entries under `folder`, which must be new and empty, reading exactly the
/// listed bytes and then requiring the end of the stream. Returns the top-level items.
pub async fn receive_files(
    entries: &[ClipboardEntry],
    input: &mut (impl AsyncRead + Unpin),
    folder: &Path,
    mut progress: impl FnMut(u64),
) -> io::Result<Vec<PathBuf>> {
    let mut top = Vec::new();
    let mut received = 0;
    let mut buffer = vec![0; BUFFER];
    for entry in entries {
        if !koko_proto::safe_relative_path(&entry.path) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "unsafe path"));
        }
        let path = entry
            .path
            .split('/')
            .fold(folder.to_owned(), |path, part| path.join(part));
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

/// The error code a person sees for a failed transfer.
pub fn error_code(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::PermissionDenied => "clipboard_file_permission",
        io::ErrorKind::Unsupported => "clipboard_unsupported",
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

    #[tokio::test]
    async fn copies_folders_and_files_byte_for_byte() -> io::Result<()> {
        let source = tempfile::tempdir()?;
        let album = source.path().join("Album");
        std::fs::create_dir_all(album.join("Empty"))?;
        std::fs::write(album.join("a.bin"), vec![7; 700_000])?;
        std::fs::write(source.path().join("Notes.txt"), b"hello")?;
        std::os::unix::fs::symlink("/etc/hosts", album.join("link"))?;
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
            ],
            "symbolic links inside folders are left out"
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
}
