# dwi: DWARF Inspector

A Rust terminal browser for DWARF debugging information. Explore DIE trees,
inspect attributes, and follow references between entries.

## Install

```sh
cargo install dwi
```

## Run

```sh
dwi /path/to/program
dwi --side-by-side /path/to/program.debug
dwi --kernel
dwi --kernel 6.12.0
dwi --kernel 6.12.0 --module dummy
```

Supply one executable, object, or separate debug file. The first
version targets ELF files on Linux. Parsing uses `object` and `gimli`; the TUI uses
Ratatui and Crossterm. No elfutils or GTK libraries are required.

## Debug file discovery

Embedded DWARF takes priority. Otherwise, `dwi` first uses the input's GNU build ID
to look under `/usr/lib/debug/.build-id/` (for example, build ID `abcdef1234` maps
to `ab/cdef1234.debug`). The candidate must have the same build ID.

If no build-ID match is found, `dwi` follows `.gnu_debuglink`: beside the executable,
in its `.debug/` subdirectory, then under `/usr/lib/debug` mirroring the executable's
absolute directory. Symlinks are resolved before this path-based search. Debuglink
candidates must match the CRC. Missing or identity-mismatched candidates are
skipped. Once an ID or CRC matches, that file is selected: loading errors are
reported immediately rather than searching elsewhere. The canonical
debug-file path appears below the input path. You can also open a separate debug
file directly.

### Supplementary objects

Supplementary objects referenced by `.gnu_debugaltlink` or DWARF 5 `.debug_sup`
are loaded relative to the file containing that link (including a separate debug
file), or using an absolute linked path. GNU links are checked against the
supplement's build ID, with a fallback search under `/usr/lib/debug/.build-id`.
DWARF 5 links are checked against the supplement's `.debug_sup` identifier.
Supplementary discovery also stops at the first matching identity.
The supplementary path appears in the header; a missing or mismatched supplement
is reported while the main file remains browsable.

## Kernels and modules

### Kernel images

On Linux, `--kernel` discovers an ELF `vmlinux` for the running kernel (`uname -r`),
or the release supplied with `--kernel RELEASE`. For the running release, its
build ID from `/sys/kernel/notes` is used when available, including build-ID
debuginfo lookup. Discovery then checks release-specific module and debuginfo
directories under `/lib/modules`, `/usr/lib/modules`, and `/usr/lib/debug`, plus
`/boot/vmlinux-RELEASE` and `/usr/lib/debug/boot/vmlinux-RELEASE`. Candidates must
match the running kernel's build ID when it is known. The canonical image path
appears in the header. Compressed `vmlinuz` boot images need extraction before
they can be opened.

### Modules

`--module NAME` discovers a module for the running kernel; combine it with
`--kernel RELEASE` for an installed release. Lookup uses `modules.dep`, treating
hyphens and underscores as equivalent. If the index has no match, module and
debuginfo trees are searched by filename, including debuginfo-only installations.
Ambiguous matches can be opened by their explicit path. Built-in modules are
identified using `modules.builtin`, with a command to browse their `vmlinux`.
Lookup accepts module names rather than modprobe aliases.

Plain `.ko` files and gzip-, xz-, or zstd-compressed modules are supported, including
as positional file arguments. Their ELF build IDs and debuglinks use the same
debuginfo discovery as executables. The header shows the canonical module path
and its separate debug file when present.

## Browsing

The DIE pane shows `.debug_info` followed by `.debug_types`, preserving file order
within each section. DWARF 5 type units live in `.debug_info`, so they may appear
before compilation units. Expand a unit to browse its DIEs.
Supplementary units follow the main file's units, with `sup:info:` or `sup:types:`
offset labels to distinguish their address space.
Selecting a DIE reads its attributes immediately. Reference attributes start
collapsed; expanding one reads the target's attributes. Reference cycles are
marked and can still be followed with Enter. Sibling references are followable
but do not expand inline.

In the DIE pane, `imported_unit` entries show a blue link to the unit referenced
by `DW_AT_import`. Press Enter to jump to that unit, and Ctrl-o to return to the
import entry.

Location-list attributes expand into address ranges and their expressions. Ranges
are half-open (`[start, end)`); DWARF 5 default locations appear as `default`, and
empty expressions as `<unavailable>`.
`ranges` attributes expand into the same half-open address ranges, without
location expressions. Both list types are decoded on expansion.

Each pane shows a scrollbar on its right border when the expanded tree exceeds
the viewport. The thumb indicates the current viewport's position and size.

### Keys

| Key | Action |
| --- | --- |
| `j` / `k`, arrows | Move selection |
| `h` / Left | Collapse or select parent |
| `l` / Right | Expand or select first child |
| Space | Toggle expansion |
| Enter | Follow an import or DIE reference, or toggle a DIE or list |
| Tab / Shift-Tab | Switch pane |
| `g` / Home, `G` / End | First / last visible row |
| Page Up / Page Down | Page through the active pane |
| `H` / `L` | Scroll horizontally |
| Ctrl-g | Go to a DIE by hexadecimal section offset |
| Ctrl-o / Alt-Left | Jump history: back |
| Ctrl-y / Alt-Right | Jump history: forward |
| `d` | Toggle offsets, attribute forms, and raw values |
| `s` | Toggle stacked / side-by-side panes |
| `f` | Toggle full-screen active pane |
| `?` | Help |
| `q` / Ctrl-c | Quit |

### Layout and history

Panes default to stacked, with DIEs above attributes. Use `--side-by-side` to start
with a horizontal layout; narrow terminals automatically use stacked panes.
History preserves DIE folds, attribute expansion and selection, scrolling, and
pane focus (up to 100 jumps).

### Go to DIE

The Go to DIE dialog prefills `info:0x` or `types:0x` for the selected DIE's
section. Enter a section-relative offset, matching the labels in detail mode.
A bare hexadecimal offset uses the section selected when the dialog opened.
Enter jumps, Esc cancels, and Ctrl-u clears the input; Ctrl-o returns after a
jump. Invalid offsets stay in the dialog with an error.
Supplementary offsets use `sup:info:0x1234` or `sup:types:0x1234`; these prefixes
are prefilled when a supplementary DIE is selected.

## Loading and memory

Sections are decompressed into owned memory. Only visited DIE summaries and child
lists are cached. Finding a previously unseen reference's ancestry scans its unit;
enumerating siblings may scan intervening subtrees when DW_AT_sibling is absent.

### Decompression guard

Compressed sections have a **256× expansion guard**, with a **16 MiB allowance**
for small sections: the declared decoded size must not exceed
`max(16 MiB, compressed payload size × 256)`. This is checked before allocation,
including for supplementary files and compressed `.debug_sup` metadata. Compressed
module files use the same expansion allowance while decoding. Unusual
files can use a higher ratio:

```sh
dwi --max-decompression-ratio 1024 /path/to/program
```

Uncompressed sections have no size cap, and there is no aggregate memory budget.
The decoder still checks output sizes against the compression header. Large
debug files can use substantial memory; lazy section loading remains future work.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Tests generate DWARF fixtures in memory, including cross-unit references, cycles,
type units, both endiannesses, and DWARF 4/5. UI tests exercise layouts without a
real terminal.

For interactive testing, browse a debug build of `dwi` itself:

```sh
cargo build
cargo run -- target/debug/dwi
```

### Current scope

- Lazy DIE tree; attributes are eager for each selected/expanded DIE.
- Cross-unit and type-signature reference navigation.
- Unresolved DIE references display an inline error explaining the failure.
- Imported-unit links with jump history.
- Expandable DWARF 4/5 location and range lists, including indexed DWARF 5 lists.
- DWARF 4/5 units, compressed sections, and object-file relocations.
- GNU build-ID discovery followed by debuglink fallback, validating IDs or CRCs.
- Supplementary strings, DIE references, and imports via GNU debugaltlink and
  DWARF 5 supplementary-file links.
- Readable strings, enumeration constants, and source-file attributes; expressions
  show colored opcode names without the `DW_OP_` prefix, with decoded operands
  (for example, `fbreg -16`).
- DIE names follow abstract-origin and specification references, with linkage
  names as a fallback.
- Attribute forms and raw encodings remain inspectable in detail mode. Long byte
  blocks are previewed up to 256 bytes; expressions up to 32 instructions,
  including nested entry-value expressions.

### Planned work

- Split-DWARF (`.dwo`/`.dwp`) loading.
- Search.
- Synthesized type names.
- Flattened imports.

## License

Licensed under either the [MIT license](LICENSE-MIT) or the
[Apache License, Version 2.0](LICENSE-APACHE), at your option.

## Inspiration

- [dwarvish](https://github.com/cuviper/dwarvish): the original GTK DWARF browser.
- [jless](https://jless.io): folding trees and keyboard navigation.
- [dua](https://github.com/Byron/dua-cli): pane-based terminal browsing.
