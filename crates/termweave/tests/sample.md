# Terminal Markdown

A **styled buffer** with *emphasis*, ~~revisions~~, `inline code`, and Unicode: 界 é 👩🏽‍💻.

> Readable while it streams.
>
> - [x] Wrapped beneath the nick
> - [ ] Ready for the next chunk

1. Render the current snapshot.
2. Replace it when another chunk arrives.
   - Keep links [visible](https://example.com/docs).

```rust
let document = render("**hello**", &options);
println!("{}", document.ansi());
```

| Output | Width | State |
| :--- | ---: | :---: |
| ANSI | 60 | **ready** |
| Ratatui | 80 | *ready* |

A footnote[^note] and a literal escape: \*text\*.

[^note]: The application owns run completion.
