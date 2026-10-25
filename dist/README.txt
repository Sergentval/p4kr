p4kr - .p4k archive reader
==========================

p4kr reads a .p4k data archive (ZIP64 with AES encryption and zstd
compression), lists its contents, extracts files and checks their
integrity. With the --xml option, binary XML files and the DataForge
database (.dcb) are converted to text XML, byte for byte like unp4k and
unforge on Linux or macOS. Line endings are always LF: unforge on Windows
writes CRLF, the only difference (same XML content).

p4kr is a command-line program: it has no window. Open a terminal
(PowerShell on Windows, Terminal on macOS and Linux) in the folder that
contains p4kr.

In that folder the command is ./p4kr on macOS and Linux, and .\p4kr.exe in
PowerShell. The examples below use ./p4kr; on Windows, replace it
everywhere with .\p4kr.exe.

Language: the guided mode follows the system language (French or English);
to force one, set P4KR_LANG=en or P4KR_LANG=fr. Command-line output is always
in English. LISEZMOI.txt is the French version of this file.


1. GUIDED MODE (the easiest)
----------------------------

Run ./p4kr with no argument in a terminal, or:

    ./p4kr guide

(On Windows, double-clicking p4kr.exe also opens a console.)

A menu offers three choices:

    1) DataForge database -> XML
    2) Targeted extraction
    3) Verify the archive
    q) Quit

The guide then asks for:

  - Archive (.p4k): the path of the archive. You can drag the file into
    the terminal; escaped spaces and quotes are understood.
  - Key file: the key file (see below). Empty answer or "-" = no key. If a
    key is already remembered, it is offered by default; "-" always means
    "no key" and clears the remembered key.
  - Choice 2 only: a folder (f) or an extension (e) of the archive, then
    whether binary XML files should be converted to text XML.
  - Output folder (choices 1 and 2 only): offered under the remembered
    folder, with today's date (UTC), for example dataforge-YYYY-MM-DD or
    extraction-YYYY-MM-DD.

The guide summarizes what it will do and asks for confirmation before
starting. When the extraction is done, it prints "Done in <duration>" and,
in a terminal, offers to open the output folder (choices 1 and 2).
It remembers the archive, the key file and the output folder (never the
content of the key) in a p4kr.toml file:

  Windows : %APPDATA%\p4kr
  macOS   : ~/Library/Application Support/p4kr
  Linux   : $XDG_CONFIG_HOME/p4kr, otherwise ~/.config/p4kr

The variable P4KR_CONFIG_DIR=<folder> overrides this location. Ctrl+D
(Ctrl+Z then Enter on Windows) ends the guide cleanly.


2. COMMANDS
-----------

List entries (--prefix, --ext or --exact to select):

    ./p4kr list Data.p4k --ext .dcb

Each option can be repeated: OR between its values, AND between --prefix
and --ext.

    ./p4kr list Data.p4k --ext .xml --ext .ini --ext .dcb
    ./p4kr list Data.p4k --prefix Data/Libs --ext .xml

Extract to a folder, with XML conversion:

    ./p4kr extract Data.p4k --out o --xml --key-file k.toml
    ./p4kr extract Data.p4k --out o --prefix Data/Libs --xml --key-file k

Verify (CRC32C and sha256), writing nothing:

    ./p4kr verify Data.p4k --key-file key.toml

Convert a local DataForge database to one XML file per record:

    ./p4kr dcb Game.dcb --out output

Write a single entry to standard output:

    ./p4kr cat Data.p4k --exact Data/a.xml --xml --key-file k.toml > a.xml

Convert a local binary XML file:

    ./p4kr xml input.xml output.xml

Useful options: --quiet (no progress bar), --threads N (0 = all cores;
extract, verify, dcb), --verify (extract: also check the sha256),
--fail-fast (extract: stop at the first entry in error).
Exit codes: 0 all good, 1 some entries are in error, 2 configuration,
3 cannot open.

When a DataForge record has the same path as an archive entry, the record
is written there and the entry is kept next to it as
<name>.p4k-entry.<extension>, with a warning (exit code unchanged).


3. KEY FILE
-----------

No key is provided with p4kr. If your archive needs one, create a text file
(for example key.toml) containing a single line:

    aes_key = "<32 hexadecimal characters>"

then pass it with --key-file key.toml, or give its path to the guide. Keep
that file to yourself.


4. FREE SPACE
-------------

Before an extraction, p4kr requires 20 GiB free on the output disk. To change
this threshold (in GiB), set the variable P4KR_MIN_FREE_GB:

    Linux, macOS :   P4KR_MIN_FREE_GB=5 ./p4kr extract ...
    PowerShell   :   $env:P4KR_MIN_FREE_GB = "5"


5. FIRST RUN ON EACH SYSTEM
---------------------------

First check the downloaded archive against the SHA256SUMS file published
next to it (for example "sha256sum -c SHA256SUMS" on Linux, "shasum -a 256
-c SHA256SUMS" on macOS).

Windows: check the archive in PowerShell, then compare with SHA256SUMS:

    Get-FileHash p4kr-<version>-windows-x64.zip

p4kr is not signed. On first run, SmartScreen may show "Windows protected
your PC". Click "More info", then "Run anyway".

macOS: p4kr is not signed. The first time, right-click p4kr, then Open. Or,
in the terminal, remove the quarantine flag:

    xattr -d com.apple.quarantine p4kr

On macOS 15 and later, if it is still blocked: System Settings -> Privacy &
Security, then "Open Anyway" next to the message about p4kr.

Linux: the binary needs glibc 2.35 or later (Ubuntu 22.04 and later, Debian
12 and later, Fedora 36 and later). Alpine is not supported. Make the file
executable once:

    chmod +x p4kr


6. LICENSE
----------

p4kr is dual-licensed under MIT or Apache-2.0, at your option: see
LICENSE-MIT and LICENSE-APACHE.

Not affiliated with or endorsed by Cloud Imperium Games. Star Citizen is a
trademark of Cloud Imperium Rights LLC. p4kr ships no game data and no key.
