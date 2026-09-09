# Contributing Guidelines

## Pull Request Flow
- **Feature branches**: Create a new branch for each feature or bug‑fix. Do **not** commit directly to `main`.
- **Pull request**: Push your branch and open a PR against `main`.
- **Protected `main`**: The `main` branch is protected; it requires at least one approving review before it can be merged.
- **Review**: Address reviewer comments, push additional commits to the same branch, and let the CI pass before merging.

## Tests
- Add or update **unit tests** for any code you modify.
- Place tests inside a `#[cfg(test)]` module in the same file as the code under test.
- Run tests locally with:
  ```sh
  cargo test --release
  ```
- Ensure all tests pass before submitting a PR.

## Continuous Integration (CI)
- Every PR triggers the CI workflow which runs:
  - `cargo clippy --release -- -D warnings`
  - `cargo test --release`
- Both steps must be green. Do not introduce new Clippy warnings.

## Test Naming
- Name tests after the behavior they verify, e.g.:
  ```rust
  #[test]
  fn one_pole_lp_settles_to_dc() { ... }
  ```
- Avoid generic names like `test1` or `my_test`; descriptive names make the intent clear.

---

Thank you for contributing! 🎉