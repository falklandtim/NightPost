#!/usr/bin/env bash
# The code audit's numbers (see docs/code-audit.md for the process).
#
#   scripts/audit.sh           tests, clippy and rustdoc, then the code checks
#   scripts/audit.sh --quick   only the code checks (no build; a few seconds)
#
# Prints a summary to compare with the last audit's line in
# docs/code-audit.md. It never changes anything.
set -uo pipefail
cd "$(dirname "$0")/.."

quick=0
[[ "${1:-}" == "--quick" ]] && quick=1

tests="-" ; clippy="-" ; rustdoc="-" ; advisories="-" ; future="-" ; updates="-"
if [[ $quick == 0 ]]; then
    echo "== cargo test --workspace"
    out=$(cargo test --workspace 2>&1)
    passed=$(grep -E "^test result:" <<<"$out" | awk '{p+=$4; f+=$6} END {print p+0, f+0}')
    tests="${passed% *} passed, ${passed#* } failed"
    grep -E "^test result:|FAILED|panicked" <<<"$out"
    echo
    echo "== cargo clippy --workspace --all-targets"
    out=$(cargo clippy --workspace --all-targets 2>&1)
    # Real warnings only: not the "generated N warnings" summaries or the
    # future-incompatibility note about dependencies.
    clippy=$(grep -E "^(warning|error)" <<<"$out" | grep -vE "generated [0-9]+ warning|future version of Rust" | wc -l | tr -d ' ')
    grep -A1 -E "^(warning|error)" <<<"$out" | grep -vE "generated [0-9]+ warning|future version of Rust|^--$" | sed 's/^/  /'
    echo
    echo "== cargo doc --workspace --no-deps"
    out=$(cargo doc --workspace --no-deps 2>&1)
    rustdoc=$(grep -E "^warning" <<<"$out" | grep -vE "generated [0-9]+ warning|future version of Rust" | wc -l | tr -d ' ')
    grep -A1 -E "^warning" <<<"$out" | grep -vE "generated [0-9]+ warning|future version of Rust|^--$" | sed 's/^/  /'
    echo
    # Dependencies (AGENTS.md section 6). These need the network.
    echo "== cargo audit (security advisories)"
    if cargo audit --version >/dev/null 2>&1; then
        out=$(cargo audit 2>&1)
        # Vulnerabilities count, plus warnings such as "unmaintained".
        advisories=$(grep -E "^Crate:" <<<"$out" | wc -l | tr -d ' ')
        grep -E "^(Crate|Version|Title|ID|Warning):" <<<"$out" | sed 's/^/  /'
        [[ "$advisories" == 0 ]] && echo "  none"
    else
        advisories="not installed"
        echo "  cargo-audit isn't installed: cargo install cargo-audit"
    fi
    echo
    echo "== Future-incompatibility warnings"
    out=$(cargo report future-incompatibilities 2>&1)
    # "The package `imap-proto v0.10.2` currently triggers ..."
    future=$(grep -oE "package \`[a-z0-9_-]+ v[^\`]+\`" <<<"$out" | sort -u | wc -l | tr -d ' ')
    grep -oE "package \`[a-z0-9_-]+ v[^\`]+\`" <<<"$out" | sort -u | sed 's/^package /  /'
    [[ "$future" == 0 ]] && echo "  none"
    echo
    echo "== Compatible updates waiting (cargo update --dry-run)"
    out=$(cargo update --dry-run 2>&1)
    updates=$(grep -cE "^\s+Updating [a-z0-9_-]+ v" <<<"$out" | tr -d ' ')
    grep -E "^\s+Updating [a-z0-9_-]+ v" <<<"$out" | sed 's/^ */  /'
    echo
fi

TESTS="$tests" CLIPPY="$clippy" RUSTDOC="$rustdoc" ADVISORIES="$advisories" FUTURE="$future" UPDATES="$updates" python3 - <<'PY'
import os, re, subprocess

FILE_LIMIT, FN_LIMIT = 1500, 100
# Tracked and new (untracked) files that exist now.
listed = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "crates/*.rs"]).decode().split()
files = sorted({f for f in listed if os.path.exists(f)})

def code_part(lines, path):
    """The lines before the file's test module (none for a test-only file)."""
    if path.endswith("tests.rs") or "/tests/" in path:
        return []
    for i, l in enumerate(lines):
        if l.startswith("#[cfg(test)]"):
            return lines[:i]
    return lines

item = re.compile(r"\s*(pub(\([a-z]+\))?\s+)?(async\s+)?(unsafe\s+)?(fn|struct|enum|trait|const|static|type|mod)\s+(\w+)")
pub_total = undocumented = 0
per_file_undoc = []
no_header = []
big_files = []
long_fns = []
unsafe_total = unsafe_bare = 0
unsafe_where = []
panics = []
todos = 0

for f in files:
    text = open(f, encoding="utf-8").read()
    all_lines = text.split("\n")
    lines = code_part(all_lines, f)
    if not any(l.startswith("//!") for l in all_lines[:15]):
        no_header.append(f)
    if len(all_lines) > FILE_LIMIT:
        big_files.append((len(all_lines), f))
    undoc_here = pub_here = 0
    for i, l in enumerate(lines):
        m = item.match(l)
        if m:
            if m.group(1) and m.group(5) != "mod" and not m.group(2):
                pub_here += 1
                j = i - 1
                while j >= 0 and lines[j].strip().startswith("#["):
                    j -= 1
                if j < 0 or not lines[j].strip().startswith("///"):
                    undoc_here += 1
            if m.group(5) == "fn" and not l.rstrip().endswith(";"):
                indent = len(l) - len(l.lstrip())
                for k in range(i + 1, len(lines)):
                    s = lines[k]
                    if s.startswith(" " * indent + "}") and len(s) - len(s.lstrip()) == indent:
                        if k - i + 1 > FN_LIMIT:
                            long_fns.append((k - i + 1, f"{f}:{i + 1}", m.group(6)))
                        break
        code = l.split("//")[0]
        if re.search(r"\bunsafe\s*(\{|fn\b|impl\b)", code):
            unsafe_total += 1
            near = "\n".join(lines[max(0, i - 4): i + 1])
            if "SAFETY" not in near:
                unsafe_bare += 1
                unsafe_where.append(f"{f}:{i + 1}")
        if re.search(r"\.unwrap\(\)|\.expect\(|\bpanic!\(|\bunreachable!\(|\btodo!\(|\bunimplemented!\(", code):
            panics.append(f"{f}:{i + 1}")
        if re.search(r"\b(TODO|FIXME|XXX|HACK)\b", l):
            todos += 1
    pub_total += pub_here
    undocumented += undoc_here
    if undoc_here:
        per_file_undoc.append((undoc_here, pub_here, f))

def section(title, rows):
    print(f"== {title}")
    for r in rows:
        print(f"  {r}")
    if not rows:
        print("  none")
    print()

documented = pub_total - undocumented
pct = round(100 * documented / pub_total) if pub_total else 100
section(f"Public items without a doc comment ({undocumented} of {pub_total}), worst files",
        [f"{u:3}/{p:<3} {f}" for u, p, f in sorted(per_file_undoc, reverse=True)[:10]])
section("Files without a //! summary", no_header)
section(f"Files over {FILE_LIMIT} lines", [f"{n:5}  {f}" for n, f in sorted(big_files, reverse=True)])
section(f"Functions over {FN_LIMIT} lines ({len(long_fns)})", [f"{n:4}  {w}  {name}" for n, w, name in sorted(long_fns, reverse=True)])
section(f"unsafe without a SAFETY comment ({unsafe_bare} of {unsafe_total})", unsafe_where)
by_file = {}
for p in panics:
    by_file.setdefault(p.rsplit(":", 1)[0], 0)
    by_file[p.rsplit(":", 1)[0]] += 1
section(f"unwrap/expect/panic outside tests ({len(panics)})", [f"{n:3}  {f}" for f, n in sorted(by_file.items(), key=lambda x: -x[1])])

total_lines = sum(len(open(f, encoding="utf-8").read().split("\n")) for f in files)
summary = [
    ("Rust lines", total_lines),
    ("Tests", os.environ["TESTS"]),
    ("Clippy warnings", os.environ["CLIPPY"]),
    ("Rustdoc warnings", os.environ["RUSTDOC"]),
    ("Public items documented", f"{documented}/{pub_total} ({pct}%)"),
    ("Files without a summary", len(no_header)),
    (f"Files over {FILE_LIMIT} lines", len(big_files)),
    (f"Functions over {FN_LIMIT} lines", len(long_fns)),
    ("unsafe without SAFETY", f"{unsafe_bare} of {unsafe_total}"),
    ("unwrap/expect/panic outside tests", len(panics)),
    ("TODO/FIXME", todos),
    ("Security advisories (cargo audit)", os.environ["ADVISORIES"]),
    ("Crates with future-incompat warnings", os.environ["FUTURE"]),
    ("Compatible updates waiting", os.environ["UPDATES"]),
]
print("== Summary")
for k, v in summary:
    print(f"  {k:<36} {v}")
print()
PY

# For the README check: what changed since the last audit recorded in
# docs/code-audit.md (its newest "### YYYY-MM-DD" heading).
last=$(grep -oE "^### [0-9]{4}-[0-9]{2}-[0-9]{2}" docs/code-audit.md 2>/dev/null | tail -1 | cut -c5-)
if [[ -n "$last" ]]; then
    echo "== Commits since the last audit ($last): is each in the README?"
    git log --since="$last" --format="  %h %s"
fi
