#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUTPUT=${1:-"$ROOT/target/ui-tests/minimacs.png"}
INPUT=${2:-}
shift "$(( $# > 0 ? 1 : 0 ))"
shift "$(( $# > 0 ? 1 : 0 ))"

case "$OUTPUT" in
    /*) ;;
    *) OUTPUT="$ROOT/$OUTPUT" ;;
esac

TEMP_INPUT=
if [ -z "$INPUT" ]; then
    TEMP_INPUT=$(mktemp -t minimacs-native-ui)
    INPUT=$TEMP_INPUT
    printf '%s' 'There is no async runtime. The terminal' >"$INPUT"
elif [ "${INPUT#/}" = "$INPUT" ]; then
    INPUT="$ROOT/$INPUT"
fi

APP="$ROOT/target/macos/Minimacs.app"
if [ "${MINIMACS_CAPTURE_SKIP_BUILD:-0}" != 1 ]; then
    "$ROOT/macos/build.sh" >/dev/null
fi

mkdir -p "$(dirname -- "$OUTPUT")"
"$APP/Contents/MacOS/Minimacs" "$INPUT" >/dev/null 2>&1 &
PID=$!
cleanup() {
    kill "$PID" 2>/dev/null || true
    [ -z "$TEMP_INPUT" ] || rm -f "$TEMP_INPUT"
}
trap cleanup EXIT HUP INT TERM

# Address the process by PID so another minimacs instance cannot be selected.
# System Events also makes scripted key presses use the same AppKit path as a
# user action. The invoking terminal needs Accessibility permission.
for _ in $(jot 100); do
    if osascript - "$PID" <<'APPLESCRIPT' 2>/dev/null | grep -q '^ready$'
on run argv
  set targetPid to item 1 of argv as integer
  tell application "System Events"
    if exists (first process whose unix id is targetPid) then
      tell (first process whose unix id is targetPid)
        if (count windows) > 0 then return "ready"
      end tell
    end if
  end tell
  return "waiting"
end run
APPLESCRIPT
    then
        break
    fi
    sleep 0.05
done

osascript - "$PID" "$@" <<'APPLESCRIPT'
on run argv
  set targetPid to item 1 of argv as integer
  tell application "System Events"
    tell (first process whose unix id is targetPid)
      set frontmost to true
      set position of window 1 to {100, 100}
      set size of window 1 to {960, 668}
      if (count argv) > 1 then
        repeat with keyCodeArgument in items 2 thru -1 of argv
          key code (keyCodeArgument as integer)
          delay 0.05
        end repeat
      end if
    end tell
  end tell
end run
APPLESCRIPT
sleep 0.2

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
