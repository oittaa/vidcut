#!/usr/bin/env python3
"""Writes crates/transcribe-cpp-sys/src/transcribe_sys.rs from the
bindings the transcribe-cpp-sys crate ships, so that each C function is
called through the library vidcut loads at run time
(transcribe_cpp_sys::load) instead of one linked at build time.

    python3 scripts/transcribe-sys.py BINDINGS

BINDINGS is transcribe-cpp-sys's bindings/rust/sys/src/transcribe_sys.rs,
for example in ~/.cargo/registry/src/*/transcribe-cpp-sys-0.4.0/. The
types and constants are kept as they are, comments left out. Each
`extern "C"` function becomes a function of the same signature that calls
the symbol of that name from the loaded library (Api), and panics naming
it if the library has none.
"""

import re
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "crates" / "transcribe-cpp-sys" / "src" / "transcribe_sys.rs"
BLOCK = re.compile(r'unsafe extern "C" \{\s*pub fn (\w+)\((.*?)\)\s*(?:->\s*([^;]+?))?\s*;\s*\}', re.S)


def split_args(text):
    args, depth, cur = [], 0, ""
    for ch in text:
        if ch in "(<[":
            depth += 1
        elif ch in ")>]":
            depth -= 1
        if ch == "," and depth == 0:
            args.append(cur)
            cur = ""
        else:
            cur += ch
    args.append(cur)
    return [" ".join(a.split()) for a in args if a.strip()]


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    source = Path(sys.argv[1]).read_text(encoding="utf-8")
    source = "\n".join(
        line for line in source.splitlines() if not line.lstrip().startswith(("//", "/*"))
    )
    functions = []

    def wrapper(m):
        name, args, ret = m.group(1), split_args(m.group(2)), m.group(3)
        ret = f" -> {' '.join(ret.split())}" if ret else ""
        names = ", ".join(a.split(":", 1)[0].strip() for a in args)
        functions.append((name, ", ".join(args), ret))
        return (
            f"pub unsafe fn {name}({', '.join(args)}){ret} {{\n"
            f"    unsafe {{ (crate::api().{name}.expect(\"{name}\"))({names}) }}\n"
            f"}}"
        )

    body = BLOCK.sub(wrapper, source)
    if re.search(r'extern "C" \{', body):
        sys.exit("an extern block this script doesn't understand is left")
    fields = "\n".join(f"    {n}: Option<unsafe extern \"C\" fn({a}){r}>," for n, a, r in functions)
    resolved = "\n".join(f"                {n}: lib.get(c\"{n}\").ok().map(|s| *s)," for n, _, _ in functions)
    body += (
        f"\n\npub struct Api {{\n{fields}\n}}\n\n"
        "impl Api {\n"
        "    pub unsafe fn resolve(lib: &libloading::Library) -> Self {\n"
        "        unsafe {\n"
        f"            Self {{\n{resolved}\n            }}\n"
        "        }\n"
        "    }\n"
        "}\n"
    )
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(body.encode("utf-8"))
    print(f"{OUT}: {len(functions)} functions")


if __name__ == "__main__":
    main()
