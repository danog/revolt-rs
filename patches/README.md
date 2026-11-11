# Patched dependencies

`[patch.crates-io]` in the root Cargo.toml replaces these crates for builds of this workspace.

- `ext-php-rs-build` 0.2.0 (ext-php-rs's build-time PHP detection, MIT OR Apache-2.0, by the
  ext-php-rs authors, https://github.com/extphprs/ext-php-rs): unchanged except that PHP 8.6's
  API number 20261112 (php-src bumped it from 20260924 after 8.6.0RC2) is accepted. Remove the
  patch once an ext-php-rs-build release accepts it.

An extension embedding revolt-rs that builds on PHP 8.6 needs the same patch in its own
workspace (Cargo applies only the root workspace's `[patch]` section).
