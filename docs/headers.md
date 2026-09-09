# Headers and masthead studio

`/header mascot` always selects the mascot. Smaller terminals get a compact sprite or a tiny
skull, never a random wordmark. `/header mascotascii` selects the plain ASCII version.
The old names `ainz` and `ainzascii` remain aliases. Both participate in the built-in rotation.

Selecting a header in the full-screen interface previews it immediately without clearing your
conversation. Any key returns to the buffer. The choice is remembered for future empty buffers.
`/header NAME` and `/headers` reload discovery, including files added while Ainz is running.

The standalone files are [`ainz.ans`](../assets/mascot/ainz.ans) and
[`ainz.txt`](../assets/mascot/ainz.txt). Regenerate the color artwork with
`python3 scripts/mascot.py`. It is a hand-drawn terminal interpretation of Ainz Ooal Gown;
the [official character artwork](https://overlord-anime.com/) was the visual reference.

## Create and install a header

Open the [masthead studio](https://wess.io/ainz/masthead/). Nothing needs installing to use the
editor, and drawings are stored in your browser rather than uploaded.

1. Start with **Mascot**, **Wordmark**, or **Blank**, or **Open artwork** to extend a half-block
   ANSI file. Paint, erase, fill, pick colors, or stamp text. Undo and redo include drawing,
   resizing, clearing, and loading another starting image.
2. Check the terminal preview. Two pixel rows make one terminal row. The size hint includes
   the space needed for the roster, header, prompt, and footer.
3. Name the header, choose macOS or Linux, and select **All my projects** or **This project only**.
4. **Copy install command**, then paste it into your shell. For project scope, run it from the
   project folder. The command carries the artwork itself and refuses to replace an existing
   file with the same name. Choose a new name to keep both versions.
5. Copy and run the matching `/header NAME` command inside Ainz to preview and select it.

The editor does not change your machine directly or execute artwork. It generates a shell command
that writes one ANSI file. **Download .ans instead** is available for sharing or manual setup.
An older Ainz build may need one restart to discover newly installed files.

Drafts stay in local browser storage. Undo history lasts for the open page. Download a copy to
keep the art outside that browser. The editor supports keyboard drawing: focus the canvas,
move with arrow keys, and press Space to apply the tool. The pixel canvas cannot edit ordinary
ASCII letters, though Ainz can display ASCII headers.

## Manual installation

Ainz can render user-made ASCII and ANSI artwork on an empty transcript. Paint one in the
[masthead studio](https://wess.io/ainz/masthead/) or bring your own. Put files in either:

- `headers/` under the config directory (`~/Library/Application Support/ainz` on macOS,
  `~/.config/ainz` on Linux) for every workspace
- `.ainz/headers/` in a project for that project and its descendants

Files must use a `.ans`, `.ansi`, or `.txt` extension. The filename becomes the header name, so
`neon-city.ans` is selected with `/header neon-city`. A nearer project definition replaces a user
header with the same name.

Inside Ainz, use:

```text
/headers
/header neon-city
/header random
/header builtin
```

`random` chooses from built-in and custom artwork at startup. `builtin` keeps random selection
inside Ainz's built-in collection. A named choice is remembered in `config.toml` across runs.
Headers appear on an empty transcript or in the selection preview. If custom artwork does not
fit, Ainz uses a responsive built-in header for that render. The bundled `mascot` choice instead
uses a compact or tiny mascot; it never falls back to a wordmark.

## Pixel art

The [masthead studio](https://wess.io/ainz/masthead/) paints on a grid where every pixel is
half a terminal cell, then writes ordinary ANSI: one `▀`, `▄` or `█` per cell, with the top
pixel as the foreground colour and the bottom as the background. Nothing about that encoding is
special to Ainz, so the files open in other ANSI editors and in `less -R`, and the studio
reopens any half-block art, including art it did not write. The bundled studio starter uses the native compact mascot
grid; small pixel drawings are not downsampled from the large version. It stops at anything it cannot place
on the grid, such as shade characters or letters.

## ANSI files

Artwork is UTF-8 text. Plain ASCII works as-is. ANSI files may use Select Graphic Rendition (SGR)
sequences for:

- normal and bright 16-color foreground/background colors
- 256-color sequences such as `ESC[38;5;45m`
- truecolor sequences such as `ESC[38;2;72;205;214m`
- bold, dim, italic, underline, reverse, and strike-through
- `ESC[0m` to reset the style

`ESC` means the byte `0x1b`, not the three visible characters `E`, `S`, `C`. Most ANSI editors
write it automatically. A small shell-generated example is:

```sh
mkdir -p .ainz/headers
printf '\033[38;2;72;205;214;1m  AINZ  \033[0m\n' > .ainz/headers/neon.ans
```

Cursor movement, screen clearing, OSC titles and links, and every other terminal control sequence
are rejected. Ainz converts allowed SGR styles into Ratatui cells; it never writes artwork's raw
escape sequences to the terminal.

## Layout guidelines

- Design for a black background; do not fill every cell with a background color.
- With the roster visible, allow 26 terminal columns beyond the artwork width. A 48-column
  header fits an 80-column terminal; a 72-column header needs at least 98 columns.
- Allow seven terminal rows beyond the artwork height for the header, activity, status, prompt,
  and artwork footer. Multiline prompts need additional rows.
- Use leading spaces to place shapes within the composition. Ainz centers the complete canvas
  using the widest line, preserving the alignment of shorter lines.
- Reset color at the end of each line. Ainz preserves style across lines, but explicit resets make
  the file display correctly in other ANSI viewers.
- Use Unicode block characters (`▀`, `▄`, `█`, `▌`, `▐`) for pixel art, but test ambiguous-width
  symbols in a real terminal.
- Keep text readable without color. This helps limited-color terminals and screenshots.
- Prefer one strong silhouette, two or three depth values, and a small highlight color. Dense random
  color usually loses the ACiD-style sense of material and shadow.

Limits are 128 KiB, 80 lines, and 240 terminal columns. Files must be regular files; symlinks are
ignored. `/headers` reports invalid artwork and its reason, and an unreadable headers directory
is reported the same way instead of stopping the chat.

For a quick preview outside Ainz, use `less -R artwork.ans`. Always open downloaded ANSI files in
a text editor before previewing them in another terminal program; other viewers may execute control
sequences that Ainz intentionally rejects.

UI colors are separate from header artwork. Use the [Theme Designer](https://wess.io/ainz/theme/)
and [theme guide](themes.md) to customize the chat palette.
