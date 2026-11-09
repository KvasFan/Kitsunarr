#!/usr/bin/env python3
"""Extract NUnit [TestCase(...)] rows from Sonarr's C# test fixtures into JSON.

Kitsunarr's parser is a port of Sonarr's. Its tests replay Sonarr's own test corpus, so the
port is checked against exactly the inputs and expectations upstream maintains. This script
only *extracts* data: every row becomes the raw argument list of its test method. What each
method asserts is encoded on the Rust side (crates/kitsunarr-parser/tests/), where it can be
reviewed next to the code under test.

* Rows that upstream has commented out (known failures) are skipped, as upstream skips them.
* C# literals are decoded: strings (regular and verbatim), numbers, booleans, null, arrays
  (`new[] { ... }`, `new int[0]`) and enum references, which become {"enum": "Quality.HDTV720p"}.
* Named arguments (e.g. `ExpectedResult = ...`) are kept under "named".

Development tool only; Kitsunarr itself never runs Python.

usage: python3 -I sonarr_test_cases.py <sonarr-checkout> <revision> <out-dir> <fixture.cs>...
       (fixture paths are relative to the checkout)
"""
import json
import os
import re
import sys

TEST_CASE = re.compile(r"^\s*\[TestCase\((?P<args>.*)\)\]\s*$")
METHOD = re.compile(r"^\s*public\s+(?:async\s+)?[\w<>\[\],\s]+?\s+(?P<name>\w+)\s*\((?P<params>[^)]*)\)")
ESCAPES = {"\\": "\\", '"': '"', "'": "'", "0": "\0", "a": "\a", "b": "\b", "f": "\f", "n": "\n", "r": "\r", "t": "\t", "v": "\v"}


class Lexer:
    def __init__(self, text):
        self.s, self.i = text, 0

    def ws(self):
        while self.i < len(self.s) and self.s[self.i].isspace():
            self.i += 1

    def peek(self, n=1):
        self.ws()
        return self.s[self.i:self.i + n]

    def expect(self, tok):
        self.ws()
        if not self.s.startswith(tok, self.i):
            raise ValueError(f"expected {tok!r} at {self.i}: {self.s[self.i:self.i + 30]!r}")
        self.i += len(tok)

    def string(self):
        verbatim = self.s[self.i] == "@"
        self.i += 2 if verbatim else 1
        out = []
        while True:
            c = self.s[self.i]
            if verbatim:
                if c == '"':
                    if self.s[self.i + 1:self.i + 2] == '"':
                        out.append('"')
                        self.i += 2
                        continue
                    self.i += 1
                    return "".join(out)
                out.append(c)
                self.i += 1
                continue
            if c == '"':
                self.i += 1
                return "".join(out)
            if c == "\\":
                e = self.s[self.i + 1]
                if e == "u":
                    out.append(chr(int(self.s[self.i + 2:self.i + 6], 16)))
                    self.i += 6
                    continue
                if e == "x":
                    m = re.match(r"[0-9a-fA-F]{1,4}", self.s[self.i + 2:])
                    out.append(chr(int(m.group(0), 16)))
                    self.i += 2 + len(m.group(0))
                    continue
                out.append(ESCAPES[e])
                self.i += 2
                continue
            out.append(c)
            self.i += 1

    def value(self):
        self.ws()
        c = self.s[self.i]
        if c == '"' or self.s.startswith('@"', self.i):
            return self.string()
        if self.s.startswith("new", self.i):
            return self.array()
        m = re.compile(r"-?\d+(?:\.\d+)?[lLdDfFmM]?").match(self.s, self.i)
        if m:
            self.i = m.end()
            text = m.group(0).rstrip("lLdDfFmM")
            return float(text) if "." in text else int(text)
        m = re.compile(r"[A-Za-z_][\w.]*").match(self.s, self.i)
        if not m:
            raise ValueError(f"unexpected {self.s[self.i:self.i + 30]!r}")
        self.i = m.end()
        word = m.group(0)
        if word in ("true", "false"):
            return word == "true"
        if word == "null":
            return None
        return {"enum": word}

    def array(self):
        self.expect("new")
        m = re.compile(r"\s*[\w.]*\s*\[\s*(\d*)\s*\]").match(self.s, self.i)
        if not m:
            raise ValueError(f"bad array at {self.s[self.i:self.i + 30]!r}")
        self.i = m.end()
        if m.group(1) == "0":
            return []
        self.expect("{")
        items = []
        while self.peek() != "}":
            items.append(self.value())
            if self.peek() == ",":
                self.expect(",")
        self.expect("}")
        return items

    def arguments(self):
        positional, named = [], {}
        while self.i < len(self.s):
            self.ws()
            if self.i >= len(self.s):
                break
            m = re.compile(r"([A-Za-z_]\w*)\s*=(?!=)").match(self.s, self.i)
            if m:
                self.i = m.end()
                named[m.group(1)] = self.value()
            else:
                positional.append(self.value())
            self.ws()
            if self.i < len(self.s):
                self.expect(",")
        return positional, named


def parse_params(text):
    params = []
    for p in filter(None, (x.strip() for x in text.split(","))):
        p = re.sub(r"\s*=.*$", "", p)
        kind, name = p.rsplit(None, 1)
        params.append([kind, name])
    return params


def extract(path):
    methods, pending = {}, []
    with open(path, encoding="utf-8-sig") as f:
        for line in f:
            if line.lstrip().startswith("//"):
                continue
            m = TEST_CASE.match(line)
            if m:
                positional, named = Lexer(m.group("args")).arguments()
                row = {"args": positional}
                if named:
                    row["named"] = named
                pending.append(row)
                continue
            m = METHOD.match(line)
            if m and pending:
                name = m.group("name")
                entry = methods.setdefault(name, {"params": parse_params(m.group("params")), "cases": []})
                entry["cases"].extend(pending)
                pending = []
    return methods


def main():
    checkout, revision, out_dir, *fixtures = sys.argv[1:]
    os.makedirs(out_dir, exist_ok=True)
    total = 0
    for rel in fixtures:
        methods = extract(os.path.join(checkout, rel))
        count = sum(len(m["cases"]) for m in methods.values())
        total += count
        name = os.path.splitext(os.path.basename(rel))[0]
        doc = {"source": rel.replace(os.sep, "/"), "revision": revision, "methods": methods}
        with open(os.path.join(out_dir, f"{name}.json"), "w", encoding="utf-8") as f:
            json.dump(doc, f, ensure_ascii=False, indent=1)
            f.write("\n")
        print(f"{name}: {count} cases in {len(methods)} methods")
    print(f"total: {total}")


if __name__ == "__main__":
    main()
