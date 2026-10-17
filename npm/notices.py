#!/usr/bin/env python3
"""The third-party notices for one kzoo build: every crate compiled into the
program for a target, with the licence texts that cover it.

    notices.py <repository root> <target triple> <output file>

The crates are what `cargo tree -p kzoo -e normal,no-proc-macro --target
<triple>` builds into the program: proc-macros, and what only they use, run
while building and are not in it. The workspace's own crates are left out
(their licence is the package's LICENSE). Nothing is built.

Each crate's licence expression must be covered by texts: those the crate
ships (LICENSE*, COPYING*, NOTICE*, ...), recognised by their wording, and
for a crate that ships none, the upstream texts kept in `npm/licences/upstream/`
for its exact version (`npm/licences/upstream.json`). Apache-2.0 and MPL-2.0
are covered by their full texts, appended once. Every crate under MPL-2.0 is
noted with where its source is published, and must come from crates.io,
unmodified. Code vendored into kzoo's own crates is noticed from
`npm/vendored-notices.txt`.

Anything not covered fails the run, naming each crate, and nothing is
written: the output appears whole or not at all.
"""

import json
import os
import re
import subprocess
import sys
import tempfile

CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
TEXT_NAMES = ("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT", "UNLICENSE")
# Licences whose full text is appended once, so a crate under one is covered.
APPENDED = ("Apache-2.0", "MPL-2.0")

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

# Wording that marks a licence's text, by SPDX id: every phrase in `has`
# appears, and none in `lacks` does (each lacks what a stricter or looser
# licence that shares its wording adds).
SIGNS = {
    "MIT": {"has": ["Permission is hereby granted, free of charge",
                    "The above copyright notice and this permission notice shall be included"]},
    "Apache-2.0": {"has": ["TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION", "Apache License"]},
    "MPL-2.0": {"has": ["Mozilla Public License"]},
    "BSD-3-Clause": {"has": ["Redistribution and use in source and binary forms", "Neither the name"]},
    "BSD-2-Clause": {"has": ["Redistribution and use in source and binary forms", "Redistributions in binary form"],
                     "lacks": ["Neither the name"]},
    "BSD-1-Clause": {"has": ["Redistribution and use in source and binary forms", "Redistributions of source code"],
                     "lacks": ["Neither the name", "Redistributions in binary form"]},
    "ISC": {"has": ["Permission to use, copy, modify, and/or distribute this software for any",
                    "provided that the above copyright notice and this permission notice appear in all copies"]},
    "0BSD": {"has": ["Permission to use, copy, modify, and/or distribute this software for any"],
             "lacks": ["provided that the above copyright notice and this permission notice appear in all copies"]},
    "Zlib": {"has": ["This software is provided 'as-is'"]},
    "Unlicense": {"has": ["This is free and unencumbered software released into the public domain"]},
    "BSL-1.0": {"has": ["Boost Software License"]},
    "WTFPL": {"has": ["DO WHAT THE FUCK YOU WANT"]},
    "Unicode-3.0": {"has": ["UNICODE LICENSE V3"]},
    "Unicode-DFS-2016": {"has": ["UNICODE, INC. LICENSE AGREEMENT - DATA FILES AND SOFTWARE"]},
    "LLVM-exception": {"has": ["LLVM Exceptions to the Apache 2.0 License"]},
}


def fail(message):
    sys.stderr.write("notices.py: " + message + "\n")
    sys.exit(1)


def run(root, *args):
    try:
        return subprocess.run(
            ["cargo", *args, "--manifest-path", os.path.join(root, "Cargo.toml")],
            check=True, capture_output=True, text=True,
        ).stdout
    except subprocess.CalledProcessError as error:
        fail(f"cargo {args[0]} failed:\n{error.stderr}")


def built(root, target):
    """(name, version) of every package `cargo tree` builds into kzoo."""
    tree = run(root, "tree", "--locked", "-p", "kzoo", "-e", "normal,no-proc-macro",
               "--target", target, "--prefix", "none", "-f", "{p}")
    found = set()
    for line in tree.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[1].startswith("v"):
            found.add((parts[0], parts[1][1:]))
    if not found:
        fail("cargo tree listed nothing for kzoo")
    return found


def identify(text):
    """The licences a text is the text of, by its wording."""
    flat = " ".join(text.split())

    def found(phrase):
        return " ".join(phrase.split()) in flat

    return {
        spdx
        for spdx, sign in SIGNS.items()
        if all(map(found, sign["has"])) and not any(map(found, sign.get("lacks", [])))
    }


def options(expression):
    """The ways an SPDX expression can be met, each a set of ids (a WITH
    exception is an id of its own), or None if it cannot be read."""
    tokens = re.findall(r"\(|\)|[A-Za-z0-9.+-]+", expression.replace("/", " OR "))
    position = 0

    def peek():
        return tokens[position] if position < len(tokens) else None

    def take():
        nonlocal position
        position += 1
        return tokens[position - 1]

    def either():
        ways = both()
        while peek() == "OR":
            take()
            ways = ways + both()
        return ways

    def both():
        ways = one()
        while peek() == "AND":
            take()
            right = one()
            ways = [a | b for a in ways for b in right]
        return ways

    def one():
        if peek() == "(":
            take()
            ways = either()
            if take() != ")":
                raise ValueError(expression)
            return ways
        name = take()
        if name in ("AND", "OR", "WITH", ")"):
            raise ValueError(expression)
        if peek() == "WITH":
            take()
            exception = take()
            if exception in ("AND", "OR", "WITH", "(", ")"):
                raise ValueError(expression)
            return [{name, exception}]
        return [{name}]

    try:
        ways = either()
    except (IndexError, ValueError):
        return None
    return ways if position == len(tokens) else None


def shipped(package):
    folder = os.path.dirname(package["manifest_path"])
    found = []
    if package.get("license_file"):
        found.append(os.path.join(folder, package["license_file"]))
    for name in sorted(os.listdir(folder)):
        path = os.path.join(folder, name)
        if os.path.isfile(path) and name.upper().startswith(TEXT_NAMES) and path not in found:
            found.append(path)
    return found


def read(path):
    with open(path, encoding="utf-8", errors="replace") as text:
        return text.read().rstrip() + "\n"


def upstream(root):
    with open(os.path.join(root, "npm", "licences", "upstream.json"), encoding="utf-8") as text:
        entries = json.load(text)["entries"]
    by_crate = {}
    for entry in entries:
        for crate in entry["crates"]:
            by_crate[crate] = entry
    return by_crate


def main():
    root, target, output = sys.argv[1], sys.argv[2], sys.argv[3]
    meta = json.loads(run(root, "metadata", "--locked", "--format-version", "1",
                          "--filter-platform", target))
    members = set(meta["workspace_members"])
    packages = {(p["name"], p["version"]): p for p in meta["packages"]}
    kept = upstream(root)
    crates, problems, used = [], [], set()
    for key in sorted(built(root, target)):
        package = packages.get(key)
        if package is None:
            problems.append(f"{key[0]} {key[1]}: built but not in cargo metadata")
            continue
        if package["id"] in members:
            continue
        if any("proc-macro" in kind for t in package["targets"] for kind in t["kind"]):
            continue
        crates.append(package)

    entries = []
    for package in crates:
        name, version = package["name"], package["version"]
        label = f"{name} {version} ({package.get('license')})"
        texts = [(os.path.basename(path), read(path)) for path in shipped(package)]
        entry = None
        if not texts:
            entry = kept.get(f"{name}@{version}")
            if entry is None:
                problems.append(f"{label}: ships no licence text and none is kept for it")
                continue
            folder = os.path.join(root, "npm", "licences", "upstream")
            texts = [(file, read(os.path.join(folder, file))) for file in entry["files"]]
        have = set().union(*(identify(text) for _, text in texts)) | set(APPENDED)
        if entry is not None and entry["licence"] == "MIT" and not entry["files"]:
            have.add("MIT")
        ways = options(package.get("license") or "")
        if ways is None:
            problems.append(f"{label}: its licence expression cannot be read")
            continue
        met = [way for way in ways if way <= have]
        if not met:
            problems.append(f"{label}: no way to meet it is covered by {sorted(have)}")
            continue
        mpl = any("MPL" in spdx for way in ways for spdx in way)
        if mpl and package.get("source") != CRATES_IO:
            problems.append(f"{label}: MPL code from {package.get('source')}, not crates.io")
            continue
        used |= set(APPENDED) & set().union(*met)
        entries.append((package, texts, entry, mpl))

    if problems:
        fail("the notices cannot be written:\n  " + "\n  ".join(problems))

    folder = os.path.dirname(os.path.abspath(output))
    out = tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=folder, delete=False)
    try:
        write(out, target, entries, used, root)
        out.close()
        os.chmod(out.name, 0o644)
        os.replace(out.name, output)
    except BaseException:
        out.close()
        os.unlink(out.name)
        raise


def write(out, target, entries, used, root):
    """The notices, into the open file `out`."""
    out.write(
        f"Third-party notices for kzoo, the program in this package, built for {target}.\n\n"
        "kzoo is free software under the GNU Affero General Public License, version 3 or "
        f"later (see LICENSE). Compiled into it are the {len(entries)} crates below, each "
        "under its own licence, whose texts follow its name, and the vendored code at the "
        "end. Procedural macros, and crates only they use, run while it is built and are "
        "not part of it, so they are left out.\n"
    )
    for package, texts, entry, mpl in entries:
        out.write("\n" + "=" * 78 + "\n")
        out.write(f'{package["name"]} {package["version"]}\n')
        out.write(f'Licence: {package.get("license")}\n')
        if package.get("repository"):
            out.write(f'Source: {package["repository"]}\n')
        if mpl:
            out.write(
                "Used unmodified. Its Source Code Form is published at "
                f'https://crates.io/crates/{package["name"]}/{package["version"]}.\n'
            )
        if entry is not None:
            out.write(f'The crate ships no licence text; kept from {entry["source"]}.\n')
            if entry.get("copyright"):
                out.write(f'{entry["copyright"]}\n')
            if not entry["files"]:
                out.write(f"\nUsed under the MIT License.\n\n{MIT}")
        for file, text in texts:
            out.write(f"\n--- {file} ---\n\n{text}")
    out.write("\n" + "=" * 78 + "\n")
    out.write(read(os.path.join(root, "npm", "vendored-notices.txt")))
    for spdx in APPENDED:
        if spdx in used:
            out.write("\n" + "=" * 78 + "\n")
            out.write(f"The {spdx} licence, as named above\n\n")
            out.write(read(os.path.join(root, "npm", "licences", spdx + ".txt")))


if __name__ == "__main__":
    main()
