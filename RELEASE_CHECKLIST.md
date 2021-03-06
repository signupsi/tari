# Point Release checklist

THings to do before pushing a new commit to `master`:

* Create new `rc` branch off development.
* Update crate version numbers
* Check that all tests pass in development (`cargo test`, `cargo test --release`)
* Publish new crates to crates.io (`./scripts/publish_crates.sh`)
  * Fix any issues with publishing
* Rebase onto master (from rc branch, `git reset --soft master` and `git commit`)
* Tag commit
* Write release notes on GitHub.
* Merge back into development (where appropriate)
* Delete branch