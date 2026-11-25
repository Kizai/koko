"""Run a GUI-approved localhost capture and publish only a verified archive."""
import argparse
import contextlib
import datetime
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
            (state / "stop-bridge").touch()
            deadline = time.monotonic() + 8
            while (state / "bridge-running").exists() and time.monotonic() < deadline:
                time.sleep(0.05)
            for pid in app_pids(executable):
                if pid != proc.pid:
                    try:
                        os.kill(pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass


def wait_for(state, name, app, seconds):
    deadline = time.monotonic() + seconds
    missing_since = None
    while time.monotonic() < deadline:
        if (state / "bridge-error").exists():
            raise CaptureError("Capture bridge stopped. Check the private output space and remove visible credentials from the test desktop.")
        if app.poll() is not None and not app_pids(app.args[0]):
            missing_since = missing_since or time.monotonic()
            if time.monotonic() - missing_since > 12:
                raise CaptureError("Test app closed before capture finished. Recheck screen recording and accessibility permissions.")
            time.sleep(0.2)
            continue
        missing_since = None
        if (state / name).exists():
            return
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


def run_session(state, output):
    env = dict(os.environ, HOME=str(state / "home"), CFFIXED_USER_HOME=str(state / "home"),
               KOKO_CAPTURE_STATE=str(state))
    (state / "home").mkdir()
    folder = state / "artifacts"
    folder.mkdir()
    home = run(["/usr/bin/swift", "-e", "import Foundation; print(FileManager.default.homeDirectoryForCurrentUser.path)"],
               env=env, timeout=120).decode().strip()
    if Path(home).resolve() != (state / "home"):
        raise CaptureError("Foundation did not select the private identity directory.")
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
    agent = helpers / "koko-agent"
    # PermissionGuide relaunches through LaunchServices. Preserve private identity paths there too.
    plist["LSEnvironment"] = {key: env[key] for key in ("HOME", "CFFIXED_USER_HOME", "KOKO_CAPTURE_STATE")}
    (app / "Contents/Info.plist").write_bytes(plistlib.dumps(plist))
    daemon.rename(real)
    shutil.copyfile(ROOT / "scripts/capture_bridge.py", daemon)
    daemon.chmod(0o700)
    run(["codesign", "--force", "--sign", "-", daemon])
    run(["codesign", "--force", "--sign", "-", app])
    run(["codesign", "--verify", "--deep", "--strict", app])
    host = run([real, "identity", state / "host.key"], env=env).decode().strip()
    if not re.fullmatch('[a-f0-9]{64}', host):
        raise ValueError("unexpected host public identity")
    with test_app(app / "Contents/MacOS" / plist["CFBundleExecutable"], state, env) as gui:
        print("请在测试 App 中授予权限、开启共享；看到 Protocol Capture 配对请求时点击仅允许本次。", flush=True)
        wait_for(state, "listener.json", gui, 300)
        listener = json.loads((state / "listener.json").read_bytes())
        address = listener["address"]
        key = state / "controller.key"
        paired = json.loads(run([agent, "pair", key, "-", "--known", f"{host}/{address}",
                                 "--name", "Protocol Capture"], env=env, timeout=150,
                                data=(listener["device_id"] + "\n").encode()))
        if paired.get("host_id") != host or paired.get("status") != "paired":
            raise ValueError("local GUI pairing failed")
        wait_for(state, "approved", gui, 30)
        print("请隐藏 Koko 的连接码窗口并切换到专用测试窗口。15 秒后开始记录画面和测试声音。", flush=True)
        time.sleep(15)
        target = [key, host, address]
        cli = []
        settings = []
        def remote(action, expected):
            result = run([agent, "remote-stream", *target], env=env,
                         data=json.dumps(action).encode() + b'\n')
            reply = json.loads(result)
            if reply.get("status") != expected:
                raise CaptureError("Desktop request failed: " + action["action"] + ". Check the test app permissions.")
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
            start = time.monotonic()
            with video.open("xb") as stream, owned([agent, "remote-video", *target,
                    json.dumps(request)], stdin=subprocess.PIPE, stdout=stream, env=env) as recorder:
                for _ in range(4):
                    time.sleep(2)
                    run(["/usr/bin/afplay", "/System/Library/Sounds/Glass.aiff"], timeout=8)
                    if recorder.poll() is not None:
                        raise ValueError("media stream ended early")
                recorder.stdin.close()
                if recorder.wait(15) != 0:
                    raise ValueError("recording failed")
            with video.open("rb") as source, (folder / f"audio-feedback-{label}.bin").open("xb") as audio:
                try:
                    reports[label] = inspect_stream(source, audio, feedback)
                except ValueError:
                    raise CaptureError("Incomplete or invalid media in feedback-" + label +
                                       " capture. Check screen recording and system audio availability.") from None
            reports[label]["elapsed_seconds"] = round(time.monotonic() - start, 2)
        remote({"action": "disconnect"}, "ok")
        (folder / "desktop.cli.jsonl").write_bytes(b"".join(cli))
        for name in ("requests.jsonl", "replies.jsonl"):
            shutil.copyfile(state / name, folder / name)
        requests = desktop_lines(folder / "requests.jsonl", b"Desktop request: ")
        replies = desktop_lines(folder / "replies.jsonl", b"desktop-reply ")
        if requests.keys() != replies.keys():
            raise ValueError("incomplete desktop exchanges")
        for action in ("video", "audio"):
            matched = [n for n, req in requests.items() if req.get("action") == action]
            if len(matched) < 2 or any(replies[n].get("status") != "ok" for n in matched):
                raise ValueError("missing media handshake")
        meta = dict(commit=run(["git", "-C", ROOT, "rev-parse", "HEAD"]).decode().strip(),
                    macos=platform.mac_ver()[0], app_version=plist["CFBundleShortVersionString"],
                    utc=datetime.datetime.now(datetime.timezone.utc).isoformat(), displays=displays,
                    requests=settings, media=reports)
        (folder / "metadata.json").write_text(json.dumps(meta, indent=2) + "\n")
        def verify():
            (state / "scan-request").touch()
            wait_for(state, "scan-complete", gui, 60)
        package(folder, output, verify)


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
    parser.add_argument("--output", type=Path, default=Path("capture-" + time.strftime("%Y%m%d-%H%M%S") + ".tar.gz"))
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("macOS GUI login required")
    if args.output.exists() or not args.output.parent.is_dir():
        parser.error("choose a new archive path in an existing directory")
    os.umask(0o077)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(1))
    print("Building the local ad hoc app…", flush=True)
    subprocess.run([ROOT / "scripts/build-macos.sh", "--allow-ad-hoc"], check=True,
                   env=dict(os.environ, KOKO_SIGN_IDENTITY="-"), timeout=1800)
    with tempfile.TemporaryDirectory(prefix="koko-capture-") as tmp:
        run_session(Path(tmp).resolve(), args.output.resolve())
    print("Archive:", args.output)


if __name__ == "__main__":
    try:
        main()
    except CaptureError as error:
        sys.exit(str(error))
    except (KeyboardInterrupt, subprocess.SubprocessError, OSError, ValueError, KeyError, IndexError):
        sys.exit("Capture stopped. Check test-app permissions, pairing approval, screen and audio availability.")
