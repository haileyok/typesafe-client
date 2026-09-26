#!/usr/bin/env python3
"""Compile every complete program in the READMEs.

Go: every ```go block that starts with `package main`.
Rust: every ```rust block that defines `fn main` (rustdoc-hidden `# ` lines are
un-hidden first).

Each program is built in a throwaway project, the way a user would paste it:
against the local source by default (CI), or with --published against the
released packages from proxy.golang.org and crates.io.

Usage: scripts/check_readme_examples.py [--published]
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
READMES = [REPO / "README.md", REPO / "go" / "README.md", REPO / "rust" / "README.md"]
GO_MODULE = "github.com/haileyok/typesafe-client/go"
CRATE = "typesafe-system-one"

FENCE = re.compile(r"^```([A-Za-z0-9_,+-]*)\s*$")


def blocks(path):
    """Yield (lang, first_line_number, code) for each fenced block."""
    lines = path.read_text().splitlines()
    i = 0
    while i < len(lines):
        m = FENCE.match(lines[i])
        if m and m.group(1):
            lang, start, body = m.group(1).split(",")[0], i + 2, []
            i += 1
            while i < len(lines) and not lines[i].startswith("```"):
                body.append(lines[i])
                i += 1
            yield lang, start, "\n".join(body) + "\n"
        i += 1


def run(cmd, cwd, env=None):
    result = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True)
    return result.returncode, (result.stdout + result.stderr).strip()


def unhide_rustdoc(code):
    out = []
    for line in code.splitlines():
        if line == "#":
            out.append("")
        elif line.startswith("# "):
            out.append(line[2:])
        else:
            out.append(line)
    return "\n".join(out) + "\n"


def check_go(programs, published, work):
    failures = 0
    for where, code in programs:
        d = work / f"go-{len(os.listdir(work))}"
        d.mkdir()
        (d / "main.go").write_text(code)
        run(["go", "mod", "init", "example.com/readme"], d)
        if published:
            env = dict(os.environ, GOPROXY="https://proxy.golang.org", GOFLAGS="-mod=mod")
            code_, out = run(["go", "get", f"{GO_MODULE}@latest"], d, env)
        else:
            run(["go", "mod", "edit", f"-require={GO_MODULE}@v0.0.0", f"-replace={GO_MODULE}={REPO / 'go'}"], d)
            code_, out = 0, ""
        steps = [["gofmt", "-l", "main.go"], ["go", "vet", "."], ["go", "build", "-o", os.devnull, "."]]
        for step in steps:
            if code_ != 0:
                break
            code_, out = run(step, d)
            if step[0] == "gofmt" and out:
                code_, out = 1, "not gofmt-formatted:\n" + run(["gofmt", "-d", "main.go"], d)[1]
        status = "ok" if code_ == 0 else "FAIL"
        print(f"  [{status}] go    {where}")
        if code_ != 0:
            failures += 1
            print("        " + out.replace("\n", "\n        "))
    return failures


def check_rust(programs, published, work):
    if not programs:
        return 0
    d = work / "rust"
    d.mkdir()
    (d / "src").mkdir()
    dep = '"0.1"' if published else f'{{ path = "{REPO / "rust"}" }}'
    (d / "Cargo.toml").write_text(
        "[package]\nname = \"readme-example\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n"
        f"[dependencies]\n{CRATE} = {dep}\n"
        'tokio = { version = "1", features = ["macros", "rt-multi-thread"] }\n'
        'serde_json = "1"\n\n[workspace]\n'
    )
    env = dict(os.environ)
    # Reuse one target dir across programs (and CI runs) so only the first
    # build compiles dependencies.
    env.setdefault("CARGO_TARGET_DIR", str(REPO / "rust" / "target" / "readme-examples"))
    failures = 0
    for where, code in programs:
        (d / "src" / "main.rs").write_text(unhide_rustdoc(code))
        code_, out = run(["cargo", "build", "--quiet"], d, env)
        status = "ok" if code_ == 0 else "FAIL"
        print(f"  [{status}] rust  {where}")
        if code_ != 0:
            failures += 1
            print("        " + out.replace("\n", "\n        "))
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--published", action="store_true", help="build against the released packages")
    args = parser.parse_args()

    go_programs, rust_programs = [], []
    for readme in READMES:
        rel = readme.relative_to(REPO)
        for lang, line, code in blocks(readme):
            where = f"{rel}:{line}"
            if lang == "go" and code.lstrip().startswith("package main"):
                go_programs.append((where, code))
            elif lang == "rust" and re.search(r"^\s*(async\s+)?fn main\s*\(", unhide_rustdoc(code), re.M):
                rust_programs.append((where, code))

    source = "published packages" if args.published else "local source"
    print(f"Compiling {len(go_programs)} Go and {len(rust_programs)} Rust README programs against the {source}:")
    if not go_programs and not rust_programs:
        print("No README programs found; the extraction patterns are probably broken.")
        return 1

    work = Path(tempfile.mkdtemp(prefix="readme-examples-"))
    try:
        failures = check_go(go_programs, args.published, work) + check_rust(rust_programs, args.published, work)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    if failures:
        print(f"{failures} README program(s) failed to compile.")
        return 1
    print("All README programs compile.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
