#!/usr/bin/env bash
# Smoke test for bifaria-translate, the Apple Translation helper (macOS 26+).
#   scripts/test_apple_translate.sh [helper-path]
# Checks that "Привіт, як справи?" translates Ukrainian → English when that pair is downloaded,
# that a pair that isn't downloaded gives a clean "not_installed" error, and that bad input gives
# clean errors. Also times a translation in a fresh process against one in a long-lived worker.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
HELPER="${1:-$ROOT_DIR/src-tauri/binaries/bifaria-translate-universal-apple-darwin}"
[ -x "$HELPER" ] || "$SCRIPT_DIR/build_apple_translate.sh" "$HELPER"

failures=0
pass() { echo "  ok    $1"; }
fail() { echo "  FAIL  $1: $2"; failures=$((failures + 1)); }
field() { python3 -c 'import json,sys; v=json.loads(sys.stdin.readline()).get(sys.argv[1]); print("" if v is None else v)' "$1"; }
ask() { printf '%s\n' "$1" | BIFARIA_TRANSLATE_MODEL="${MODEL:-}" "$HELPER"; }

major="$(sw_vers -productVersion | cut -d. -f1)"
if [ "$major" -lt 26 ]; then
  echo "macOS $(sw_vers -productVersion): Apple Translation needs macOS 26+, skipping."
  exit 0
fi

echo "bifaria-translate smoke test ($HELPER)"

answer="$(ask '{"op":"ping"}')"
[ "$(echo "$answer" | field ok)" = "True" ] && pass "ping" || fail "ping" "$answer"

status="$(ask '{"op":"status","source":"uk","target":"en"}' | field status)"
echo "  uk → en is $status"
answer="$(ask '{"op":"translate","text":"Привіт, як справи?","source":"uk","target":"en"}')"
if [ "$status" = "installed" ]; then
  text="$(echo "$answer" | field text)"
  if echo "$text" | grep -qi "hello\|hi,"; then pass "uk → en: \"$text\""; else fail "uk → en" "$answer"; fi
else
  [ "$(echo "$answer" | field error)" = "not_installed" ] && pass "uk → en not downloaded: not_installed" || fail "uk → en" "$answer"
fi

# A pair that isn't downloaded. On macOS 26.4+ each pair has two models; the low-latency one is
# often not downloaded, which gives a real not_installed without changing anything on this Mac.
missing=""
for model in lowLatency highFidelity; do
  if [ "$(MODEL=$model ask '{"op":"status","source":"uk","target":"en"}' | field status)" = "supported" ]; then
    missing="$model"
    break
  fi
done
if [ -n "$missing" ]; then
  answer="$(MODEL=$missing ask '{"op":"translate","text":"Привіт, як справи?","source":"uk","target":"en"}')"
  [ "$(echo "$answer" | field error)" = "not_installed" ] && pass "pair not downloaded ($missing model): not_installed" || fail "not_installed" "$answer"
else
  echo "  skip  not_installed: every model for uk → en is downloaded on this Mac"
fi

answer="$(ask '{"op":"translate","text":"Hi","source":"be","target":"en"}')"
[ "$(echo "$answer" | field error)" = "unsupported" ] && pass "be → en: unsupported" || fail "unsupported" "$answer"
answer="$(ask '{"op":"translate","text":"  ","source":"uk","target":"en"}')"
[ "$(echo "$answer" | field error)" = "nothing_to_translate" ] && pass "empty text: nothing_to_translate" || fail "empty text" "$answer"
answer="$(ask 'not json')"
[ "$(echo "$answer" | field error)" = "bad_request" ] && pass "bad JSON: bad_request" || fail "bad JSON" "$answer"
answer="$(ask '{"op":"languages","target":"en"}')"
[ "$(echo "$answer" | field ok)" = "True" ] && pass "languages: $(echo "$answer" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["languages"]))') rows" || fail "languages" "$answer"

if [ "$status" = "installed" ]; then
  echo "Latency (uk → en, ms):"
  req='{"op":"translate","text":"Я написав код учора ввечері, і він працює.","source":"uk","target":"en"}'
  python3 - "$HELPER" "$req" <<'EOF'
import json, subprocess, sys, time
helper, req = sys.argv[1], sys.argv[2]
def one_shot():
    t = time.time()
    subprocess.run([helper], input=(req + "\n").encode(), capture_output=True, check=True)
    return (time.time() - t) * 1000
shots = [one_shot() for _ in range(3)]
print("  new process per call (spawn + model load + translate):", ", ".join(f"{s:.0f}" for s in shots))
p = subprocess.Popen([helper], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
def call(line):
    t = time.time()
    p.stdin.write((line + "\n").encode()); p.stdin.flush()
    answer = json.loads(p.stdout.readline())
    return (time.time() - t) * 1000, answer.get("ms")
first = call(req)
warm = [call(req) for _ in range(5)]
ping = [call('{"op":"ping"}')[0] for _ in range(5)]
p.stdin.close(); p.wait()
print(f"  long-lived worker, first call: {first[0]:.0f}")
print("  long-lived worker, warm calls (round trip / in Apple's framework):",
      ", ".join(f"{r:.0f}/{m}" for r, m in warm))
print("  pipe round trip (ping):", ", ".join(f"{r:.1f}" for r in ping))
EOF
fi

if [ "$failures" -gt 0 ]; then
  echo "$failures check(s) failed"
  exit 1
fi
echo "All checks passed"
