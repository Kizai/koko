"""Run a GUI-approved localhost capture and publish only a verified archive."""
import argparse
import contextlib
from concurrent.futures import ThreadPoolExecutor
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time

from capture_media import inspect_stream


ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "target/koko-macos/local/Koko.app"
NAMES = ("requests.jsonl", "replies.jsonl", "desktop.cli.jsonl", "metadata.json",
         "video-feedback-on.bin", "video-feedback-off.bin",
         "audio-feedback-on.bin", "audio-feedback-off.bin")


class CaptureError(Exception):
    """Operator-facing diagnosis containing no captured data."""


def run(args, *, data=None, env=None, timeout=30):
    return subprocess.run(args, input=data, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                          check=True, timeout=timeout, env=env).stdout


@contextlib.contextmanager
def owned(args, **kwargs):
    process = subprocess.Popen(args, stderr=subprocess.DEVNULL, start_new_session=True, **kwargs)
    try:
        yield process
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def app_pids(executable):
    # LaunchServices may relaunch this exact private bundle after a permission grant.
    lines = run(["/bin/ps", "-axo", "pid=,comm="]).decode().splitlines()
    matches = []
    for line in lines:
        fields = line.strip().split(None, 1)
        if len(fields) == 2 and fields[1] == str(executable):
            matches.append(int(fields[0]))
    return matches


@contextlib.contextmanager
def test_app(executable, state, env):
    with owned([executable], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL) as proc:
        try:
            yield proc
        finally:
            # Retire the whole private bundle: macOS can deny renaming a running
            # Mach-O inside an app. The pending open -n URL now has no launch target.
            marker = state / "cleanup-incomplete"
            marker.touch()
            bundle = executable.parents[2]
            retired = state / "retired-bundle"
            bundle.rename(retired)
            (state / "stop-bridge").touch()
            daemon = executable.parents[1] / "Helpers/koko-daemon-real"
            paths = (executable, retired / executable.relative_to(bundle),
                     daemon, retired / daemon.relative_to(bundle))
            stop_owned_processes(paths, proc, state)
            marker.unlink()


def active_bridge(state):
    for path in state.glob("bridge-*/owner.lock"):
        with path.open("rb") as owner:
            try:
                fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return True
    return False


def relaunch_pids(bundle):
    # PermissionRelaunch.schedule uses these exact trailing argv for its shell and open.
    url = re.escape(str(bundle))
    pattern = (r" koko-reopen [0-9]+ " + url + r" --review-desktop-permissions$|"
               r"^(/usr/bin/)?open -n " + url + r" --args --review-desktop-permissions$")
    result = subprocess.run(["/usr/bin/pgrep", "-f", pattern], capture_output=True)
    if result.returncode not in (0, 1):
        raise CaptureError("Could not verify the private permission relaunch process.")
    return {int(pid) for pid in result.stdout.splitlines()}


def stop_owned_processes(executables, initial, state):
    start = time.monotonic()
    quiet_since = None
    while time.monotonic() - start < 40:
        initial.poll()  # Reap our own child so ps does not report a zombie.
        pids = {pid for executable in executables for pid in app_pids(executable)}
        pids.update(relaunch_pids(executables[0].parents[2]))
        if initial.poll() is None:
            pids.add(initial.pid)
        if pids:
            quiet_since = None
            for pid in pids:
                try:
                    os.kill(pid, signal.SIGTERM if time.monotonic() - start < 3 else signal.SIGKILL)
                except ProcessLookupError:
                    pass
        elif active_bridge(state):
            quiet_since = None
        else:
            quiet_since = quiet_since or time.monotonic()
            if time.monotonic() - quiet_since >= 1:
                return
        time.sleep(0.1)
    raise CaptureError("Private capture processes did not exit.")


def check_session(state):
    if (state / "bridge-error").exists():
        raise CaptureError("Capture bridge rejected data or failed to forward it. Start a fresh capture.")


def publish(state, staged, destination):
    # All writers have exited. A failed earlier instance invalidates the entire run.
    check_session(state)
    os.link(staged, destination)


def wait_for(state, name, app, seconds, instance=None):
    deadline = time.monotonic() + seconds
    missing_since = None
    while time.monotonic() < deadline:
        check_session(state)
        current = (state / "current").resolve()
        if instance is not None and (instance != current or not (instance / "running").exists()):
            raise CaptureError("Test daemon restarted during capture; rerun after permission setup.")
        if (current / "failed").exists():
            raise CaptureError("Capture bridge stopped. Check the private output space and remove visible credentials from the test desktop.")
        if app.poll() is not None and not app_pids(app.args[0]):
            missing_since = missing_since or time.monotonic()
            if time.monotonic() - missing_since > 12:
                raise CaptureError("Test app closed before capture finished. Recheck screen recording and accessibility permissions.")
            time.sleep(0.2)
            continue
        missing_since = None
        if (current / "running").exists() and (current / name).exists():
            return current
        time.sleep(0.1)
    raise CaptureError("Timed out waiting for " + name + ". Check sharing, permissions, and the GUI approval prompt.")


def package(folder, destination, verify):
    names = set(NAMES)
    if set(p.name for p in folder.iterdir()) != names:
        raise ValueError("unexpected artifact set")
    lines = []
    for name in sorted(names):
        path = folder / name
        if path.is_symlink() or not path.is_file():
            raise ValueError("unexpected artifact type")
        digest = hashlib.sha256()
        with path.open("rb") as source:
            for block in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(block)
        lines.append(f"{digest.hexdigest()}  {name}\n")
    (folder / "SHA256SUMS").write_text("".join(lines), encoding="ascii")
    verify()
    with tempfile.NamedTemporaryFile(dir=destination.parent) as tmp:
        with tarfile.open(fileobj=tmp, mode="w:gz") as archive:
            for name in sorted(names | {"SHA256SUMS"}):
                archive.add(folder / name, arcname=name, recursive=False)
        tmp.flush()
        os.link(tmp.name, destination)  # Publish once; never overwrite a previous run.


def configure_capture_plist(path, env):
    plist = plistlib.loads(path.read_bytes())
    plist.update(CFBundleIdentifier="io.github.kizai.koko.capture",
                 CFBundleName="Koko Protocol Capture", CFBundleDisplayName="Koko Protocol Capture")
    # LaunchServices must preserve private identity paths when reopening after a permission grant.
    plist["LSEnvironment"] = {key: env[key] for key in ("HOME", "CFFIXED_USER_HOME", "KOKO_CAPTURE_STATE")}
    path.write_bytes(plistlib.dumps(plist))
    return plist


def check_build_paths():
    # Check ancestors first, before walking a possibly redirected build output.
    target = ROOT / "target"
    for path in (target, target / "koko-macos", APP.parent, APP, target / "sparkle"):
        if path.is_symlink():
            raise CaptureError("Local build path is a symlink; capture stopped before writing.")
    for arch in ("aarch64-apple-darwin", "x86_64-apple-darwin"):
        release = target / arch / "release"
        for path in (release.parent, release, release / "koko-agent", release / "koko-daemon"):
            if path.is_symlink():
                raise CaptureError("Local Rust build path is a symlink.")
    if not APP.parent.exists():
        return
    for path in APP.parent.iterdir():
        if path.is_symlink():
            raise CaptureError("Symlink in local build outputs.")
    framework = APP / "Contents/Frameworks/Sparkle.framework"
    for tree in (APP, APP.parent / "KoKo.iconset"):
        for parent, dirs, files in os.walk(tree, followlinks=False):
            for name in dirs + files:
                path = Path(parent) / name
                if not path.is_symlink():
                    continue
                # Sparkle uses relative version links within its own framework.
                if framework in path.parents:
                    link = os.readlink(path)
                    if not os.path.isabs(link) and framework in Path(os.path.abspath(path.parent / link)).parents:
                        continue
                raise CaptureError("Unsafe symlink in the local build output.")


def new_output(path):
    if path is None:
        target = ROOT / "target"
        target.mkdir(mode=0o700, exist_ok=True)
        return Path(tempfile.mkdtemp(prefix="protocol-capture-", dir=target))
    directory = path.parent.resolve() / path.name
    if not directory.parent.is_dir() or any(part.lower().endswith(".app") for part in directory.parts):
        raise CaptureError("Choose a new output directory outside application bundles.")
    try:
        directory.mkdir(mode=0o700)
    except FileExistsError:
        raise CaptureError("Output directory already exists; choose a new name.") from None
    return directory


@contextlib.contextmanager
def private_session():
    check_build_paths()
    state = Path(tempfile.mkdtemp(prefix=".capture-", dir=APP.parent))
    try:
        yield state
    finally:
        if (state / "cleanup-incomplete").exists():
            print("Process cleanup could not be verified; private state retained at", state, file=sys.stderr)
        else:
            shutil.rmtree(state)


def prepare_capture_app(state, env):
    if state.parent != APP.parent or not state.name.startswith(".capture-") or state.is_symlink():
        raise CaptureError("Disposable app must be inside the private local build session.")
    app = state / "Koko.app"
    run(["/usr/bin/ditto", APP, app], timeout=120)
    plist = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    signature = subprocess.run(["codesign", "-dv", "--verbose=4", app],
                               capture_output=True, check=True).stderr.splitlines()
    if plist.get("KokoAdHocSigned") is not True or b"Signature=adhoc" not in signature:
        raise CaptureError("Capture requires a verified local ad hoc build.")
    run(["codesign", "--verify", "--deep", "--strict", app])
    helpers = app / "Contents/Helpers"
    daemon = helpers / "koko-daemon"
    real = helpers / "koko-daemon-real"
    plist = configure_capture_plist(app / "Contents/Info.plist", env)
    daemon.rename(real)
    shutil.copyfile(ROOT / "scripts/capture_bridge.py", daemon)
    daemon.chmod(0o700)
    run(["codesign", "--force", "--sign", "-", daemon])
    run(["codesign", "--force", "--sign", "-", app])
    run(["codesign", "--verify", "--deep", "--strict", app])
    return app, plist


def run_session(state, output, source_dirty):
    env = dict(os.environ, HOME=str(state / "home"), CFFIXED_USER_HOME=str(state / "home"),
               KOKO_CAPTURE_STATE=str(state))
    (state / "home").mkdir()
    folder = state / "artifacts"
    folder.mkdir()
    home = run(["/usr/bin/swift", "-e", "import Foundation; print(FileManager.default.homeDirectoryForCurrentUser.path)"],
               env=env, timeout=120).decode().strip()
    if Path(home).resolve() != (state / "home"):
        raise CaptureError("Foundation did not select the private identity directory.")
    app, plist = prepare_capture_app(state, env)
    helpers = app / "Contents/Helpers"
    real = helpers / "koko-daemon-real"
    agent = helpers / "koko-agent"
    host = run([real, "identity", state / "host.key"], env=env).decode().strip()
    if not re.fullmatch('[a-f0-9]{64}', host):
        raise ValueError("unexpected host public identity")
    print("如果 Koko Protocol Capture 请求访问日常 Koko 自定义验证码的钥匙串项目，请点“拒绝”。", flush=True)
    with test_app(app / "Contents/MacOS" / plist["CFBundleExecutable"], state, env) as gui:
        print("请在测试 App 中授予权限、开启共享；看到 Protocol Capture 配对请求时点击仅允许本次。", flush=True)
        instance = wait_for(state, "listener.json", gui, 300)
        listener = json.loads((instance / "listener.json").read_bytes())
        address = listener["address"]
        key = state / "controller.key"
        paired = json.loads(run([agent, "pair", key, "-", "--known", f"{host}/{address}",
                                 "--name", "Protocol Capture"], env=env, timeout=150,
                                data=(listener["device_id"] + "\n").encode()))
        if paired.get("host_id") != host or paired.get("status") != "paired":
            raise ValueError("local GUI pairing failed")
        wait_for(state, "approved", gui, 30, instance)
        print("请隐藏 Koko 的连接码窗口并切换到专用测试窗口。15 秒后开始记录画面和测试声音。", flush=True)
        time.sleep(15)
        target = [key, host, address]
        cli = []
        settings = []
        expected_exchanges = []
        def remote(action, expected):
            result = run([agent, "remote-stream", *target], env=env,
                         data=json.dumps(action).encode() + b'\n')
            reply = json.loads(result)
            if reply.get("status") != expected:
                raise CaptureError("Desktop request failed: " + action["action"] + ". Check the test app permissions.")
            if action["action"] != "disconnect":
                expected_exchanges.append((action, expected))
            cli.append(result)
            return reply

        displays = remote({"action": "displays"}, "displays")["displays"]
        display = displays[0]["id"]
        remote({"action": "ui_tree", "max_nodes": 50}, "ui_tree")
        remote({"action": "screenshot"}, "frame")
        remote({"action": "wallpaper"}, "image")
        for fmt in ("png", "jpeg"):
            remote(dict(action="capture", display=display, edge=960, quality=70, format=fmt), "image")
        reports = {}
        for feedback, label in ((False, "off"), (True, "on")):
            request = dict(action="video", display=display, edge=1280, fps=30,
                           quality=70, audio=True, hevc=False, feedback=feedback)
            settings.append(request)
            print("Recording H.264 and system audio, feedback " + label + "…", flush=True)
            video = folder / f"video-feedback-{label}.bin"
            expected_exchanges.extend([(request, "ok"), ({"action": "audio"}, "ok")])
            audio_path = folder / f"audio-feedback-{label}.bin"
            reports[label] = record_media([agent, "remote-video", *target, json.dumps(request)],
                                          video, audio_path, feedback, env)
        remote({"action": "disconnect"}, "ok")
        (folder / "desktop.cli.jsonl").write_bytes(b"".join(cli))
        for name in ("requests.jsonl", "replies.jsonl"):
            shutil.copyfile(instance / name, folder / name)
        requests = desktop_lines(folder / "requests.jsonl", b"Desktop request: ")
        replies = desktop_lines(folder / "replies.jsonl", b"desktop-reply ")
        verify_exchanges(requests, replies, expected_exchanges)
        meta = dict(commit=run(["git", "-C", ROOT, "rev-parse", "HEAD"]).decode().strip(),
                    bundle_id=plist["CFBundleIdentifier"], source_dirty=source_dirty, macos=platform.mac_ver()[0], app_version=plist["CFBundleShortVersionString"],
                    utc=datetime.datetime.now(datetime.timezone.utc).isoformat(), displays=displays,
                    requests=settings, media=reports)
        (folder / "metadata.json").write_text(json.dumps(meta, indent=2) + "\n")
        def verify():
            (instance / "scan-request").touch()
            wait_for(state, "scan-complete", gui, 60, instance)
        package(folder, output, verify)


def verify_audio_windows(arrivals, windows):
    counts = [sum(begin <= packet <= end for packet in arrivals) for begin, end in windows]
    if not counts or not all(counts):
        raise CaptureError("Audio stopped during a test sound. Check system audio capture.")
    return counts


def record_media(command, video_path, audio_path, feedback, env):
    start = time.monotonic()
    arrivals = []
    video_first = None
    video_last = None
    windows = []
    with video_path.open("xb") as video, audio_path.open("xb") as audio:
        def record(kind, header, body):
            nonlocal video_first, video_last
            observed = time.monotonic() - start
            if kind == 6:
                arrivals.append(observed)
            elif kind in (2, 9):
                if video_first is None:
                    video_first = observed
                video_last = observed
            video.write(header)
            video.write(body)

        # Closing the process before joining the reader guarantees EOF on cancellation.
        with ThreadPoolExecutor(max_workers=1) as reader, owned(command, env=env,
                stdin=subprocess.PIPE, stdout=subprocess.PIPE) as recorder:
            result = reader.submit(inspect_stream, recorder.stdout, audio, feedback, record)
            try:
                for _ in range(4):
                    time.sleep(2)
                    if result.done():
                        raise CaptureError("Media stream ended before the recording window.")
                    begin = time.monotonic() - start
                    run(["/usr/bin/afplay", "/System/Library/Sounds/Glass.aiff"], timeout=8)
                    time.sleep(0.4)  # Allow encoded audio from the last playback buffer to arrive.
                    windows.append((begin, time.monotonic() - start))
                recorder.stdin.close()
                if recorder.wait(15) != 0:
                    raise CaptureError("Media recorder failed.")
                report = result.result(timeout=10)
            except ValueError:
                raise CaptureError("Invalid or incomplete H.264 or system audio recording.") from None
    report["elapsed_seconds"] = round(time.monotonic() - start, 2)
    report["video_first_last_seconds"] = [video_first, video_last]
    report["playback_windows_seconds"] = windows
    report["audio_packets_per_window"] = verify_audio_windows(arrivals, windows)
    return report


def verify_exchanges(requests, replies, expected):
    if requests.keys() != replies.keys():
        raise CaptureError("Incomplete desktop exchanges.")
    unused = list(requests)
    for wanted, status in expected:
        for number in unused:
            request, reply = requests[number], replies[number]
            # Rust omits false feedback/hevc flags when serializing the host request.
            if not all(request.get(key, False if key in ("hevc", "feedback") else None) == value
                       for key, value in wanted.items()):
                continue
            if reply.get("status") != status:
                continue
            if wanted["action"] in ("video", "audio") and not request.get("video_socket"):
                continue
            if status == "image" and (reply.get("format") != wanted.get("format", "jpeg")
                                      or not reply.get("image")):
                continue
            if status == "frame" and not reply.get("png"):
                continue
            if status == "displays" and not reply.get("displays"):
                continue
            if status == "ui_tree" and not isinstance(reply.get("nodes"), list):
                continue
            unused.remove(number)
            break
        else:
            raise CaptureError("Missing required raw desktop exchange: " + wanted["action"])
    if unused:
        raise CaptureError("Unexpected desktop exchange in the approved capture instance.")


def desktop_lines(path, prefix):
    result = {}
    with path.open("rb") as source:
        for line in source:
            if not line.startswith(prefix):
                raise ValueError("unexpected desktop line")
            number, value = line[len(prefix):].split(b' ', 1)
            if number in result:
                raise ValueError("duplicate desktop number")
            result[number] = json.loads(value)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, metavar="NEW_DIRECTORY",
                        help="create a private directory for capture.tar.gz (default: target/protocol-capture-*)")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("macOS GUI login required")
    os.umask(0o077)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(1))
    check_build_paths()
    output_directory = new_output(args.output)
    print("Building the local ad hoc app…", flush=True)
    source_dirty = bool(run(["git", "-C", ROOT, "status", "--porcelain"]))
    build_env = dict(os.environ, KOKO_SIGN_IDENTITY="-", KOKO_UPDATE_ASSETS="0")
    build_env.pop("KOKO_PREVIOUS_APP", None)
    with owned([ROOT / "scripts/build-macos.sh", "--allow-ad-hoc"], env=build_env) as build:
        try:
            if build.wait(timeout=1800):
                raise CaptureError("Local ad hoc build failed. Check the repository's macOS build prerequisites.")
        finally:
            # Build tools inherit this new session. On interruption, stop the whole owned group.
            if build.poll() is None:
                os.killpg(build.pid, signal.SIGTERM)
                try:
                    build.wait(5)
                except subprocess.TimeoutExpired:
                    pass
                try:
                    os.killpg(build.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
    destination = output_directory / "capture.tar.gz"
    with private_session() as state, tempfile.TemporaryDirectory(dir=output_directory) as staging:
        pending = Path(staging) / "verified.tar.gz"
        run_session(state, pending, source_dirty)
        # Publication follows confirmed GUI, wrapper and daemon termination.
        publish(state, pending, destination)
    print("Archive:", destination)


if __name__ == "__main__":
    try:
        main()
    except CaptureError as error:
        sys.exit(str(error))
    except (KeyboardInterrupt, subprocess.SubprocessError, OSError, ValueError, KeyError, IndexError):
        sys.exit("Capture stopped. Check test-app permissions, pairing approval, screen and audio availability.")
