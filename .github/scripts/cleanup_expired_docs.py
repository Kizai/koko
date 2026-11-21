import argparse
from datetime import date, datetime
from pathlib import Path
import subprocess
from zoneinfo import ZoneInfo


REPO_ROOT = Path(__file__).resolve().parents[2]


def expired_notes(today: date) -> list[str]:
    tracked_notes = subprocess.check_output(
        ["git", "ls-files", "-z", "--", ":(glob)docs/notes/**/*.md"],
        cwd=REPO_ROOT,
    ).decode("utf-8").split("\0")

    expired = []
    for filename in filter(None, tracked_notes):
        note = REPO_ROOT / filename
        if note.is_symlink() or not note.is_file():
            continue

        with note.open(encoding="utf-8") as document:
            first_line = document.readline().rstrip("\n")
        if not first_line.startswith("<!-- expires-on:"):
            continue

        date_text = first_line.removeprefix("<!-- expires-on: ").removesuffix(" -->")
        try:
            expiry = date.fromisoformat(date_text)
            if first_line != f"<!-- expires-on: {expiry.isoformat()} -->":
                raise ValueError
        except ValueError:
            raise SystemExit(f"{filename}: expected <!-- expires-on: YYYY-MM-DD -->") from None

        if expiry <= today:
            expired.append(filename)
    return expired


def main() -> None:
    parser = argparse.ArgumentParser(description="Find expired tracked documentation notes.")
    parser.add_argument("--apply", action="store_true", help="Stage the deletion of expired notes.")
    args = parser.parse_args()
    expired = expired_notes(datetime.now(ZoneInfo("Asia/Shanghai")).date())

    if not expired:
        print("No expired documentation notes.")
    elif args.apply:
        subprocess.run(["git", "--literal-pathspecs", "rm", "--", *expired], cwd=REPO_ROOT, check=True)
    else:
        for filename in expired:
            print(f"Would remove: {filename}")


if __name__ == "__main__":
    main()
