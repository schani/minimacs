#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUTPUT=${1:-"$ROOT/target/ui-tests/minimacs.png"}
INPUT=${2:-}
[ "$#" -eq 0 ] || shift
[ "$#" -eq 0 ] || shift

case "$OUTPUT" in
    /*) ;;
    *) OUTPUT="$ROOT/$OUTPUT" ;;
esac
case "$INPUT" in
    "" | /*) ;;
    *) INPUT="$ROOT/$INPUT" ;;
esac

WORK_DIR=$(mktemp -d -t minimacs-native-capture)
PID=
cleanup() {
    [ -z "$PID" ] || kill "$PID" 2>/dev/null || true
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT HUP INT TERM

if [ -z "$INPUT" ]; then
    INPUT="$WORK_DIR/cursor-drift.txt"
    printf '%s' 'There is no async runtime. The terminal' >"$INPUT"
fi
[ -f "$INPUT" ] || { printf 'Input is not a file: %s\n' "$INPUT" >&2; exit 2; }

mkdir -p "$(dirname -- "$OUTPUT")"
canonical_path() {
    path_dir=$(CDPATH= cd -- "$(dirname -- "$1")" && pwd -P)
    printf '%s/%s\n' "$path_dir" "$(basename -- "$1")"
}
INPUT_PATH=$(canonical_path "$INPUT")
OUTPUT_PATH=$(canonical_path "$OUTPUT")
if [ "$INPUT_PATH" = "$OUTPUT_PATH" ] || { [ -e "$OUTPUT" ] && [ "$INPUT" -ef "$OUTPUT" ]; }; then
    printf 'Refusing to overwrite the input file: %s\n' "$INPUT" >&2
    exit 2
fi

APP="$ROOT/target/macos/Minimacs.app"
if [ "${MINIMACS_CAPTURE_SKIP_BUILD:-0}" != 1 ]; then
    "$ROOT/macos/build.sh" >/dev/null
fi

READY_FILE="$WORK_DIR/ready"
FAILED_FILE="$READY_FILE.failed"
MINIMACS_UI_READY_FILE="$READY_FILE" \
    "$APP/Contents/MacOS/Minimacs" "$INPUT" >/dev/null 2>&1 &
PID=$!

wait_until_ready() {
    # The app writes this marker only after the startup file is open,
    # background syntax work is consumed, and the frame has been displayed.
    for _ in $(jot 200); do
        [ ! -f "$FAILED_FILE" ] || { echo "minimacs could not open the input" >&2; exit 1; }
        [ ! -f "$READY_FILE" ] || return 0
        kill -0 "$PID" 2>/dev/null || { echo "minimacs exited before capture" >&2; exit 1; }
        sleep 0.05
    done
    echo "timed out waiting for minimacs to render" >&2
    exit 1
}
wait_until_ready

# Address the process by PID so another minimacs instance cannot be selected.
# System Events makes scripted key presses use the same AppKit path as a user
# action. The invoking terminal needs Accessibility permission.

osascript - "$PID" "$@" <<'APPLESCRIPT'
on run argv
  set targetPid to item 1 of argv as integer
  tell application "System Events"
    tell (first process whose unix id is targetPid)
      set frontmost to true
      set position of window 1 to {100, 100}
      set size of window 1 to {960, 668}
      if (count windows) = 0 then error "minimacs has no window"
      if (count argv) > 1 then
        repeat with keyCodeArgument in items 2 thru -1 of argv
          key code (keyCodeArgument as integer)
          delay 0.05
        end repeat
      end if
      delay 0.1
    end tell
  end tell
end run
APPLESCRIPT

# Consume syntax work caused by scripted keys and display their final frame.
# SIGUSR1 is handled only when this test-only ready path is configured.
rm -f "$READY_FILE"
kill -USR1 "$PID"
wait_until_ready

# AXWindowNumber is not available on every macOS release. Core Graphics can
# reliably identify the on-screen layer-zero window from the launched PID.
WINDOW_ID=$(swift - "$PID" <<'SWIFT'
import CoreGraphics
import Foundation

let pid = Int32(CommandLine.arguments[1])!
let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID)! as NSArray
for case let window as [String: Any] in windows {
  let owner = (window[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value
  let layer = (window[kCGWindowLayer as String] as? NSNumber)?.intValue
  if owner == pid, layer == 0,
     let number = window[kCGWindowNumber as String] as? NSNumber {
    print(number.uint32Value)
    exit(0)
  }
}
fputs("Could not find the minimacs window\n", stderr)
exit(1)
SWIFT
)

screencapture -x -o -l "$WINDOW_ID" "$OUTPUT"
printf '%s\n' "$OUTPUT"
