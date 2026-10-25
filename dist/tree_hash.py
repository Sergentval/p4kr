"""Hash of an output tree: one `sha256  relative/path` line per file, `/` separators, sorted.
Identical on Windows, Linux and macOS; used to compare the outputs of the 4 release binaries."""
import hashlib
import sys
from pathlib import Path


def main(root: str) -> int:
    base = Path(root)
    rows = sorted((p.relative_to(base).as_posix(), hashlib.sha256(p.read_bytes()).hexdigest())
                  for p in base.rglob("*") if p.is_file())
    # Raw bytes: Windows text mode would turn `\n` into `\r\n` and skew the comparison.
    sys.stdout.buffer.write("".join(f"{digest}  {rel}\n" for rel, digest in rows).encode("utf-8"))
    return 0 if rows else 1


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.stderr.write("usage: tree_hash.py FOLDER\n")
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
