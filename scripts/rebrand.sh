#!/usr/bin/env bash
# Rename the product across the whole tree, safely and idempotently.
# usage: scripts/rebrand.sh <newlower> <bundle-id> <oldlower>
set -uo pipefail
NEW="${1:?usage: rebrand.sh <newlower> <bundle-id> <oldlower>}"
BUNDLE="${2:?usage: rebrand.sh <newlower> <bundle-id> <oldlower>}"
OLD="${3:?usage: rebrand.sh <newlower> <bundle-id> <oldlower>}"
NEW_CAP="$(python3 -c 'import sys;print(sys.argv[1].title())' "$NEW")"
OLD_CAP="$(python3 -c 'import sys;print(sys.argv[1].title())' "$OLD")"
echo "rebrand: $OLD_CAP -> $NEW_CAP   (bundle $BUNDLE)"
python3 - "$OLD" "$NEW" "$OLD_CAP" "$NEW_CAP" "$BUNDLE" <<'PY'
import subprocess, sys, re, pathlib
OLD, NEW, OLD_CAP, NEW_CAP, BUNDLE = sys.argv[1:6]
files = subprocess.run(["git","ls-files"],capture_output=True,text=True).stdout.split("\n")
sub = {OLD.upper(): NEW.upper(), OLD_CAP: NEW_CAP, OLD: NEW}
pat = re.compile(r'\b(' + "|".join(map(re.escape, sub)) + r')\b')
# bundle ids: com.<anything containing old>.<tld>
bid = re.compile(r'com\.[A-Za-z0-9.\-]*' + re.escape(OLD) + r'[A-Za-z0-9.\-]*')
changed = 0
for f in files:
    if not f: continue
    p = pathlib.Path(f)
    try: raw = p.read_bytes()
    except Exception: continue
    if b"\x00" in raw[:4000]: continue
    try: t = raw.decode("utf-8")
    except Exception: continue
    new = bid.sub(BUNDLE, t)
    new = pat.sub(lambda m: sub[m.group(1)], new)
    if new != t:
        p.write_text(new); changed += 1
print(f"  rewrote {changed} files")
PY
echo "done"
