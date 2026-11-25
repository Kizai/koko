//! Create the daemon with its Job membership and its stdio fixed at launch.
use super::windows_command::append_argument;
use crate::{Event, parse_frame};
use std::{
    env,
    ffi::{OsStr, OsString, c_void},
    fs::File,
    io::{self, BufRead, BufReader, Write},
    net::SocketAddr,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    process::ExitStatus,
    thread,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};
use windows::{
    Win32::{
        Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0},
        System::{
            JobObjects::*,
            Pipes::CreatePipe,
            SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW},
            Threading::*,
        },
    },
    core::{PCWSTR, PWSTR},
};

pub(super) struct Child {
    job: OwnedHandle,
    exit: oneshot::Receiver<io::Result<ExitStatus>>,
}

impl Child {
    pub(super) async fn shutdown(mut self, grace: Duration) -> io::Result<ExitStatus> {
        let waited = timeout(grace, &mut self.exit).await;
        // Even if the daemon exited, its descendants are still in the Job.
        drop(self.job);
        let result = match waited {
            Ok(result) => result,
            Err(_) => self.exit.await,
        };
        result.map_err(|_| io::ErrorKind::BrokenPipe)?
    }
}

type WriteRequest = (String, oneshot::Sender<io::Result<()>>);
pub(super) struct ChildStdin(mpsc::Sender<WriteRequest>);

impl ChildStdin {
    pub(super) async fn write(&self, frame: String) -> io::Result<()> {
        let (sent, result) = oneshot::channel();
        self.0
            .send((frame, sent))
            .await
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        result.await.map_err(|_| io::ErrorKind::BrokenPipe)?
    }
}

pub(super) fn spawn(
    executable: &Path,
    key: &Path,
    bind: SocketAddr,
    name: &str,
    events: mpsc::Sender<io::Result<Event>>,
) -> io::Result<(Child, ChildStdin)> {
    let executable = resolve_executable(executable)?;
    let mut application: Vec<u16> = executable.as_os_str().encode_wide().collect();
    let mut command = Vec::new();
    let bind = bind.to_string();
    for argument in [
        executable.as_os_str(),
        OsStr::new("serve-pair"),
        key.as_os_str(),
        OsStr::new(&bind),
        OsStr::new("--app-desktop"),
        OsStr::new(name),
    ] {
        if !command.is_empty() {
            command.push(b' ' as u16);
        }
        append_argument(&mut command, argument.encode_wide())?;
    }
    application.push(0);
    command.push(0);

    let Created {
        job,
        process,
        mut stdin,
        stdout,
        mut stderr,
    } = create(application, command)?;
    let (writer, mut pending) = mpsc::channel::<WriteRequest>(1);
    thread::Builder::new()
        .name("koko-daemon-stdin".into())
        .spawn(move || {
            while let Some((frame, reply)) = pending.blocking_recv() {
                let result = stdin
                    .write_all(frame.as_bytes())
                    .and_then(|()| stdin.flush());
                let failed = result.is_err();
                let _ = reply.send(result);
                if failed {
                    break;
                }
            }
        })?;
    thread::Builder::new()
        .name("koko-daemon-stdout".into())
        .spawn(move || {
            let mut stdout = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                let event = match stdout.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => parse_frame(&line),
                    Err(error) => Err(error),
                };
                let failed = event.is_err();
                // Continue to drain final status lines after Host closes its receiver.
                let _ = events.blocking_send(event);
                if failed {
                    break;
                }
            }
        })?;
    thread::Builder::new()
        .name("koko-daemon-stderr".into())
        .spawn(move || {
            // Keep draining if the parent diagnostics stream has closed.
            if io::copy(&mut stderr, &mut io::stderr()).is_err() {
                let _ = io::copy(&mut stderr, &mut io::sink());
            }
        })?;
    let (terminated, exit) = oneshot::channel();
    thread::Builder::new()
        .name("koko-daemon-wait".into())
        .spawn(move || {
            let _ = terminated.send(wait(&process));
        })?;
    Ok((Child { job, exit }, ChildStdin(writer)))
}

fn resolve_executable(program: &Path) -> io::Result<PathBuf> {
    let name = program.as_os_str();
    let bytes = name.as_encoded_bytes();
    if bytes.is_empty() || bytes.contains(&0) || bytes.ends_with(b"\\") || bytes.ends_with(b"/") {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let has_exe_suffix = program
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));
    if bytes.iter().any(|byte| matches!(byte, b'/' | b'\\' | b':')) {
        let path = std::path::absolute(program)?;
        if !has_exe_suffix {
            let mut suffixed = path.as_os_str().to_os_string();
            suffixed.push(".exe");
            let candidate = PathBuf::from(suffixed);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        return Ok(path);
    }

    let mut filename = name.to_os_string();
    if !bytes.contains(&b'.') {
        filename.push(".exe");
    }

    // Follow Command's Windows search order. In particular a bare name does
    // not make the current directory an implicit lookup location.
    let application_dir = env::current_exe()
        .ok()
        .and_then(|mut path| path.pop().then_some(path));
    let system_dir = windows_directory(|buf| unsafe { GetSystemDirectoryW(buf) });
    let windows_dir = windows_directory(|buf| unsafe { GetWindowsDirectoryW(buf) });
    let path = env::var_os("PATH").unwrap_or_default();
    let candidate = [application_dir, system_dir, windows_dir]
        .into_iter()
        .flatten()
        .chain(env::split_paths(&path))
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(&filename))
        .find(|path| path.is_file())
        .ok_or(io::ErrorKind::NotFound)?;
    std::path::absolute(candidate)
}

fn windows_directory(directory: impl Fn(Option<&mut [u16]>) -> u32) -> Option<PathBuf> {
    let capacity = directory(None);
    if capacity == 0 {
        return None;
    }
    let mut buffer = vec![0; capacity as usize];
    let len = directory(Some(&mut buffer)) as usize;
    (len != 0 && len < buffer.len()).then(|| PathBuf::from(OsString::from_wide(&buffer[..len])))
}

struct Created {
    job: OwnedHandle,
    process: OwnedHandle,
    stdin: File,
    stdout: File,
    stderr: File,
}

fn create(application: Vec<u16>, mut command: Vec<u16>) -> io::Result<Created> {
    unsafe {
        let job =
            OwnedHandle::from_raw_handle(CreateJobObjectW(None, PCWSTR::null()).map_err(win_io)?.0);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            raw(&job),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of_val(&limits) as u32,
        )
        .map_err(win_io)?;

        let (stdin_child, stdin) = pipe()?;
        let (stdout, stdout_child) = pipe()?;
        let (stderr, stderr_child) = pipe()?;
        let child_ends = [raw(&stdin_child), raw(&stdout_child), raw(&stderr_child)];
        for handle in child_ends {
            SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT)
                .map_err(win_io)?;
        }
        let job_list = [raw(&job)];
        let attributes = Attributes::new()?;
        attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &job_list)?;
        attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &child_ends)?;

        let mut info = STARTUPINFOEXW::default();
        info.StartupInfo.cb = size_of_val(&info) as u32;
        info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        info.StartupInfo.hStdInput = child_ends[0];
        info.StartupInfo.hStdOutput = child_ends[1];
        info.StartupInfo.hStdError = child_ends[2];
        info.lpAttributeList = attributes.list;
        let mut created = PROCESS_INFORMATION::default();
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
            None,
            PCWSTR::null(),
            &info.StartupInfo,
            &mut created,
        )
        .map_err(win_io)?;
        let process = OwnedHandle::from_raw_handle(created.hProcess.0);
        drop(OwnedHandle::from_raw_handle(created.hThread.0));
        Ok(Created {
            job,
            process,
            stdin: stdin.into(),
            stdout: stdout.into(),
            stderr: stderr.into(),
        })
    }
}

fn wait(process: &OwnedHandle) -> io::Result<ExitStatus> {
    unsafe {
        if WaitForSingleObject(raw(process), INFINITE) != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        let mut code = 0;
        GetExitCodeProcess(raw(process), &mut code).map_err(win_io)?;
        Ok(ExitStatus::from_raw(code))
    }
}

fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
    unsafe {
        CreatePipe(&mut read, &mut write, None, 0).map_err(win_io)?;
        Ok((
            OwnedHandle::from_raw_handle(read.0),
            OwnedHandle::from_raw_handle(write.0),
        ))
    }
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}

fn win_io(error: windows::core::Error) -> io::Error {
    let code = error.code().0 as u32;
    if code & 0xffff_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xffff) as i32)
    } else {
        io::Error::other(error)
    }
}

struct Attributes {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _storage: Vec<usize>,
}
impl Attributes {
    fn new() -> io::Result<Self> {
        let mut size = 0;
        unsafe {
            // The sizing call reports ERROR_INSUFFICIENT_BUFFER and sets size.
            let sizing = InitializeProcThreadAttributeList(None, 2, None, &mut size);
            if size == 0 {
                sizing.map_err(win_io)?;
            }
            let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
            let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
            InitializeProcThreadAttributeList(Some(list), 2, None, &mut size).map_err(win_io)?;
            Ok(Self {
                list,
                _storage: storage,
            })
        }
    }

    fn set(&self, attribute: u32, handles: &[HANDLE]) -> io::Result<()> {
        unsafe {
            UpdateProcThreadAttribute(
                self.list,
                0,
                attribute as usize,
                Some(handles.as_ptr().cast::<c_void>()),
                size_of_val(handles),
                None,
                None,
            )
            .map_err(win_io)
        }
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::WAIT_TIMEOUT;

    #[test]
    #[ignore = "launched as a child by closing_job_terminates_child"]
    fn blocked_child() {
        let mut line = String::new();
        io::stdin().read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "hello child");
        println!("ready from child");
        thread::sleep(Duration::from_secs(30));
    }

    #[test]
    fn closing_job_terminates_child() -> io::Result<()> {
        let path = std::env::current_exe()?;
        let mut application: Vec<_> = path.as_os_str().encode_wide().collect();
        let mut command = Vec::new();
        append_argument(&mut command, path.as_os_str().encode_wide())?;
        command.extend(
            " --ignored --nocapture --exact process::windows_child::tests::blocked_child"
                .encode_utf16(),
        );
        application.push(0);
        command.push(0);

        let Created {
            job,
            process,
            mut stdin,
            stdout,
            stderr: _stderr,
        } = create(application, command)?;
        let mut in_job = Default::default();
        unsafe { IsProcessInJob(raw(&process), Some(raw(&job)), &mut in_job).map_err(win_io)? };
        assert!(in_job.as_bool());

        writeln!(stdin, "hello child")?;
        let (sender, ready) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.ends_with("ready from child") {
                    let _ = sender.send(());
                    break;
                }
            }
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(raw(&process), 0) },
            WAIT_TIMEOUT
        );

        drop(job);
        assert_eq!(
            unsafe { WaitForSingleObject(raw(&process), 5000) },
            WAIT_OBJECT_0
        );
        assert!(!wait(&process)?.success());
        Ok(())
    }
}
