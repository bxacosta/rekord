# Publishing

1. Set the new version in `Cargo.toml`.
2. Run `cargo build` to update `Cargo.lock`. CI builds with `--locked`, so a
   stale lock file fails the release.
3. Commit both files and push to `main`.
4. Tag and push the tag:

   ```sh
   git tag -a v0.2.0 -m "rekord 0.2.0"
   git push origin v0.2.0
   ```

The `Release` workflow runs CI, checks that the tag matches the version in
`Cargo.toml`, builds `rekord-vX.Y.Z-windows-x64.exe` and publishes it with its
SHA-256 in a GitHub release, with notes generated from the commits.

If it fails, fix the problem, then move the tag to the new commit:

```sh
git tag -d v0.2.0
git push origin :refs/tags/v0.2.0
gh release delete v0.2.0 --yes   # only if the release was created
```

and repeat step 4.
