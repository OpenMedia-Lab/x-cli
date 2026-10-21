# x-cli

Rust command-line access to X: search and read posts, inspect accounts and timelines, publish images, reply, and delete posts. Native commands return JSON for use by people and agents.

This project is a fork of [sferik/x-cli](https://github.com/sferik/x-cli), with the upstream Git history, MIT license, legacy Ruby compatibility sources, and existing local Rust cookie-backend work preserved. The OpenMedia-Lab additions provide a JSON command interface and extend the native browser-session backend. See [upstream documentation](README-UPSTREAM.md) for the inherited developer-API command surface.

## Install

```sh
cargo install --path . --locked
```

The binary is `x`. Building locally with `cargo build --release` leaves an existing installation untouched.

## Browser-session commands

Log into x.com in Chrome, Brave, Edge, Arc, Vivaldi, or Chromium on the same computer. The CLI reads the local session through the Rust `rookie` library. macOS may request Keychain access to decrypt the browser's cookies. Credentials are never stored in this repository or printed by the CLI.

```sh
x native me
x native user momoyeyu
x native search 'Rust lang:en' --count 20
x native timeline momoyeyu --count 20
x native read 1234567890123456789
x native post 'Hello from Rust' --image first.png --image second.png
x native reply 1234567890123456789 'A reply' --image image.png
x native delete 1234567890123456789
```

Search and timeline return `{ "data": [...], "next_cursor": "..." }`; pass the cursor back with `--cursor`. Post/reply return the created ID. Read output includes full note text where provided by X, media, author, timestamp, reply parent, and engagement counts. Errors are JSON on stderr and return a nonzero exit status. Up to four image files can be attached to a post or reply. Thread posts can be chained using each returned ID as the next reply parent.

Native commands use the web API directly from Rust; no OpenCLI or browser automation process is required. X can change these undocumented endpoints, session requirements, or account entitlements. Operation identifiers and feature switches are discovered from the live client. Search requests carry a fresh transaction header computed in Rust from the current homepage and client chunk; the vendored MIT [twr-tx](vendor/twr-tx/NOTICE) derivation retains its original attribution. API failures are surfaced instead of reported as success.

## Other commands

The inherited command surface uses developer API credentials from `~/.xrc` (and legacy `~/.trc`). `X_BACKEND=web` selects the cookie backend for the legacy post/reply/read/delete paths; prefer `x native` for JSON, search, profiles, timelines, and multiple images. Inherited commands beyond those native paths retain their upstream API requirements.

Post editing, X Articles, video uploads, bookmarks, and messaging are not implemented by the native interface. Reading long note text does not imply an account can publish long posts or Articles. X decides access, rate limits, and subscription capabilities.

## Development

```sh
cargo fmt --all -- --check
cargo test --locked --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --locked --release
```

Do not add cookie dumps, browser databases, or `.env` files to source control. Live verification should use uniquely labeled artifacts, record created IDs, and delete only those artifacts.

## License

[MIT](LICENSE.md). Original copyright Erik Berlin; upstream attribution is preserved.
