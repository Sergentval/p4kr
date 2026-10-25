"""Fail if a tracked text file outside the allowlist contains French (words or accented letters).

A line containing the marker `check-french: allow` is skipped; use it only for
data lines (output bytes, Unicode test inputs), never for comments or messages.
"""
import re
import subprocess
import sys

ALLOW = {
    "p4kr/src/lang.rs",
    "README.fr.md",
    "dist/LISEZMOI.txt",
    "dist/check_french.py",
}
MARKER = "check-french: allow"
SKIP_SUFFIXES = (".p4k", ".json", ".lock", ".svg", ".csv")
TESTDATA_SUFFIXES = (".p4k", ".json", ".toml")
WORDS = re.compile(r"\b(entrées?|fichiers?|dossiers?|clés?|erreurs?|enregistrements?|octets?|chemins?|sorties?|"
                   r"déjà|être|avec|dans|pour|sans(?!-serif)|cette|sont|les|des|une|est|pas|aussi|mais|où)\b", re.IGNORECASE)
ACCENTS = re.compile(r"[éèêëàâçùûîïôœÉÈÊÀÂÇÙÛÎÏÔŒ]")


def offending(path: str, text: str) -> list[str]:
    return [f"{path}:{n}: {line.strip()[:100]}" for n, line in enumerate(text.splitlines(), 1)
            if MARKER not in line and (WORDS.search(line) or ACCENTS.search(line))]


def main() -> int:
    files = subprocess.run(["git", "ls-files"], capture_output=True, text=True, check=True).stdout.split("\n")
    bad = []
    for path in filter(None, files):
        if path in ALLOW or path.endswith(SKIP_SUFFIXES) or (path.startswith("testdata/") and path.endswith(TESTDATA_SUFFIXES)):
            continue
        try:
            with open(path, encoding="utf-8") as handle:
                bad += offending(path, handle.read())
        except UnicodeDecodeError:
            continue
    print("\n".join(bad) or "no French outside the allowlist")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
