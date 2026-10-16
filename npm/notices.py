#!/usr/bin/env python3
"""The third-party notices for one kzoo build: every crate compiled into the
program for a target, with its licence and the licence texts it ships.

    notices.py <repository root> <target triple>  > THIRD-PARTY-NOTICES

Reads `cargo metadata` for the target (nothing is built), walks kzoo's
normal dependencies (what is linked into the program, proc-macros included;
not build scripts' or tests'), and leaves out the workspace's own crates,
whose licence is the package's LICENSE. Vendored code inside the workspace
carries its own notice in `npm/vendored-notices.txt`, appended last.

A crate that ships no licence text is noticed under the licence it
declares: MIT where it offers MIT, with its authors as the copyright holders
(the MIT text is short and written out each time); otherwise Apache-2.0 or
MPL-2.0, whose full texts (`npm/licences/`) are appended once. A crate with
neither is a failure, named, so a release never goes out with a notice
missing. MPL-2.0 code is used unmodified from crates.io, where its source is
published; each such crate's notice says where.
"""

import json
import os
import subprocess
import sys

TEXT_NAMES = ("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT", "UNLICENSE")

MIT = """Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""

# Licences whose full text is appended once, for crates that ship none.
APPENDED = ("Apache-2.0", "MPL-2.0")


def metadata(root, target):
    out = subprocess.run(
        [
            "cargo", "metadata", "--locked", "--format-version", "1",
            "--filter-platform", target,
            "--manifest-path", os.path.join(root, "Cargo.toml"),
        ],
        check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(out)


def linked(meta):
    """Every package kzoo links, by id, following normal dependencies."""
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(p["id"] for p in meta["packages"] if p["name"] == "kzoo")
    seen, stack = set(), [root]
    while stack:
        node = nodes[stack.pop()]
        if node["id"] in seen:
            continue
        seen.add(node["id"])
        for dep in node["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    members = set(meta["workspace_members"])
    return [packages[i] for i in sorted(seen - members, key=lambda i: (packages[i]["name"], packages[i]["version"]))]


def texts(package):
    """The licence texts a crate ships: its `license-file`, then any file in
    its root whose name starts with one of TEXT_NAMES."""
    folder = os.path.dirname(package["manifest_path"])
    found = []
    if package.get("license_file"):
        found.append(os.path.join(folder, package["license_file"]))
    for name in sorted(os.listdir(folder)):
        path = os.path.join(folder, name)
        if os.path.isfile(path) and name.upper().startswith(TEXT_NAMES) and path not in found:
            found.append(path)
    return found


def options(package):
    """The licences a crate's SPDX expression lets it be used under, one of
    which is enough; None when the expression joins licences with AND."""
    expression = (package.get("license") or "").replace("/", " OR ")
    if " AND " in expression or "(" in expression:
        return None
    return [option.strip() for option in expression.split(" OR ") if option.strip()]


def declared(package):
    """The licence a crate that ships no text is noticed under, or None."""
    offered = options(package) or []
    for choice in ("MIT",) + APPENDED:
        if choice in offered:
            return choice
    return None


def main():
    root, target = sys.argv[1], sys.argv[2]
    crates = linked(metadata(root, target))
    missing = [
        f'{p["name"]} {p["version"]} ({p.get("license")})'
        for p in crates
        if not texts(p) and declared(p) is None
    ]
    if missing:
        sys.stderr.write("notices.py: no licence text shipped by: " + ", ".join(missing) + "\n")
        sys.exit(1)
    out = sys.stdout
    used = set()
    out.write(
        "Third-party notices for kzoo, the program in this package, built for "
        f"{target}.\n\nkzoo is free software under the GNU Affero General Public "
        "License, version 3 or later (see LICENSE). It is built with the "
        f"{len(crates)} crates below, each under its own licence, whose texts "
        "follow its name.\n"
    )
    for package in crates:
        out.write("\n" + "=" * 78 + "\n")
        out.write(f'{package["name"]} {package["version"]}\n')
        out.write(f'Licence: {package.get("license") or "see the text below"}\n')
        if package.get("repository"):
            out.write(f'Source: {package["repository"]}\n')
        shipped = texts(package)
        for path in shipped:
            out.write("\n--- " + os.path.basename(path) + " ---\n\n")
            with open(path, encoding="utf-8", errors="replace") as text:
                out.write(text.read().rstrip() + "\n")
        if shipped:
            continue
        choice = declared(package)
        holders = ", ".join(package["authors"]) or f'the {package["name"]} developers'
        if choice == "MIT":
            out.write(f"\nUsed under the MIT License.\n\nCopyright (c) {holders}\n\n{MIT}")
        else:
            used.add(choice)
            out.write(f"\nUsed under the {choice} licence, Copyright (c) {holders}; its text is at the end of this file.\n")
            if choice == "MPL-2.0":
                out.write(
                    "Used unmodified. Its Source Code Form is published at "
                    f'https://crates.io/crates/{package["name"]}/{package["version"]}'
                    + (f' and {package["repository"]}' if package.get("repository") else "")
                    + ".\n"
                )
    vendored = os.path.join(root, "npm", "vendored-notices.txt")
    out.write("\n" + "=" * 78 + "\n")
    with open(vendored, encoding="utf-8") as text:
        out.write(text.read().rstrip() + "\n")
    for choice in APPENDED:
        if choice in used:
            out.write("\n" + "=" * 78 + "\n")
            out.write(f"The {choice} licence, as named above\n\n")
            with open(os.path.join(root, "npm", "licences", choice + ".txt"), encoding="utf-8") as text:
                out.write(text.read().rstrip() + "\n")


if __name__ == "__main__":
    main()
