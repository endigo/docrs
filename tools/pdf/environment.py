"""Usage: python3 -I tools/pdf/environment.py DIR > DIR/environment.json"""

import glob
import hashlib
import json
import os
import plistlib
import re
import struct
import subprocess
import sys
import zipfile

WORD = "/Applications/Microsoft Word.app"
FONT_DIRS = [
    "/System/Library/Fonts",
    "/Library/Fonts",
    os.path.expanduser("~/Library/Fonts"),
    WORD + "/Contents/Resources/DFonts",
    os.path.expanduser("~/Library/Group Containers/UBF8T346G9.Office/FontCache"),
]
NAMED = re.compile(rb'(?:w:ascii|w:hAnsi|<w:font w:name|<a:latin typeface)="([^"]+)"')


def families(directory):
    out = set()
    for path in glob.glob(os.path.join(directory, "*.docx")):
        with zipfile.ZipFile(path) as package:
            for name in package.namelist():
                if name.startswith("word/") and name.endswith(".xml"):
                    out.update(m.decode("utf-8") for m in NAMED.findall(package.read(name)))
    return {name for name in out if name and not name.startswith("+")}


def face_names(data):
    """(family, subfamily) of each face, preferring the typographic names."""
    offsets = [0]
    if data[:4] == b"ttcf":
        count = struct.unpack(">I", data[8:12])[0]
        offsets = list(struct.unpack(f">{count}I", data[12 : 12 + 4 * count]))
    faces = []
    for offset in offsets:
        tables = struct.unpack(">H", data[offset + 4 : offset + 6])[0]
        names = {}
        for i in range(tables):
            record = offset + 12 + 16 * i
            if data[record : record + 4] != b"name":
                continue
            start = struct.unpack(">I", data[record + 8 : record + 12])[0]
            count, strings = struct.unpack(">HH", data[start + 2 : start + 6])
            for j in range(count):
                platform, _, language, name_id, length, at = struct.unpack(
                    ">HHHHHH", data[start + 6 + 12 * j : start + 18 + 12 * j]
                )
                raw = data[start + strings + at : start + strings + at + length]
                if platform == 3 and language == 0x409:
                    names.setdefault(name_id, raw.decode("utf-16-be", "replace"))
                elif platform == 1 and language == 0:
                    names.setdefault(name_id, raw.decode("mac-roman", "replace"))
        faces.append((names.get(16) or names.get(1), names.get(17) or names.get(2)))
    return faces


def home_relative(path):
    home = os.path.expanduser("~")
    return "~" + path[len(home):] if path == home or path.startswith(home + os.sep) else path


def fonts(wanted):
    wanted = {name.lower() for name in wanted}
    out = []
    for folder in FONT_DIRS:
        for dirpath, _, files in os.walk(folder):
            for name in sorted(files):
                if not name.lower().endswith((".ttf", ".otf", ".ttc")):
                    continue
                path = os.path.join(dirpath, name)
                try:
                    with open(path, "rb") as handle:
                        data = handle.read()
                    faces = face_names(data)
                except (OSError, struct.error):
                    continue
                matched = [f"{family} {style}" for family, style in faces if family and family.lower() in wanted]
                if matched:
                    out.append({
                        "path": home_relative(path),
                        "faces": matched,
                        "bytes": len(data),
                        "sha256": hashlib.sha256(data).hexdigest(),
                    })
    return out


def word():
    try:
        with open(WORD + "/Contents/Info.plist", "rb") as handle:
            info = plistlib.load(handle)
    except OSError:
        return None
    return {"version": info.get("CFBundleShortVersionString"), "build": info.get("CFBundleVersion")}


def macos():
    def sw_vers(flag):
        return subprocess.run(["sw_vers", flag], capture_output=True, text=True).stdout.strip()
    return {"version": sw_vers("-productVersion"), "build": sw_vers("-buildVersion")}


def docrs():
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

    def git(*args):
        return subprocess.run(["git", "-C", root, *args], capture_output=True, text=True).stdout.strip()
    return {"commit": git("rev-parse", "HEAD"), "dirty": bool(git("status", "--porcelain"))}


def main():
    wanted = families(sys.argv[1])
    json.dump({
        "docrs": docrs(),
        "word": word(),
        "macos": macos(),
        "families": sorted(wanted),
        "fonts": fonts(wanted),
    }, sys.stdout, indent=1, ensure_ascii=False)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
