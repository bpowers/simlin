# Rust Development Standards

## Error Handling

- **Strongly** prefer idiomatic use of `Result`/`Option` rather than `.unwrap()`. Avoid `.unwrap_or_default()` when it would silently mask an error condition; use it when the default is genuinely the correct value (e.g. `map.get(&key).unwrap_or_default()` for missing keys).
- If a case (e.g. match arm) is expected to be unreachable, use `unreachable!()`, not a comment.

## Testing

- Do NOT write one-off Rust files compiled with `rustc` to test hypotheses. Write unit tests close to the source of the problem instead -- they serve as both verification and documentation.
- Tests should err on the side of brittleness: if a required test file is missing, fail loudly rather than skipping.

## Code Quality

- No placeholder comments ("this is a placeholder"). Use `todo!()` or `unimplemented!()` macros for stubbed-out code, but generally continue working until the implementation is complete.
- Target 95%+ code coverage for new code.
