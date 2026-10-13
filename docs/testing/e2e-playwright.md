# Browser E2E tests (Playwright)

Run `tests/e2e/run-e2e.sh` after `cargo build` (it uses `target/debug/` binaries). It starts
the backend and the UI on throwaway ports and runs `test-ui.spec.mjs` in headless Firefox.

## Where Playwright comes from

The script takes both the runner (`nixpkgs#playwright-test`) and the browsers
(`nixpkgs#playwright-driver.browsers`) from the same nixpkgs revision, so their versions
match. The previous `npx @playwright/test@latest` runner asked for a Firefox build that
nixpkgs does not ship. The spec is an ES module, which ignores `NODE_PATH`, so the script
links `tests/e2e/node_modules` to the runner's package set (the link and `test-results/`
are git-ignored). The repo's flake dev shell does not provide Playwright.

## Gotchas

- A `LD_LIBRARY_PATH` that points at a different glibc than the browser's (seen:
  `alsa-lib` built against glibc 2.43 while the browsers use 2.42) makes Firefox fail with
  `GLIBC_2.43 not found`. Run with `env -u LD_LIBRARY_PATH tests/e2e/run-e2e.sh`.
- Status when this was written: 23 of 26 tests pass. Failing: "Edit Ports CRUD > shows all
  port fields as editable", "Edit Ports CRUD > add a port ...", and "Edit Mirrors CRUD > add
  a mirror session ...". These were not investigated (spec/UI drift is suspected).
