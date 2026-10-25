# p4kr

English | [Français](README.fr.md) <!-- check-french: allow -->

A native reader for `.p4k` archives, written in Rust: ZIP64 with the format's own extra blocks, AES-128-CBC, zstd, CRC32C and sha256 checks. It lists, extracts and verifies archives, and converts binary XML (CryXmlB) and DataForge databases (`.dcb`) to text XML.

## What it does

- Lists, extracts and verifies the entries of a `.p4k` archive (CRC32C and sha256), with parallel extraction.
- With `--xml`, converts CryXmlB files and DataForge databases to text XML, byte for byte the same as unp4k and unforge (see [Line endings](#line-endings)).
- Opens an archive in about half a second, much faster than the usual tools (see [Benchmarks](#benchmarks)).
- Has a guided mode for people who do not want to type options.

| crate | role |
|---|---|
| `p4kr-core` | central directory, entry reading, key file, filters, parallel extraction |
| `p4kr-cryxml` | CryXmlB to text XML, identical to unp4k + unforge |
| `p4kr-xmlwrite` | XML tree and indented writer shared by the `DocumentSave` and `DataForge` profiles |
| `p4kr-dataforge` | DataForge database (`.dcb`, version 8) to one XML file per record, identical to unforge |
| `p4kr` | the CLI: `list`, `extract`, `verify`, `cat`, `xml`, `dcb` (exit codes: 0 ok, 1 entries in error, 2 configuration, 3 cannot open) |

## Download

Get the archive for your system from the [Releases](../../releases) page (Windows x64, Linux x64, macOS arm64 and x64), together with `SHA256SUMS`. Check it before unpacking, for example `sha256sum -c SHA256SUMS` on Linux, `shasum -a 256 -c SHA256SUMS` on macOS, or `Get-FileHash <archive>` in PowerShell. Each archive holds the binary, `README.txt` (English), `LISEZMOI.txt` (French) and both licenses.

p4kr is not code-signed:

- Windows: SmartScreen may show "Windows protected your PC". Click "More info", then "Run anyway".
- macOS: right-click `p4kr`, then Open; or run `xattr -d com.apple.quarantine p4kr`. On macOS 15 and later, if it is still blocked, use System Settings -> Privacy & Security -> "Open Anyway".
- Linux: the binary needs glibc 2.35 or later (Ubuntu 22.04+, Debian 12+, Fedora 36+). Alpine is not supported. Run `chmod +x p4kr` once.

## Guided mode

Run `p4kr` with no argument in a terminal, or `p4kr guide`, for a step-by-step mode. Three choices: 1) DataForge database to XML, 2) targeted extraction (one folder or one extension of the archive), 3) verify the archive without writing anything.

The guide asks for the archive, then the key file (empty or `-` = no key; `-` also clears the remembered key), then the output folder (by default `<remembered folder>/dataforge-YYYY-MM-DD` or `extraction-YYYY-MM-DD`, UTC date). In choice 2 it asks for a folder (`f`) or an extension (`e`), then whether to convert binary XML to text XML. It summarizes what it will do and asks for confirmation before starting. At the end it prints `Done in <duration>` (for example `9.4 s`, `2 min 44 s`, `1 h 05 min`) and, if input and output are terminals, offers to open the output folder (choices 1 and 2; file manager, `open` or `xdg-open` depending on the system; end of input means "no"). Prompts read like `Prompt [default]: `. Paths dragged into the terminal (escaped spaces or quotes) are understood. End of input (Ctrl+D, Ctrl+Z, closed pipe) ends the guide cleanly with exit code 0.

- Language: the guided mode follows the system language (French or English); `P4KR_LANG=en` or `P4KR_LANG=fr` forces one. The command-line output is always English.
- Remembered settings: the archive, the key file and the output folder (never the key itself), in `p4kr.toml` under `%APPDATA%\p4kr` (Windows), `~/Library/Application Support/p4kr` (macOS) or `$XDG_CONFIG_HOME/p4kr`, else `~/.config/p4kr` (Linux). `P4KR_CONFIG_DIR=<folder>` overrides this location. A damaged file or a remembered path that no longer exists gives a warning, not an error.

## Commands

On Windows use `p4kr.exe`; the examples use the Unix form.

```bash
p4kr list ARCHIVE [selection]
p4kr extract ARCHIVE --out DIR [selection] [--xml] [--verify] [--fail-fast] [--key-file K] [--threads N] [--quiet]
p4kr verify ARCHIVE [selection] [--key-file K] [--threads N] [--quiet]
p4kr cat ARCHIVE --exact PATH [--xml] [--key-file K]
p4kr xml INPUT OUTPUT
p4kr dcb FILE.dcb --out DIR [--threads N] [--quiet]
```

- `extract --xml` converts binary XML (CryXmlB) in memory between reading and writing; the binary is written as is and the `.xml` next to it, like unp4k then unforge. A failed conversion is counted in the report (exit code 1).
- `extract --verify` also checks the sha256 of the stored bytes (CRC32C is always checked); `--fail-fast` stops at the first entry in error. `--threads N` (extract, verify, dcb; 0 = all cores, the default) sets the number of threads.
- `cat` writes one entry to standard output (converted with `--xml`), without creating a file.
- `xml` converts a local binary XML file.
- `dcb` writes one XML file per record under `DIR`, at the record's path, byte for byte like unforge. Exit code 1 if the file is refused or a record fails.
- `extract --xml` on a `.dcb` entry (extension exactly `.dcb`): the entry is written raw, then every record is written under the `.dcb`'s folder.
- A `.dcb` file with an unknown format version (only version 8 is supported) or an inconsistent file (sizes, end of file, schema) is refused entirely, with a message saying what and where, and no XML is written. An absolute record path, one containing `..`, or an empty one refuses only that record.
- Progress bar of `extract` and `verify`: on standard error, only if it is a terminal; `--quiet` removes it.

## Selecting entries

`list`, `extract` and `verify` accept `--exact PATH`, `--prefix FOLDER` and `--ext .EXT`, each repeatable: OR between the values of one option (`--ext .xml --ext .ini`), AND between `--prefix` and `--ext` (`--prefix Data/Libs --ext .xml` = the `.xml` files under `Data/Libs`). `--exact` cannot be combined with the other two. With no option: all entries.

## Line endings

Converted XML files are written with LF line endings on every system. They are byte-for-byte identical to unforge run on Linux or macOS; unforge on Windows writes CRLF, the only difference (same XML tree, the XML standard normalizes line endings). DataForge records are identical in both cases.

## DataForge record vs entry collision

When a DataForge record has the same path as an archive entry already extracted (the raw file or the XML written next to it), the record takes that path and the entry is kept beside it as `<name>.p4k-entry.<extension>` (`<name>.p4k-entry` with no extension). p4kr prints a warning on standard error; the exit code is unchanged (0). These remain errors, with nothing moved: a collision with a `.dcb`, between two records, between a file and a folder, or a side name already taken.

## Key

p4kr ships no key. If your archive needs one, create a text file (for example `key.toml`) with a single line:

```toml
aes_key = "<32 hexadecimal characters>"
```

and pass it with `--key-file key.toml`, or give its path to the guide. Keep that file to yourself.

## Free space

Before an extraction, p4kr requires 20 GiB free on the output disk. `P4KR_MIN_FREE_GB=<n>` replaces that threshold; it is counted in GiB (smaller disks, tests).

## Build

```bash
cargo build --release --locked
```

The toolchain is pinned in `rust-toolchain.toml`. No key is provided: the CLI reads a local key file passed as an argument.

## Test

`cargo test --release` (timing tests need `--release`). The tests read `testdata/`, generated by the maintainers' test-fixture tool; regenerate nothing by hand. A local run with less than 20 GiB free needs `P4KR_MIN_FREE_GB`.

## Benchmarks

![Opening a large archive and extracting: p4kr vs unp4k and StarBreaker](docs/benchmarks/open-extract.svg)

- Correctness: 1,379,308 / 1,379,308 entries identical to the output of unp4k.
- Opening a 1,379,308-entry archive (warm cache, wall-clock): 0.55 s against 7.29 s for unp4k, 13.2 times faster.
- Details and method: [docs/benchmarks.md](docs/benchmarks.md).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

## Disclaimer

Not affiliated with or endorsed by Cloud Imperium Games. Star Citizen® is a trademark of Cloud Imperium Rights LLC. p4kr ships no game data and no key.
