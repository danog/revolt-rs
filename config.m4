dnl phpize/PIE build of the revolt extension (revolt-rs). The extension is pure Rust:
dnl ./configure only checks for cargo, and `make` runs `cargo build` (Makefile.frag) and puts the
dnl result in modules/revolt.so, where `make install` and PIE expect it.

PHP_ARG_ENABLE([revolt],
  [whether to build the revolt extension (revolt-rs)],
  [AS_HELP_STRING([--enable-revolt], [Build the revolt extension (needs Rust: cargo)])],
  [yes], [no])

if test "$PHP_REVOLT" != "no"; then
  AC_PATH_PROG([CARGO], [cargo], [no], [$PATH:$HOME/.cargo/bin])
  if test "$CARGO" = "no"; then
    AC_MSG_ERROR([cargo was not found: this extension is written in Rust, install it from https://rustup.rs (or set CARGO=/path/to/cargo)])
  fi
  PHP_SUBST([CARGO])
  PHP_SUBST([PHP_CONFIG])
  PHP_ADD_MAKEFILE_FRAGMENT([$abs_srcdir/Makefile.frag], [$abs_srcdir], [$abs_builddir])
fi
