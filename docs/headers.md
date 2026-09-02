# Custom headers

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
Headers appear only on an empty transcript. If the selected artwork does not fit the current
terminal width and height, Ainz uses a responsive built-in header for that render.

## Pixel art

The [masthead studio](https://wess.io/ainz/masthead/) paints on a grid where every pixel is
half a terminal cell, then writes ordinary ANSI: one `▀`, `▄` or `█` per cell, with the top
pixel as the foreground colour and the bottom as the background. Nothing about that encoding is
special to Ainz, so the files open in other ANSI editors and in `less -R`, and the studio
reopens any half-block art, including art it did not write. It stops at anything it cannot place
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
- Keep the main version at or below 72 columns so it also fits beside the agent roster.
- Keep it near 12–20 lines tall; leave three rows for Ainz's footer and shortcuts.
- Use spaces inside the composition instead of leading spaces for centering. Ainz centers each
  line from its measured terminal-cell width.
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
