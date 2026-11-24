# Escape sequences sent to the PTY

This reference describes sequences that NeoWarp encodes for terminal input. It does not describe every sequence
that an application can write to the terminal or that NeoWarp can parse from the shell.

The escape character `ESC` is decimal 27, or `0x1b`. The encoder uses 7-bit forms such as `ESC [` for CSI and
`ESC O` for SS3. It does not emit the single 8-bit C1 bytes `0x9b` and `0x8f` for these forms.

The terminal mode and keyboard protocol affect the selected sequence. See `escape_sequences.rs` for the encoder and
`escape_sequences_tests.rs` for exact byte assertions.

## C1 reference

| 7-bit form | Control | Meaning |
| --- | --- | --- |
| `ESC D` | IND | Index |
| `ESC E` | NEL | Next line |
| `ESC H` | HTS | Tab set |
| `ESC M` | RI | Reverse index |
| `ESC N` | SS2 | Single shift to G2 for the next character |
| `ESC O` | SS3 | Single shift to G3 for the next character |
| `ESC P` | DCS | Device control string |
| `ESC V` | SPA | Start of guarded area |
| `ESC W` | EPA | End of guarded area |
| `ESC X` | SOS | Start of string |
| `ESC Z` | DECID | Obsolete return terminal ID form |
| `ESC [` | CSI | Control sequence introducer |
| `ESC \` | ST | String terminator |
| `ESC ]` | OSC | Operating system command |
| `ESC ^` | PM | Privacy message |
| `ESC _` | APC | Application program command |

NeoWarp's input encoder primarily uses CSI and SS3. CSI is the default for cursor keys. Unmodified cursor keys use
SS3 when `TermMode::APP_CURSOR` is active. Unmodified F1 through F4 also use SS3, regardless of that mode.

## SGR mouse reports

`MouseState` uses the SGR mouse format:

```text
CSI <button>;<column>;<row>M
CSI <button>;<column>;<row>m
```

The final byte follows the row immediately. There is no semicolon between the row and the final byte.

- `M` represents press, drag, motion, and wheel events.
- `m` represents release.
- Column and row are 1-based coordinates.
- Left, right, left-drag, and motion use button codes 0, 2, 32, and 35.
- Wheel up and wheel down use 64 and 65. The encoder repeats the sequence for the number of scrolled lines.

The current encoder does not add modifier bits to these button codes.

## Cursor keys

Unmodified arrow, Home, and End keys depend on `APP_CURSOR` mode:

| Key | Normal mode | `APP_CURSOR` mode |
| --- | --- | --- |
| Up | `CSI A` | `SS3 A` |
| Down | `CSI B` | `SS3 B` |
| Right | `CSI C` | `SS3 C` |
| Left | `CSI D` | `SS3 D` |
| Home | `CSI H` | `SS3 H` |
| End | `CSI F` | `SS3 F` |

A modified cursor key always uses CSI, even in `APP_CURSOR` mode:

```text
CSI 1;<modifier><key>
```

The `1` is the xterm key number for these cursor keys. `<key>` is the final letter from the table above. For
example, Cmd+Left uses `CSI 1;9D`.

## Function keys

Unmodified F1 through F4 use `SS3 P`, `SS3 Q`, `SS3 R`, and `SS3 S`. Unmodified F5 through F20 use CSI with the
key's numeric code and a final `~`. For example, F5 uses `CSI 15~` and F12 uses `CSI 24~`.

Modified F1 through F4 use `CSI 1;<modifier>P` through `CSI 1;<modifier>S`. Other modified function keys insert the
modifier parameter before `~`. For example, Shift+F5 uses `CSI 15;2~`.

## Modifier parameters

The xterm and Kitty modifier parameter is one plus the modifier bitmask:

| Bit | Modifier |
| --- | --- |
| 1 | Shift |
| 2 | Alt, or Meta when Option as Meta is enabled on macOS |
| 4 | Ctrl |
| 8 | Cmd or Super |

This produces the following values:

| Value | Modifiers |
| --- | --- |
| 1 | None |
| 2 | Shift |
| 3 | Alt |
| 4 | Shift+Alt |
| 5 | Ctrl |
| 6 | Ctrl+Shift |
| 7 | Ctrl+Alt |
| 8 | Ctrl+Shift+Alt |
| 9 | Cmd |
| 10 | Shift+Cmd |
| 11 | Alt+Cmd |
| 12 | Shift+Alt+Cmd |
| 13 | Ctrl+Cmd |
| 14 | Ctrl+Shift+Cmd |
| 15 | Ctrl+Alt+Cmd |
| 16 | Ctrl+Shift+Alt+Cmd |

The modifier value is omitted in some legacy forms when it is 1. Modified cursor and function keys include it.

## Encoding order and other forms

`KeystrokeWithDetails::to_escape_sequence` first checks whether the Kitty keyboard protocol supplies a CSI u
sequence. Its legacy fallback then checks function keys, C0 control codes, cursor keys, Delete, Meta-prefixed keys,
and Backspace. Therefore, special-key sequences do not all start with CSI, and modified keys do not all use the
legacy cursor format.

`to_pty_bytes` adds fallbacks for Ctrl+letter control bytes, printable UTF-8 text, and named control keys. The
module also defines bracketed paste delimiters, focus reports, and tmux DCS passthrough.

## References

- [VT100 user guide](https://vt100.net/docs/vt100-ug/chapter3.html)
- [Xterm control sequences](https://www.xfree86.org/current/ctlseqs.html)
- [ANSI escape code overview](https://en.wikipedia.org/wiki/ANSI_escape_code)
