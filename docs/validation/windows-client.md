# Windows connection checkpoint

Verified on the current Windows development host, 2026-11-19:

- `cargo test --workspace`: 32 passing tests. Two top-level tests are opt-in:
  the 100,000-track measurement (run separately at S4) and the subprocess helper
  (invoked by the passing single-instance test).
- `cargo clippy --workspace --all-targets --features dagaz-windows/ui -- -D warnings`:
  passed, including the native UI target.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo build --release -p dagaz-windows --features ui`: passed.
- Native release window: launch/close, agreed green primary action, resize to
  minimum 760×580 client size, complete two-line validation error, entered address
  preserved. Previous native checks also cover Cyrillic input and Tab focus.
- Real server HTTP/SQLite/RustFS integration: pair, encrypted save and replacement,
  restore, authenticated request and revocation. Client-only tests also cover
  reused/incorrect pairing codes, tampered encrypted data, origin rejection,
  redirect refusal and a second process attempting the same profile.
- Fresh independent code review of W1 and S4 found no Important/Critical issue.

Visual check found that direct theme-field mutation left GPUI render tokens
unchanged: the primary button stayed white. Using `Theme::update` synchronizes
colors, tokens and the base theme. Native release rerun confirmed the green
button after this correction; no new visual snapshot framework was introduced.

Not yet verified: Windows 10, alternate DPI/text scaling, mutex behavior across
separate Windows logon sessions, live public HTTPS, power loss during filesystem
replacement, or the final whole-product acceptance checks. The release binary
currently provides connection setup, not the planned library/player UI.
