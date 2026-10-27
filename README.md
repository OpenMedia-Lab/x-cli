<p align="center"><strong>English</strong> · <a href="README_ZH.md">简体中文</a></p>

<p align="center"><img src="docs/assets/readme-hero.png" alt="x-cli: browser sessions meet the terminal" width="100%"></p>

# x-cli

**Search, read, post and reply on X from a Rust CLI.** The native interface uses your local browser session and returns JSON. The binary is `x`.

[Quick start](#quick-start) · [Post and reply](#post-and-reply) · [Command guide](docs/usage.md) · [中文](README_ZH.md)

- **Explore with context.** Search, profiles, timelines and full note text where X provides it.
- **Publish from scripts.** Text, up to four images, replies and deletion; created IDs let you chain a thread.
- **Use an existing session.** Native commands make Rust HTTP requests with browser cookies, without an OpenCLI or browser automation process for normal operations.

## Quick start

Requires a current stable Rust toolchain and a supported desktop browser. Install from source:

```sh
git clone https://github.com/OpenMedia-Lab/x-cli.git
cd x-cli
cargo install --path . --locked

x native login --qr --timeout 180
x native me
x native search 'Rust lang:en' --count 10
```

The login command reuses a verified browser session or opens official X login in Chrome. **QR is the browser/OS cross-device Passkey flow**, available only with an enrolled Passkey and compatible devices. Otherwise complete normal browser login. It is not an X app-scan QR. See [login details](docs/usage.md#interactive-login-and-passkey-qr).

Chrome, Brave, Edge, Arc, Vivaldi and Chromium sessions are supported. macOS may request Keychain access to decrypt local cookies. Native commands do not require X developer API credentials.

## Read and paginate

```sh
x native user momoyeyu
x native timeline momoyeyu --count 20
x native read POST_ID
x native search 'Rust lang:en' --count 20 --cursor 'CURSOR_FROM_PREVIOUS_RESULT'
```

Search and timeline return `data` and `next_cursor`; use a nonempty cursor for the next page. A read includes the available author, media, timestamp, reply parent and engagement fields. Errors are JSON on stderr with a nonzero exit status.

## Post and reply

**These commands publish publicly immediately.** X has no private/draft posting mode in this interface. Prepare your own image file and review the text first.

```sh
x native post 'A project I built: https://example.com' --image cover.png
x native reply POST_ID 'More implementation details'
x native delete POST_ID
```

Use the ID returned by `post` as the next reply's parent to build a thread. Post and reply accept up to four repeated `--image` flags.

## Native and inherited interfaces

| Interface | Authentication | Scope |
|---|---|---|
| `x native …` | Local browser session | Search, read, accounts, timelines, post, reply, delete; JSON |
| Inherited commands | Developer API credentials from `~/.xrc` | [Upstream command surface](README-UPSTREAM.md) |

The native backend does **not** implement post editing, X Articles, video uploads, bookmarks or messages. Reading long text does not grant long-post publishing entitlement. X controls account access and rate limits; undocumented web endpoints can change. See the [complete guide](docs/usage.md) for backend selection and protocol details.

## Contribute

Include a minimal command and redacted error JSON when reporting problems. Never attach cookie databases or credentials. For source changes:

```sh
cargo fmt --all -- --check
cargo test --locked --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Works with

[postdeck](https://github.com/OpenMedia-Lab/postdeck) adapts articles into cards; separate CLIs handle platform operations: [xhs-cli](https://github.com/OpenMedia-Lab/xhs-cli), [zhihu-cli](https://github.com/OpenMedia-Lab/zhihu-cli), [x-cli](https://github.com/OpenMedia-Lab/x-cli). Explore the rest of [OpenMedia-Lab](https://github.com/OpenMedia-Lab).

## License and origins

[MIT](LICENSE.md). Forked from [sferik/x-cli](https://github.com/sferik/x-cli), preserving upstream history, license and [legacy sources](legacy/README.md). OpenMedia-Lab maintains the native Rust JSON/browser-session interface. The transaction-header derivation retains the [twr-tx attribution](vendor/twr-tx/NOTICE).
