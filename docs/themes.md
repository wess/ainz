# Themes

Named themes change the colors of Ainz's chat interface: buffer text, topic and status bars,
roster, prompt, and dialogs. Header artwork keeps its own palette. Themes are small TOML files;
loading one does not run code or start another process.

## Design and install

1. Open the [Theme Designer](https://wess.io/ainz/theme/).
2. Choose Classic, Nazarick, or Paper, or open a `.toml` file previously exported by the designer.
3. Change colors while watching the buffer preview. Contrast ratios cover text, muted text,
   and bar labels; a terminal-default background is previewed on `#10171a`.
4. Name the theme and choose macOS or Linux, then all projects or the current project.
5. Copy the installation command and run it in your shell. For a project theme, run it from
   that project's directory. The command writes one file, refuses to overwrite existing files
   or symlinks, and downloads nothing.
6. Inside Ainz, run the displayed `/theme NAME` command.

The designer saves a draft in browser storage. Undo restores earlier colors; Download and Copy
TOML let you save or share the file. Its importer accepts a flat `[colors]` table with quoted
hex colors and comments. Ainz itself accepts the full TOML syntax for that table.

Use a build that includes `/theme`; see [source installation](install.md) if your installed
release does not recognize the command. Installing a theme file does not update the executable.

## Switch and reload

```text
/themes           list themes, file locations, and invalid-file errors
/theme            show the current theme
/theme mytheme    discover files, apply mytheme, and remember the choice
/theme default    restore Ainz's built-in palette
```

Edit a file and run `/theme NAME` again to reload it. A newly installed file is available without
restarting. The selection is stored as `theme = "mytheme"` under `[ui]` in Ainz's configuration.
If it is unavailable at startup, Ainz reports that and uses the default palette.

## Paths

| Scope | Directory |
| --- | --- |
| macOS user | `~/Library/Application Support/ainz/themes/` |
| Linux user | `$XDG_CONFIG_HOME/ainz/themes/`, or `~/.config/ainz/themes/` |
| Project | `.ainz/themes/` in the workspace or its ancestors |

The nearest project definition wins over an ancestor or user theme of the same name. Files must
be regular `.toml` files, at most 16 KiB; symlinks are skipped. The filename without its extension
is the theme name. Use ASCII letters, numbers, underscores, and hyphens. `default` is reserved.
The designer uses names of 1–40 characters starting with a letter or number.

## File format

Save this as `mytheme.toml` in a theme directory:

```toml
[colors]
background = "#14121b"
text = "#e3decd"
muted = "#9e93af"
accent = "#d5b879"
success = "#afcb91"
bar = "#492958"
bar_text = "#f5ead4"
border = "#755087"
info = "#ba9ed1"
warning = "#e6c75c"
error = "#e06767"
special = "#d48ebd"
bright = "#ffffff"
```

| Role | Used for |
| --- | --- |
| `background` | Terminal canvas; also accepts `"default"` to use the terminal background |
| `text` | Messages and tool output |
| `muted` | Timestamps and secondary details |
| `accent` | Prompt and active controls |
| `success` | Replies and completed runs |
| `bar` / `bar_text` | Status and selection backgrounds / labels |
| `border` | Pane dividers and frames |
| `info` | Your nick, channel label, and tools |
| `warning` | Working and approval states |
| `error` | Failed runs and errors |
| `special` | Code accents and highlights |
| `bright` | Emphasized text |

Omitted roles keep their built-in colors. Unknown roles or tables, duplicate keys, and invalid
colors reject the file. `/themes` explains the error; a rejected selection keeps the current theme.

Setup screens keep the built-in colors. Themes do not change nick formats, timestamps, or pane
layout. Inline output already flushed into terminal scrollback retains its earlier colors;
new output uses the selected theme. ANSI artwork is independent: use the
[Masthead Studio](https://wess.io/ainz/masthead/) and [header guide](headers.md) to customize it.
